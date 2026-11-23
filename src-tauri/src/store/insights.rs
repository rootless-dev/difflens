//! Insights of a review: each analysis (the head it read, who wrote it and
//! when), its calls, and the findings with the user's verdicts and, once
//! fixed, how they were resolved. Only rows; what they mean against the code
//! is worked out in `insights`.

use std::collections::HashMap;

use rusqlite::{params, OptionalExtension, Row};
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::Store;
use crate::error::{app_error, AppError, AppResult, ErrorCode};
use crate::language::Language;
use crate::model::{
    FindingCategory, FindingResolution, FindingTranslation, FindingVerdict, InsightsBatchStatus,
    LocationProblem, Severity,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisRecord {
    pub id: String,
    pub review_id: String,
    /// `None` for the working tree.
    pub head_sha: Option<String>,
    /// Key of the provider, such as "anthropic".
    pub provider: String,
    pub model: String,
    pub created_at: String,
    /// The language its findings are written in; English before languages.
    pub language: Language,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchRecord {
    pub index: u32,
    pub paths: Vec<String>,
    pub tokens: u64,
    pub status: InsightsBatchStatus,
    pub error: Option<AppError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingRecord {
    pub id: String,
    pub analysis_id: String,
    /// The call that wrote it, and its place in that answer.
    pub batch: u32,
    pub ordinal: u32,
    pub title: String,
    pub severity: Severity,
    pub category: FindingCategory,
    pub path: String,
    pub line: u32,
    pub end_line: u32,
    pub explanation: String,
    pub suggested_direction: String,
    pub evidence: Vec<String>,
    pub related: Vec<String>,
    /// Why its location was not found when it was written; `None` when it was.
    pub location_problem: Option<LocationProblem>,
    /// The cited lines then, to find them again; empty when not found.
    pub cited_text: String,
    pub verdict: FindingVerdict,
    pub verdict_at: Option<String>,
    /// Of the code around the cited lines when the verdict was given.
    pub verdict_fingerprint: Option<String>,
    /// How it was marked fixed; `None` while it is open.
    pub resolution: Option<FindingResolution>,
}

const FINDING_COLUMNS: &str =
    "f.id, f.analysis_id, f.batch, f.ordinal, f.title, f.severity, f.category, f.path, \
     f.line, f.end_line, f.explanation, f.suggested_direction, f.evidence, f.related, \
     f.location_problem, f.cited_text, f.verdict, f.verdict_at, f.verdict_fingerprint, \
     f.resolution, f.resolved_at, f.resolved_model, f.resolution_reason";

impl Store {
    pub fn insert_analysis(
        &self,
        analysis: &AnalysisRecord,
        batches: &[BatchRecord],
    ) -> AppResult<()> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO insight_analyses (id, review_id, head_sha, provider, model, created_at,
                 language)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                analysis.id,
                analysis.review_id,
                analysis.head_sha,
                analysis.provider,
                analysis.model,
                analysis.created_at,
                analysis.language.tag()
            ],
        )?;
        for batch in batches {
            transaction.execute(
                "INSERT INTO insight_batches (analysis_id, position, paths, tokens, status,
                     error_code, error_message, error_reason, error_params, error_detail)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    analysis.id,
                    batch.index,
                    to_json(&batch.paths)?,
                    i64::try_from(batch.tokens).unwrap_or(i64::MAX),
                    key(batch.status)?,
                    batch
                        .error
                        .as_ref()
                        .map(|error| key(error.code))
                        .transpose()?,
                    batch.error.as_ref().map(|error| error.message.as_str()),
                    batch
                        .error
                        .as_ref()
                        .and_then(|error| error.reason)
                        .map(|reason| reason.key()),
                    batch.error.as_ref().and_then(AppError::params_json),
                    batch
                        .error
                        .as_ref()
                        .and_then(|error| error.detail.as_deref()),
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// The analyses of a review, newest first.
    pub fn analyses(&self, review_id: &str) -> AppResult<Vec<AnalysisRecord>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id, review_id, head_sha, provider, model, created_at, language
             FROM insight_analyses
             WHERE review_id = ?1 ORDER BY created_at DESC, rowid DESC",
        )?;
        let analyses = statement
            .query_map(params![review_id], |row| {
                Ok(AnalysisRecord {
                    id: row.get(0)?,
                    review_id: row.get(1)?,
                    head_sha: row.get(2)?,
                    provider: row.get(3)?,
                    model: row.get(4)?,
                    created_at: row.get(5)?,
                    language: language_of(&row.get::<_, String>(6)?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(analyses)
    }

    pub fn set_analysis_model(&self, analysis_id: &str, model: &str) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE insight_analyses SET model = ?2 WHERE id = ?1",
            params![analysis_id, model],
        )?;
        Ok(())
    }

    /// The calls of an analysis, in order.
    pub fn batches(&self, analysis_id: &str) -> AppResult<Vec<BatchRecord>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT position, paths, tokens, status, error_code, error_message, error_reason,
                 error_params, error_detail
             FROM insight_batches WHERE analysis_id = ?1 ORDER BY position",
        )?;
        let rows = statement
            .query_map(params![analysis_id], |row| {
                Ok((
                    (
                        row.get::<_, u32>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ),
                    (
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                    ),
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |((index, paths, tokens, status), (code, message, reason, error_params, detail))| {
                    Ok(BatchRecord {
                        index,
                        paths: from_json(&paths)?,
                        tokens: u64::try_from(tokens).unwrap_or(0),
                        status: from_key(&status)?,
                        error: match (code, message) {
                            (Some(code), Some(message)) => Some(AppError::restore(
                                ErrorCode::from_key(&code).unwrap_or(ErrorCode::Internal),
                                message,
                                reason.as_deref(),
                                error_params.as_deref(),
                                detail,
                            )),
                            _ => None,
                        },
                    })
                },
            )
            .collect()
    }

    /// Records how a call ended; `error` only for a failed one.
    pub fn finish_batch(
        &self,
        analysis_id: &str,
        index: u32,
        status: InsightsBatchStatus,
        error: Option<&AppError>,
    ) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE insight_batches SET status = ?3, error_code = ?4, error_message = ?5,
                 error_reason = ?6, error_params = ?7, error_detail = ?8
             WHERE analysis_id = ?1 AND position = ?2",
            params![
                analysis_id,
                index,
                key(status)?,
                error.map(|error| key(error.code)).transpose()?,
                error.map(|error| error.message.as_str()),
                error.and_then(|error| error.reason).map(|reason| reason.key()),
                error.and_then(AppError::params_json),
                error.and_then(|error| error.detail.as_deref()),
            ],
        )?;
        Ok(())
    }

    /// Every finding of a review, oldest analysis first, in answer order.
    pub fn findings(&self, review_id: &str) -> AppResult<Vec<FindingRecord>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(&format!(
            "SELECT {FINDING_COLUMNS} FROM insight_findings f
             JOIN insight_analyses a ON a.id = f.analysis_id
             WHERE a.review_id = ?1
             ORDER BY a.created_at, a.rowid, f.batch, f.ordinal"
        ))?;
        let findings = statement
            .query_map(params![review_id], read_finding)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(findings)
    }

    /// A finding of the review, `None` when it has none with that id.
    pub fn finding(&self, review_id: &str, id: &str) -> AppResult<Option<FindingRecord>> {
        let finding = self
            .lock()?
            .query_row(
                &format!(
                    "SELECT {FINDING_COLUMNS} FROM insight_findings f
                     JOIN insight_analyses a ON a.id = f.analysis_id
                     WHERE a.review_id = ?1 AND f.id = ?2"
                ),
                params![review_id, id],
                read_finding,
            )
            .optional()?;
        Ok(finding)
    }

    pub fn insert_findings(&self, findings: &[FindingRecord]) -> AppResult<()> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        for finding in findings {
            transaction.execute(
                "INSERT INTO insight_findings (id, analysis_id, batch, ordinal, title, severity,
                     category, path, line, end_line, explanation, suggested_direction, evidence,
                     related, location_problem, cited_text, verdict, verdict_at,
                     verdict_fingerprint, resolution, resolved_at, resolved_model,
                     resolution_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                     ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
                params![
                    finding.id,
                    finding.analysis_id,
                    finding.batch,
                    finding.ordinal,
                    finding.title,
                    key(finding.severity)?,
                    key(finding.category)?,
                    finding.path,
                    finding.line,
                    finding.end_line,
                    finding.explanation,
                    finding.suggested_direction,
                    to_json(&finding.evidence)?,
                    to_json(&finding.related)?,
                    finding.location_problem.map(key).transpose()?,
                    finding.cited_text,
                    key(finding.verdict)?,
                    finding.verdict_at,
                    finding.verdict_fingerprint,
                    finding
                        .resolution
                        .as_ref()
                        .map(|resolution| key(resolution.kind))
                        .transpose()?,
                    finding.resolution.as_ref().map(|resolution| resolution.at.as_str()),
                    finding
                        .resolution
                        .as_ref()
                        .and_then(|resolution| resolution.model.as_deref()),
                    finding
                        .resolution
                        .as_ref()
                        .and_then(|resolution| resolution.reason.as_deref()),
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Deletes the review's analyses other than `keep` that own no finding
    /// any more, with their calls. An analysis is kept while it owns one:
    /// findings are never deleted here.
    pub fn drop_empty_analyses(&self, review_id: &str, keep: &str) -> AppResult<()> {
        self.lock()?.execute(
            "DELETE FROM insight_analyses
             WHERE review_id = ?1 AND id <> ?2
               AND NOT EXISTS (SELECT 1 FROM insight_findings f WHERE f.analysis_id = insight_analyses.id)",
            params![review_id, keep],
        )?;
        Ok(())
    }

    /// Marks a finding fixed as `resolution` says, or open again with
    /// `None`. Its verdict is left as it is.
    pub fn set_resolution(
        &self,
        finding_id: &str,
        resolution: Option<&FindingResolution>,
    ) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE insight_findings SET resolution = ?2, resolved_at = ?3, resolved_model = ?4,
                 resolution_reason = ?5
             WHERE id = ?1",
            params![
                finding_id,
                resolution.map(|resolution| key(resolution.kind)).transpose()?,
                resolution.map(|resolution| resolution.at.as_str()),
                resolution.and_then(|resolution| resolution.model.as_deref()),
                resolution.and_then(|resolution| resolution.reason.as_deref()),
            ],
        )?;
        Ok(())
    }

    /// Moves a finding to the lines `line` to `end_line`, which now read
    /// `cited_text`, as a later analysis re-checking it pointed at them.
    pub fn relocate_finding(
        &self,
        finding_id: &str,
        line: u32,
        end_line: u32,
        cited_text: &str,
    ) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE insight_findings SET line = ?2, end_line = ?3, cited_text = ?4,
                 location_problem = NULL
             WHERE id = ?1",
            params![finding_id, line, end_line, cited_text],
        )?;
        Ok(())
    }

    /// Records the user's verdict, the time and fingerprint it was given at.
    pub fn set_verdict(
        &self,
        finding_id: &str,
        verdict: FindingVerdict,
        at: Option<&str>,
        fingerprint: Option<&str>,
    ) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE insight_findings SET verdict = ?2, verdict_at = ?3, verdict_fingerprint = ?4
             WHERE id = ?1",
            params![finding_id, key(verdict)?, at, fingerprint],
        )?;
        Ok(())
    }

    /// The translations of the review's findings, by finding id, in the
    /// order of their languages.
    pub fn finding_translations(
        &self,
        review_id: &str,
    ) -> AppResult<HashMap<String, Vec<FindingTranslation>>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT t.finding_id, t.language, t.title, t.explanation, t.suggested_direction,
                 t.evidence, t.related
             FROM insight_finding_translations t
             JOIN insight_findings f ON f.id = t.finding_id
             JOIN insight_analyses a ON a.id = f.analysis_id
             WHERE a.review_id = ?1
             ORDER BY t.language",
        )?;
        let rows = statement
            .query_map(params![review_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut translations: HashMap<String, Vec<FindingTranslation>> = HashMap::new();
        for (finding_id, language, title, explanation, suggested_direction, evidence, related) in
            rows
        {
            // A language this version does not know is skipped, not failed.
            let Some(language) = Language::from_tag(&language) else {
                continue;
            };
            translations
                .entry(finding_id)
                .or_default()
                .push(FindingTranslation {
                    language,
                    title,
                    explanation,
                    suggested_direction,
                    evidence: from_json(&evidence)?,
                    related: from_json(&related)?,
                });
        }
        Ok(translations)
    }

    /// Keeps a translation of each finding, replacing an earlier one in the
    /// same language.
    pub fn save_finding_translations(
        &self,
        translations: &[(String, FindingTranslation)],
    ) -> AppResult<()> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        for (finding_id, translation) in translations {
            transaction.execute(
                "INSERT INTO insight_finding_translations (finding_id, language, title,
                     explanation, suggested_direction, evidence, related)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (finding_id, language) DO UPDATE SET
                     title = excluded.title, explanation = excluded.explanation,
                     suggested_direction = excluded.suggested_direction,
                     evidence = excluded.evidence, related = excluded.related",
                params![
                    finding_id,
                    translation.language.tag(),
                    translation.title,
                    translation.explanation,
                    translation.suggested_direction,
                    to_json(&translation.evidence)?,
                    to_json(&translation.related)?
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
}

/// A stored language tag; English for one this version does not know.
fn language_of(tag: &str) -> Language {
    Language::from_tag(tag).unwrap_or_default()
}

fn read_finding(row: &Row<'_>) -> rusqlite::Result<FindingRecord> {
    let text = |index: usize| -> rusqlite::Result<String> { row.get(index) };
    let decode = |index: usize, error: AppError| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    };
    Ok(FindingRecord {
        id: text(0)?,
        analysis_id: text(1)?,
        batch: row.get(2)?,
        ordinal: row.get(3)?,
        title: text(4)?,
        severity: from_key(&text(5)?).map_err(|error| decode(5, error))?,
        category: from_key(&text(6)?).map_err(|error| decode(6, error))?,
        path: text(7)?,
        line: row.get(8)?,
        end_line: row.get(9)?,
        explanation: text(10)?,
        suggested_direction: text(11)?,
        evidence: from_json(&text(12)?).map_err(|error| decode(12, error))?,
        related: from_json(&text(13)?).map_err(|error| decode(13, error))?,
        location_problem: row
            .get::<_, Option<String>>(14)?
            .map(|value| from_key(&value))
            .transpose()
            .map_err(|error| decode(14, error))?,
        cited_text: text(15)?,
        verdict: from_key(&text(16)?).map_err(|error| decode(16, error))?,
        verdict_at: row.get(17)?,
        verdict_fingerprint: row.get(18)?,
        resolution: match row.get::<_, Option<String>>(19)? {
            Some(kind) => Some(FindingResolution {
                kind: from_key(&kind).map_err(|error| decode(19, error))?,
                at: row.get::<_, Option<String>>(20)?.unwrap_or_default(),
                model: row.get(21)?,
                reason: row.get(22)?,
            }),
            None => None,
        },
    })
}

/// The spelling the frontend contract gives a value, such as "data_loss".
fn key<T: Serialize>(value: T) -> AppResult<String> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => Ok(text),
        _ => Err(app_error!(
            Internal,
            StoredValueUnnamed,
            "A stored value has no name."
        )),
    }
}

