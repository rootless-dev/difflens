//! App-wide settings by key, the choices made for each model provider, and
//! the model chosen for each task that does not use the default one. A
//! provider's API key lives in the Keychain (`secrets`), never here: this
//! only records when one was saved.

use rusqlite::{params, OptionalExtension};

use super::Store;
use crate::error::AppResult;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelProviderRecord {
    /// Key of the provider, such as "anthropic".
    pub provider: String,
    pub model: Option<String>,
    /// Such as "high"; only providers that take an effort have one.
    pub effort: Option<String>,
    /// When an API key was saved to the Keychain; `None` when there is none.
    pub key_saved_at: Option<String>,
}

/// The model chosen for a task instead of the default one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTaskRecord {
    /// Key of the task, such as "insights".
    pub task: String,
    /// Key of the provider, such as "anthropic".
    pub provider: String,
    pub model: String,
    /// Such as "high"; only providers that take an effort have one.
    pub effort: Option<String>,
}

impl Store {
    pub fn setting(&self, key: &str) -> AppResult<Option<String>> {
        let value = self
            .lock()?
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> AppResult<()> {
        self.lock()?.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn model_provider(&self, provider: &str) -> AppResult<Option<ModelProviderRecord>> {
        let record = self
            .lock()?
            .query_row(
                "SELECT provider, model, effort, key_saved_at
                 FROM model_providers WHERE provider = ?1",
                params![provider],
                |row| {
                    Ok(ModelProviderRecord {
                        provider: row.get(0)?,
                        model: row.get(1)?,
                        effort: row.get(2)?,
                        key_saved_at: row.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(record)
    }

    /// Saves the model and effort chosen for `provider`, keeping whether it
    /// has a key.
    pub fn save_model_choice(
        &self,
        provider: &str,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> AppResult<()> {
        self.lock()?.execute(
            "INSERT INTO model_providers (provider, model, effort) VALUES (?1, ?2, ?3)
             ON CONFLICT (provider) DO UPDATE SET
                 model = excluded.model, effort = excluded.effort",
            params![provider, model, effort],
        )?;
        Ok(())
    }

    /// Records that `provider` has a key in the Keychain since `saved_at`, or
    /// (`None`) that it has none.
    pub fn set_model_key_saved(&self, provider: &str, saved_at: Option<&str>) -> AppResult<()> {
        self.lock()?.execute(
            "INSERT INTO model_providers (provider, key_saved_at) VALUES (?1, ?2)
             ON CONFLICT (provider) DO UPDATE SET key_saved_at = excluded.key_saved_at",
            params![provider, saved_at],
        )?;
        Ok(())
    }

    /// Every task with a model of its own; the others use the default.
    pub fn model_tasks(&self) -> AppResult<Vec<ModelTaskRecord>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT task, provider, model, effort FROM model_tasks ORDER BY task")?;
        let records = statement
            .query_map([], read_task)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    /// The model chosen for `task`; `None` when it uses the default.
    pub fn model_task(&self, task: &str) -> AppResult<Option<ModelTaskRecord>> {
        let record = self
            .lock()?
            .query_row(
                "SELECT task, provider, model, effort FROM model_tasks WHERE task = ?1",
                params![task],
                read_task,
            )
            .optional()?;
        Ok(record)
    }

    /// Gives a task a model of its own, replacing the one it had.
    pub fn set_model_task(&self, record: &ModelTaskRecord) -> AppResult<()> {
        self.lock()?.execute(
            "INSERT INTO model_tasks (task, provider, model, effort) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (task) DO UPDATE SET
                 provider = excluded.provider, model = excluded.model, effort = excluded.effort",
            params![record.task, record.provider, record.model, record.effort],
        )?;
        Ok(())
    }

    /// Puts `task` back on the default model.
    pub fn clear_model_task(&self, task: &str) -> AppResult<()> {
        self.lock()?
            .execute("DELETE FROM model_tasks WHERE task = ?1", params![task])?;
        Ok(())
    }
}

fn read_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<ModelTaskRecord> {
    Ok(ModelTaskRecord {
        task: row.get(0)?,
        provider: row.get(1)?,
        model: row.get(2)?,
        effort: row.get(3)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setting_is_saved_and_replaced() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.setting("model.provider").unwrap(), None);
        store.set_setting("model.provider", "anthropic").unwrap();
        store.set_setting("model.provider", "openai").unwrap();
        assert_eq!(
            store.setting("model.provider").unwrap().as_deref(),
            Some("openai")
        );
    }

    #[test]
    fn the_choice_and_the_key_of_a_provider_are_kept_apart() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.model_provider("anthropic").unwrap(), None);

        store
            .set_model_key_saved("anthropic", Some("2026-11-19T10:00:00Z"))
            .unwrap();
        store
            .save_model_choice("anthropic", Some("claude-opus-5-5"), Some("high"))
            .unwrap();
        let expected = ModelProviderRecord {
            provider: "anthropic".to_string(),
            model: Some("claude-opus-5-5".to_string()),
            effort: Some("high".to_string()),
            key_saved_at: Some("2026-11-19T10:00:00Z".to_string()),
        };
        assert_eq!(
            store.model_provider("anthropic").unwrap(),
            Some(expected.clone())
        );

        store.set_model_key_saved("anthropic", None).unwrap();
        assert_eq!(
            store.model_provider("anthropic").unwrap(),
            Some(ModelProviderRecord {
                key_saved_at: None,
                ..expected
            })
        );
        assert_eq!(store.model_provider("openai").unwrap(), None);
    }

    #[test]
    fn a_task_uses_the_default_until_given_a_model_and_after_it_is_cleared() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.model_tasks().unwrap(), Vec::new());
        assert_eq!(store.model_task("insights").unwrap(), None);

        let insights = ModelTaskRecord {
            task: "insights".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-5-5".to_string(),
            effort: Some("low".to_string()),
        };
        store.set_model_task(&insights).unwrap();
        let translation = ModelTaskRecord {
            task: "translation".to_string(),
            provider: "claude_cli".to_string(),
            model: "haiku".to_string(),
            effort: None,
        };
        store.set_model_task(&translation).unwrap();
        // Replaced, not added.
        let insights = ModelTaskRecord {
            model: "claude-opus-5-5".to_string(),
            ..insights
        };
        store.set_model_task(&insights).unwrap();
        assert_eq!(
            store.model_tasks().unwrap(),
            vec![insights.clone(), translation.clone()]
        );
        assert_eq!(store.model_task("insights").unwrap(), Some(insights));

        store.clear_model_task("insights").unwrap();
        assert_eq!(store.model_task("insights").unwrap(), None);
        assert_eq!(store.model_tasks().unwrap(), vec![translation]);
        // Clearing a task on the default already changes nothing.
        store.clear_model_task("summary").unwrap();
    }

    #[test]
    fn a_database_from_before_tasks_keeps_its_default_model_and_every_task_on_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("difflens.db");
        {
            let store = Store::open(&file).unwrap();
            store.set_setting("model.provider", "anthropic").unwrap();
            store
                .save_model_choice("anthropic", Some("claude-opus-5-5"), Some("high"))
                .unwrap();
            // As a database of the version before: without the tasks table
            // and what came after it (the explanations of failed tests, then
            // general comments, then drafts written from signals, then fixed
            // findings), five migrations behind.
            let connection = store.lock().unwrap();
            connection
                .execute_batch(
                    "ALTER TABLE insight_findings DROP COLUMN resolution;
                     ALTER TABLE insight_findings DROP COLUMN resolved_at;
                     ALTER TABLE insight_findings DROP COLUMN resolved_model;
                     ALTER TABLE insight_findings DROP COLUMN resolution_reason;
                     DROP TABLE signal_comments;
                     DROP TABLE test_diagnosis_translations; DROP TABLE test_diagnoses;
                     DROP TABLE model_tasks; ALTER TABLE review_comments DROP COLUMN kind",
                )
                .unwrap();
            let version: i64 = connection
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            connection
                .pragma_update(None, "user_version", version - 5)
                .unwrap();
        }

        let store = Store::open(&file).unwrap();
        assert_eq!(
            store.setting("model.provider").unwrap().as_deref(),
            Some("anthropic")
        );
        assert_eq!(
            store
                .model_provider("anthropic")
                .unwrap()
                .unwrap()
                .model
                .as_deref(),
            Some("claude-opus-5-5")
        );
        assert_eq!(store.model_tasks().unwrap(), Vec::new());
    }
}
