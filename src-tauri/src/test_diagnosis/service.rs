//! What the commands ask of the explanations of failed tests: the kept ones
//! read against the last run, explaining one failure, and translating what
//! was written.
//!
//! Only `diagnose` and `translate` ask the model, and only after a click:
//! Explain the failure (or Explain again) on a failure's card, or Translate.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};

use super::inputs::{self, Found, Prepared, Subject};
use crate::ai::{ModelError, ModelRequest, ModelService};
use crate::analysis::Function;
use crate::error::{app_error, escaped, AppResult};
use crate::explanations::{self, ensure_ready};
use crate::flow;
use crate::language::Language;
use crate::model::{
    DiagnosisTranslation, ModelProviderKind, ModelTask, ReviewTarget, TestDiagnosis, TestRun,
    TestRunStatus, TranslationCall, TranslationPlan,
};
use crate::review_state::ReviewSession;
use crate::store::{DiagnosisRecord, DiagnosisTranslationRecord};
use crate::test_runs::DerivedKey;
use crate::translation::{self, DiagnosisProse};

/// What the model is told about explaining a failed test; see `prompt.md`.
pub const SYSTEM_PROMPT: &str = include_str!("prompt.md");

/// Runs and sides whose prepared calls are kept: a review's, and a few the
/// user went back and forth between.
const KEEP_PREPARED: usize = 4;

/// What the last run of the review says, as the commands read it.
#[derive(Clone, Copy)]
pub struct Facts<'a> {
    /// The last run of the review's new side; None without one.
    pub run: Option<&'a TestRun>,
    /// The new side's functions in the run's language; empty when the sides
    /// could not be analysed.
    pub functions: &'a [Function],
    /// Where the calls prepared for this run and these sides are kept; None
    /// to prepare each anew.
    pub kept: Option<KeptInputs<'a>>,
}

/// The calls prepared for one derived run (see `DerivedKey`).
#[derive(Clone, Copy)]
pub struct KeptInputs<'a> {
    pub memo: &'a InputsMemo,
    pub key: &'a DerivedKey,
}

/// The calls prepared for the failures of the last runs, by run and sides
/// (`DerivedKey`, which changes with either) and review target, so reading
/// the explanations again reads no file and assembles no code: what tells
/// an explanation is outdated is computed once per run and sides.
#[derive(Default)]
pub struct InputsMemo {
    kept: Mutex<VecDeque<(Scope, PreparedByFailure)>>,
}

/// What the calls kept together were prepared for.
type Scope = (DerivedKey, ReviewTarget);
/// By failure key.
type PreparedByFailure = HashMap<String, Arc<Prepared>>;

impl InputsMemo {
    fn get(&self, scope: &Scope, failure: &str) -> Option<Arc<Prepared>> {
        let kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        kept.iter()
            .find(|(kept_scope, _)| kept_scope == scope)
            .and_then(|(_, prepared)| prepared.get(failure).cloned())
    }

    fn put(&self, scope: &Scope, failure: String, prepared: Arc<Prepared>) {
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        let index = match kept.iter().position(|(kept_scope, _)| kept_scope == scope) {
            Some(index) => index,
            None => {
                kept.push_back((scope.clone(), HashMap::new()));
                while kept.len() > KEEP_PREPARED {
                    kept.pop_front();
                }
                kept.len() - 1
            }
        };
        kept[index].1.insert(failure, prepared);
    }
}

