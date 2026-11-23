//! Pull request host connections: who the user is on each host. The secret
//! itself (an API token) lives in the Keychain (`pull_requests::credentials`),
//! never in this database.

use rusqlite::{params, OptionalExtension};

use super::Store;
use crate::error::AppResult;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionRecord {
    /// Key of the host, such as "bitbucket".
    pub host: String,
    /// What the host calls the user in its credential: the Atlassian account
    /// email for Bitbucket.
    pub username: String,
    /// The account the credential was found to belong to when it was saved,
    /// so comments by it can be told apart from everyone else's.
    pub account_id: String,
    pub account_name: String,
    pub connected_at: String,
}

impl Store {
    pub fn connection(&self, host: &str) -> AppResult<Option<ConnectionRecord>> {
        let record = self
            .lock()?
            .query_row(
                "SELECT host, username, account_id, account_name, connected_at
                 FROM connections WHERE host = ?1",
                params![host],
                |row| {
                    Ok(ConnectionRecord {
                        host: row.get(0)?,
                        username: row.get(1)?,
                        account_id: row.get(2)?,
                        account_name: row.get(3)?,
                        connected_at: row.get(4)?,
                    })
                },
            )
            .optional()?;
        Ok(record)
    }

    /// Saves the connection of `record.host`, replacing an earlier one.
    pub fn save_connection(&self, record: &ConnectionRecord) -> AppResult<()> {
        self.lock()?.execute(
            "INSERT INTO connections (host, username, account_id, account_name, connected_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (host) DO UPDATE SET
                 username = excluded.username, account_id = excluded.account_id,
                 account_name = excluded.account_name, connected_at = excluded.connected_at",
            params![
                record.host,
                record.username,
                record.account_id,
                record.account_name,
                record.connected_at
            ],
        )?;
        Ok(())
    }

    pub fn delete_connection(&self, host: &str) -> AppResult<()> {
        self.lock()?
            .execute("DELETE FROM connections WHERE host = ?1", params![host])?;
        Ok(())
    }

    /// Ids of every connected account: comments by them are the user's own.
    pub fn connected_account_ids(&self) -> AppResult<Vec<String>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare("SELECT account_id FROM connections")?;
        let ids = statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(username: &str) -> ConnectionRecord {
        ConnectionRecord {
            host: "bitbucket".to_string(),
            username: username.to_string(),
            account_id: format!("{{{username}}}"),
            account_name: "Dana".to_string(),
            connected_at: "2026-11-19T10:00:00Z".to_string(),
        }
    }

    #[test]
    fn a_connection_is_saved_replaced_and_removed_per_host() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.connection("bitbucket").unwrap(), None);

        store.save_connection(&record("a@example.com")).unwrap();
        store.save_connection(&record("b@example.com")).unwrap();
        assert_eq!(
            store.connection("bitbucket").unwrap(),
            Some(record("b@example.com"))
        );
        assert_eq!(
            store.connected_account_ids().unwrap(),
            vec!["{b@example.com}".to_string()]
        );

        store.delete_connection("bitbucket").unwrap();
        assert_eq!(store.connection("bitbucket").unwrap(), None);
        assert!(store.connected_account_ids().unwrap().is_empty());
    }
}
