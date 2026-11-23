//! The Overview summary of a review: the latest one the model wrote, with
//! the head it read, who wrote it and in what language, and the translations
//! of its text the user asked for. Writing a new one replaces it and drops
//! those translations.

use rusqlite::{params, OptionalExtension};

use super::Store;
use crate::error::{app_error, AppError, AppResult};
use crate::language::Language;
use crate::model::{Criterion, CriterionText, SummaryTranslation};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryRecord {
    pub review_id: String,
    /// `None` for the working tree.
    pub head_sha: Option<String>,
    /// Key of the provider, such as "anthropic".
    pub provider: String,
    pub model: String,
    pub created_at: String,
    pub condensed: bool,
    pub card_key: Option<String>,
    pub card_comments: u32,
    pub title: String,
    /// One sentence; the whole paragraph in a summary without `points`.
    pub lead: String,
    /// How the change works, a few short points; none in a summary written
    /// before they existed.
    pub points: Vec<String>,
    /// What the reviewer should know before reading; often none.
    pub attention: Vec<String>,
    pub card_summary: String,
    pub criteria: Vec<Criterion>,
    /// The language its text is written in; English before languages.
    pub language: Language,
}

fn unreadable(error: serde_json::Error) -> AppError {
    app_error!(
        Internal,
        StoredSummaryUnreadable,
        "A stored summary is unreadable: {detail}",
        detail = error
    )
}

fn unstorable(error: serde_json::Error) -> AppError {
    app_error!(
        Internal,
        StoreSummaryFailed,
        "Could not store a summary: {detail}",
        detail = error
    )
}