/// The calls explaining each of `found` would make now, with the code read
/// at the review's new side; the ones prepared before for the same run and
/// sides as they were.
fn prepare_all(
    session: &ReviewSession<'_>,
    facts: Facts<'_>,
    run: &TestRun,
    found: &[Found<'_>],
) -> AppResult<Vec<Arc<Prepared>>> {
    let scope = facts
        .kept
        .map(|kept| (kept.key.clone(), session.review().target.clone()));
    let memo = |failure: &Found<'_>| {
        let kept = facts.kept?;
        kept.memo.get(scope.as_ref()?, &failure.subject.key())
    };
    let mut prepared: Vec<Option<Arc<Prepared>>> = found.iter().map(memo).collect();
    let missing: Vec<usize> = (0..found.len())
        .filter(|index| prepared[*index].is_none())
        .collect();
    if !missing.is_empty() {
        let mut paths: HashSet<String> = HashSet::new();
        for index in &missing {
            paths.extend(inputs::wanted_paths(&found[*index], facts.functions, &run.language));
        }
        let sides = session.changed_sides()?;
        let changed: HashSet<String> = sides.iter().map(|file| file.path.clone()).collect();
        let texts = explanations::file_texts(session, &sides, &paths)?;
        for index in missing {
            let made = Arc::new(inputs::prepare(
                &found[index],
                run,
                facts.functions,
                &texts,
                &changed,
                Some(&session.review().target),
            ));
            if let (Some(kept), Some(scope)) = (facts.kept, scope.as_ref()) {
                kept.memo
                    .put(scope, found[index].subject.key(), Arc::clone(&made));
            }
            prepared[index] = Some(made);
        }
    }
    Ok(prepared.into_iter().flatten().collect())
}

/// When the run that wrote the results shown started: a partial run merged
/// into it, or the run itself.
fn run_started_at(run: &TestRun) -> String {
    run.partial
        .as_ref()
        .filter(|partial| partial.status == TestRunStatus::Completed)
        .map_or_else(|| run.started_at.clone(), |partial| partial.started_at.clone())
}

fn gone() -> crate::error::AppError {
    app_error!(
        Internal,
        DiagnosisFailureGone,
        "This failure is not in the last test run any more. Run the tests again."
    )
}

/// The review's explanations of failures, each read against the last run:
/// one is outdated when that run no longer has its failure, reads the
/// failure otherwise (other failed subtests, another kind, message,
/// expected or actual value, or other places in the code; goroutine
/// numbers, addresses and durations aside), or when the code the model was
/// shown reads otherwise now. Asks no model.
pub fn diagnoses(session: &ReviewSession<'_>, facts: Facts<'_>) -> AppResult<Vec<TestDiagnosis>> {
    let Some(record) = session.existing_record()? else {
        return Ok(Vec::new());
    };
    let store = session.store();
    let kept = store.diagnoses(&record.id)?;
    if kept.is_empty() {
        return Ok(Vec::new());
    }
    let subjects: Vec<Subject> = kept
        .iter()
        .map(|record| Subject::stored(&record.package, &record.test))
        .collect();
    let mut now: HashMap<String, (String, String)> = HashMap::new();
    if let Some(run) = facts.run {
        let found: Vec<Found<'_>> = subjects
            .iter()
            .filter_map(|subject| inputs::find(run, subject))
            .collect();
        if !found.is_empty() {
            for (failure, prepared) in found.iter().zip(prepare_all(session, facts, run, &found)?) {
                now.insert(
                    failure.subject.key(),
                    (
                        prepared.failure_fingerprint.clone(),
                        prepared.code_fingerprint.clone(),
                    ),
                );
            }
        }
    }
    let mut translations = store.diagnosis_translations(&record.id)?;
    let mut comments = store.diagnosis_comments(&record.id)?;
    kept.into_iter()
        .map(|record| {
            let outdated = now
                .get(&inputs::key_of(&record.package, &record.test))
                .is_none_or(|(failure, code)| {
                    *failure != record.failure_fingerprint || *code != record.code_fingerprint
                });
            let subject = (record.package.clone(), record.test.clone());
            let versions = translations.remove(&subject).unwrap_or_default();
            let comment = comments.remove(&subject);
            present(record, versions, outdated, comment)
        })
        .collect()
}

