//! Explanations of a review's failed tests (`test_diagnoses`): per failing
//! test, by its package and top-level name (an empty name for a package that
//! failed outside its tests), the latest explanation a model wrote, with when
//! the run it explained started, fingerprints of the failure and of the code
//! the model was shown, who wrote it and in what language, and the
//! translations of its prose the user asked for. A new explanation of a
//! failure replaces the earlier one and drops its translations. The review
//! comment written from an explanation is linked to its failure, and stays
//! linked when the failure is explained again, until the comment is deleted.

use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};

use super::Store;
use crate::error::AppResult;
use crate::language::Language;
use crate::model::{DiagnosisConfidence, DiagnosisFault, SourceRef};

/// An explanation of a failure, as kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosisRecord {
    pub review_id: String,
    pub package: String,
    /// Empty for a package that failed outside its tests.
    pub test: String,
    /// When the run it explained started (RFC 3339).
    pub run_started_at: String,
    /// Of the failure as that run read it (see `test_diagnosis`).
    pub failure_fingerprint: String,
    /// Of the code the model was shown.
    pub code_fingerprint: String,
    /// `None` for the working tree.
    pub head_sha: Option<String>,
    /// Key of the provider, such as "anthropic".
    pub provider: String,
    pub model: String,
    pub created_at: String,
    pub language: Language,
    pub cause: String,
    pub fault: DiagnosisFault,
    pub confidence: DiagnosisConfidence,
    pub location: Option<SourceRef>,
    pub fix: String,
    pub fix_code: String,
    /// The code around `location` as the model was shown it; None without a
    /// location, or for an explanation kept before this was.
    pub code_excerpt: Option<String>,
    /// Such as "data race"; None for an explanation kept before this was.
    pub failure_kind: Option<String>,
}

/// The prose of an explanation of a failure in another language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosisTranslationRecord {
    pub language: Language,
    pub cause: String,
    pub fix: String,
}

pub fn fault_key(fault: DiagnosisFault) -> &'static str {
    match fault {
        DiagnosisFault::Code => "code",
        DiagnosisFault::Test => "test",
        DiagnosisFault::Flaky => "flaky",
        DiagnosisFault::Environment => "environment",
    }
}

pub fn fault_of(key: &str) -> Option<DiagnosisFault> {
    match key {
        "code" => Some(DiagnosisFault::Code),
        "test" => Some(DiagnosisFault::Test),
        "flaky" => Some(DiagnosisFault::Flaky),
        "environment" => Some(DiagnosisFault::Environment),
        _ => None,
    }
}

pub fn confidence_key(confidence: DiagnosisConfidence) -> &'static str {
    match confidence {
        DiagnosisConfidence::High => "high",
        DiagnosisConfidence::Medium => "medium",
        DiagnosisConfidence::Low => "low",
    }
}

pub fn confidence_of(key: &str) -> Option<DiagnosisConfidence> {
    match key {
        "high" => Some(DiagnosisConfidence::High),
        "medium" => Some(DiagnosisConfidence::Medium),
        "low" => Some(DiagnosisConfidence::Low),
        _ => None,
    }
}