impl Store {
    pub fn overview_summary(&self, review_id: &str) -> AppResult<Option<SummaryRecord>> {
        let row = self
            .lock()?
            .query_row(
                "SELECT review_id, head_sha, provider, model, created_at, condensed, card_key,
                     card_comments, title, lead, card_summary, criteria, language, points,
                     attention
                 FROM overview_summaries WHERE review_id = ?1",
                params![review_id],
                |row| {
                    Ok((
                        SummaryRecord {
                            review_id: row.get(0)?,
                            head_sha: row.get(1)?,
                            provider: row.get(2)?,
                            model: row.get(3)?,
                            created_at: row.get(4)?,
                            condensed: row.get(5)?,
                            card_key: row.get(6)?,
                            card_comments: row.get(7)?,
                            title: row.get(8)?,
                            lead: row.get(9)?,
                            points: Vec::new(),
                            attention: Vec::new(),
                            card_summary: row.get(10)?,
                            criteria: Vec::new(),
                            language: Language::from_tag(&row.get::<_, String>(12)?)
                                .unwrap_or_default(),
                        },
                        row.get::<_, String>(11)?,
                        row.get::<_, String>(13)?,
                        row.get::<_, String>(14)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(mut record, criteria, points, attention)| {
            record.criteria = serde_json::from_str(&criteria).map_err(unreadable)?;
            record.points = serde_json::from_str(&points).map_err(unreadable)?;
            record.attention = serde_json::from_str(&attention).map_err(unreadable)?;
            Ok(record)
        })
        .transpose()
    }

    /// Saves the summary of `record.review_id`, replacing an earlier one and
    /// the translations of it.
    pub fn save_overview_summary(&self, record: &SummaryRecord) -> AppResult<()> {
        let criteria = serde_json::to_string(&record.criteria).map_err(unstorable)?;
        let points = serde_json::to_string(&record.points).map_err(unstorable)?;
        let attention = serde_json::to_string(&record.attention).map_err(unstorable)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO overview_summaries (review_id, head_sha, provider, model, created_at,
                 condensed, card_key, card_comments, title, lead, card_summary, criteria, language,
                 points, attention)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT (review_id) DO UPDATE SET
                 head_sha = excluded.head_sha, provider = excluded.provider,
                 model = excluded.model, created_at = excluded.created_at,
                 condensed = excluded.condensed, card_key = excluded.card_key,
                 card_comments = excluded.card_comments, title = excluded.title,
                 lead = excluded.lead, card_summary = excluded.card_summary,
                 criteria = excluded.criteria, language = excluded.language,
                 points = excluded.points, attention = excluded.attention",
            params![
                record.review_id,
                record.head_sha,
                record.provider,
                record.model,
                record.created_at,
                record.condensed,
                record.card_key,
                record.card_comments,
                record.title,
                record.lead,
                record.card_summary,
                criteria,
                record.language.tag(),
                points,
                attention
            ],
        )?;
        transaction.execute(
            "DELETE FROM overview_summary_translations WHERE review_id = ?1",
            params![record.review_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// The translations of the review's summary, by language.
    pub fn summary_translations(&self, review_id: &str) -> AppResult<Vec<SummaryTranslation>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT language, title, lead, card_summary, criteria, points, attention
             FROM overview_summary_translations WHERE review_id = ?1 ORDER BY language",
        )?;
        let rows = statement
            .query_map(params![review_id], |row| {
                Ok((
                    [
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ],
                    [
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ],
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut translations = Vec::new();
        for ([language, title, lead, card_summary], [criteria, points, attention]) in rows {
            let Some(language) = Language::from_tag(&language) else {
                continue;
            };
            let criteria: Vec<CriterionText> =
                serde_json::from_str(&criteria).map_err(unreadable)?;
            translations.push(SummaryTranslation {
                language,
                title,
                lead,
                points: serde_json::from_str(&points).map_err(unreadable)?,
                attention: serde_json::from_str(&attention).map_err(unreadable)?,
                card_summary,
                criteria,
            });
        }
        Ok(translations)
    }

    /// Keeps a translation of the review's summary, replacing an earlier one
    /// in the same language.
    pub fn save_summary_translation(
        &self,
        review_id: &str,
        translation: &SummaryTranslation,
    ) -> AppResult<()> {
        let criteria = serde_json::to_string(&translation.criteria).map_err(unstorable)?;
        let points = serde_json::to_string(&translation.points).map_err(unstorable)?;
        let attention = serde_json::to_string(&translation.attention).map_err(unstorable)?;
        self.lock()?.execute(
            "INSERT INTO overview_summary_translations (review_id, language, title, lead,
                 card_summary, criteria, points, attention)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (review_id, language) DO UPDATE SET
                 title = excluded.title, lead = excluded.lead,
                 card_summary = excluded.card_summary, criteria = excluded.criteria,
                 points = excluded.points, attention = excluded.attention",
            params![
                review_id,
                translation.language.tag(),
                translation.title,
                translation.lead,
                translation.card_summary,
                criteria,
                points,
                attention
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CriterionVerdict, ReviewTarget};
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

    fn summary(review_id: &str, title: &str) -> SummaryRecord {
        SummaryRecord {
            review_id: review_id.to_string(),
            head_sha: Some("e7f3b10".to_string()),
            provider: "anthropic".to_string(),
            model: "claude-opus-5-5".to_string(),
            created_at: "2026-11-19T10:00:00Z".to_string(),
            condensed: false,
            card_key: Some("PAY-482".to_string()),
            card_comments: 4,
            title: title.to_string(),
            lead: "Customers can get part of a payment back.".to_string(),
            points: vec![
                "A new `POST /refunds` endpoint takes the payment and the amount.".to_string(),
                "The request is checked, then the gateway is asked to pay it back.".to_string(),
            ],
            attention: vec!["The branch also carries the payouts work.".to_string()],
            card_summary: "Support wants refunds.".to_string(),
            criteria: vec![Criterion {
                criterion: "A settled payment can be refunded".to_string(),
                verdict: CriterionVerdict::PartlyMet,
                reasoning: "No test refunds part of a payment.".to_string(),
                path: Some("refund/validate.go".to_string()),
                line: Some(19),
                located: true,
            }],
            language: Language::English,
        }
    }

    fn portuguese(title: &str) -> SummaryTranslation {
        SummaryTranslation {
            language: Language::BrazilianPortuguese,
            title: title.to_string(),
            lead: "Clientes podem receber parte de um pagamento de volta.".to_string(),
            points: vec![
                "Um novo endpoint `POST /refunds` recebe o pagamento e o valor.".to_string(),
                "O pedido é validado e o gateway é chamado para devolver.".to_string(),
            ],
            attention: vec!["A branch também traz o trabalho de repasses.".to_string()],
            card_summary: "O suporte quer reembolsos.".to_string(),
            criteria: vec![CriterionText {
                criterion: "Um pagamento liquidado pode ser reembolsado".to_string(),
                reasoning: "Nenhum teste reembolsa parte de um pagamento.".to_string(),
            }],
        }
    }

    #[test]
    fn a_summary_is_saved_read_and_replaced_per_review() {
        let store = Store::open_in_memory().unwrap();
        let review_id = review(&store);
        assert_eq!(store.overview_summary(&review_id).unwrap(), None);

        store
            .save_overview_summary(&summary(&review_id, "Adds refunds"))
            .unwrap();
        store
            .save_overview_summary(&summary(&review_id, "Adds the refund flow"))
            .unwrap();
        assert_eq!(
            store.overview_summary(&review_id).unwrap(),
            Some(summary(&review_id, "Adds the refund flow"))
        );
    }

    #[test]
    fn a_summary_keeps_its_language_and_translations_until_a_new_one_replaces_it() {
        let store = Store::open_in_memory().unwrap();
        let review_id = review(&store);
        let mut written = summary(&review_id, "Adds refunds");
        written.language = Language::BrazilianPortuguese;
        store.save_overview_summary(&written).unwrap();
        assert_eq!(
            store.overview_summary(&review_id).unwrap().unwrap().language,
            Language::BrazilianPortuguese
        );

        store
            .save_overview_summary(&summary(&review_id, "Adds refunds"))
            .unwrap();
        store
            .save_summary_translation(&review_id, &portuguese("Primeiro"))
            .unwrap();
        store
            .save_summary_translation(&review_id, &portuguese("Adiciona reembolsos"))
            .unwrap();
        assert_eq!(
            store.summary_translations(&review_id).unwrap(),
            vec![portuguese("Adiciona reembolsos")]
        );

        store
            .save_overview_summary(&summary(&review_id, "Adds the refund flow"))
            .unwrap();
        assert!(store.summary_translations(&review_id).unwrap().is_empty());
    }

    #[test]
    fn a_summary_stored_before_points_reads_with_none() {
        let store = Store::open_in_memory().unwrap();
        let review_id = review(&store);
        // As the migration leaves a row written before points existed.
        store
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO overview_summaries (review_id, head_sha, provider, model,
                     created_at, condensed, card_key, card_comments, title, lead, card_summary,
                     criteria, language)
                 VALUES (?1, 'e7f3b10', 'claude_cli', 'claude-opus-5-5',
                     '2026-11-19T10:00:00Z', 0, NULL, 0, 'Adds refunds',
                     'A long paragraph about `refund.Validate` and more.', '', '[]', 'pt-BR')",
                params![review_id],
            )
            .unwrap();
        store
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO overview_summary_translations (review_id, language, title, lead,
                     card_summary, criteria)
                 VALUES (?1, 'en', 'Adds refunds', 'A paragraph.', '', '[]')",
                params![review_id],
            )
            .unwrap();

        let read = store.overview_summary(&review_id).unwrap().unwrap();
        assert_eq!(read.lead, "A long paragraph about `refund.Validate` and more.");
        assert!(read.points.is_empty() && read.attention.is_empty());
        let translations = store.summary_translations(&review_id).unwrap();
        assert!(translations[0].points.is_empty() && translations[0].attention.is_empty());
    }
}