/// Asks the model, under `run_id` (which `cancel_model_run` stops, as does
/// raising `cancel`), why the failure `subject` of the last run failed, in
/// `language`, only after a click on Explain the failure; keeps the answer
/// in place of an earlier one. Fails when the last run no longer has that
/// failure.
pub fn diagnose(
    session: &ReviewSession<'_>,
    models: &ModelService,
    facts: Facts<'_>,
    subject: &Subject,
    language: Language,
    run_id: &str,
    cancel: Arc<AtomicBool>,
) -> AppResult<TestDiagnosis> {
    let store = session.store();
    ensure_ready(&models.in_use(store, ModelTask::TestDiagnosis)?)?;
    let run = facts.run.ok_or_else(gone)?;
    let found = inputs::find(run, subject).ok_or_else(gone)?;
    let prepared = prepare_all(session, facts, run, std::slice::from_ref(&found))?
        .pop()
        .ok_or_else(gone)?;
    let request = ModelRequest {
        system: SYSTEM_PROMPT.to_string(),
        prompt: format!("{}{}", language.prose_instruction(), prepared.prompt),
        schema: prepared.schema.clone(),
        schema_name: inputs::SCHEMA_NAME.to_string(),
    };
    let answer = models.ask_flagged(store, ModelTask::TestDiagnosis, run_id, request, cancel)?;
    let written =
        inputs::parse(&answer.value, &prepared.code).map_err(|detail| ModelError::InvalidOutput {
            provider: answer.provider,
            detail,
        })?;
    let code_excerpt = written
        .location
        .as_ref()
        .and_then(|location| inputs::excerpt(&prepared.code, &location.file, location.line));
    let review_id = session.record()?.id;
    let record = DiagnosisRecord {
        review_id: review_id.clone(),
        package: subject.package.clone(),
        test: subject.stored_test().to_string(),
        run_started_at: run_started_at(run),
        failure_fingerprint: prepared.failure_fingerprint.clone(),
        code_fingerprint: prepared.code_fingerprint.clone(),
        head_sha: session.review().new_sha.clone(),
        provider: answer.provider.key().to_string(),
        model: answer.model,
        created_at: flow::now()?,
        language,
        cause: written.cause,
        fault: written.fault,
        confidence: written.confidence,
        location: written.location,
        fix: written.fix,
        fix_code: written.fix_code,
        code_excerpt,
        failure_kind: Some(inputs::failure_kind(&found).to_string()),
    };
    store.save_diagnosis(&record)?;
    // The comment written from an earlier explanation stays linked.
    let comment = store.diagnosis_comment(&review_id, &record.package, &record.test)?;
    present(record, Vec::new(), false, comment)
}

/// What translating the explanations `keys` into `language` would send:
/// one item per explanation not yet in it. Asks no model.
pub fn plan_translation(
    session: &ReviewSession<'_>,
    models: &ModelService,
    language: Language,
    keys: &[String],
) -> AppResult<TranslationPlan> {
    let prose = untranslated(session, language, keys)?;
    let calls: Vec<TranslationCall> = translation::pack_diagnoses(language, &prose)
        .into_iter()
        .map(|(ids, tokens)| TranslationCall { ids, tokens })
        .collect();
    Ok(TranslationPlan {
        items: u32::try_from(prose.len()).unwrap_or(u32::MAX),
        tokens: calls.iter().map(|call| call.tokens).sum(),
        calls,
        model: models.in_use(session.store(), ModelTask::Translation)?,
    })
}

