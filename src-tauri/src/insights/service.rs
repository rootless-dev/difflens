//! What the commands ask of Insights: the findings of a review read against
//! its code, what Analyze would send, starting an analysis, running one of
//! its calls, the user's verdicts, and marking a finding fixed.
//!
//! Only `run_batch`, `run_calls`, `translate` and `write_comment` ask the
//! model, through `ModelService`: the frontend calls the first after a click
//! on Analyze or Retry, one call at a time; a full analysis, confirmed by a
//! click, runs the second in the core, every call of an analysis with the
//! provider's parallelism; the last writes a review comment from one
//! finding after a click on Write comment.
//!
//! A new analysis never deletes a finding. Each call also shows the model
//! the earlier findings still open (not dismissed, not fixed) in its files
//! and asks whether each is still there or fixed: a fixed one is marked so
//! and kept, one still there stays (moved to where the model points), and
//! one the answer leaves out, or whose call fails, stays as it was. An open
//! finding whose file is no longer part of the change is marked fixed when
//! the analysis starts, with no model asked.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde_json::Value;
use uuid::Uuid;

use super::comment;
use super::findings::{self, Checked, RecheckStatus};
use super::locate;
use super::plan::{self, FileInput, Prepared};
use crate::ai::{Calls, ModelAnswer, ModelError, ModelRequest, ModelService};
use crate::error::{app_error, AppError, AppResult, ErrorCode};
use crate::fingerprint;
use crate::flow;
use crate::language::Language;
use crate::parallel;
use crate::model::{
    Finding, FindingLocation, FindingResolution, FindingSource, FindingTranslation,
    FindingVerdict, Insights, InsightsAnalysis, InsightsBatch, InsightsBatchStatus, InsightsPlan,
    LeftOut, LocationProblem, ModelInUse, ModelProviderKind, ModelTask, PlannedCall,
    ResolutionKind, ReviewSide, SignalDraft, SkippedFile, TranslationCall, TranslationPlan,
};
use crate::hunks::ChangedLines;
use crate::review_state::{self, ChangedSides, ReviewSession};
use crate::store::{AnalysisRecord, BatchRecord, FindingRecord, Store};
use crate::translation::{self, FindingProse};

/// What the model is told about reviewing; see `prompt.md`.
pub const SYSTEM_PROMPT: &str = include_str!("prompt.md");

#[derive(Default)]
pub struct InsightsService {
    /// Calls answer side by side; their findings are compared with those
    /// already kept and saved one answer at a time.
    merging: Mutex<()>,
}

/// What an analysis of the change would send.
struct Planned {
    listing: String,
    calls: Vec<PlannedCall>,
    skipped: Vec<SkippedFile>,
    files: usize,
}

impl InsightsService {
    pub fn new() -> Self {
        Self::default()
    }

    /// The findings of the review, read against its code, and its latest
    /// analysis. Asks no model.
    pub fn insights(&self, session: &ReviewSession<'_>) -> AppResult<Insights> {
        match session.existing_record()? {
            Some(record) => read(session, &record.id),
            None => Ok(Insights {
                analysis: None,
                findings: Vec::new(),
            }),
        }
    }

    /// What Analyze would send and to whom. Asks no model.
    pub fn plan(
        &self,
        session: &ReviewSession<'_>,
        models: &ModelService,
    ) -> AppResult<InsightsPlan> {
        let planned = plan_change(&session.changed_sides()?);
        Ok(InsightsPlan {
            files: count(planned.files),
            tokens: planned.calls.iter().map(|call| call.tokens).sum(),
            skipped: planned.skipped,
            calls: planned.calls,
            model: models.in_use(session.store(), ModelTask::Insights)?,
        })
    }

    /// Records a new analysis of the change and its calls, none asked yet,
    /// to be written in `language`, and marks fixed the open findings whose
    /// file is no longer part of the change. Asks no model: the frontend
    /// runs each call with `run_batch`.
    pub fn start(
        &self,
        session: &ReviewSession<'_>,
        models: &ModelService,
        language: Language,
    ) -> AppResult<Insights> {
        let store = session.store();
        let model = models.in_use(store, ModelTask::Insights)?;
        ensure_ready(&model)?;
        let files = session.changed_sides()?;
        let planned = plan_change(&files);
        if planned.calls.is_empty() {
            return Err(app_error!(
                Internal,
                NothingToSend,
                "Nothing in this change can be sent to the model: every changed file is deleted, \
                 binary or too large to read."
            ));
        }
        let record = session.record()?;
        let analysis = AnalysisRecord {
            id: Uuid::new_v4().to_string(),
            review_id: record.id.clone(),
            head_sha: session.review().new_sha.clone(),
            provider: model.provider.key().to_string(),
            model: model.model.unwrap_or_default(),
            created_at: flow::now()?,
            language,
        };
        let batches: Vec<BatchRecord> = planned
            .calls
            .into_iter()
            .enumerate()
            .map(|(index, call)| BatchRecord {
                index: count(index),
                paths: call.paths,
                tokens: call.tokens,
                status: InsightsBatchStatus::Pending,
                error: None,
            })
            .collect();
        store.insert_analysis(&analysis, &batches)?;
        let changed: HashSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
        resolve_left_files(store, &record.id, &changed, &analysis.created_at)?;
        read(session, &record.id)
    }

    /// Asks the model call `index` of the latest analysis, under `run_id`,
    /// and keeps what it found. A failed or stopped call is recorded on the
    /// call, with its error, for Retry; it fails the command only when the
    /// analysis or the call is gone.
    pub fn run_batch(
        &self,
        session: &ReviewSession<'_>,
        models: &ModelService,
        analysis_id: &str,
        index: u32,
        run_id: &str,
    ) -> AppResult<Insights> {
        let store = session.store();
        self.ask_batch(session, analysis_id, index, &|request| {
            models.ask(store, ModelTask::Insights, run_id, request)
        })?;
        read(session, &session.record()?.id)
    }

    /// Asks the calls `indexes` of the analysis `analysis_id` through
    /// `calls`, `parallel` at a time, as Analyze does from the screen, and
    /// tells `report` as each starts and ends. No call starts once `calls` is
    /// cancelled or a newer analysis replaced this one; a call that did not
    /// start stays pending, for Retry. Returns how each call ended, `None`
    /// for one that did not start.
    #[allow(clippy::too_many_arguments)]
    pub fn run_calls(
        &self,
        session: &ReviewSession<'_>,
        models: &ModelService,
        calls: &Calls,
        analysis_id: &str,
        indexes: &[u32],
        parallel: usize,
        report: &(dyn Fn(u32, Option<&AppResult<CallEnd>>) + Sync),
    ) -> Vec<Option<AppResult<CallEnd>>> {
        let store = session.store();
        let replaced = AtomicBool::new(false);
        parallel::run(
            indexes,
            parallel,
            || calls.is_cancelled() || replaced.load(Ordering::Relaxed),
            |_, index| {
                report(*index, None);
                let end = self.ask_batch(session, analysis_id, *index, &|request| {
                    calls.ask(models, store, ModelTask::Insights, request)
                });
                if matches!(&end, Err(error) if error.code == ErrorCode::UnknownFinding) {
                    replaced.store(true, Ordering::Relaxed);
                }
                report(*index, Some(&end));
                end
            },
        )
    }

    /// Asks call `index` of the analysis `analysis_id` with `ask`, keeps
    /// what it found, and says how it ended. A failed or stopped call is
    /// recorded on the call, with its error; this fails only when the
    /// analysis or the call is gone.
    fn ask_batch(
        &self,
        session: &ReviewSession<'_>,
        analysis_id: &str,
        index: u32,
        ask: &dyn Fn(ModelRequest) -> Result<ModelAnswer, ModelError>,
    ) -> AppResult<CallEnd> {
        let store = session.store();
        let record = session.record()?;
        let analyses = store.analyses(&record.id)?;
        let gone = || {
            app_error!(
                UnknownFinding,
                AnalysisReplaced,
                "A newer analysis replaced this one. Click Analyze to start again."
            )
        };
        if analyses.first().map(|latest| latest.id.as_str()) != Some(analysis_id) {
            return Err(gone());
        }
        let batches = store.batches(analysis_id)?;
        let batch = batches
            .iter()
            .find(|batch| batch.index == index)
            .ok_or_else(gone)?;
        if batch.status == InsightsBatchStatus::Done {
            return Ok(CallEnd::Settled);
        }

        let files = session.changed_sides()?;
        let planned = plan_change(&files);
        let capacity = plan::content_budget(overhead_tokens(&planned.listing));
        let by_path: HashMap<&str, &ChangedSides> = files
            .iter()
            .map(|file| (file.path.as_str(), file))
            .collect();
        let inputs: Vec<FileInput> = batch
            .paths
            .iter()
            .filter_map(|path| by_path.get(path.as_str()))
            .filter_map(|file| match plan::prepare(file, capacity) {
                Prepared::Send(input) => Some(input),
                Prepared::Skip(_) => None,
            })
            .collect();
        if inputs.is_empty() {
            // Its files were deleted or emptied since: nothing to read.
            store.finish_batch(analysis_id, index, InsightsBatchStatus::Done, None)?;
            return Ok(CallEnd::Settled);
        }

        let sent: HashMap<String, Vec<String>> = inputs
            .iter()
            .filter_map(|input| {
                let text = by_path.get(input.path.as_str())?.new.text.as_deref()?;
                Some((input.path.clone(), owned_lines(text)))
            })
            .collect();
        let earlier = to_recheck(store, &record.id, analysis_id, &sent)?;
        let language = analyses[0].language;
        let request = ModelRequest {
            system: SYSTEM_PROMPT.to_string(),
            // The language opens the question; the system prompt says what
            // stays as it is.
            prompt: format!(
                "{}{}{}",
                language.prose_instruction(),
                prompt(&planned.listing, batches.len(), index, &inputs),
                earlier_prompt(&earlier, &sent)
            ),
            schema: findings::schema(),
            schema_name: findings::SCHEMA_NAME.to_string(),
        };
        match ask(request) {
            Ok(answer) => {
                let changed: HashSet<&str> = by_path.keys().copied().collect();
                let answer = Answer {
                    value: &answer.value,
                    model: &answer.model,
                    sent: &sent,
                    changed: &changed,
                    earlier: &earlier,
                };
                let findings = self.keep(store, &record.id, analysis_id, index, &answer)?;
                Ok(CallEnd::Answered {
                    findings: count(findings),
                })
            }
            Err(error) => {
                let error = AppError::from(error);
                store.finish_batch(
                    analysis_id,
                    index,
                    InsightsBatchStatus::Failed,
                    Some(&error),
                )?;
                Ok(CallEnd::Failed(error))
            }
        }
    }

