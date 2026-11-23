//! The Jira Cloud connection: which site, who the user is there, and how
//! their token reaches it. The token itself lives in the Keychain (`secrets`),
//! never in this database. There is one connection at most.

use rusqlite::{params, OptionalExtension};

use super::Store;
use crate::error::AppResult;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraRecord {
    /// Such as "acme.atlassian.net".
    pub site: String,
    /// The Atlassian account email the token belongs to.
    pub email: String,
    /// Where the REST API is reached with this token: the site itself, or
    /// the Atlassian API gateway for the site's cloud id.
    pub api_base: String,
    /// "classic" or "scoped".
    pub token_kind: String,
    pub account_id: String,
    pub account_name: String,
    pub connected_at: String,
}

impl Store {
    pub fn jira_connection(&self) -> AppResult<Option<JiraRecord>> {
        let record = self
            .lock()?
            .query_row(
                "SELECT site, email, api_base, token_kind, account_id, account_name, connected_at
                 FROM jira_connection WHERE id = 1",
                [],
                |row| {
                    Ok(JiraRecord {
                        site: row.get(0)?,
                        email: row.get(1)?,
                        api_base: row.get(2)?,
                        token_kind: row.get(3)?,
                        account_id: row.get(4)?,
                        account_name: row.get(5)?,
                        connected_at: row.get(6)?,
                    })
                },
            )
            .optional()?;
        Ok(record)
    }

    /// Saves the connection, replacing an earlier one.
    pub fn save_jira_connection(&self, record: &JiraRecord) -> AppResult<()> {
        self.lock()?.execute(
            "INSERT INTO jira_connection
                 (id, site, email, api_base, token_kind, account_id, account_name, connected_at)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (id) DO UPDATE SET
                 site = excluded.site, email = excluded.email, api_base = excluded.api_base,
                 token_kind = excluded.token_kind, account_id = excluded.account_id,
                 account_name = excluded.account_name, connected_at = excluded.connected_at",
            params![
                record.site,
                record.email,
                record.api_base,
                record.token_kind,
                record.account_id,
                record.account_name,
                record.connected_at
            ],
        )?;
        Ok(())
    }

    pub fn delete_jira_connection(&self) -> AppResult<()> {
        self.lock()?
            .execute("DELETE FROM jira_connection WHERE id = 1", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(email: &str) -> JiraRecord {
        JiraRecord {
            site: "acme.atlassian.net".to_string(),
            email: email.to_string(),
            api_base: "https://acme.atlassian.net".to_string(),
            token_kind: "classic".to_string(),
            account_id: "5b10a2844c20165700ede21g".to_string(),
            account_name: "Dana".to_string(),
            connected_at: "2026-11-19T10:00:00Z".to_string(),
        }
    }

    #[test]
    fn the_connection_is_saved_replaced_and_removed() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.jira_connection().unwrap(), None);

        store
            .save_jira_connection(&record("a@example.com"))
            .unwrap();
        store
            .save_jira_connection(&record("b@example.com"))
            .unwrap();
        assert_eq!(
            store.jira_connection().unwrap(),
            Some(record("b@example.com"))
        );

        store.delete_jira_connection().unwrap();
        assert_eq!(store.jira_connection().unwrap(), None);
    }
}