/// Asks the model, under `run_id`, to translate the explanations `keys`
/// into `language`, only after a click on Translate, and keeps each as one
/// more version of it. Nothing is kept when the call fails. Then the
/// review's explanations, read against the last run.
pub fn translate(
    session: &ReviewSession<'_>,
    models: &ModelService,
    facts: Facts<'_>,
    language: Language,
    keys: &[String],
    run_id: &str,
) -> AppResult<Vec<TestDiagnosis>> {
    let store = session.store();
    ensure_ready(&models.in_use(store, ModelTask::Translation)?)?;
    let Some(record) = session.existing_record()? else {
        return Ok(Vec::new());
    };
    let prose = untranslated(session, language, keys)?;
    if prose.is_empty() {
        return diagnoses(session, facts);
    }
    let sent: Vec<String> = prose.iter().map(|diagnosis| diagnosis.id.clone()).collect();
    let request = ModelRequest {
        system: translation::SYSTEM_PROMPT.to_string(),
        prompt: translation::diagnoses_prompt(language, &prose),
        schema: translation::diagnoses_schema(&sent),
        schema_name: translation::DIAGNOSES_SCHEMA_NAME.to_string(),
    };
    let answer = models.ask(store, ModelTask::Translation, run_id, request)?;
    let translated = translation::parse_diagnoses(&answer.value, &prose).map_err(|detail| {
        ModelError::InvalidOutput {
            provider: answer.provider,
            detail,
        }
    })?;
    let subjects: HashMap<String, (String, String)> = store
        .diagnoses(&record.id)?
        .into_iter()
        .map(|kept| {
            (
                inputs::key_of(&kept.package, &kept.test),
                (kept.package, kept.test),
            )
        })
        .collect();
    let versions: Vec<((String, String), DiagnosisTranslationRecord)> = translated
        .into_iter()
        .filter_map(|diagnosis| {
            let subject = subjects.get(&diagnosis.id)?.clone();
            Some((
                subject,
                DiagnosisTranslationRecord {
                    language,
                    cause: diagnosis.cause,
                    fix: diagnosis.fix,
                },
            ))
        })
        .collect();
    store.save_diagnosis_translations(&record.id, &versions)?;
    diagnoses(session, facts)
}

/// The prose of the explanations `keys` written in another language than
/// `language` and not yet translated into it.
fn untranslated(
    session: &ReviewSession<'_>,
    language: Language,
    keys: &[String],
) -> AppResult<Vec<DiagnosisProse>> {
    let Some(record) = session.existing_record()? else {
        return Ok(Vec::new());
    };
    let store = session.store();
    let wanted: HashSet<&str> = keys.iter().map(String::as_str).collect();
    let translations = store.diagnosis_translations(&record.id)?;
    Ok(store
        .diagnoses(&record.id)?
        .into_iter()
        .filter(|kept| {
            let key = inputs::key_of(&kept.package, &kept.test);
            let translated = translations
                .get(&(kept.package.clone(), kept.test.clone()))
                .is_some_and(|versions| {
                    versions.iter().any(|version| version.language == language)
                });
            wanted.contains(key.as_str()) && kept.language != language && !translated
        })
        .map(|kept| DiagnosisProse {
            id: inputs::key_of(&kept.package, &kept.test),
            cause: kept.cause,
            fix: kept.fix,
        })
        .collect())
}

fn present(
    record: DiagnosisRecord,
    translations: Vec<DiagnosisTranslationRecord>,
    outdated: bool,
    comment_id: Option<String>,
) -> AppResult<TestDiagnosis> {
    let provider = ModelProviderKind::from_key(&record.provider).ok_or_else(|| {
        app_error!(
            Internal,
            DiagnosisProviderUnknown,
            "An explanation of a failure was written by an unknown provider \"{provider}\".",
            provider = escaped(&record.provider)
        )
    })?;
    Ok(TestDiagnosis {
        key: inputs::key_of(&record.package, &record.test),
        test: (!record.test.is_empty()).then(|| record.test.clone()),
        package: record.package,
        cause: record.cause,
        fault: record.fault,
        confidence: record.confidence,
        location: record.location,
        fix: record.fix,
        fix_code: record.fix_code,
        translations: translations
            .into_iter()
            .filter(|version| version.language != record.language)
            .map(|version| DiagnosisTranslation {
                language: version.language,
                cause: version.cause,
                fix: version.fix,
            })
            .collect(),
        language: record.language,
        provider,
        provider_name: provider.name().to_string(),
        model: record.model,
        created_at: record.created_at,
        run_started_at: record.run_started_at,
        comment_id,
        outdated,
    })
}
