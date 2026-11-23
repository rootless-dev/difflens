//! What the commands and a full analysis ask of explanations: the stored
//! ones (both levels) read against the code as it is now, what explaining
//! would send, explaining itself, and translating what was written.
//!
//! Only `run` (through `explain` or a full analysis) and `translate` ask the
//! model, and only after a click: a node's or a group's Explain button, a
//! function's Explain in detail, the confirmation of a full analysis, or
//! Translate.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use super::inputs::{self, Call, Context, FileTexts, Prepared, Target};
use crate::ai::{Calls, ModelError, ModelRequest, ModelService};
use crate::error::{app_error, AppResult};
use crate::flow;
use crate::git::Blob;
use crate::insights::plan::estimate_tokens;
use crate::language::Language;
use crate::model::{
    BriefTranslation, ExplanationCall, ExplanationKind, ExplanationLevel, ExplanationPlan,
    ExplanationTranslation, Explanations, FlowGraph, FlowNodeState, FunctionBrief,
    FunctionExplanation, FunctionSpan, ModelInUse, ModelProviderKind, ModelTask, TranslationCall,
    TranslationPlan,
};
use crate::parallel;
use crate::review::{self, MAX_TEXT_BYTES};
use crate::review_state::{ChangedSides, ReviewSession};
use crate::store::{BriefRecord, ExplanationRecord};
use crate::translation::{self, FunctionProse};

/// What the model is told about explaining functions in detail, in three
/// parts; see `prompt.md`.
pub const SYSTEM_PROMPT: &str = include_str!("prompt.md");
/// What the model is told about explaining functions in short; see
/// `brief_prompt.md`.
pub const BRIEF_SYSTEM_PROMPT: &str = include_str!("brief_prompt.md");

/// The system prompt of `level`.
pub fn system_prompt(level: ExplanationLevel) -> &'static str {
    match level {
        ExplanationLevel::Brief => BRIEF_SYSTEM_PROMPT,
        ExplanationLevel::Detail => SYSTEM_PROMPT,
    }
}

/// The task whose model writes explanations at `level`.
pub fn task_of(level: ExplanationLevel) -> ModelTask {
    match level {
        ExplanationLevel::Brief => ModelTask::BriefExplanation,
        ExplanationLevel::Detail => ModelTask::DetailedExplanation,
    }
}

/// Which functions to explain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// The changed production functions, as a full analysis explains them.
    Full,
    /// The functions with these ids (a node's or a group's button).
    Functions(Vec<String>),
}

/// The calls explaining a scope of a review would make, ready to ask.
pub struct Planned {
    pub level: ExplanationLevel,
    pub calls: Vec<Call>,
    pub context: String,
}

impl Planned {
    pub fn count(&self, kind: ExplanationKind) -> u32 {
        count(
            self.calls
                .iter()
                .flat_map(|call| &call.functions)
                .filter(|function| function.target.kind == kind)
                .count(),
        )
    }
}

#[derive(Default)]
pub struct ExplanationService {
    /// The pieces of work started by a click, by the run id the frontend
    /// chose, so Cancel stops every call of one.
    running: Mutex<HashMap<String, Arc<Calls>>>,
}

impl ExplanationService {
    pub fn new() -> Self {
        Self::default()
    }

    /// The review's explanations, both levels, each read against `spans`,
    /// the functions as the review's graph has them now: one whose lines
    /// changed since it was written is outdated. Asks no model.
    pub fn explanations(
        &self,
        session: &ReviewSession<'_>,
        spans: &[FunctionSpan],
    ) -> AppResult<Explanations> {
        let Some(record) = session.existing_record()? else {
            return Ok(Explanations::none());
        };
        let store = session.store();
        let details = store.explanations(&record.id)?;
        let briefs = store.briefs(&record.id)?;
        if details.is_empty() && briefs.is_empty() {
            return Ok(Explanations::none());
        }
        let explained: HashSet<&str> = details
            .iter()
            .map(|record| record.function_id.as_str())
            .chain(briefs.iter().map(|record| record.function_id.as_str()))
            .collect();
        let spans: HashMap<&str, &FunctionSpan> = spans
            .iter()
            .filter(|span| explained.contains(span.id.as_str()))
            .map(|span| (span.id.as_str(), span))
            .collect();
        let paths: HashSet<String> = spans
            .values()
            .filter(|span| !span.deleted)
            .map(|span| span.file.clone())
            .collect();
        let texts = file_texts(session, &session.changed_sides()?, &paths)?;
        let outdated = |function_id: &str, fingerprint: &str| {
            spans.get(function_id).is_some_and(|span| {
                let text = texts.get(&span.file).and_then(|texts| texts.new.as_deref());
                inputs::fingerprint_of(span.deleted, text, span.start_line, span.end_line)
                    != fingerprint
            })
        };
        let mut detail_translations = store.explanation_translations(&record.id)?;
        let mut functions = Vec::new();
        for record in details {
            let versions = detail_translations
                .remove(&record.function_id)
                .unwrap_or_default()
                .into_iter()
                .filter(|version| version.language != record.language)
                .collect();
            let outdated = outdated(&record.function_id, &record.fingerprint);
            functions.push(present(record, versions, outdated)?);
        }
        let mut brief_translations = store.brief_translations(&record.id)?;
        let mut shorts = Vec::new();
        for record in briefs {
            let versions = brief_translations
                .remove(&record.function_id)
                .unwrap_or_default()
                .into_iter()
                .filter(|version| version.language != record.language)
                .collect();
            let outdated = outdated(&record.function_id, &record.fingerprint);
            shorts.push(present_brief(record, versions, outdated)?);
        }
        Ok(Explanations {
            functions,
            briefs: shorts,
        })
    }

