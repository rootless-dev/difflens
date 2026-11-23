//! Explanations of a review's functions, at two levels kept apart: the
//! detailed one in three parts (`function_explanations`, which also holds
//! every explanation written before short ones existed) and the short one
//! (`function_briefs`). Per function and level, the latest one a model
//! wrote, with a fingerprint of the function's lines then, who wrote it and
//! in what language, and the translations of its text the user asked for. A
//! new explanation of a function replaces the one of its level and drops its
//! translations; the other level stays.

use std::collections::HashMap;

use rusqlite::params;

use super::Store;
use crate::error::AppResult;
use crate::language::Language;
use crate::model::{BriefTranslation, ExplanationKind, ExplanationTranslation};

/// A short explanation, as kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefRecord {
    pub review_id: String,
    pub function_id: String,
    pub kind: ExplanationKind,
    /// Of the function's lines when it was explained (see `fingerprint`).
    pub fingerprint: String,
    /// `None` for the working tree.
    pub head_sha: Option<String>,
    /// Key of the provider, such as "anthropic".
    pub provider: String,
    pub model: String,
    pub created_at: String,
    pub language: Language,
    pub text: String,
}

fn kind_of(key: &str) -> ExplanationKind {
    if key == "context" {
        ExplanationKind::Context
    } else {
        ExplanationKind::Changed
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplanationRecord {
    pub review_id: String,
    pub function_id: String,
    pub kind: ExplanationKind,
    /// Of the function's lines when it was explained (see `fingerprint`).
    pub fingerprint: String,
    /// `None` for the working tree.
    pub head_sha: Option<String>,
    /// Key of the provider, such as "anthropic".
    pub provider: String,
    pub model: String,
    pub created_at: String,
    pub language: Language,
    pub what_changed: String,
    pub why: String,
    pub why_inferred: bool,
    pub purpose: String,
}

fn kind_key(kind: ExplanationKind) -> &'static str {
    match kind {
        ExplanationKind::Changed => "changed",
        ExplanationKind::Context => "context",
    }
}

impl Store {
    /// The review's explanations, by function id.
    pub fn explanations(&self, review_id: &str) -> AppResult<Vec<ExplanationRecord>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT review_id, function_id, kind, fingerprint, head_sha, provider, model,
                 created_at, language, what_changed, why, why_inferred, purpose
             FROM function_explanations WHERE review_id = ?1 ORDER BY function_id",
        )?;
        let records = statement
            .query_map(params![review_id], |row| {
                let kind: String = row.get(2)?;
                let language: String = row.get(8)?;
                Ok(ExplanationRecord {
                    review_id: row.get(0)?,
                    function_id: row.get(1)?,
                    kind: kind_of(&kind),
                    fingerprint: row.get(3)?,
                    head_sha: row.get(4)?,
                    provider: row.get(5)?,
                    model: row.get(6)?,
                    created_at: row.get(7)?,
                    language: Language::from_tag(&language).unwrap_or_default(),
                    what_changed: row.get(9)?,
                    why: row.get(10)?,
                    why_inferred: row.get(11)?,
                    purpose: row.get(12)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    /// Saves explanations, each replacing an earlier one of its function and
    /// the translations of it.
    pub fn save_explanations(&self, records: &[ExplanationRecord]) -> AppResult<()> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        for record in records {
            transaction.execute(
                "DELETE FROM function_explanation_translations
                 WHERE review_id = ?1 AND function_id = ?2",
                params![record.review_id, record.function_id],
            )?;
            transaction.execute(
                "INSERT INTO function_explanations (review_id, function_id, kind, fingerprint,
                     head_sha, provider, model, created_at, language, what_changed, why,
                     why_inferred, purpose)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT (review_id, function_id) DO UPDATE SET
                     kind = excluded.kind, fingerprint = excluded.fingerprint,
                     head_sha = excluded.head_sha, provider = excluded.provider,
                     model = excluded.model, created_at = excluded.created_at,
                     language = excluded.language, what_changed = excluded.what_changed,
                     why = excluded.why, why_inferred = excluded.why_inferred,
                     purpose = excluded.purpose",
                params![
                    record.review_id,
                    record.function_id,
                    kind_key(record.kind),
                    record.fingerprint,
                    record.head_sha,
                    record.provider,
                    record.model,
                    record.created_at,
                    record.language.tag(),
                    record.what_changed,
                    record.why,
                    record.why_inferred,
                    record.purpose
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// The translations of the review's explanations, by function id.
    pub fn explanation_translations(
        &self,
        review_id: &str,
    ) -> AppResult<HashMap<String, Vec<ExplanationTranslation>>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT function_id, language, what_changed, why, purpose
             FROM function_explanation_translations WHERE review_id = ?1
             ORDER BY function_id, language",
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
        let mut translations: HashMap<String, Vec<ExplanationTranslation>> = HashMap::new();
        for (function_id, language, what_changed, why, purpose) in rows {
            let Some(language) = Language::from_tag(&language) else {
                continue;
            };
            translations
                .entry(function_id)
                .or_default()
                .push(ExplanationTranslation {
                    language,
                    what_changed,
                    why,
                    purpose,
                });
        }
        Ok(translations)
    }

    /// Keeps translations of explanations, each replacing an earlier one of
    /// its function in the same language. One whose explanation is gone is
    /// left out.
    pub fn save_explanation_translations(
        &self,
        review_id: &str,
        translations: &[(String, ExplanationTranslation)],
    ) -> AppResult<()> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        for (function_id, translation) in translations {
            transaction.execute(
                "INSERT INTO function_explanation_translations (review_id, function_id,
                     language, what_changed, why, purpose)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6
                 WHERE EXISTS (SELECT 1 FROM function_explanations
                               WHERE review_id = ?1 AND function_id = ?2)
                 ON CONFLICT (review_id, function_id, language) DO UPDATE SET
                     what_changed = excluded.what_changed, why = excluded.why,
                     purpose = excluded.purpose",
                params![
                    review_id,
                    function_id,
                    translation.language.tag(),
                    translation.what_changed,
                    translation.why,
                    translation.purpose
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// The review's short explanations, by function id.
    pub fn briefs(&self, review_id: &str) -> AppResult<Vec<BriefRecord>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT review_id, function_id, kind, fingerprint, head_sha, provider, model,
                 created_at, language, text
             FROM function_briefs WHERE review_id = ?1 ORDER BY function_id",
        )?;
        let records = statement
            .query_map(params![review_id], |row| {
                let kind: String = row.get(2)?;
                let language: String = row.get(8)?;
                Ok(BriefRecord {
                    review_id: row.get(0)?,
                    function_id: row.get(1)?,
                    kind: kind_of(&kind),
                    fingerprint: row.get(3)?,
                    head_sha: row.get(4)?,
                    provider: row.get(5)?,
                    model: row.get(6)?,
                    created_at: row.get(7)?,
                    language: Language::from_tag(&language).unwrap_or_default(),
                    text: row.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    /// Saves short explanations, each replacing an earlier short one of its
    /// function and the translations of it; the detailed one stays.
    pub fn save_briefs(&self, records: &[BriefRecord]) -> AppResult<()> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        for record in records {
            transaction.execute(
                "DELETE FROM function_brief_translations WHERE review_id = ?1 AND function_id = ?2",
                params![record.review_id, record.function_id],
            )?;
            transaction.execute(
                "INSERT INTO function_briefs (review_id, function_id, kind, fingerprint, head_sha,
                     provider, model, created_at, language, text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT (review_id, function_id) DO UPDATE SET
                     kind = excluded.kind, fingerprint = excluded.fingerprint,
                     head_sha = excluded.head_sha, provider = excluded.provider,
                     model = excluded.model, created_at = excluded.created_at,
                     language = excluded.language, text = excluded.text",
                params![
                    record.review_id,
                    record.function_id,
                    kind_key(record.kind),
                    record.fingerprint,
                    record.head_sha,
                    record.provider,
                    record.model,
                    record.created_at,
                    record.language.tag(),
                    record.text
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// The translations of the review's short explanations, by function id.
    pub fn brief_translations(
        &self,
        review_id: &str,
    ) -> AppResult<HashMap<String, Vec<BriefTranslation>>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT function_id, language, text FROM function_brief_translations
             WHERE review_id = ?1 ORDER BY function_id, language",
        )?;
        let rows = statement
            .query_map(params![review_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut translations: HashMap<String, Vec<BriefTranslation>> = HashMap::new();
        for (function_id, language, text) in rows {
            let Some(language) = Language::from_tag(&language) else {
                continue;
            };
            translations
                .entry(function_id)
                .or_default()
                .push(BriefTranslation { language, text });
        }
        Ok(translations)
    }

    /// Keeps translations of short explanations, each replacing an earlier
    /// one of its function in the same language. One whose short
    /// explanation is gone is left out.
    pub fn save_brief_translations(
        &self,
        review_id: &str,
        translations: &[(String, BriefTranslation)],
    ) -> AppResult<()> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        for (function_id, translation) in translations {
            transaction.execute(
                "INSERT INTO function_brief_translations (review_id, function_id, language, text)
                 SELECT ?1, ?2, ?3, ?4
                 WHERE EXISTS (SELECT 1 FROM function_briefs
                               WHERE review_id = ?1 AND function_id = ?2)
                 ON CONFLICT (review_id, function_id, language) DO UPDATE SET
                     text = excluded.text",
                params![
                    review_id,
                    function_id,
                    translation.language.tag(),
                    translation.text
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
                    branch: "feature/PAY-482".to_string(),
                    base: "main".to_string(),
                },
                "2026-11-01T00:00:00Z",
            )
            .unwrap()
            .id
    }

    fn record(review_id: &str, function_id: &str, purpose: &str) -> ExplanationRecord {
        ExplanationRecord {
            review_id: review_id.to_string(),
            function_id: function_id.to_string(),
            kind: ExplanationKind::Changed,
            fingerprint: "sha256:abc".to_string(),
            head_sha: Some("e7f3b10".to_string()),
            provider: "anthropic".to_string(),
            model: "claude-opus-5-5".to_string(),
            created_at: "2026-11-19T10:00:00Z".to_string(),
            language: Language::English,
            what_changed: "Calls the gateway first.".to_string(),
            why: "Inferred: refunds must be paid out.".to_string(),
            why_inferred: true,
            purpose: purpose.to_string(),
        }
    }

    fn portuguese(purpose: &str) -> ExplanationTranslation {
        ExplanationTranslation {
            language: Language::BrazilianPortuguese,
            what_changed: "Chama o gateway antes.".to_string(),
            why: "Inferência: reembolsos precisam ser pagos.".to_string(),
            purpose: purpose.to_string(),
        }
    }

    #[test]
    fn explanations_are_kept_per_function_and_a_new_one_drops_its_translations() {
        let store = Store::open_in_memory().unwrap();
        let review_id = review(&store);
        assert!(store.explanations(&review_id).unwrap().is_empty());

        let mut context = record(&review_id, "payment.Get", "Loads a payment.");
        context.kind = ExplanationKind::Context;
        store
            .save_explanations(&[
                record(&review_id, "refund.Request", "Requests a refund."),
                context.clone(),
            ])
            .unwrap();
        store
            .save_explanation_translations(
                &review_id,
                &[
                    (
                        "refund.Request".to_string(),
                        portuguese("Pede um reembolso."),
                    ),
                    ("gone".to_string(), portuguese("x")),
                ],
            )
            .unwrap();
        let translations = store.explanation_translations(&review_id).unwrap();
        assert_eq!(translations.len(), 1);
        assert_eq!(
            translations["refund.Request"],
            vec![portuguese("Pede um reembolso.")]
        );

        store
            .save_explanations(&[record(&review_id, "refund.Request", "Refunds a payment.")])
            .unwrap();
        let kept = store.explanations(&review_id).unwrap();
        assert_eq!(
            kept,
            vec![
                context,
                record(&review_id, "refund.Request", "Refunds a payment.")
            ]
        );
        assert!(store
            .explanation_translations(&review_id)
            .unwrap()
            .is_empty());
    }

    fn brief(review_id: &str, function_id: &str, text: &str) -> BriefRecord {
        BriefRecord {
            review_id: review_id.to_string(),
            function_id: function_id.to_string(),
            kind: ExplanationKind::Changed,
            fingerprint: "sha256:abc".to_string(),
            head_sha: Some("e7f3b10".to_string()),
            provider: "anthropic".to_string(),
            model: "claude-opus-5-5".to_string(),
            created_at: "2026-11-21T10:00:00Z".to_string(),
            language: Language::English,
            text: text.to_string(),
        }
    }

    #[test]
    fn both_levels_are_kept_apart_and_each_replaces_only_its_own() {
        let store = Store::open_in_memory().unwrap();
        let review_id = review(&store);
        // A detailed one, as every explanation written before short ones is.
        store
            .save_explanations(&[record(&review_id, "refund.Request", "Requests a refund.")])
            .unwrap();
        assert!(store.briefs(&review_id).unwrap().is_empty());

        store
            .save_briefs(&[brief(&review_id, "refund.Request", "Now pays first.")])
            .unwrap();
        store
            .save_brief_translations(
                &review_id,
                &[
                    (
                        "refund.Request".to_string(),
                        BriefTranslation {
                            language: Language::BrazilianPortuguese,
                            text: "Agora paga antes.".to_string(),
                        },
                    ),
                    (
                        "gone".to_string(),
                        BriefTranslation {
                            language: Language::BrazilianPortuguese,
                            text: "x".to_string(),
                        },
                    ),
                ],
            )
            .unwrap();
        assert_eq!(store.brief_translations(&review_id).unwrap().len(), 1);

        // A new short one drops its translations and leaves the detailed one.
        store
            .save_briefs(&[brief(&review_id, "refund.Request", "Pays before saving.")])
            .unwrap();
        assert_eq!(
            store.briefs(&review_id).unwrap(),
            vec![brief(&review_id, "refund.Request", "Pays before saving.")]
        );
        assert!(store.brief_translations(&review_id).unwrap().is_empty());
        assert_eq!(store.explanations(&review_id).unwrap().len(), 1);

        // A new detailed one leaves the short one.
        store
            .save_explanations(&[record(&review_id, "refund.Request", "Refunds a payment.")])
            .unwrap();
        assert_eq!(store.briefs(&review_id).unwrap().len(), 1);
    }
}