    /// Records the user's verdict on a finding. A confirmed or dismissed
    /// one keeps a fingerprint of the code around it, to tell later that it
    /// changed.
    pub fn set_verdict(
        &self,
        session: &ReviewSession<'_>,
        finding_id: &str,
        verdict: FindingVerdict,
    ) -> AppResult<Insights> {
        let store = session.store();
        let gone = || {
            app_error!(
                UnknownFinding,
                FindingGone,
                "That finding no longer exists."
            )
        };
        let record = session.existing_record()?.ok_or_else(gone)?;
        let finding = store.finding(&record.id, finding_id)?.ok_or_else(gone)?;
        if verdict == FindingVerdict::Pending {
            store.set_verdict(&finding.id, verdict, None, None)?;
            return read(session, &record.id);
        }
        let lines = new_lines(session, std::slice::from_ref(&finding))?;
        let print = current_fingerprint(&finding, lines.get(&finding.path));
        store.set_verdict(&finding.id, verdict, Some(&flow::now()?), Some(&print))?;
        read(session, &record.id)
    }

    /// Marks a finding fixed by the user, with no model asked, or opens it
    /// again (`fixed` false), whoever marked it. Its verdict stays as it is;
    /// marking fixed one that already is keeps how it was fixed.
    pub fn set_fixed(
        &self,
        session: &ReviewSession<'_>,
        finding_id: &str,
        fixed: bool,
    ) -> AppResult<Insights> {
        let store = session.store();
        let gone = || {
            app_error!(
                UnknownFinding,
                FindingGone,
                "That finding no longer exists."
            )
        };
        let record = session.existing_record()?.ok_or_else(gone)?;
        let finding = store.finding(&record.id, finding_id)?.ok_or_else(gone)?;
        if !fixed {
            store.set_resolution(&finding.id, None)?;
        } else if finding.resolution.is_none() {
            let marked = FindingResolution {
                kind: ResolutionKind::UserMarked,
                model: None,
                at: flow::now()?,
                reason: None,
            };
            store.set_resolution(&finding.id, Some(&marked))?;
        }
        read(session, &record.id)
    }

    /// Asks the model of the Insights task, only after a click on Write
    /// comment, to write a short review comment from the finding
    /// `finding_id`, and keeps it as a draft linked to the finding; returns
    /// the draft written before, asking nothing, while it is there. See
    /// `comment`.
    pub fn write_comment(
        &self,
        session: &ReviewSession<'_>,
        models: &ModelService,
        changed: &ChangedLines,
        finding_id: &str,
        language: Language,
        run_id: &str,
    ) -> AppResult<SignalDraft> {
        comment::write(session, models, changed, finding_id, language, run_id)
    }

    /// What translating the review's findings into `language` would send:
    /// the findings written in another language that have no version in it
    /// yet, in calls. Asks no model.
    pub fn plan_translation(
        &self,
        session: &ReviewSession<'_>,
        models: &ModelService,
        language: Language,
    ) -> AppResult<TranslationPlan> {
        let prose = untranslated(session, language)?;
        let calls: Vec<TranslationCall> = translation::pack_findings(language, &prose)
            .into_iter()
            .map(|(ids, tokens)| TranslationCall { ids, tokens })
            .collect();
        Ok(TranslationPlan {
            items: count(prose.len()),
            tokens: calls.iter().map(|call| call.tokens).sum(),
            calls,
            model: models.in_use(session.store(), ModelTask::Translation)?,
        })
    }

    /// Asks the model, under `run_id`, to translate the text of the findings
    /// `ids` into `language`, only after a click on Translate, and keeps each
    /// translation as one more version of its finding: the findings, their
    /// verdicts and locations stay as they are. A finding the answer leaves
    /// out keeps only its original. Nothing is kept when the call fails.
    pub fn translate(
        &self,
        session: &ReviewSession<'_>,
        models: &ModelService,
        language: Language,
        ids: &[String],
        run_id: &str,
    ) -> AppResult<Insights> {
        let store = session.store();
        ensure_ready(&models.in_use(store, ModelTask::Translation)?)?;
        let Some(record) = session.existing_record()? else {
            return self.insights(session);
        };
        let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let prose: Vec<FindingProse> = untranslated(session, language)?
            .into_iter()
            .filter(|finding| wanted.contains(finding.id.as_str()))
            .collect();
        if prose.is_empty() {
            return read(session, &record.id);
        }
        let sent: Vec<String> = prose.iter().map(|finding| finding.id.clone()).collect();
        let request = ModelRequest {
            system: translation::SYSTEM_PROMPT.to_string(),
            prompt: translation::findings_prompt(language, &prose),
            schema: translation::findings_schema(&sent),
            schema_name: translation::FINDINGS_SCHEMA_NAME.to_string(),
        };
        let answer = models.ask(store, ModelTask::Translation, run_id, request)?;
        let translated = translation::parse_findings(&answer.value, &prose).map_err(|detail| {
            ModelError::InvalidOutput {
                provider: answer.provider,
                detail,
            }
        })?;
        // Only the findings still there are kept: the call may have been
        // out while they went.
        let present: HashSet<String> = store
            .findings(&record.id)?
            .into_iter()
            .map(|finding| finding.id)
            .collect();
        let kept: Vec<(String, FindingTranslation)> = translated
            .into_iter()
            .filter(|finding| present.contains(&finding.id))
            .map(|finding| {
                (
                    finding.id,
                    FindingTranslation {
                        language,
                        title: finding.title,
                        explanation: finding.explanation,
                        suggested_direction: finding.suggested_direction,
                        evidence: finding.evidence,
                        related: finding.related,
                    },
                )
            })
            .collect();
        store.save_finding_translations(&kept)?;
        read(session, &record.id)
    }

    /// Applies what one answer says of the earlier findings it was asked to
    /// re-check, keeps its findings that say something not already kept
    /// open, and ends the call; returns how many new findings were kept.
    /// Nothing kept before is deleted.
    fn keep(
        &self,
        store: &Store,
        review_id: &str,
        analysis_id: &str,
        index: u32,
        answer: &Answer<'_>,
    ) -> AppResult<usize> {
        let _merging = self
            .merging
            .lock()
            .map_err(|_| {
                app_error!(
                    Internal,
                    InsightsUnavailable,
                    "Insights are unavailable after a failure."
                )
            })?;
        // A newer analysis started while this call was out: its answer is
        // dropped rather than let replace the newer one's findings.
        let latest = store.analyses(review_id)?.into_iter().next();
        if latest.map(|analysis| analysis.id).as_deref() != Some(analysis_id) {
            return Ok(0);
        }
        apply_rechecks(store, review_id, answer)?;
        // A new finding that says what an open one already says is that one
        // said again; a fixed one does not stand for it, as the problem is
        // back.
        let kept: Vec<FindingRecord> = store
            .findings(review_id)?
            .into_iter()
            .filter(|finding| finding.resolution.is_none())
            .collect();
        let mut fresh: Vec<FindingRecord> = Vec::new();
        for (ordinal, raw) in findings::parse(answer.value).into_iter().enumerate() {
            let checked = findings::check(&raw, answer.sent, |path| answer.changed.contains(path));
            let (end_line, location_problem, cited_text) = match checked {
                Checked::Located {
                    end_line,
                    cited_text,
                    ..
                } => (end_line, None, cited_text),
                Checked::NotLocated(problem) => (raw.end_line, Some(problem), String::new()),
            };
            let place = (raw.path.as_str(), raw.line, end_line, raw.title.as_str());
            let said = kept.iter().chain(&fresh).any(|other| {
                findings::same(
                    (&other.path, other.line, other.end_line, &other.title),
                    place,
                )
            });
            if said {
                continue;
            }
            fresh.push(FindingRecord {
                id: Uuid::new_v4().to_string(),
                analysis_id: analysis_id.to_string(),
                batch: index,
                ordinal: count(ordinal),
                title: raw.title,
                severity: raw.severity,
                category: raw.category,
                path: raw.path,
                line: raw.line,
                end_line,
                explanation: raw.explanation,
                suggested_direction: raw.suggested_direction,
                evidence: raw.evidence,
                related: raw.related,
                location_problem,
                cited_text,
                verdict: FindingVerdict::Pending,
                verdict_at: None,
                verdict_fingerprint: None,
                resolution: None,
            });
        }
        store.insert_findings(&fresh)?;
        store.drop_empty_analyses(review_id, analysis_id)?;
        store.set_analysis_model(analysis_id, answer.model)?;
        store.finish_batch(analysis_id, index, InsightsBatchStatus::Done, None)?;
        Ok(fresh.len())
    }
}

/// How one call of an analysis ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallEnd {
    /// It answered, and this many new findings were kept from it.
    Answered { findings: u32 },
    /// It had answered already, or its files had nothing left to read.
    Settled,
    /// It failed or was stopped; the call keeps the error, for Retry.
    Failed(AppError),
}

/// A model's answer to one call, and what that call was shown.
struct Answer<'a> {
    value: &'a Value,
    model: &'a str,
    /// New-side lines of the files the call sent, by path.
    sent: &'a HashMap<String, Vec<String>>,
    changed: &'a HashSet<&'a str>,
    /// The earlier findings the call asked the model to re-check.
    earlier: &'a [Earlier],
}

/// An earlier finding a call asks the model to re-check, under the short
/// reference the prompt gives it ("E1").
#[derive(Debug, Clone)]
struct Earlier {
    reference: String,
    finding: FindingRecord,
}

/// Not dismissed and not fixed: what a new analysis re-checks.
fn is_open(finding: &FindingRecord) -> bool {
    finding.verdict != FindingVerdict::Dismissed && finding.resolution.is_none()
}