impl Store {
    /// The review's explanations of failures, by package and test.
    pub fn diagnoses(&self, review_id: &str) -> AppResult<Vec<DiagnosisRecord>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT review_id, package, test, run_started_at, failure_fingerprint,
                 code_fingerprint, head_sha, provider, model, created_at, language, cause, fault,
                 confidence, location_path, location_line, fix, fix_code, code_excerpt,
                 failure_kind
             FROM test_diagnoses WHERE review_id = ?1 ORDER BY package, test",
        )?;
        let records = statement
            .query_map(params![review_id], |row| {
                let language: String = row.get(10)?;
                let fault: String = row.get(12)?;
                let confidence: String = row.get(13)?;
                let path: Option<String> = row.get(14)?;
                let line: Option<u32> = row.get(15)?;
                Ok(DiagnosisRecord {
                    review_id: row.get(0)?,
                    package: row.get(1)?,
                    test: row.get(2)?,
                    run_started_at: row.get(3)?,
                    failure_fingerprint: row.get(4)?,
                    code_fingerprint: row.get(5)?,
                    head_sha: row.get(6)?,
                    provider: row.get(7)?,
                    model: row.get(8)?,
                    created_at: row.get(9)?,
                    language: Language::from_tag(&language).unwrap_or_default(),
                    cause: row.get(11)?,
                    // A value a later version wrote reads as the plainest one.
                    fault: fault_of(&fault).unwrap_or(DiagnosisFault::Code),
                    confidence: confidence_of(&confidence).unwrap_or(DiagnosisConfidence::Low),
                    location: path
                        .zip(line)
                        .map(|(file, line)| SourceRef { file, line }),
                    fix: row.get(16)?,
                    fix_code: row.get(17)?,
                    code_excerpt: row.get(18)?,
                    failure_kind: row.get(19)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    /// The review comments written from the review's explanations, by
    /// package and test.
    pub fn diagnosis_comments(&self, review_id: &str) -> AppResult<HashMap<(String, String), String>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT package, test, comment_id FROM test_diagnoses
             WHERE review_id = ?1 AND comment_id IS NOT NULL",
        )?;
        let links = statement
            .query_map(params![review_id], |row| {
                Ok(((row.get(0)?, row.get(1)?), row.get(2)?))
            })?
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(links)
    }

    /// The review comment written from the explanation of one failure.
    pub fn diagnosis_comment(
        &self,
        review_id: &str,
        package: &str,
        test: &str,
    ) -> AppResult<Option<String>> {
        let found = self
            .lock()?
            .query_row(
                "SELECT comment_id FROM test_diagnoses
                 WHERE review_id = ?1 AND package = ?2 AND test = ?3",
                params![review_id, package, test],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        Ok(found.flatten())
    }

    /// Links the review comment `comment_id` to the explanation of a
    /// failure, in place of an earlier one. Nothing happens when the failure
    /// has no explanation.
    pub fn link_diagnosis_comment(
        &self,
        review_id: &str,
        package: &str,
        test: &str,
        comment_id: &str,
    ) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE test_diagnoses SET comment_id = ?4
             WHERE review_id = ?1 AND package = ?2 AND test = ?3",
            params![review_id, package, test, comment_id],
        )?;
        Ok(())
    }

    /// Saves an explanation of a failure, replacing the earlier one of the
    /// same failure and the translations of it; the review comment written
    /// from the earlier one stays linked.
    pub fn save_diagnosis(&self, record: &DiagnosisRecord) -> AppResult<()> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM test_diagnosis_translations
             WHERE review_id = ?1 AND package = ?2 AND test = ?3",
            params![record.review_id, record.package, record.test],
        )?;
        transaction.execute(
            "INSERT INTO test_diagnoses (review_id, package, test, run_started_at,
                 failure_fingerprint, code_fingerprint, head_sha, provider, model, created_at,
                 language, cause, fault, confidence, location_path, location_line, fix, fix_code,
                 code_excerpt, failure_kind)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                 ?18, ?19, ?20)
             ON CONFLICT (review_id, package, test) DO UPDATE SET
                 run_started_at = excluded.run_started_at,
                 failure_fingerprint = excluded.failure_fingerprint,
                 code_fingerprint = excluded.code_fingerprint, head_sha = excluded.head_sha,
                 provider = excluded.provider, model = excluded.model,
                 created_at = excluded.created_at, language = excluded.language,
                 cause = excluded.cause, fault = excluded.fault,
                 confidence = excluded.confidence, location_path = excluded.location_path,
                 location_line = excluded.location_line, fix = excluded.fix,
                 fix_code = excluded.fix_code, code_excerpt = excluded.code_excerpt,
                 failure_kind = excluded.failure_kind",
            params![
                record.review_id,
                record.package,
                record.test,
                record.run_started_at,
                record.failure_fingerprint,
                record.code_fingerprint,
                record.head_sha,
                record.provider,
                record.model,
                record.created_at,
                record.language.tag(),
                record.cause,
                fault_key(record.fault),
                confidence_key(record.confidence),
                record.location.as_ref().map(|location| location.file.as_str()),
                record.location.as_ref().map(|location| location.line),
                record.fix,
                record.fix_code,
                record.code_excerpt,
                record.failure_kind
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// The translations of the review's explanations of failures, by package
    /// and test.
    pub fn diagnosis_translations(
        &self,
        review_id: &str,
    ) -> AppResult<HashMap<(String, String), Vec<DiagnosisTranslationRecord>>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT package, test, language, cause, fix FROM test_diagnosis_translations
             WHERE review_id = ?1 ORDER BY package, test, language",
        )?;
        let rows = statement
            .query_map(params![review_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut translations: HashMap<(String, String), Vec<DiagnosisTranslationRecord>> =
            HashMap::new();
        for (package, test, language, cause, fix) in rows {
            let Some(language) = Language::from_tag(&language) else {
                continue;
            };
            translations
                .entry((package, test))
                .or_default()
                .push(DiagnosisTranslationRecord {
                    language,
                    cause,
                    fix,
                });
        }
        Ok(translations)
    }

    /// Keeps translations of explanations of failures, each replacing an
    /// earlier one of its failure in the same language. One whose
    /// explanation is gone is left out.
    pub fn save_diagnosis_translations(
        &self,
        review_id: &str,
        translations: &[((String, String), DiagnosisTranslationRecord)],
    ) -> AppResult<()> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        for ((package, test), translation) in translations {
            transaction.execute(
                "INSERT INTO test_diagnosis_translations (review_id, package, test, language,
                     cause, fix)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6
                 WHERE EXISTS (SELECT 1 FROM test_diagnoses
                               WHERE review_id = ?1 AND package = ?2 AND test = ?3)
                 ON CONFLICT (review_id, package, test, language) DO UPDATE SET
                     cause = excluded.cause, fix = excluded.fix",
                params![
                    review_id,
                    package,
                    test,
                    translation.language.tag(),
                    translation.cause,
                    translation.fix
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
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
                    branch: "feature/AUTH-12".to_string(),
                    base: "main".to_string(),
                },
                "2026-11-01T00:00:00Z",
            )
            .unwrap()
            .id
    }

    fn record(review_id: &str, package: &str, test: &str, cause: &str) -> DiagnosisRecord {
        DiagnosisRecord {
            review_id: review_id.to_string(),
            package: package.to_string(),
            test: test.to_string(),
            run_started_at: "2026-11-21T09:00:00Z".to_string(),
            failure_fingerprint: "sha256:failure".to_string(),
            code_fingerprint: "sha256:code".to_string(),
            head_sha: Some("e7f3b10".to_string()),
            provider: "anthropic".to_string(),
            model: "claude-opus-5-5".to_string(),
            created_at: "2026-11-21T10:00:00Z".to_string(),
            language: Language::English,
            cause: cause.to_string(),
            fault: DiagnosisFault::Code,
            confidence: DiagnosisConfidence::High,
            location: Some(SourceRef {
                file: "app/services/auth/service.go".to_string(),
                line: 661,
            }),
            fix: "Hold the mutex while writing lastLogin.".to_string(),
            fix_code: "s.mu.Lock()\ndefer s.mu.Unlock()".to_string(),
            code_excerpt: Some("+   661 | \ts.lastLogin = at\n".to_string()),
            failure_kind: Some("data race".to_string()),
        }
    }

    fn portuguese(cause: &str) -> DiagnosisTranslationRecord {
        DiagnosisTranslationRecord {
            language: Language::BrazilianPortuguese,
            cause: cause.to_string(),
            fix: "Segure o mutex ao gravar lastLogin.".to_string(),
        }
    }

    fn key(package: &str, test: &str) -> (String, String) {
        (package.to_string(), test.to_string())
    }

    #[test]
    fn a_diagnosis_is_kept_per_failure_and_a_new_one_drops_its_translations() {
        let store = Store::open_in_memory().unwrap();
        let review_id = review(&store);
        assert!(store.diagnoses(&review_id).unwrap().is_empty());

        let test = record(&review_id, "app/services/auth", "TestLogin", "The write races.");
        let mut build = record(&review_id, "app/containers", "", "NewService takes three.");
        build.location = None;
        build.fault = DiagnosisFault::Environment;
        build.confidence = DiagnosisConfidence::Low;
        build.head_sha = None;
        store.save_diagnosis(&test).unwrap();
        store.save_diagnosis(&build).unwrap();
        assert_eq!(store.diagnoses(&review_id).unwrap(), vec![build.clone(), test.clone()]);

        store
            .save_diagnosis_translations(
                &review_id,
                &[
                    (key("app/services/auth", "TestLogin"), portuguese("A escrita corre.")),
                    (key("app/services/auth", "TestGone"), portuguese("x")),
                ],
            )
            .unwrap();
        let translations = store.diagnosis_translations(&review_id).unwrap();
        assert_eq!(translations.len(), 1);
        assert_eq!(
            translations[&key("app/services/auth", "TestLogin")],
            vec![portuguese("A escrita corre.")]
        );

        // Explained again: replaced, and its translations go.
        let again = record(&review_id, "app/services/auth", "TestLogin", "The read races.");
        store.save_diagnosis(&again).unwrap();
        assert_eq!(store.diagnoses(&review_id).unwrap(), vec![build, again]);
        assert!(store.diagnosis_translations(&review_id).unwrap().is_empty());
    }

    #[test]
    fn the_comment_written_from_an_explanation_stays_linked_until_it_is_deleted() {
        use crate::model::{CommentKind, CommentState, ReviewSide};
        use crate::store::CommentRecord;

        let store = Store::open_in_memory().unwrap();
        let review_id = review(&store);
        store
            .save_diagnosis(&record(&review_id, "app/auth", "TestLogin", "The write races."))
            .unwrap();
        assert_eq!(store.diagnosis_comment(&review_id, "app/auth", "TestLogin").unwrap(), None);
        let comment = CommentRecord {
            id: "c-1".to_string(),
            review_id: review_id.clone(),
            kind: CommentKind::General,
            parent_id: None,
            path: String::new(),
            side: ReviewSide::New,
            line: 0,
            line_text: String::new(),
            commit_sha: None,
            body: "Hold the mutex.".to_string(),
            state: CommentState::Draft,
            remote_id: None,
            remote_author_id: None,
            remote_author_name: None,
            resolved: false,
            created_at: "t".to_string(),
            updated_at: "t".to_string(),
            remote_pull_request: None,
            edited_locally: false,
            delete_pending: false,
            publish_error: None,
        };
        store.insert_comment(&comment).unwrap();
        store
            .link_diagnosis_comment(&review_id, "app/auth", "TestLogin", "c-1")
            .unwrap();
        // A failure never explained has nothing to link.
        store
            .link_diagnosis_comment(&review_id, "app/auth", "TestGone", "c-1")
            .unwrap();
        assert_eq!(
            store.diagnosis_comments(&review_id).unwrap(),
            HashMap::from([(key("app/auth", "TestLogin"), "c-1".to_string())])
        );

        // Explained again: still linked.
        store
            .save_diagnosis(&record(&review_id, "app/auth", "TestLogin", "The read races."))
            .unwrap();
        assert_eq!(
            store.diagnosis_comment(&review_id, "app/auth", "TestLogin").unwrap().as_deref(),
            Some("c-1")
        );
        // The comment deleted: the link goes with it.
        store.delete_comment("c-1").unwrap();
        assert_eq!(store.diagnosis_comment(&review_id, "app/auth", "TestLogin").unwrap(), None);
        assert_eq!(store.diagnoses(&review_id).unwrap()[0].cause, "The read races.");
    }

    #[test]
    fn a_diagnosis_survives_reopening_the_database() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("difflens.sqlite");
        let review_id = {
            let store = Store::open(&file).unwrap();
            let review_id = review(&store);
            store
                .save_diagnosis(&record(&review_id, "app/x", "TestX", "Cause."))
                .unwrap();
            review_id
        };
        let store = Store::open(&file).unwrap();
        assert_eq!(store.diagnoses(&review_id).unwrap()[0].cause, "Cause.");
    }

    #[test]
    fn a_database_from_before_diagnoses_gets_them_and_keeps_its_reviews() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("difflens.sqlite");
        let review_id = {
            let store = Store::open(&file).unwrap();
            let review_id = review(&store);
            // As a database of the version before diagnoses: four migrations
            // behind (diagnoses, then general comments and the diagnoses'
            // links to comments, then drafts written from signals, then
            // fixed findings).
            let connection = store.lock().unwrap();
            connection
                .execute_batch(
                    "ALTER TABLE insight_findings DROP COLUMN resolution;
                     ALTER TABLE insight_findings DROP COLUMN resolved_at;
                     ALTER TABLE insight_findings DROP COLUMN resolved_model;
                     ALTER TABLE insight_findings DROP COLUMN resolution_reason;
                     DROP TABLE signal_comments;
                     DROP TABLE test_diagnosis_translations; DROP TABLE test_diagnoses;
                     ALTER TABLE review_comments DROP COLUMN kind",
                )
                .unwrap();
            let version: i64 = connection
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            connection
                .pragma_update(None, "user_version", version - 4)
                .unwrap();
            review_id
        };
        let store = Store::open(&file).unwrap();
        assert!(store.diagnoses(&review_id).unwrap().is_empty());
        store
            .save_diagnosis(&record(&review_id, "app/x", "TestX", "Cause."))
            .unwrap();
        assert_eq!(store.diagnoses(&review_id).unwrap().len(), 1);
    }

    #[test]
    fn keys_round_trip() {
        for fault in [
            DiagnosisFault::Code,
            DiagnosisFault::Test,
            DiagnosisFault::Flaky,
            DiagnosisFault::Environment,
        ] {
            assert_eq!(fault_of(fault_key(fault)), Some(fault));
        }
        for confidence in [
            DiagnosisConfidence::High,
            DiagnosisConfidence::Medium,
            DiagnosisConfidence::Low,
        ] {
            assert_eq!(confidence_of(confidence_key(confidence)), Some(confidence));
        }
        assert_eq!(fault_of("bad luck"), None);
    }
}