fn from_key<T: DeserializeOwned>(text: &str) -> AppResult<T> {
    serde_json::from_value(serde_json::Value::String(text.to_string())).map_err(|_| {
        app_error!(
            Internal,
            StoredValueUnknown,
            "Stored insights hold an unknown value \"{value}\".",
            value = crate::error::escaped(text)
        )
    })
}

fn to_json(items: &[String]) -> AppResult<String> {
    serde_json::to_string(items).map_err(|error| {
        app_error!(
            Internal,
            StoreListFailed,
            "Could not store a list: {detail}",
            detail = error
        )
    })
}

fn from_json(text: &str) -> AppResult<Vec<String>> {
    serde_json::from_str(text).map_err(|error| {
        app_error!(
            Internal,
            StoredListUnreadable,
            "A stored list is unreadable: {detail}",
            detail = error
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ReviewTarget;
    use crate::store::ProjectRecord;

    fn review(store: &Store) -> String {
        store
            .insert(&ProjectRecord {
                id: "p".to_string(),
                path: "/repos/p".to_string(),
                integration_branches: vec!["main".to_string()],
                last_fetched_at: None,
                added_at: "2026-01-01T00:00:00Z".to_string(),
            })
            .unwrap();
        store
            .open_review(
                "p",
                &ReviewTarget::Branch {
                    branch: "feature".to_string(),
                    base: "main".to_string(),
                },
                "2026-11-01T00:00:00Z",
            )
            .unwrap()
            .id
    }

    fn analysis(review_id: &str, id: &str, at: &str) -> AnalysisRecord {
        AnalysisRecord {
            id: id.to_string(),
            review_id: review_id.to_string(),
            head_sha: Some("e7f3b10".to_string()),
            provider: "anthropic".to_string(),
            model: "claude-opus-5-5".to_string(),
            created_at: at.to_string(),
            language: Language::English,
        }
    }

    fn translation(language: Language, title: &str) -> FindingTranslation {
        FindingTranslation {
            language,
            title: title.to_string(),
            explanation: "Explicado.".to_string(),
            suggested_direction: "Direcionado.".to_string(),
            evidence: vec!["A linha 52 chama o gateway.".to_string()],
            related: Vec::new(),
        }
    }

    #[test]
    fn an_analysis_keeps_its_language_and_its_findings_their_translations() {
        let store = Store::open_in_memory().unwrap();
        let review_id = review(&store);
        let mut portuguese = analysis(&review_id, "a1", "2026-11-12T10:00:00Z");
        portuguese.language = Language::BrazilianPortuguese;
        store.insert_analysis(&portuguese, &[batch(0, &["a.go"])]).unwrap();
        store.insert_findings(&[finding("a1", "f1")]).unwrap();
        assert_eq!(
            store.analyses(&review_id).unwrap()[0].language,
            Language::BrazilianPortuguese
        );

        store
            .save_finding_translations(&[("f1".to_string(), translation(Language::English, "First"))])
            .unwrap();
        store
            .save_finding_translations(&[("f1".to_string(), translation(Language::English, "Second"))])
            .unwrap();
        let translations = store.finding_translations(&review_id).unwrap();
        assert_eq!(translations["f1"], vec![translation(Language::English, "Second")]);

        // A finding marked fixed keeps its translations.
        store
            .set_resolution("f1", Some(&user_marked("2026-11-12T12:00:00Z")))
            .unwrap();
        assert_eq!(store.finding_translations(&review_id).unwrap()["f1"].len(), 1);

        // A finding that goes with its review takes its translations with it.
        store.delete("p").unwrap();
        assert!(store.finding_translations(&review_id).unwrap().is_empty());
    }

    fn user_marked(at: &str) -> FindingResolution {
        FindingResolution {
            kind: crate::model::ResolutionKind::UserMarked,
            model: None,
            at: at.to_string(),
            reason: None,
        }
    }

    #[test]
    fn an_analysis_from_before_languages_reads_as_english() {
        let store = Store::open_in_memory().unwrap();
        let review_id = review(&store);
        store
            .insert_analysis(&analysis(&review_id, "a1", "2026-11-12T10:00:00Z"), &[])
            .unwrap();
        store
            .lock()
            .unwrap()
            .execute("UPDATE insight_analyses SET language = 'xx' WHERE id = 'a1'", [])
            .unwrap();
        assert_eq!(store.analyses(&review_id).unwrap()[0].language, Language::English);
    }

    fn batch(index: u32, paths: &[&str]) -> BatchRecord {
        BatchRecord {
            index,
            paths: paths.iter().map(|path| path.to_string()).collect(),
            tokens: 1200,
            status: InsightsBatchStatus::Pending,
            error: None,
        }
    }

    fn finding(analysis_id: &str, id: &str) -> FindingRecord {
        FindingRecord {
            id: id.to_string(),
            analysis_id: analysis_id.to_string(),
            batch: 0,
            ordinal: 0,
            title: format!("Finding {id}"),
            severity: Severity::High,
            category: FindingCategory::DataLoss,
            path: "refund/service.go".to_string(),
            line: 52,
            end_line: 56,
            explanation: "Explained.".to_string(),
            suggested_direction: "Directed.".to_string(),
            evidence: vec!["Line 52 calls the gateway.".to_string()],
            related: Vec::new(),
            location_problem: None,
            cited_text: "ref, err := s.gateway.Refund(ctx)".to_string(),
            verdict: FindingVerdict::Pending,
            verdict_at: None,
            verdict_fingerprint: None,
            resolution: None,
        }
    }

    #[test]
    fn keeps_an_analysis_its_calls_and_how_each_ended() {
        let store = Store::open_in_memory().unwrap();
        let review_id = review(&store);
        store
            .insert_analysis(
                &analysis(&review_id, "a1", "2026-11-12T10:00:00Z"),
                &[batch(0, &["a.go", "a_test.go"]), batch(1, &["b.go"])],
            )
            .unwrap();
        let failure = AppError::untranslated(ErrorCode::RateLimited, "Slow down.");
        store
            .finish_batch("a1", 1, InsightsBatchStatus::Failed, Some(&failure))
            .unwrap();
        store
            .finish_batch("a1", 0, InsightsBatchStatus::Done, None)
            .unwrap();
        store
            .set_analysis_model("a1", "claude-opus-5-5-20261101")
            .unwrap();

        let batches = store.batches("a1").unwrap();
        assert_eq!(batches[0].paths, ["a.go", "a_test.go"]);
        assert_eq!(batches[0].status, InsightsBatchStatus::Done);
        assert_eq!(batches[1].status, InsightsBatchStatus::Failed);
        assert_eq!(batches[1].error, Some(failure));
        assert_eq!(
            store.analyses(&review_id).unwrap()[0].model,
            "claude-opus-5-5-20261101"
        );
    }

    #[test]
    fn a_failed_call_keeps_the_reason_params_and_detail_of_its_error() {
        let store = Store::open_in_memory().unwrap();
        let review_id = review(&store);
        let mut failed = batch(0, &["a.go"]);
        failed.status = InsightsBatchStatus::Failed;
        failed.error = Some(app_error!(
            RateLimited,
            ModelRateLimitedRetryAfterSaid,
            "{provider} is limiting requests; try again in {seconds} seconds. It said: {detail}",
            provider = crate::ai::provider_phrase(crate::model::ModelProviderKind::Anthropic, true),
            seconds = "30",
            detail = "Slow down."
        ));
        store
            .insert_analysis(
                &analysis(&review_id, "a1", "2026-11-12T10:00:00Z"),
                &[failed.clone(), batch(1, &["b.go"])],
            )
            .unwrap();
        let cancelled = app_error!(Cancelled, Stopped, "Stopped.");
        store
            .finish_batch("a1", 1, InsightsBatchStatus::Failed, Some(&cancelled))
            .unwrap();

        let batches = store.batches("a1").unwrap();
        assert_eq!(batches[0].error, failed.error);
        assert_eq!(batches[1].error, Some(cancelled));
    }

    #[test]
    fn a_failure_stored_before_errors_had_reasons_reads_back_by_its_message() {
        let store = Store::open_in_memory().unwrap();
        let review_id = review(&store);
        store
            .insert_analysis(
                &analysis(&review_id, "a1", "2026-11-12T10:00:00Z"),
                &[batch(0, &["a.go"]), batch(1, &["b.go"])],
            )
            .unwrap();
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE insight_batches SET status = 'failed', error_code = 'rate_limited',
                     error_message = 'Slow down.' WHERE position = 0",
                [],
            )
            .unwrap();
        // A reason this version does not know reads back the same way.
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE insight_batches SET status = 'failed', error_code = 'internal',
                     error_message = 'Gone.', error_reason = 'no_such_reason',
                     error_params = '{\"path\":\"a.go\"}' WHERE position = 1",
                [],
            )
            .unwrap();

        let batches = store.batches("a1").unwrap();
        let legacy = batches[0].error.clone().unwrap();
        assert_eq!(
            legacy,
            AppError::untranslated(ErrorCode::RateLimited, "Slow down.")
        );
        assert_eq!(legacy.reason, None);
        assert_eq!(
            batches[1].error,
            Some(AppError::untranslated(ErrorCode::Internal, "Gone."))
        );
    }

    #[test]
    fn a_newer_analysis_keeps_every_earlier_finding_and_drops_only_empty_analyses() {
        let store = Store::open_in_memory().unwrap();
        let review_id = review(&store);
        store
            .insert_analysis(&analysis(&review_id, "old", "2026-11-12T10:00:00Z"), &[])
            .unwrap();
        store
            .insert_analysis(&analysis(&review_id, "empty", "2026-11-11T10:00:00Z"), &[])
            .unwrap();
        store
            .insert_findings(&[finding("old", "pending"), finding("old", "confirmed")])
            .unwrap();
        store
            .set_verdict(
                "confirmed",
                FindingVerdict::Confirmed,
                Some("2026-11-12T11:00:00Z"),
                Some("sha256:abc"),
            )
            .unwrap();
        store
            .insert_analysis(&analysis(&review_id, "new", "2026-11-19T10:00:00Z"), &[])
            .unwrap();

        store.drop_empty_analyses(&review_id, "new").unwrap();

        let ids: Vec<String> = store
            .findings(&review_id)
            .unwrap()
            .into_iter()
            .map(|finding| finding.id)
            .collect();
        assert_eq!(ids, ["pending", "confirmed"]);
        let analyses: Vec<String> = store
            .analyses(&review_id)
            .unwrap()
            .into_iter()
            .map(|analysis| analysis.id)
            .collect();
        // The newest is kept even with no finding of its own.
        assert_eq!(analyses, ["new", "old"]);

        let kept = store.finding(&review_id, "confirmed").unwrap().unwrap();
        assert_eq!(kept.verdict, FindingVerdict::Confirmed);
        assert_eq!(kept.verdict_fingerprint.as_deref(), Some("sha256:abc"));
        assert_eq!(kept.category, FindingCategory::DataLoss);
        assert_eq!(kept.evidence, ["Line 52 calls the gateway."]);
        assert_eq!(kept.resolution, None);
        assert_eq!(store.finding(&review_id, "missing").unwrap(), None);
    }

    #[test]
    fn a_finding_keeps_how_it_was_fixed_until_it_is_reopened_and_its_verdict_throughout() {
        let store = Store::open_in_memory().unwrap();
        let review_id = review(&store);
        store
            .insert_analysis(&analysis(&review_id, "a1", "2026-11-12T10:00:00Z"), &[])
            .unwrap();
        store.insert_findings(&[finding("a1", "f1")]).unwrap();
        store
            .set_verdict("f1", FindingVerdict::Confirmed, Some("2026-11-12T11:00:00Z"), None)
            .unwrap();
        let checked = FindingResolution {
            kind: crate::model::ResolutionKind::ModelChecked,
            model: Some("claude-opus-5-5-20261101".to_string()),
            at: "2026-11-19T10:00:00Z".to_string(),
            reason: Some("The refund is now saved before the gateway call.".to_string()),
        };

        store.set_resolution("f1", Some(&checked)).unwrap();
        let fixed = store.finding(&review_id, "f1").unwrap().unwrap();
        assert_eq!(fixed.resolution, Some(checked));
        assert_eq!(fixed.verdict, FindingVerdict::Confirmed);

        store.set_resolution("f1", None).unwrap();
        let reopened = store.finding(&review_id, "f1").unwrap().unwrap();
        assert_eq!(reopened.resolution, None);
        assert_eq!(reopened.verdict, FindingVerdict::Confirmed);
    }

    #[test]
    fn a_relocated_finding_points_at_its_new_lines() {
        let store = Store::open_in_memory().unwrap();
        let review_id = review(&store);
        store
            .insert_analysis(&analysis(&review_id, "a1", "2026-11-12T10:00:00Z"), &[])
            .unwrap();
        let mut lost = finding("a1", "f1");
        lost.location_problem = Some(LocationProblem::OutsideFile);
        store.insert_findings(&[lost]).unwrap();

        store.relocate_finding("f1", 60, 61, "a\nb").unwrap();

        let moved = store.finding(&review_id, "f1").unwrap().unwrap();
        assert_eq!((moved.line, moved.end_line, moved.cited_text.as_str()), (60, 61, "a\nb"));
        assert_eq!(moved.location_problem, None);
    }

    #[test]
    fn findings_kept_before_fixes_existed_read_as_open() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("difflens.db");
        let before = super::super::MIGRATIONS.len() - 1;
        {
            let connection = rusqlite::Connection::open(&file).unwrap();
            for migration in &super::super::MIGRATIONS[..before] {
                connection.execute_batch(migration).unwrap();
            }
            connection.pragma_update(None, "user_version", before).unwrap();
            connection
                .execute_batch(
                    "INSERT INTO projects (id, path, added_at, integration_branches)
                         VALUES ('p', '/repos/p', 't', 'main');
                     INSERT INTO reviews (id, project_id, target_kind, branch, base, created_at)
                         VALUES ('r', 'p', 'branch', 'feature', 'main', 't');
                     INSERT INTO insight_analyses (id, review_id, head_sha, provider, model,
                         created_at, language)
                         VALUES ('a1', 'r', 'e7f3b10', 'anthropic', 'm', 't', 'en');
                     INSERT INTO insight_findings (id, analysis_id, batch, ordinal, title,
                         severity, category, path, line, end_line, explanation,
                         suggested_direction, evidence, related, cited_text, verdict)
                         VALUES ('f1', 'a1', 0, 0, 'Kept', 'high', 'correctness', 'a.go', 3, 4,
                         'E.', 'D.', '[]', '[]', 'x', 'confirmed');",
                )
                .unwrap();
        }

        let store = Store::open(&file).unwrap();
        let kept = store.finding("r", "f1").unwrap().unwrap();
        assert_eq!((kept.title.as_str(), kept.verdict), ("Kept", FindingVerdict::Confirmed));
        assert_eq!(kept.resolution, None);
        store
            .set_resolution("f1", Some(&user_marked("2026-11-20T09:00:00Z")))
            .unwrap();
        assert_eq!(
            store.finding("r", "f1").unwrap().unwrap().resolution,
            Some(user_marked("2026-11-20T09:00:00Z"))
        );
    }

    #[test]
    fn findings_go_with_their_review() {
        let store = Store::open_in_memory().unwrap();
        let review_id = review(&store);
        store
            .insert_analysis(
                &analysis(&review_id, "a1", "2026-11-12T10:00:00Z"),
                &[batch(0, &["a.go"])],
            )
            .unwrap();
        store.insert_findings(&[finding("a1", "f1")]).unwrap();
        store.delete("p").unwrap();
        assert!(store.findings(&review_id).unwrap().is_empty());
        assert!(store.batches("a1").unwrap().is_empty());
    }
}