    /// The calls explaining `scope` of the review at `level` would make,
    /// with the functions of `graph`. Asks no model.
    pub fn prepare(
        &self,
        session: &ReviewSession<'_>,
        graph: &FlowGraph,
        scope: &Scope,
        level: ExplanationLevel,
        context: &Context,
    ) -> AppResult<Planned> {
        let (targets, max_functions): (Vec<Target>, usize) = match scope {
            Scope::Full => (inputs::full_targets(graph), inputs::CALL_MAX_FUNCTIONS),
            Scope::Functions(ids) => (
                inputs::chosen_targets(graph, ids),
                inputs::GROUP_MAX_FUNCTIONS,
            ),
        };
        let paths: HashSet<String> = targets.iter().map(|target| target.file.clone()).collect();
        let texts = file_texts(session, &session.changed_sides()?, &paths)?;
        let prepared: Vec<Prepared> = targets
            .into_iter()
            .map(|target| {
                let file = texts.get(&target.file).cloned().unwrap_or_default();
                inputs::prepare(target, &file)
            })
            .collect();
        let context = inputs::context_text(context);
        let overhead = estimate_tokens(system_prompt(level))
            + estimate_tokens(&context)
            + estimate_tokens(&Language::BrazilianPortuguese.prose_instruction())
            + 20;
        Ok(Planned {
            level,
            calls: inputs::pack(prepared, overhead, max_functions),
            context,
        })
    }

    /// What explaining would send and to whom. Asks no model.
    pub fn plan(&self, planned: &Planned, model: ModelInUse) -> ExplanationPlan {
        ExplanationPlan {
            changed: planned.count(ExplanationKind::Changed),
            tokens: planned.calls.iter().map(|call| call.tokens).sum(),
            calls: planned
                .calls
                .iter()
                .map(|call| ExplanationCall {
                    ids: call.ids(),
                    tokens: call.tokens,
                })
                .collect(),
            model,
        }
    }