/// The open findings of the review's analyses before `analysis_id` in the
/// files a call sends (`sent`), in the order they are kept, each under its
/// reference in that call.
fn to_recheck(
    store: &Store,
    review_id: &str,
    analysis_id: &str,
    sent: &HashMap<String, Vec<String>>,
) -> AppResult<Vec<Earlier>> {
    Ok(store
        .findings(review_id)?
        .into_iter()
        .filter(|finding| {
            finding.analysis_id != analysis_id
                && is_open(finding)
                && sent.contains_key(&finding.path)
        })
        .enumerate()
        .map(|(position, finding)| Earlier {
            reference: format!("E{}", position + 1),
            finding,
        })
        .collect())
}

/// The part of a call's question listing the earlier findings to re-check,
/// each where it is now in `sent` when its cited lines are still found;
/// empty when there are none.
fn earlier_prompt(earlier: &[Earlier], sent: &HashMap<String, Vec<String>>) -> String {
    if earlier.is_empty() {
        return String::new();
    }
    let mut text = format!(
        "\nEarlier findings to re-check ({}). An earlier review of this change reported them in \
         the files shown above, and the code may have changed since. Answer each one in \
         \"earlier\" by its reference; do not report them again in \"findings\".\n\n",
        earlier.len()
    );
    for Earlier { reference, finding } in earlier {
        let place = match current_place(finding, sent.get(&finding.path)) {
            Some((line, end_line, _)) => format!("{}:{}", finding.path, span(line, end_line)),
            None => format!(
                "{}, cited at {} {} of an earlier version; that code changed since",
                finding.path,
                if finding.end_line > finding.line {
                    "lines"
                } else {
                    "line"
                },
                span(finding.line, finding.end_line)
            ),
        };
        let _ = writeln!(text, "- {reference}: {place}\n  {}\n  {}", finding.title, finding.explanation);
    }
    text
}

/// "8-12", or "8" for one line.
fn span(line: u32, end_line: u32) -> String {
    if end_line > line {
        format!("{line}-{end_line}")
    } else {
        line.to_string()
    }
}