    /// Asks the calls of `planned` through `calls`, `parallel` at a time,
    /// each written in `language`, and keeps each explanation (at the level
    /// of `planned`) as soon as its call answers. Tells `report` as each
    /// call starts and ends (with how many functions it explained). No call
    /// starts once `calls` is cancelled. Returns how each call ended, `None`
    /// for one not started.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        session: &ReviewSession<'_>,
        models: &ModelService,
        planned: &Planned,
        language: Language,
        calls: &Calls,
        parallel: usize,
        report: &(dyn Fn(u32, Option<&AppResult<u32>>) + Sync),
    ) -> Vec<Option<AppResult<u32>>> {
        let numbered: Vec<(u32, &Call)> = planned
            .calls
            .iter()
            .enumerate()
            .map(|(index, call)| (count(index), call))
            .collect();
        parallel::run(
            &numbered,
            parallel,
            || calls.is_cancelled(),
            |_, (index, call)| {
                report(*index, None);
                let end = self.ask(session, models, planned, call, language, calls);
                report(*index, Some(&end));
                end
            },
        )
    }

    /// Explains the functions `ids` of `graph` at `level`, after a click on
    /// a node's or a group's button (short) or on a function's Explain in
    /// detail, under `run_id`, which `cancel` stops; then the review's
    /// explanations, read against `graph`. Fails with the first call's error
    /// when a call failed; what other calls wrote is kept.
    #[allow(clippy::too_many_arguments)]
    pub fn explain(
        &self,
        session: &ReviewSession<'_>,
        models: &ModelService,
        graph: &FlowGraph,
        ids: &[String],
        level: ExplanationLevel,
        context: &Context,
        language: Language,
        run_id: &str,
    ) -> AppResult<Explanations> {
        let model = models.in_use(session.store(), task_of(level))?;
        ensure_ready(&model)?;
        let planned = self.prepare(
            session,
            graph,
            &Scope::Functions(ids.to_vec()),
            level,
            context,
        )?;
        let calls = Arc::new(Calls::new(format!("explain-{run_id}")));
        self.lock().insert(run_id.to_string(), Arc::clone(&calls));
        let ends = self.run(
            session,
            models,
            &planned,
            language,
            &calls,
            model.parallel_calls as usize,
            &|_, _| {},
        );
        self.lock().remove(run_id);
        let failure = ends.into_iter().find_map(|end| match end {
            Some(Err(error)) => Some(error),
            None => Some(ModelError::Cancelled.into()),
            Some(Ok(_)) => None,
        });
        if let Some(error) = failure {
            return Err(error);
        }
        self.explanations(session, &spans_of(graph))
    }

    /// Stops the explanations started under `run_id`. Nothing happens when
    /// they already ended.
    pub fn cancel(&self, run_id: &str) {
        if let Some(calls) = self.lock().get(run_id) {
            calls.cancel();
        }
    }

    /// What translating the review's explanations (both levels) into
    /// `language` would send: one item per function with something to
    /// translate. Asks no model.
    pub fn plan_translation(
        &self,
        session: &ReviewSession<'_>,
        models: &ModelService,
        language: Language,
    ) -> AppResult<TranslationPlan> {
        let prose = untranslated(session, language)?;
        let calls: Vec<TranslationCall> = translation::pack_functions(language, &prose)
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

    /// Asks the model, under `run_id`, to translate the explanations of the
    /// functions `ids` into `language`, only after a click on Translate, and
    /// keeps each level sent as one more version of it. Nothing is kept
    /// when the call fails. Then the review's explanations, read against
    /// `spans`.
    pub fn translate(
        &self,
        session: &ReviewSession<'_>,
        models: &ModelService,
        language: Language,
        ids: &[String],
        spans: &[FunctionSpan],
        run_id: &str,
    ) -> AppResult<Explanations> {
        let store = session.store();
        ensure_ready(&models.in_use(store, ModelTask::Translation)?)?;
        let Some(record) = session.existing_record()? else {
            return self.explanations(session, spans);
        };
        let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let prose: Vec<FunctionProse> = untranslated(session, language)?
            .into_iter()
            .filter(|function| wanted.contains(function.id.as_str()))
            .collect();
        if prose.is_empty() {
            return self.explanations(session, spans);
        }
        let sent: Vec<String> = prose.iter().map(|function| function.id.clone()).collect();
        let request = ModelRequest {
            system: translation::SYSTEM_PROMPT.to_string(),
            prompt: translation::functions_prompt(language, &prose),
            schema: translation::functions_schema(&sent),
            schema_name: translation::FUNCTIONS_SCHEMA_NAME.to_string(),
        };
        let answer = models.ask(store, ModelTask::Translation, run_id, request)?;
        let translated = translation::parse_functions(&answer.value, &prose).map_err(|detail| {
            ModelError::InvalidOutput {
                provider: answer.provider,
                detail,
            }
        })?;
        let originals: HashMap<&str, &FunctionProse> = prose
            .iter()
            .map(|function| (function.id.as_str(), function))
            .collect();
        let mut details: Vec<(String, ExplanationTranslation)> = Vec::new();
        let mut briefs: Vec<(String, BriefTranslation)> = Vec::new();
        for function in translated {
            let Some(original) = originals.get(function.id.as_str()) else {
                continue;
            };
            if !original.brief.trim().is_empty() {
                briefs.push((
                    function.id.clone(),
                    BriefTranslation {
                        language,
                        text: function.brief,
                    },
                ));
            }
            if !original.purpose.trim().is_empty() {
                // A part the original leaves empty (what changed and why, for
                // a function of the context) stays empty.
                let kept = |original: &str, translated: String| {
                    if original.trim().is_empty() {
                        String::new()
                    } else {
                        translated
                    }
                };
                details.push((
                    function.id,
                    ExplanationTranslation {
                        language,
                        what_changed: kept(&original.what_changed, function.what_changed),
                        why: kept(&original.why, function.why),
                        purpose: function.purpose,
                    },
                ));
            }
        }
        store.save_explanation_translations(&record.id, &details)?;
        store.save_brief_translations(&record.id, &briefs)?;
        self.explanations(session, spans)
    }

    /// Asks one call and keeps what it explained, at the level of
    /// `planned`; returns how many.
    fn ask(
        &self,
        session: &ReviewSession<'_>,
        models: &ModelService,
        planned: &Planned,
        call: &Call,
        language: Language,
        calls: &Calls,
    ) -> AppResult<u32> {
        let store = session.store();
        let ids = call.ids();
        let (schema, schema_name) = match planned.level {
            ExplanationLevel::Brief => (inputs::brief_schema(&ids), inputs::BRIEF_SCHEMA_NAME),
            ExplanationLevel::Detail => (inputs::schema(&ids), inputs::SCHEMA_NAME),
        };
        let request = ModelRequest {
            system: system_prompt(planned.level).to_string(),
            prompt: format!(
                "{}{}",
                language.prose_instruction(),
                inputs::prompt(&planned.context, &call.functions)
            ),
            schema,
            schema_name: schema_name.to_string(),
        };
        let answer = calls.ask(models, store, task_of(planned.level), request)?;
        let invalid = |detail| ModelError::InvalidOutput {
            provider: answer.provider,
            detail,
        };
        let by_id: HashMap<&str, &Prepared> = call
            .functions
            .iter()
            .map(|function| (function.target.id.as_str(), function))
            .collect();
        let review_id = session.record()?.id;
        let created_at = flow::now()?;
        let head_sha = session.review().new_sha.clone();
        let provider = answer.provider.key().to_string();
        match planned.level {
            ExplanationLevel::Brief => {
                let written =
                    inputs::parse_briefs(&answer.value, &call.functions).map_err(invalid)?;
                let records: Vec<BriefRecord> = written
                    .into_iter()
                    .filter_map(|brief| {
                        let sent = by_id.get(brief.id.as_str())?;
                        Some(BriefRecord {
                            review_id: review_id.clone(),
                            function_id: brief.id,
                            kind: sent.target.kind,
                            fingerprint: sent.fingerprint.clone(),
                            head_sha: head_sha.clone(),
                            provider: provider.clone(),
                            model: answer.model.clone(),
                            created_at: created_at.clone(),
                            language,
                            text: brief.text,
                        })
                    })
                    .collect();
                store.save_briefs(&records)?;
                Ok(count(records.len()))
            }
            ExplanationLevel::Detail => {
                let written = inputs::parse(&answer.value, &call.functions).map_err(invalid)?;
                let records: Vec<ExplanationRecord> = written
                    .into_iter()
                    .filter_map(|explanation| {
                        let sent = by_id.get(explanation.id.as_str())?;
                        Some(ExplanationRecord {
                            review_id: review_id.clone(),
                            function_id: explanation.id,
                            kind: sent.target.kind,
                            fingerprint: sent.fingerprint.clone(),
                            head_sha: head_sha.clone(),
                            provider: provider.clone(),
                            model: answer.model.clone(),
                            created_at: created_at.clone(),
                            language,
                            what_changed: explanation.what_changed,
                            why: explanation.why,
                            why_inferred: explanation.why_inferred,
                            purpose: explanation.purpose,
                        })
                    })
                    .collect();
                store.save_explanations(&records)?;
                Ok(count(records.len()))
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Calls>>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Every function of `graph` with a place in the code, as the screen gives
/// them to `explanations`.
pub fn spans_of(graph: &FlowGraph) -> Vec<FunctionSpan> {
    graph
        .nodes
        .iter()
        .filter_map(|node| {
            Some(FunctionSpan {
                id: node.id.clone(),
                file: node.file.clone()?,
                start_line: node.start_line?,
                end_line: node.end_line?,
                deleted: node.state == FlowNodeState::Deleted,
            })
        })
        .collect()
}

/// Fails as explaining would when the model in use cannot be asked.
pub fn ensure_ready(model: &ModelInUse) -> AppResult<()> {
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

/// Both sides of the files `paths`: from the review's changed files (by
/// their path, or their old path for a function deleted with a rename), and
/// for a file the review does not change, its text at the new side.
pub fn file_texts(
    session: &ReviewSession<'_>,
    files: &[ChangedSides],
    paths: &HashSet<String>,
) -> AppResult<HashMap<String, FileTexts>> {
    let mut texts: HashMap<String, FileTexts> = HashMap::new();
    for file in files {
        let both = FileTexts {
            old: file.old.text.clone(),
            new: file.new.text.clone(),
        };
        for path in std::iter::once(&file.path).chain(&file.old_path) {
            if paths.contains(path) && !texts.contains_key(path) {
                texts.insert(path.clone(), both.clone());
            }
        }
    }
    let unchanged: Vec<&str> = paths
        .iter()
        .filter(|path| !texts.contains_key(*path))
        .map(String::as_str)
        .filter(|path| review::check_path(path).is_ok())
        .collect();
    if unchanged.is_empty() {
        return Ok(texts);
    }
    let read: Vec<Option<String>> = match session.review().new_sha.as_deref() {
        Some(sha) => session
            .git()
            .files_at(session.repo(), sha, &unchanged, MAX_TEXT_BYTES)?
            .into_iter()
            .map(|blob| match blob {
                Blob::Content(bytes) => String::from_utf8(bytes).ok(),
                _ => None,
            })
            .collect(),
        None => unchanged
            .iter()
            .map(|path| std::fs::read_to_string(session.repo().join(path)).ok())
            .collect(),
    };
    for (path, text) in unchanged.into_iter().zip(read) {
        texts.insert(
            path.to_string(),
            FileTexts {
                old: text.clone(),
                new: text,
            },
        );
    }
    Ok(texts)
}

/// The prose of the review's explanations written in another language than
/// `language` and not yet translated into it, one item per function: its
/// short explanation and its detailed one, each only when it needs it.
fn untranslated(session: &ReviewSession<'_>, language: Language) -> AppResult<Vec<FunctionProse>> {
    let Some(record) = session.existing_record()? else {
        return Ok(Vec::new());
    };
    let store = session.store();
    let detail_translations = store.explanation_translations(&record.id)?;
    let brief_translations = store.brief_translations(&record.id)?;
    let mut prose: HashMap<String, FunctionProse> = HashMap::new();
    let empty = |id: &str| FunctionProse {
        id: id.to_string(),
        brief: String::new(),
        what_changed: String::new(),
        why: String::new(),
        purpose: String::new(),
    };
    for brief in store.briefs(&record.id)? {
        let translated = brief_translations
            .get(&brief.function_id)
            .is_some_and(|versions| versions.iter().any(|version| version.language == language));
        if brief.language != language && !translated {
            prose
                .entry(brief.function_id.clone())
                .or_insert_with(|| empty(&brief.function_id))
                .brief = brief.text;
        }
    }
    for detail in store.explanations(&record.id)? {
        let translated = detail_translations
            .get(&detail.function_id)
            .is_some_and(|versions| versions.iter().any(|version| version.language == language));
        if detail.language != language && !translated {
            let entry = prose
                .entry(detail.function_id.clone())
                .or_insert_with(|| empty(&detail.function_id));
            entry.what_changed = detail.what_changed;
            entry.why = detail.why;
            entry.purpose = detail.purpose;
        }
    }
    let mut prose: Vec<FunctionProse> = prose.into_values().collect();
    prose.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(prose)
}

fn provider_of(key: &str) -> AppResult<ModelProviderKind> {
    ModelProviderKind::from_key(key).ok_or_else(|| {
        app_error!(
            Internal,
            ExplanationProviderUnknown,
            "An explanation was written by an unknown provider \"{provider}\".",
            provider = crate::error::escaped(key)
        )
    })
}

fn present(
    record: ExplanationRecord,
    translations: Vec<ExplanationTranslation>,
    outdated: bool,
) -> AppResult<FunctionExplanation> {
    let provider = provider_of(&record.provider)?;
    Ok(FunctionExplanation {
        function_id: record.function_id,
        kind: record.kind,
        what_changed: record.what_changed,
        why: record.why,
        why_inferred: record.why_inferred,
        purpose: record.purpose,
        language: record.language,
        translations,
        provider,
        provider_name: provider.name().to_string(),
        model: record.model,
        created_at: record.created_at,
        head_sha: record.head_sha,
        outdated,
    })
}

fn present_brief(
    record: BriefRecord,
    translations: Vec<BriefTranslation>,
    outdated: bool,
) -> AppResult<FunctionBrief> {
    let provider = provider_of(&record.provider)?;
    Ok(FunctionBrief {
        function_id: record.function_id,
        kind: record.kind,
        text: record.text,
        language: record.language,
        translations,
        provider,
        provider_name: provider.name().to_string(),
        model: record.model,
        created_at: record.created_at,
        head_sha: record.head_sha,
        outdated,
    })
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}