/// Marks fixed the earlier findings the answer says are, with the model
/// that answered and its reason, and moves those still there to where it
/// points when that is in the code it was shown. A finding the answer does
/// not name, or that was dismissed or marked fixed since the call left,
/// stays as it is.
fn apply_rechecks(store: &Store, review_id: &str, answer: &Answer<'_>) -> AppResult<()> {
    let at = flow::now()?;
    for recheck in findings::parse_rechecks(answer.value) {
        let Some(earlier) = answer
            .earlier
            .iter()
            .find(|earlier| earlier.reference == recheck.reference)
        else {
            continue;
        };
        let Some(finding) = store
            .finding(review_id, &earlier.finding.id)?
            .filter(is_open)
        else {
            continue;
        };
        match recheck.status {
            RecheckStatus::Fixed => {
                let checked = FindingResolution {
                    kind: ResolutionKind::ModelChecked,
                    model: Some(answer.model.to_string()),
                    at: at.clone(),
                    reason: Some(recheck.reason).filter(|reason| !reason.is_empty()),
                };
                store.set_resolution(&finding.id, Some(&checked))?;
            }
            RecheckStatus::StillPresent => {
                let checked = findings::check_place(
                    &finding.path,
                    recheck.line,
                    recheck.end_line,
                    answer.sent,
                    |path| answer.changed.contains(path),
                );
                if let Checked::Located {
                    line,
                    end_line,
                    cited_text,
                } = checked
                {
                    let moved = (line, end_line, cited_text.as_str())
                        != (finding.line, finding.end_line, finding.cited_text.as_str());
                    if moved || finding.location_problem.is_some() {
                        store.relocate_finding(&finding.id, line, end_line, &cited_text)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Marks fixed, with no model asked, the review's open findings whose file
/// is no longer one of the `changed` files. One that never pointed at a
/// changed file is left open: its file did not leave the change.
fn resolve_left_files(
    store: &Store,
    review_id: &str,
    changed: &HashSet<&str>,
    at: &str,
) -> AppResult<()> {
    let left = FindingResolution {
        kind: ResolutionKind::FileLeftChange,
        model: None,
        at: at.to_string(),
        reason: None,
    };
    for finding in store.findings(review_id)? {
        let never_changed = finding.location_problem == Some(LocationProblem::NotChanged);
        if is_open(&finding) && !never_changed && !changed.contains(finding.path.as_str()) {
            store.set_resolution(&finding.id, Some(&left))?;
        }
    }
    Ok(())
}

/// Fails as Analyze would when the model in use cannot be asked.
fn ensure_ready(model: &ModelInUse) -> AppResult<()> {
    if model.ready {
        return Ok(());
    }
    let problem = model.problem.clone().unwrap_or_else(|| {
        app_error!(
            ModelNotConfigured,
            ModelNotSetUp,
            "Set up a model in Preferences › Model."
        )
    });
    Err(ModelError::NotConfigured(problem).into())
}

/// The prose of the review's findings written in another language than
/// `language` and not yet translated into it, in the order they are kept.
fn untranslated(session: &ReviewSession<'_>, language: Language) -> AppResult<Vec<FindingProse>> {
    let Some(record) = session.existing_record()? else {
        return Ok(Vec::new());
    };
    let store = session.store();
    let languages: HashMap<String, Language> = store
        .analyses(&record.id)?
        .into_iter()
        .map(|analysis| (analysis.id, analysis.language))
        .collect();
    let translations = store.finding_translations(&record.id)?;
    Ok(store
        .findings(&record.id)?
        .into_iter()
        .filter(|finding| {
            languages
                .get(&finding.analysis_id)
                .is_some_and(|written| *written != language)
                && !translations.get(&finding.id).is_some_and(|versions| {
                    versions.iter().any(|version| version.language == language)
                })
        })
        .map(|finding| FindingProse {
            id: finding.id,
            title: finding.title,
            explanation: finding.explanation,
            suggested_direction: finding.suggested_direction,
            evidence: finding.evidence,
            related: finding.related,
        })
        .collect())
}

/// The findings of a review against the code it shows now.
pub(super) fn read(session: &ReviewSession<'_>, review_id: &str) -> AppResult<Insights> {
    let store = session.store();
    let analyses = store.analyses(review_id)?;
    let records = store.findings(review_id)?;
    let lines = new_lines(session, &records)?;
    let latest = analyses.first().map(|analysis| analysis.id.as_str());
    let by_id: HashMap<&str, &AnalysisRecord> = analyses
        .iter()
        .map(|analysis| (analysis.id.as_str(), analysis))
        .collect();
    let mut translations = store.finding_translations(review_id)?;
    let mut shown = Vec::new();
    for record in records {
        let Some(analysis) = by_id.get(record.analysis_id.as_str()) else {
            continue;
        };
        let versions: Vec<FindingTranslation> = translations
            .remove(&record.id)
            .unwrap_or_default()
            .into_iter()
            .filter(|version| version.language != analysis.language)
            .collect();
        let provider = provider_of(analysis)?;
        let source = FindingSource {
            analysis_id: analysis.id.clone(),
            provider,
            provider_name: provider.name().to_string(),
            model: analysis.model.clone(),
            analyzed_at: analysis.created_at.clone(),
            head_sha: analysis.head_sha.clone(),
            earlier: Some(analysis.id.as_str()) != latest,
        };
        let file_lines = lines.get(&record.path);
        let mut finding = present(record, source, file_lines);
        finding.language = analysis.language;
        finding.translations = versions;
        shown.push(finding);
    }
    shown.sort_by_key(|finding| (finding.severity, finding.source.earlier));
    let analysis = match analyses.first() {
        Some(latest) => Some(analysis_view(store, latest)?),
        None => None,
    };
    Ok(Insights {
        analysis,
        findings: shown,
    })
}

fn analysis_view(store: &Store, analysis: &AnalysisRecord) -> AppResult<InsightsAnalysis> {
    let provider = provider_of(analysis)?;
    Ok(InsightsAnalysis {
        id: analysis.id.clone(),
        head_sha: analysis.head_sha.clone(),
        provider,
        provider_name: provider.name().to_string(),
        model: analysis.model.clone(),
        created_at: analysis.created_at.clone(),
        parallel_calls: provider.parallel_calls(),
        batches: store
            .batches(&analysis.id)?
            .into_iter()
            .map(|batch| InsightsBatch {
                index: batch.index,
                paths: batch.paths,
                tokens: batch.tokens,
                status: batch.status,
                error: batch.error,
            })
            .collect(),
        language: analysis.language,
    })
}

fn provider_of(analysis: &AnalysisRecord) -> AppResult<ModelProviderKind> {
    ModelProviderKind::from_key(&analysis.provider).ok_or_else(|| {
        app_error!(
            Internal,
            AnalysisProviderUnknown,
            "An analysis was written by an unknown provider \"{provider}\".",
            provider = crate::error::escaped(&analysis.provider)
        )
    })
}

/// The new-side lines, as the Diff tab numbers them, of the changed files
/// the located `findings` point at.
fn new_lines(
    session: &ReviewSession<'_>,
    findings: &[FindingRecord],
) -> AppResult<HashMap<String, Vec<String>>> {
    let wanted: HashSet<(String, ReviewSide)> = findings
        .iter()
        .filter(|finding| finding.location_problem.is_none())
        .map(|finding| (finding.path.clone(), ReviewSide::New))
        .collect();
    if wanted.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(session
        .side_lines(wanted)?
        .into_iter()
        .map(|((path, _), lines)| (path, lines))
        .collect())
}

/// Where the finding's cited lines are in `lines`, if they still are.
fn current_place(finding: &FindingRecord, lines: Option<&Vec<String>>) -> Option<(u32, u32, bool)> {
    if finding.location_problem.is_some() {
        return None;
    }
    locate::relocate(lines?, finding.line, finding.end_line, &finding.cited_text)
}

fn current_fingerprint(finding: &FindingRecord, lines: Option<&Vec<String>>) -> String {
    match (current_place(finding, lines), lines) {
        (Some((line, end_line, _)), Some(lines)) => {
            locate::fingerprint_around(lines, line, end_line)
        }
        _ => fingerprint::ABSENT.to_string(),
    }
}

fn present(record: FindingRecord, source: FindingSource, lines: Option<&Vec<String>>) -> Finding {
    let place = current_place(&record, lines);
    let (location, excerpt) = match (place, lines) {
        (Some((line, end_line, moved)), Some(lines)) => (
            FindingLocation::Found {
                line,
                end_line,
                moved,
            },
            locate::excerpt(lines, line, end_line),
        ),
        _ => (
            FindingLocation::NotFound {
                problem: record.location_problem.unwrap_or(if lines.is_some() {
                    LocationProblem::CodeChanged
                } else {
                    LocationProblem::NotChanged
                }),
            },
            Vec::new(),
        ),
    };
    // A fixed finding is not checked again, whatever its verdict.
    let recheck = record.verdict != FindingVerdict::Pending
        && record.resolution.is_none()
        && record
            .verdict_fingerprint
            .as_deref()
            .is_some_and(|then| then != current_fingerprint(&record, lines));
    Finding {
        id: record.id,
        source,
        title: record.title,
        severity: record.severity,
        category: record.category,
        path: record.path,
        line: record.line,
        end_line: record.end_line,
        location,
        excerpt,
        explanation: record.explanation,
        suggested_direction: record.suggested_direction,
        evidence: record.evidence,
        related: record.related,
        verdict: record.verdict,
        verdict_at: record.verdict_at,
        recheck,
        resolution: record.resolution,
        // `read` sets the analysis's language and the translations.
        language: Language::default(),
        translations: Vec::new(),
    }
}

/// What an analysis of `files` would not send: the files it skips, and the
/// files too large for a call, sent as their changed lines only. Asks no
/// model.
pub fn left_out(files: &[ChangedSides]) -> LeftOut {
    let planned = plan_change(files);
    LeftOut {
        skipped: planned.skipped,
        truncated: planned
            .calls
            .into_iter()
            .flat_map(|call| call.truncated)
            .collect(),
    }
}

/// The calls an analysis of `files` would make.
fn plan_change(files: &[ChangedSides]) -> Planned {
    let listing = listing(files);
    let capacity = plan::content_budget(overhead_tokens(&listing));
    let mut inputs = Vec::new();
    let mut skipped = Vec::new();
    for file in files {
        match plan::prepare(file, capacity) {
            Prepared::Send(input) => inputs.push(input),
            Prepared::Skip(file) => skipped.push(file),
        }
    }
    let overhead = overhead_tokens(&listing);
    let truncated: HashSet<&str> = inputs
        .iter()
        .filter(|input| input.truncated)
        .map(|input| input.path.as_str())
        .collect();
    let calls = plan::pack(&inputs, capacity)
        .into_iter()
        .map(|batch| PlannedCall {
            tokens: overhead + batch.content_tokens,
            truncated: batch
                .paths
                .iter()
                .filter(|path| truncated.contains(path.as_str()))
                .cloned()
                .collect(),
            paths: batch.paths,
        })
        .collect();
    Planned {
        listing,
        calls,
        skipped,
        files: inputs.len(),
    }
}

/// Every call repeats the system prompt, the list of changed files and the
/// words around the diffs.
fn overhead_tokens(listing: &str) -> u64 {
    plan::estimate_tokens(SYSTEM_PROMPT) + plan::estimate_tokens(&prompt(listing, 99, 98, &[]))
}

/// "Files this change touches (3):" and one line per file.
fn listing(files: &[ChangedSides]) -> String {
    let mut text = format!("Files this change touches ({}):\n", files.len());
    for file in files {
        let status = plan::status_text(file);
        let _ = match plan::skip_reason(file) {
            Some(reason) => writeln!(
                text,
                "- {} ({status}, {})",
                file.path,
                plan::skip_text(reason)
            ),
            None => writeln!(text, "- {} ({status})", file.path),
        };
    }
    text
}

/// The question of call `index` (0-based) of `calls`.
fn prompt(listing: &str, calls: usize, index: u32, files: &[FileInput]) -> String {
    let mut text = String::from("Review this change.\n\n");
    text.push_str(listing);
    text.push('\n');
    if calls > 1 {
        let _ = writeln!(
            text,
            "The change is too large to read at once, so it is read in {calls} calls. This is \
             call {} of {calls}; it shows {} of the changed files. The others are listed above so \
             you know what else changed, but their code is not shown here.\n",
            index + 1,
            files.len()
        );
    }
    text.push_str(
        "Diffs of the files shown in this call. Each line is a marker (\"+\" added, \"-\" \
         removed, a space for unchanged), the line number on the new side (blank for removed \
         lines), \"|\", then the code. Cite only these new-side line numbers.\n\n",
    );
    for file in files {
        text.push_str(&file.text);
        text.push('\n');
    }
    text
}

fn owned_lines(text: &str) -> Vec<String> {
    review_state::doc_lines(text)
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::ai::{ModelAnswer, ModelProvider};
    use crate::insights::findings::tests::answer_item;
    use crate::model::{
        ChangeSet, CommentKind, CommentState, ExcerptLine, FindingCategory, ModelInfo, ReviewRef,
        ReviewTarget, SkipReason,
    };
    use crate::projects::ProjectService;
    use crate::test_support::{isolated_git, TestRepo};

    /// Answers each question with the next of its answers, keeping the
    /// questions.
    #[derive(Default)]
    struct FakeModel {
        answers: Mutex<VecDeque<Result<Value, ModelError>>>,
        asked: Mutex<Vec<ModelRequest>>,
    }

    impl FakeModel {
        fn answering(answers: Vec<Result<Value, ModelError>>) -> Arc<Self> {
            Arc::new(Self {
                answers: Mutex::new(answers.into()),
                asked: Mutex::default(),
            })
        }

        fn asked(&self) -> Vec<ModelRequest> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl ModelProvider for FakeModel {
        fn ask(
            &self,
            request: &ModelRequest,
            _cancel: &AtomicBool,
        ) -> Result<ModelAnswer, ModelError> {
            self.asked.lock().unwrap().push(request.clone());
            let answer = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(json!({ "findings": [] })));
            answer.map(|value| ModelAnswer {
                value,
                provider: ModelProviderKind::Anthropic,
                model: "claude-opus-5-5-20261101".to_string(),
                usage: None,
            })
        }

        fn models(&self) -> Result<Vec<ModelInfo>, ModelError> {
            Ok(Vec::new())
        }
    }

    const SERVICE_BEFORE: &str = "package refund\n\
        \n\
        func (s *Service) Request(ctx context.Context, id string, amt int64) error {\n\
        \tp, err := s.payments.Get(ctx, id)\n\
        \tif err != nil {\n\
        \t\treturn err\n\
        \t}\n\
        \treturn s.refunds.Save(ctx, Refund{PaymentID: id, Amount: amt})\n\
        }\n";

    /// Lines 8 to 12 call the gateway, then save.
    const SERVICE_AFTER: &str = "package refund\n\
        \n\
        func (s *Service) Request(ctx context.Context, id string, amt int64) error {\n\
        \tp, err := s.payments.Get(ctx, id)\n\
        \tif err != nil {\n\
        \t\treturn err\n\
        \t}\n\
        \tref, err := s.gateway.Refund(ctx, p.ChargeID, amt)\n\
        \tif err != nil {\n\
        \t\treturn fmt.Errorf(\"gateway refund: %w\", err)\n\
        \t}\n\
        \treturn s.refunds.Save(ctx, Refund{PaymentID: id, Ref: ref, Amount: amt})\n\
        }\n";

    const TITLE: &str = "Refund can be paid out and never recorded";

    struct Fixture {
        repo: TestRepo,
        projects: ProjectService,
        id: String,
        model: Arc<FakeModel>,
        models: ModelService,
        insights: InsightsService,
    }

    impl Fixture {
        /// `feature` changes refund/service.go, adds its test and a binary
        /// logo, and deletes gone.txt, on top of `main`.
        fn new(answers: Vec<Result<Value, ModelError>>) -> Self {
            let repo = TestRepo::init();
            repo.write("refund/service.go", SERVICE_BEFORE);
            repo.write("gone.txt", "bye\n");
            repo.commit_all("initial commit");
            repo.git(&["checkout", "--quiet", "-b", "feature"]);
            repo.write("refund/service.go", SERVICE_AFTER);
            repo.write(
                "refund/service_test.go",
                "package refund\n\nfunc TestRequest(t *testing.T) {}\n",
            );
            repo.write("assets/logo.png", [0x89u8, b'P', b'N', b'G', 0, 0, 1, 2]);
            fs::remove_file(repo.path().join("gone.txt")).unwrap();
            repo.commit_all("call the gateway");
            let projects =
                ProjectService::new(Store::open_in_memory().unwrap(), isolated_git(), None);
            let id = projects
                .add_local(repo.path().to_str().unwrap())
                .unwrap()
                .id;
            let model = FakeModel::answering(answers);
            let models = ModelService::answered_by(
                Arc::clone(&model) as Arc<dyn ModelProvider>,
                ModelProviderKind::Anthropic,
                "claude-opus-5-5",
            );
            Self {
                repo,
                projects,
                id,
                model,
                models,
                insights: InsightsService::new(),
            }
        }

        /// The review as the screen would open it now.
        fn review(&self) -> ReviewRef {
            let ChangeSet {
                target,
                old_sha,
                new_sha,
                ..
            } = self
                .projects
                .changed_files(
                    &self.id,
                    ReviewTarget::Branch {
                        branch: "feature".to_string(),
                        base: "main".to_string(),
                    },
                )
                .unwrap();
            ReviewRef {
                target,
                old_sha,
                new_sha,
            }
        }

        fn session(&self) -> ReviewSession<'_> {
            ReviewSession::new(
                self.projects.store(),
                self.projects.git(),
                &self.id,
                self.repo.path(),
                self.review(),
            )
            .unwrap()
        }

        fn read(&self) -> Insights {
            self.insights.insights(&self.session()).unwrap()
        }

        fn start(&self) -> InsightsAnalysis {
            self.start_in(Language::English)
        }

        fn start_in(&self, language: Language) -> InsightsAnalysis {
            self.insights
                .start(&self.session(), &self.models, language)
                .unwrap()
                .analysis
                .unwrap()
        }

        fn plan_translation(&self, language: Language) -> TranslationPlan {
            self.insights
                .plan_translation(&self.session(), &self.models, language)
                .unwrap()
        }

        fn translate(&self, language: Language, ids: &[String]) -> AppResult<Insights> {
            self.insights
                .translate(&self.session(), &self.models, language, ids, "translate-1")
        }

        fn run(&self, analysis: &InsightsAnalysis, index: u32) -> Insights {
            self.insights
                .run_batch(
                    &self.session(),
                    &self.models,
                    &analysis.id,
                    index,
                    &format!("run-{index}"),
                )
                .unwrap()
        }

        /// Starts an analysis and runs its only call.
        fn analyze(&self) -> Insights {
            let analysis = self.start();
            assert_eq!(analysis.batches.len(), 1);
            self.run(&analysis, 0)
        }

        fn verdict(&self, finding: &Finding, verdict: FindingVerdict) -> Insights {
            self.insights
                .set_verdict(&self.session(), &finding.id, verdict)
                .unwrap()
        }

        fn fixed(&self, finding: &Finding, fixed: bool) -> Insights {
            self.insights
                .set_fixed(&self.session(), &finding.id, fixed)
                .unwrap()
        }

        /// Commits `text` as the new refund/service.go on `feature`.
        fn change_service(&self, text: &str) {
            self.repo.write("refund/service.go", text);
            self.repo.commit_all("change the service");
        }
    }

    fn found(finding: &Finding) -> (u32, u32, bool) {
        match finding.location {
            FindingLocation::Found {
                line,
                end_line,
                moved,
            } => (line, end_line, moved),
            FindingLocation::NotFound { problem } => panic!("not found: {problem:?}"),
        }
    }

    fn findings(answer: Vec<Value>) -> Result<Value, ModelError> {
        Ok(json!({ "findings": answer, "earlier": [] }))
    }

    /// An answer with new findings and what it says of the earlier ones.
    fn rechecked(answer: Vec<Value>, earlier: Vec<Value>) -> Result<Value, ModelError> {
        Ok(json!({ "findings": answer, "earlier": earlier }))
    }

    fn recheck(reference: &str, status: &str, line: u32, end_line: u32) -> Value {
        json!({
            "ref": reference,
            "status": status,
            "reason": format!("{reference} is {status}."),
            "line": line,
            "end_line": end_line
        })
    }

    fn by_title<'a>(insights: &'a Insights, title: &str) -> &'a Finding {
        insights
            .findings
            .iter()
            .find(|finding| finding.title == title)
            .unwrap_or_else(|| panic!("no finding {title:?}"))
    }

    const GATEWAY: &str = "Gateway error loses the charge id";

    #[test]
    fn the_plan_lists_what_is_sent_and_what_is_not_and_asks_nothing() {
        let fixture = Fixture::new(Vec::new());
        let plan = fixture
            .insights
            .plan(&fixture.session(), &fixture.models)
            .unwrap();
        assert_eq!(plan.files, 2);
        assert_eq!(
            plan.skipped,
            [
                SkippedFile {
                    path: "assets/logo.png".to_string(),
                    reason: SkipReason::Binary
                },
                SkippedFile {
                    path: "gone.txt".to_string(),
                    reason: SkipReason::Deleted
                },
            ]
        );
        assert_eq!(plan.calls.len(), 1);
        assert_eq!(
            plan.calls[0].paths,
            ["refund/service.go", "refund/service_test.go"]
        );
        assert!(plan.tokens > plan::estimate_tokens(SYSTEM_PROMPT));
        assert_eq!(plan.tokens, plan.calls[0].tokens);
        assert_eq!(plan.model.provider_name, "Anthropic API");
        assert_eq!(plan.model.parallel_calls, 3);
        assert!(plan.model.ready);
        // Nothing was asked, and nothing was recorded.
        assert!(fixture.model.asked().is_empty());
        assert_eq!(
            fixture
                .projects
                .store()
                .review(&fixture.id, &fixture.review().target)
                .unwrap(),
            None
        );
        assert_eq!(fixture.read().analysis, None);
    }

    #[test]
    fn starting_an_analysis_records_its_calls_without_asking_the_model() {
        let fixture = Fixture::new(Vec::new());
        let analysis = fixture.start();
        assert_eq!(analysis.provider, ModelProviderKind::Anthropic);
        assert_eq!(analysis.model, "claude-opus-5-5");
        assert_eq!(analysis.head_sha, fixture.review().new_sha);
        assert_eq!(analysis.batches[0].status, InsightsBatchStatus::Pending);
        assert!(fixture.model.asked().is_empty());
        assert_eq!(fixture.read().analysis, Some(analysis));
    }

    #[test]
    fn insights_ask_the_model_of_their_task_and_translating_the_one_of_its_own() {
        let mut fixture = Fixture::new(Vec::new());
        // The default model must not be asked.
        fixture.models = ModelService::answered_by(
            Arc::new(crate::test_support::NeverAsked),
            ModelProviderKind::ClaudeCli,
            "opus",
        )
        .with_task(
            ModelTask::Insights,
            Arc::clone(&fixture.model) as Arc<dyn ModelProvider>,
            ModelProviderKind::OpenAi,
            "gpt-5.5",
        );
        let plan = fixture
            .insights
            .plan(&fixture.session(), &fixture.models)
            .unwrap();
        assert_eq!(
            (plan.model.provider, plan.model.model.as_deref()),
            (ModelProviderKind::OpenAi, Some("gpt-5.5"))
        );
        let analysis = fixture.start();
        assert_eq!(
            (
                analysis.provider,
                analysis.model.as_str(),
                analysis.parallel_calls
            ),
            (ModelProviderKind::OpenAi, "gpt-5.5", 3)
        );
        fixture.run(&analysis, 0);
        assert_eq!(fixture.model.asked().len(), 1);
        // Translating is a task of its own, here on the default.
        assert_eq!(
            fixture
                .plan_translation(Language::BrazilianPortuguese)
                .model
                .provider,
            ModelProviderKind::ClaudeCli
        );
    }

    #[test]
    fn a_call_sends_the_diffs_with_numbered_lines_and_the_list_of_changed_files() {
        let fixture = Fixture::new(Vec::new());
        fixture.analyze();
        let asked = fixture.model.asked();
        assert_eq!(asked.len(), 1);
        let request = &asked[0];
        assert_eq!(request.system, SYSTEM_PROMPT);
        assert!(request.system.contains("Finding nothing is a valid answer"));
        assert_eq!(request.schema_name, "review_findings");
        assert_eq!(request.schema, findings::schema());
        let prompt = &request.prompt;
        assert!(
            prompt.contains("Files this change touches (4):\n"),
            "{prompt}"
        );
        assert!(prompt.contains("- assets/logo.png (added, binary, not shown)\n"));
        assert!(prompt.contains("- gone.txt (deleted, not shown)\n"));
        assert!(prompt.contains("=== refund/service.go (modified) ===\n@@ -1,9 +1,13 @@\n"));
        assert!(prompt.contains("+    8 | \tref, err := s.gateway.Refund(ctx, p.ChargeID, amt)\n"));
        assert!(prompt.contains(
            "-      | \treturn s.refunds.Save(ctx, Refund{PaymentID: id, Amount: amt})\n"
        ));
        assert!(prompt.contains("=== refund/service_test.go (added) ===\n"));
        assert!(!prompt.contains("bye"));
        // One call: nothing says the change is split.
        assert!(!prompt.contains("calls"));
    }

    #[test]
    fn keeps_what_the_model_found_and_marks_what_it_cannot_point_at() {
        let fixture = Fixture::new(vec![findings(vec![
            answer_item(TITLE, "refund/service.go", 8, 12),
            answer_item("Elsewhere", "refund/other.go", 3, 3),
            answer_item("Past the end", "refund/service.go", 40, 41),
            answer_item(
                "  refund CAN be paid out and never recorded.",
                "./refund/service.go",
                12,
                12,
            ),
        ])]);
        let insights = fixture.analyze();

        let analysis = insights.analysis.unwrap();
        assert_eq!(analysis.batches[0].status, InsightsBatchStatus::Done);
        assert_eq!(analysis.model, "claude-opus-5-5-20261101");
        // The same finding twice is kept once.
        let titles: Vec<&str> = insights
            .findings
            .iter()
            .map(|finding| finding.title.as_str())
            .collect();
        assert_eq!(titles, [TITLE, "Elsewhere", "Past the end"]);

        let first = &insights.findings[0];
        assert_eq!(found(first), (8, 12, false));
        assert_eq!(first.category, FindingCategory::Correctness);
        assert_eq!(first.verdict, FindingVerdict::Pending);
        assert_eq!(first.source.provider_name, "Anthropic API");
        assert!(!first.source.earlier);
        assert_eq!(
            first.excerpt.first(),
            Some(&ExcerptLine {
                line: 6,
                text: "\t\treturn err".to_string(),
                cited: false
            })
        );
        assert_eq!(first.excerpt.iter().filter(|line| line.cited).count(), 5);
        assert_eq!(first.excerpt.last().unwrap().line, 14);
        assert_eq!(
            insights.findings[1].location,
            FindingLocation::NotFound {
                problem: LocationProblem::NotChanged
            }
        );
        assert!(insights.findings[1].excerpt.is_empty());
        assert_eq!(
            insights.findings[2].location,
            FindingLocation::NotFound {
                problem: LocationProblem::OutsideFile
            }
        );
        // Read again later, it is all still there.
        assert_eq!(fixture.read().findings, insights.findings);
    }

    #[test]
    fn a_failed_call_keeps_its_error_until_a_retry_answers() {
        let fixture = Fixture::new(vec![
            Err(ModelError::RateLimited {
                provider: ModelProviderKind::Anthropic,
                detail: None,
                retry_after: Some("20".to_string()),
            }),
            findings(vec![answer_item(TITLE, "refund/service.go", 8, 12)]),
        ]);
        let analysis = fixture.start();
        let failed = fixture.run(&analysis, 0);
        let batch = &failed.analysis.as_ref().unwrap().batches[0];
        assert_eq!(batch.status, InsightsBatchStatus::Failed);
        let error = batch.error.as_ref().unwrap();
        assert_eq!(error.code, ErrorCode::RateLimited);
        assert!(
            error.message.contains("try again in 20 seconds"),
            "{}",
            error.message
        );
        assert!(failed.findings.is_empty());

        let retried = fixture.run(&analysis, 0);
        assert_eq!(
            retried.analysis.unwrap().batches[0].status,
            InsightsBatchStatus::Done
        );
        assert_eq!(retried.findings.len(), 1);
        // A call that answered is not asked again.
        fixture.run(&analysis, 0);
        assert_eq!(fixture.model.asked().len(), 2);
    }

    #[test]
    fn a_cancelled_call_reads_as_stopped() {
        let fixture = Fixture::new(vec![Err(ModelError::Cancelled)]);
        let analysis = fixture.start();
        let insights = fixture.run(&analysis, 0);
        let error = insights.analysis.unwrap().batches[0].error.clone().unwrap();
        assert_eq!(
            (error.code, error.message.as_str()),
            (ErrorCode::Cancelled, "Stopped.")
        );
    }

    #[test]
    fn a_new_analysis_rechecks_the_earlier_findings_and_deletes_none() {
        let fixture = Fixture::new(vec![
            findings(vec![
                answer_item(TITLE, "refund/service.go", 8, 12),
                answer_item(GATEWAY, "refund/service.go", 10, 10),
            ]),
            rechecked(
                vec![
                    // Said again: the confirmed one stands for it.
                    answer_item(TITLE, "refund/service.go", 8, 12),
                    answer_item("Retrying refunds twice", "refund/service.go", 12, 12),
                ],
                vec![
                    recheck("E1", "still_present", 8, 12),
                    recheck("E2", "fixed", 0, 0),
                ],
            ),
        ]);
        let first = fixture.analyze();
        fixture.verdict(by_title(&first, TITLE), FindingVerdict::Confirmed);

        let analysis = fixture.start();
        // Until the new analysis answers, the earlier findings stay as they were.
        assert!(fixture
            .read()
            .findings
            .iter()
            .all(|finding| finding.resolution.is_none()));
        let second = fixture.run(&analysis, 0);

        // The open earlier findings were listed with the code they cite now.
        let prompt = &fixture.model.asked()[1].prompt;
        assert!(prompt.contains("Earlier findings to re-check (2)."), "{prompt}");
        assert!(prompt.contains(&format!("- E1: refund/service.go:8-12\n  {TITLE}\n")));
        assert!(prompt.contains(&format!("- E2: refund/service.go:10\n  {GATEWAY}\n")));

        let shown: Vec<(&str, FindingVerdict, bool, Option<ResolutionKind>)> = second
            .findings
            .iter()
            .map(|finding| {
                (
                    finding.title.as_str(),
                    finding.verdict,
                    finding.source.earlier,
                    finding.resolution.as_ref().map(|resolution| resolution.kind),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("Retrying refunds twice", FindingVerdict::Pending, false, None),
                (TITLE, FindingVerdict::Confirmed, true, None),
                (
                    GATEWAY,
                    FindingVerdict::Pending,
                    true,
                    Some(ResolutionKind::ModelChecked)
                ),
            ]
        );
        // Fixed by the model that answered, with its reason; the verdict untouched.
        let fixed = by_title(&second, GATEWAY).resolution.clone().unwrap();
        assert_eq!(fixed.model.as_deref(), Some("claude-opus-5-5-20261101"));
        assert_eq!(fixed.reason.as_deref(), Some("E2 is fixed."));
        assert!(!fixed.at.is_empty());
        // Both analyses are kept: each owns findings.
        let analyses = fixture
            .projects
            .store()
            .analyses(&fixture.session().record().unwrap().id)
            .unwrap();
        assert_eq!(analyses.len(), 2);

        // A third analysis no longer asks about the fixed one.
        fixture.run(&fixture.start(), 0);
        let prompt = &fixture.model.asked()[2].prompt;
        assert!(prompt.contains("Earlier findings to re-check (2)."), "{prompt}");
        assert!(!prompt.contains(GATEWAY));
        assert!(prompt.contains("Retrying refunds twice"));
    }

    #[test]
    fn an_earlier_finding_still_present_is_kept_once_and_follows_the_lines_the_model_gives() {
        let fixture = Fixture::new(vec![
            findings(vec![answer_item(TITLE, "refund/service.go", 8, 12)]),
            rechecked(
                // Reported again as new despite the instruction: the same title on the
                // same lines is the earlier one.
                vec![answer_item(TITLE, "refund/service.go", 8, 12)],
                vec![recheck("E1", "still_present", 8, 12)],
            ),
        ]);
        let original = fixture.analyze().findings.remove(0);
        // The cited code changes: the finding can no longer be found by its text.
        fixture.change_service(&SERVICE_AFTER.replace("p.ChargeID, amt)", "p.ChargeID, amt, key)"));
        assert_eq!(
            fixture.read().findings[0].location,
            FindingLocation::NotFound {
                problem: LocationProblem::CodeChanged
            }
        );

        let second = fixture.run(&fixture.start(), 0);

        let prompt = &fixture.model.asked()[1].prompt;
        assert!(
            prompt.contains(
                "- E1: refund/service.go, cited at lines 8-12 of an earlier version; that code \
                 changed since\n"
            ),
            "{prompt}"
        );
        assert_eq!(second.findings.len(), 1);
        let kept = &second.findings[0];
        assert_eq!(kept.id, original.id);
        assert_eq!(kept.resolution, None);
        assert!(kept.source.earlier);
        // Placed again where the model pointed, on the code as it is now.
        assert_eq!(found(kept), (8, 12, false));
        assert!(kept
            .excerpt
            .iter()
            .any(|line| line.cited && line.text.contains("amt, key)")));
    }

    #[test]
    fn an_earlier_finding_the_answer_leaves_out_or_whose_call_fails_stays_as_it_was() {
        let fixture = Fixture::new(vec![
            findings(vec![answer_item(TITLE, "refund/service.go", 8, 12)]),
            Err(ModelError::Cancelled),
            // Answers about a finding it was not asked about, and leaves E1 out.
            rechecked(Vec::new(), vec![recheck("E9", "fixed", 0, 0)]),
        ]);
        let original = fixture.analyze().findings.remove(0);

        let analysis = fixture.start();
        let failed = fixture.run(&analysis, 0);
        assert_eq!(
            failed.analysis.unwrap().batches[0].status,
            InsightsBatchStatus::Failed
        );
        let after_failure = failed.findings[0].clone();
        assert_eq!(after_failure.id, original.id);
        assert_eq!(after_failure.resolution, None);
        assert_eq!(after_failure.location, original.location);

        let answered = fixture.run(&analysis, 0);
        assert_eq!(answered.findings.len(), 1);
        assert_eq!(answered.findings[0].id, original.id);
        assert_eq!(answered.findings[0].resolution, None);
        assert_eq!(answered.findings[0].location, original.location);
        assert_eq!(answered.findings[0].verdict, FindingVerdict::Pending);
    }

    #[test]
    fn an_earlier_finding_whose_file_left_the_change_is_fixed_without_the_model() {
        let fixture = Fixture::new(vec![findings(vec![
            answer_item(TITLE, "refund/service.go", 8, 12),
            answer_item("Test asserts nothing", "refund/service_test.go", 3, 3),
            // Never a changed file: it did not leave the change.
            answer_item("Elsewhere", "refund/other.go", 3, 3),
        ])]);
        fixture.analyze();
        // service.go goes back to what main has: it is no longer part of the change.
        fixture.change_service(SERVICE_BEFORE);

        fixture.start();
        let started = fixture.read();
        let left = by_title(&started, TITLE).resolution.clone().unwrap();
        assert_eq!(
            (left.kind, left.model, left.reason),
            (ResolutionKind::FileLeftChange, None, None)
        );
        assert_eq!(by_title(&started, "Test asserts nothing").resolution, None);
        assert_eq!(by_title(&started, "Elsewhere").resolution, None);
        // No model was asked for it: starting asks none.
        assert_eq!(fixture.model.asked().len(), 1);
    }

    #[test]
    fn dismissed_and_fixed_findings_are_not_sent_for_recheck() {
        let fixture = Fixture::new(vec![
            findings(vec![
                answer_item(TITLE, "refund/service.go", 8, 12),
                answer_item(GATEWAY, "refund/service.go", 10, 10),
            ]),
            // Nothing was listed: references are not taken.
            rechecked(Vec::new(), vec![recheck("E1", "fixed", 0, 0)]),
        ]);
        let first = fixture.analyze();
        fixture.verdict(by_title(&first, TITLE), FindingVerdict::Dismissed);
        fixture.fixed(by_title(&first, GATEWAY), true);

        let second = fixture.run(&fixture.start(), 0);

        let prompt = &fixture.model.asked()[1].prompt;
        assert!(!prompt.contains("Earlier findings to re-check"), "{prompt}");
        let dismissed = by_title(&second, TITLE);
        assert_eq!(
            (dismissed.verdict, dismissed.resolution.as_ref()),
            (FindingVerdict::Dismissed, None)
        );
        assert_eq!(
            by_title(&second, GATEWAY).resolution.as_ref().unwrap().kind,
            ResolutionKind::UserMarked
        );
    }

    #[test]
    fn the_user_marks_a_finding_fixed_and_reopens_it_without_asking_the_model() {
        let fixture = Fixture::new(vec![findings(vec![answer_item(
            TITLE,
            "refund/service.go",
            8,
            12,
        )])]);
        let finding = fixture.analyze().findings.remove(0);
        let confirmed = fixture
            .verdict(&finding, FindingVerdict::Confirmed)
            .findings
            .remove(0);
        // The code near it changes: a confirmed finding would be checked again...
        fixture.change_service(&SERVICE_AFTER.replace(
            "\t\treturn err\n",
            "\t\treturn fmt.Errorf(\"load payment: %w\", err)\n",
        ));
        assert!(fixture.read().findings[0].recheck);

        let fixed = fixture.fixed(&confirmed, true).findings.remove(0);
        let marked = fixed.resolution.clone().unwrap();
        assert_eq!(
            (marked.kind, marked.model.as_deref(), marked.reason.as_deref()),
            (ResolutionKind::UserMarked, None, None)
        );
        assert_eq!(fixed.verdict, FindingVerdict::Confirmed);
        // ...but a fixed one is not.
        assert!(!fixed.recheck);
        // Marked again, it keeps when it was first marked.
        assert_eq!(
            fixture.fixed(&fixed, true).findings[0].resolution,
            Some(marked)
        );

        let reopened = fixture.fixed(&fixed, false).findings.remove(0);
        assert_eq!(reopened.resolution, None);
        assert_eq!(reopened.verdict, FindingVerdict::Confirmed);
        assert!(reopened.recheck);
        assert_eq!(fixture.model.asked().len(), 1);

        let error = fixture
            .insights
            .set_fixed(&fixture.session(), "missing", true)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::UnknownFinding);
    }

    #[test]
    fn a_call_of_a_replaced_analysis_is_refused() {
        let fixture = Fixture::new(Vec::new());
        let old = fixture.start();
        fixture.start();
        let error = fixture
            .insights
            .run_batch(&fixture.session(), &fixture.models, &old.id, 0, "run-old")
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::UnknownFinding);
        assert!(fixture.model.asked().is_empty());
    }

    #[test]
    fn a_verdict_follows_its_code_and_asks_for_a_recheck_once_it_changes() {
        let fixture = Fixture::new(vec![findings(vec![answer_item(
            TITLE,
            "refund/service.go",
            8,
            12,
        )])]);
        let finding = fixture.analyze().findings.remove(0);
        let confirmed = fixture
            .verdict(&finding, FindingVerdict::Confirmed)
            .findings
            .remove(0);
        assert_eq!(confirmed.verdict, FindingVerdict::Confirmed);
        assert!(confirmed.verdict_at.is_some());
        assert!(!confirmed.recheck);

        // Two lines above it: the finding moves with its code, unchanged.
        let moved = SERVICE_AFTER.replace("package refund\n", "package refund\n\n// Refunds.\n");
        fixture.change_service(&moved);
        let shifted = fixture.read().findings.remove(0);
        assert_eq!(found(&shifted), (10, 14, true));
        assert!(!shifted.recheck);
        assert_eq!(
            shifted.excerpt.iter().find(|line| line.cited).unwrap().line,
            10
        );

        // A line next to it changes: check it again.
        let near = moved.replace(
            "\t\treturn err\n",
            "\t\treturn fmt.Errorf(\"load payment: %w\", err)\n",
        );
        fixture.change_service(&near);
        let changed = fixture.read().findings.remove(0);
        assert_eq!(found(&changed), (10, 14, true));
        assert!(changed.recheck);

        // Classified again, it is taken against the code as it is now.
        let again = fixture
            .verdict(&changed, FindingVerdict::Dismissed)
            .findings
            .remove(0);
        assert_eq!(again.verdict, FindingVerdict::Dismissed);
        assert!(!again.recheck);

        // The cited code itself changed: not found, and to be checked.
        fixture.change_service(&near.replace("p.ChargeID, amt)", "p.ChargeID, amt, key)"));
        let gone = fixture.read().findings.remove(0);
        assert_eq!(
            gone.location,
            FindingLocation::NotFound {
                problem: LocationProblem::CodeChanged
            }
        );
        assert!(gone.recheck);

        // Back to pending, nothing is to be rechecked.
        let pending = fixture
            .verdict(&gone, FindingVerdict::Pending)
            .findings
            .remove(0);
        assert_eq!(
            (pending.verdict, pending.recheck, pending.verdict_at),
            (FindingVerdict::Pending, false, None)
        );
    }

    #[test]
    fn an_answer_that_arrives_after_a_newer_analysis_started_is_dropped() {
        let fixture = Fixture::new(vec![findings(vec![answer_item(
            TITLE,
            "refund/service.go",
            8,
            12,
        )])]);
        fixture.analyze();
        let late = fixture.start();
        fixture.start();

        let sent = HashMap::new();
        let changed = HashSet::new();
        let value = json!({ "findings": [answer_item("Late", "refund/service.go", 1, 1)] });
        let review_id = fixture.session().record().unwrap().id;
        fixture
            .insights
            .keep(
                fixture.projects.store(),
                &review_id,
                &late.id,
                0,
                &Answer {
                    value: &value,
                    model: "m",
                    sent: &sent,
                    changed: &changed,
                    earlier: &[],
                },
            )
            .unwrap();

        // The first analysis's findings stand until the newest one answers.
        let titles: Vec<String> = fixture
            .read()
            .findings
            .into_iter()
            .map(|finding| finding.title)
            .collect();
        assert_eq!(titles, [TITLE]);
    }

    #[test]
    fn the_core_runs_the_calls_of_an_analysis_and_reports_each() {
        let fixture = Fixture::new(vec![findings(vec![
            answer_item(TITLE, "refund/service.go", 8, 12),
            answer_item("Gateway error loses the charge id", "refund/service.go", 10, 10),
        ])]);
        let analysis = fixture.start();
        let reported = Mutex::new(Vec::new());
        let calls = Calls::new("full-1");
        let ends = fixture.insights.run_calls(
            &fixture.session(),
            &fixture.models,
            &calls,
            &analysis.id,
            &[0],
            3,
            &|index, end| {
                reported
                    .lock()
                    .unwrap()
                    .push((index, end.map(|end| end.clone().unwrap())));
            },
        );
        assert_eq!(ends, [Some(Ok(CallEnd::Answered { findings: 2 }))]);
        assert_eq!(
            reported.into_inner().unwrap(),
            [(0, None), (0, Some(CallEnd::Answered { findings: 2 }))]
        );
        assert_eq!(fixture.read().findings.len(), 2);

        // A cancelled piece of work starts no call: it stays pending.
        let analysis = fixture.start();
        let calls = Calls::new("full-2");
        calls.cancel();
        let ends = fixture.insights.run_calls(
            &fixture.session(),
            &fixture.models,
            &calls,
            &analysis.id,
            &[0],
            1,
            &|_, _| {},
        );
        assert_eq!(ends, [None]);
        assert_eq!(fixture.model.asked().len(), 1);
        assert_eq!(
            fixture.read().analysis.unwrap().batches[0].status,
            InsightsBatchStatus::Pending
        );
    }

    #[test]
    fn a_split_change_tells_each_call_what_it_shows() {
        let text = prompt("Files this change touches (9):\n", 3, 1, &[]);
        assert!(
            text.contains("read in 3 calls. This is call 2 of 3; it shows 0 of the changed files.")
        );
    }

    fn portuguese_answer(id: &str) -> Result<Value, ModelError> {
        Ok(json!({ "findings": [{
            "id": id,
            "title": "O reembolso pode ser pago e nunca registrado",
            "explanation": "O gateway paga antes de o reembolso ser gravado.",
            "suggested_direction": "Grave o reembolso como pendente antes.",
            "evidence": ["A linha 8 pede o reembolso; a linha 12 só o grava depois."],
            "related": []
        }]}))
    }

    #[test]
    fn an_analysis_is_asked_in_the_language_shown_and_keeps_it() {
        let fixture = Fixture::new(vec![findings(vec![answer_item(
            TITLE,
            "refund/service.go",
            8,
            12,
        )])]);
        let analysis = fixture.start_in(Language::BrazilianPortuguese);
        assert_eq!(analysis.language, Language::BrazilianPortuguese);
        let insights = fixture.run(&analysis, 0);

        let asked = fixture.model.asked();
        assert!(asked[0]
            .prompt
            .starts_with("Write every prose field of your answer in Brazilian Portuguese.\n\n"));
        assert!(asked[0].system.contains("## Language"));
        assert!(asked[0]
            .system
            .contains("Keep exactly as they are, whatever the language: the JSON keys"));
        assert_eq!(insights.findings[0].language, Language::BrazilianPortuguese);
        assert!(insights.findings[0].translations.is_empty());

        // An English analysis says so too.
        let fixture = Fixture::new(Vec::new());
        fixture.run(&fixture.start(), 0);
        assert!(fixture.model.asked()[0]
            .prompt
            .starts_with("Write every prose field of your answer in English.\n\n"));
    }

    #[test]
    fn translating_adds_a_version_of_the_text_and_keeps_the_findings() {
        let fixture = Fixture::new(vec![findings(vec![answer_item(
            TITLE,
            "refund/service.go",
            8,
            12,
        )])]);
        let analyzed = fixture.analyze();
        let original = analyzed.findings[0].clone();
        let confirmed = fixture.verdict(&original, FindingVerdict::Confirmed);
        let original = confirmed.findings[0].clone();

        // Reading and planning ask nothing.
        let plan = fixture.plan_translation(Language::BrazilianPortuguese);
        assert_eq!(plan.items, 1);
        assert_eq!(plan.calls.len(), 1);
        assert_eq!(plan.calls[0].ids, vec![original.id.clone()]);
        assert!(plan.tokens > 0 && plan.tokens == plan.calls[0].tokens);
        assert_eq!(plan.model.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(fixture.plan_translation(Language::English).items, 0);
        assert_eq!(fixture.model.asked().len(), 1);

        fixture
            .model
            .answers
            .lock()
            .unwrap()
            .push_back(portuguese_answer(&original.id));
        let translated = fixture
            .translate(Language::BrazilianPortuguese, &plan.calls[0].ids)
            .unwrap();

        let asked = fixture.model.asked();
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[1].system, translation::SYSTEM_PROMPT);
        assert_eq!(asked[1].schema_name, translation::FINDINGS_SCHEMA_NAME);
        assert_eq!(
            asked[1].schema["properties"]["findings"]["items"]["properties"]["id"]["enum"],
            json!([original.id])
        );
        assert!(asked[1].prompt.contains(TITLE));
        assert!(!asked[1].prompt.contains("ref, err := s.gateway.Refund"));

        // The same finding, verdict and place; only one more version of its text.
        let finding = &translated.findings[0];
        assert_eq!(finding.id, original.id);
        assert_eq!(finding.title, TITLE);
        assert_eq!(finding.language, Language::English);
        assert_eq!(finding.verdict, FindingVerdict::Confirmed);
        assert_eq!(finding.location, original.location);
        assert_eq!(finding.translations.len(), 1);
        let version = &finding.translations[0];
        assert_eq!(version.language, Language::BrazilianPortuguese);
        assert_eq!(version.title, "O reembolso pode ser pago e nunca registrado");
        assert_eq!(
            version.evidence,
            vec!["A linha 8 pede o reembolso; a linha 12 só o grava depois.".to_string()]
        );

        // Kept: reading shows it with no call, and nothing is left to translate.
        assert_eq!(fixture.read().findings[0].translations, finding.translations);
        assert_eq!(fixture.plan_translation(Language::BrazilianPortuguese).items, 0);
        fixture
            .translate(Language::BrazilianPortuguese, std::slice::from_ref(&original.id))
            .unwrap();
        assert_eq!(fixture.model.asked().len(), 2);
    }

    #[test]
    fn a_failed_or_stopped_translation_keeps_nothing() {
        let fixture = Fixture::new(vec![
            findings(vec![answer_item(TITLE, "refund/service.go", 8, 12)]),
            Err(ModelError::Cancelled),
            Ok(json!({ "findings": [{ "id": "not-sent", "title": "x", "explanation": "y",
                "suggested_direction": "", "evidence": [], "related": [] }] })),
        ]);
        let id = fixture.analyze().findings[0].id.clone();

        let stopped = fixture
            .translate(Language::BrazilianPortuguese, std::slice::from_ref(&id))
            .unwrap_err();
        assert_eq!(stopped.code, ErrorCode::Cancelled);
        let invented = fixture
            .translate(Language::BrazilianPortuguese, std::slice::from_ref(&id))
            .unwrap_err();
        assert_eq!(invented.code, ErrorCode::ModelInvalidOutput);
        assert!(fixture.read().findings[0].translations.is_empty());
        assert_eq!(fixture.plan_translation(Language::BrazilianPortuguese).items, 1);
    }

    fn comment_answer() -> Value {
        json!({
            "problem": "The gateway pays the refund before `Save` records it",
            "impact": "If `Save` fails, the money is gone with no record.",
            "suggestion": "Save the refund as pending before calling the gateway",
            "suggestion_code": ""
        })
    }

    impl Fixture {
        /// Writes the comment of the finding `id` on a click, `feature`
        /// adding lines 8 to 12 of refund/service.go.
        fn write_comment(&self, id: &str, language: Language) -> AppResult<SignalDraft> {
            let changed = ChangedLines::default().with_added("refund/service.go", 8, 12);
            self.insights
                .write_comment(&self.session(), &self.models, &changed, id, language, "comment-1")
        }

        fn answer_next(&self, answer: Result<Value, ModelError>) {
            self.model.answers.lock().unwrap().push_back(answer);
        }
    }

    #[test]
    fn a_comment_from_a_finding_is_written_by_the_insights_model_once_and_kept_as_a_line_draft() {
        let mut fixture = Fixture::new(vec![
            findings(vec![answer_item(TITLE, "refund/service.go", 8, 12)]),
            Ok(comment_answer()),
        ]);
        // The default model must not be asked: the Insights task has its own.
        fixture.models = ModelService::answered_by(
            Arc::new(crate::test_support::NeverAsked),
            ModelProviderKind::ClaudeCli,
            "opus",
        )
        .with_task(
            ModelTask::Insights,
            Arc::clone(&fixture.model) as Arc<dyn ModelProvider>,
            ModelProviderKind::OpenAi,
            "gpt-5.5",
        );
        let finding = fixture.analyze().findings.remove(0);

        let written = fixture.write_comment(&finding.id, Language::English).unwrap();

        let asked = fixture.model.asked();
        assert_eq!(asked.len(), 2);
        let request = &asked[1];
        assert_eq!(request.schema_name, "finding_comment");
        assert_eq!(request.system, comment::SYSTEM_PROMPT);
        assert!(request
            .prompt
            .starts_with("Write every prose field of your answer in English.\n\n"));
        assert!(request.prompt.contains("branch feature compared with main."));
        assert!(request.prompt.contains(&format!("Title: {TITLE}\n")));
        assert!(request.prompt.contains("Severity: high. Category: correctness.\n"));
        assert!(request.prompt.contains("Where: refund/service.go:8-12\n"));
        assert!(request
            .prompt
            .contains("Explanation: The gateway refund runs first and the row is saved afterwards.\n"));
        assert!(request
            .prompt
            .contains("Suggested direction: Save the refund as pending before calling the gateway.\n"));
        // The finding's lines and two around them, those the change adds marked.
        assert!(request.prompt.contains(
            "=== refund/service.go, lines 6-14 ===\n      6 | \t\treturn err\n      7 | \t}\n+     \
             8 | \tref, err := s.gateway.Refund(ctx, p.ChargeID, amt)\n"
        ));
        assert!(request
            .prompt
            .contains("+    12 | \treturn s.refunds.Save(ctx, Refund{PaymentID: id, Ref: ref, Amount: amt})\n     13 | }\n"));

        assert_eq!(written.signal, format!("finding:{}", finding.id));
        assert_eq!(written.kind, CommentKind::Line);
        let state = fixture.session().state(&[]).unwrap();
        assert_eq!(state.signal_drafts, vec![written.clone()]);
        let comment = &state.comments[0];
        assert_eq!((comment.path.as_str(), comment.line), ("refund/service.go", 12));
        assert_eq!(comment.state, CommentState::Draft);
        assert_eq!(
            comment.body,
            "The gateway pays the refund before `Save` records it. If `Save` fails, the money is \
             gone with no record.\n\n**Suggestion:** Save the refund as pending before calling the \
             gateway."
        );

        // A second click finds the draft and asks nothing.
        let again = fixture.write_comment(&finding.id, Language::English).unwrap();
        assert_eq!(again, written);
        assert_eq!(fixture.model.asked().len(), 2);
        assert_eq!(fixture.session().state(&[]).unwrap().comments.len(), 1);

        // Deleted, the next click writes a new one.
        fixture.answer_next(Ok(comment_answer()));
        review_state::delete_comment(fixture.projects.store(), &fixture.id, &written.comment_id)
            .unwrap();
        let fresh = fixture.write_comment(&finding.id, Language::English).unwrap();
        assert_ne!(fresh.comment_id, written.comment_id);
        assert_eq!(fixture.model.asked().len(), 3);
    }

    #[test]
    fn a_comment_is_written_in_the_language_asked_from_the_finding_as_translated_into_it() {
        let fixture = Fixture::new(vec![findings(vec![answer_item(
            TITLE,
            "refund/service.go",
            8,
            12,
        )])]);
        let id = fixture.analyze().findings[0].id.clone();
        fixture.answer_next(portuguese_answer(&id));
        fixture
            .translate(Language::BrazilianPortuguese, std::slice::from_ref(&id))
            .unwrap();
        fixture.answer_next(Ok(json!({
            "problem": "O gateway paga o reembolso antes de o `Save` gravá-lo",
            "impact": "",
            "suggestion": "Grave o reembolso como pendente antes",
            "suggestion_code": "if err := s.refunds.SavePending(ctx, r); err != nil {\n\treturn err\n}"
        })));

        fixture
            .write_comment(&id, Language::BrazilianPortuguese)
            .unwrap();

        let request = fixture.model.asked().pop().unwrap();
        assert!(request
            .prompt
            .starts_with("Write every prose field of your answer in Brazilian Portuguese.\n\n"));
        assert!(request
            .prompt
            .contains("Title: O reembolso pode ser pago e nunca registrado\n"));
        assert!(request
            .prompt
            .contains("Explanation: O gateway paga antes de o reembolso ser gravado.\n"));
        assert!(request
            .prompt
            .contains("Suggested direction: Grave o reembolso como pendente antes.\n"));
        assert_eq!(
            fixture.session().state(&[]).unwrap().comments[0].body,
            "O gateway paga o reembolso antes de o `Save` gravá-lo.\n\n**Sugestão:** Grave o \
             reembolso como pendente antes.\n```go\nif err := s.refunds.SavePending(ctx, r); err \
             != nil {\n\treturn err\n}\n```"
        );
    }

    #[test]
    fn a_comment_that_cannot_be_written_keeps_nothing() {
        let mut fixture = Fixture::new(vec![
            findings(vec![answer_item(TITLE, "refund/service.go", 8, 12)]),
            Ok(json!({ "problem": " ", "impact": "", "suggestion": "", "suggestion_code": "" })),
            Err(ModelError::Cancelled),
        ]);
        let id = fixture.analyze().findings[0].id.clone();
        let nothing_kept = |fixture: &Fixture| {
            let state = fixture.session().state(&[]).unwrap();
            assert!(state.comments.is_empty());
            assert!(state.signal_drafts.is_empty());
        };

        let empty = fixture.write_comment(&id, Language::English).unwrap_err();
        assert_eq!(empty.code, ErrorCode::ModelInvalidOutput);
        nothing_kept(&fixture);
        let stopped = fixture.write_comment(&id, Language::English).unwrap_err();
        assert_eq!(stopped.code, ErrorCode::Cancelled);
        nothing_kept(&fixture);
        assert_eq!(fixture.model.asked().len(), 3);

        let gone = fixture.write_comment("missing", Language::English).unwrap_err();
        assert_eq!(gone.code, ErrorCode::UnknownFinding);
        assert_eq!(fixture.model.asked().len(), 3);

        // Without a model set up for the task, nothing is asked.
        fixture.models = ModelService::unanswered();
        fixture
            .models
            .set_provider(fixture.projects.store(), ModelProviderKind::Anthropic.key())
            .unwrap();
        let unready = fixture.write_comment(&id, Language::English).unwrap_err();
        assert_eq!(unready.code, ErrorCode::ModelNotConfigured);
        nothing_kept(&fixture);
        assert_eq!(fixture.model.asked().len(), 3);
    }
}
