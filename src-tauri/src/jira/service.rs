//! What the commands ask of Jira, composed from the saved connection, the
//! Keychain and the tracker behind its trait. Only reads: the connection's
//! account, and a review's card.

use std::sync::Arc;

use super::cloud::{self, JiraCloud, Route, SCOPES};
use super::tracker::IssueTracker;
use crate::branches;
#[cfg(test)]
use crate::error::ErrorCode;
use crate::error::{app_error, AppError, AppResult};
use crate::flow;
use crate::http::{Credential, ReqwestTransport, Transport};
use crate::model::{Account, Card, CardLookup, JiraConnection, JiraTokenKind, ReviewTarget};
use crate::secrets::{secret_key, KeychainSecrets, SecretStore};
use crate::store::{JiraRecord, Store};

/// Key the connection's token is kept under in the Keychain, with its email.
const SECRET_HOST: &str = "jira";

pub struct JiraService {
    secrets: Arc<dyn SecretStore>,
    transport: Arc<dyn Transport>,
}

impl JiraService {
    pub fn new(secrets: Arc<dyn SecretStore>, transport: Arc<dyn Transport>) -> Self {
        Self { secrets, transport }
    }

    /// The token in the Keychain, over HTTPS.
    pub fn standard() -> Self {
        Self::new(Arc::new(KeychainSecrets), Arc::new(ReqwestTransport::new()))
    }

    pub fn connection(&self, store: &Store) -> AppResult<JiraConnection> {
        let record = store.jira_connection()?;
        Ok(JiraConnection {
            site: record.as_ref().map(|record| record.site.clone()),
            email: record.as_ref().map(|record| record.email.clone()),
            account: record.as_ref().map(|record| Account {
                id: record.account_id.clone(),
                name: record.account_name.clone(),
            }),
            token_kind: record.as_ref().map(|record| token_kind(&record.token_kind)),
            connected_at: record.map(|record| record.connected_at),
            scopes: SCOPES.iter().map(|scope| scope.to_string()).collect(),
        })
    }

    /// The account `email` and `token` belong to on `site`. Without a token,
    /// the saved one is used, if it was saved for that site and email.
    pub fn test_connection(
        &self,
        store: &Store,
        site: &str,
        email: &str,
        token: Option<&str>,
    ) -> AppResult<Account> {
        let site = cloud::normalize_site(site)?;
        let email = required(email, email_missing())?;
        let secret = match token.map(str::trim).filter(|token| !token.is_empty()) {
            Some(token) => token.to_string(),
            None => match store.jira_connection()? {
                Some(record) if record.site == site && record.email == email => {
                    self.secrets.get(&secret_key(SECRET_HOST, email))?
                }
                _ => None,
            }
            .ok_or_else(|| {
                app_error!(
                    NotConnected,
                    TokenMissingForTest,
                    "Enter the API token to test it."
                )
            })?,
        };
        let (_, account) = cloud::connect(&site, &credential(email, &secret), &self.transport)?;
        Ok(account)
    }

    /// Checks the token with Jira, then keeps it in the Keychain and the
    /// site, the account and the route it takes in the database.
    pub fn save_connection(
        &self,
        store: &Store,
        site: &str,
        email: &str,
        token: &str,
    ) -> AppResult<JiraConnection> {
        let site = cloud::normalize_site(site)?;
        let email = required(email, email_missing())?;
        let token = required(
            token,
            app_error!(Internal, TokenMissing, "Enter the API token."),
        )?;
        let (route, account) = cloud::connect(&site, &credential(email, token), &self.transport)?;
        if let Some(previous) = store.jira_connection()? {
            if previous.email != email {
                self.secrets
                    .delete(&secret_key(SECRET_HOST, &previous.email))?;
            }
        }
        self.secrets.set(&secret_key(SECRET_HOST, email), token)?;
        store.save_jira_connection(&JiraRecord {
            site,
            email: email.to_string(),
            api_base: route.api_base,
            token_kind: token_kind_key(route.kind).to_string(),
            account_id: account.id,
            account_name: account.name,
            connected_at: flow::now()?,
        })?;
        self.connection(store)
    }

    /// Forgets the connection and deletes its token from the Keychain.
    pub fn disconnect(&self, store: &Store) -> AppResult<()> {
        if let Some(record) = store.jira_connection()? {
            self.secrets
                .delete(&secret_key(SECRET_HOST, &record.email))?;
        }
        store.delete_jira_connection()
    }

    /// The card of a review: its key is the first one in the branch name,
    /// else in the pull request's title. Without a key, a connection or the
    /// card on Jira, says which, rather than failing.
    pub fn find_card(
        &self,
        store: &Store,
        target: &ReviewTarget,
        pull_request_title: Option<&str>,
    ) -> AppResult<CardLookup> {
        let ReviewTarget::Branch { branch, .. } = target else {
            return Ok(CardLookup::WorkingTree);
        };
        let Some(key) =
            branches::card(branch).or_else(|| pull_request_title.and_then(branches::card))
        else {
            return Ok(CardLookup::NoKey {
                branch: branch.clone(),
            });
        };
        let Some((record, tracker)) = self.tracker(store)? else {
            return Ok(CardLookup::NotConnected { key });
        };
        Ok(match tracker.card(&key)? {
            Some(card) => CardLookup::Found {
                card: Box::new(card),
            },
            None => CardLookup::NotFound {
                url: cloud::card_url(&record.site, &key),
                key,
            },
        })
    }

    /// The card a lookup found, if it found one.
    pub fn card_of(lookup: CardLookup) -> Option<Card> {
        match lookup {
            CardLookup::Found { card } => Some(*card),
            _ => None,
        }
    }

    /// The saved connection and a tracker that uses it; `None` when there is
    /// none or its token is gone from the Keychain.
    fn tracker(&self, store: &Store) -> AppResult<Option<(JiraRecord, Box<dyn IssueTracker>)>> {
        let Some(record) = store.jira_connection()? else {
            return Ok(None);
        };
        let Some(secret) = self.secrets.get(&secret_key(SECRET_HOST, &record.email))? else {
            return Ok(None);
        };
        let route = Route {
            api_base: record.api_base.clone(),
            kind: token_kind(&record.token_kind),
        };
        let tracker = JiraCloud::new(
            &record.site,
            &route,
            credential(&record.email, &secret),
            Arc::clone(&self.transport),
        );
        Ok(Some((record, Box::new(tracker))))
    }
}

fn token_kind(key: &str) -> JiraTokenKind {
    match key {
        "scoped" => JiraTokenKind::Scoped,
        _ => JiraTokenKind::Classic,
    }
}

fn token_kind_key(kind: JiraTokenKind) -> &'static str {
    match kind {
        JiraTokenKind::Classic => "classic",
        JiraTokenKind::Scoped => "scoped",
    }
}

fn credential(email: &str, secret: &str) -> Credential {
    Credential {
        username: email.to_string(),
        secret: secret.to_string(),
    }
}

fn required(text: &str, missing: AppError) -> AppResult<&str> {
    let text = text.trim();
    if text.is_empty() {
        return Err(missing);
    }
    Ok(text)
}

fn email_missing() -> AppError {
    app_error!(
        Internal,
        AtlassianEmailMissing,
        "Enter the email of your Atlassian account."
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::http::tests::CannedTransport;
    use crate::jira::cloud::tests::{comments_json, issue_json, myself};
    use crate::secrets::tests::MemorySecrets;

    struct Fixture {
        store: Store,
        secrets: Arc<MemorySecrets>,
        transport: Arc<CannedTransport>,
        service: JiraService,
    }

    fn fixture() -> Fixture {
        let secrets = Arc::new(MemorySecrets::default());
        let transport = Arc::new(CannedTransport::default());
        Fixture {
            store: Store::open_in_memory().unwrap(),
            service: JiraService::new(
                Arc::clone(&secrets) as Arc<dyn SecretStore>,
                Arc::clone(&transport) as Arc<dyn Transport>,
            ),
            secrets,
            transport,
        }
    }

    fn branch(name: &str) -> ReviewTarget {
        ReviewTarget::Branch {
            branch: name.to_string(),
            base: "main".to_string(),
        }
    }

    impl Fixture {
        fn connect(&self) {
            self.transport.push(200, myself());
            self.service
                .save_connection(&self.store, "acme", " dana@example.com ", "ATATT-secret")
                .unwrap();
        }
    }

    #[test]
    fn a_connection_keeps_its_token_in_the_secret_store_only() {
        let f = fixture();
        assert_eq!(f.service.connection(&f.store).unwrap().site, None);

        f.connect();
        let connection = f.service.connection(&f.store).unwrap();
        assert_eq!(connection.site.as_deref(), Some("acme.atlassian.net"));
        assert_eq!(connection.email.as_deref(), Some("dana@example.com"));
        assert_eq!(connection.account.unwrap().name, "Dana");
        assert_eq!(connection.token_kind, Some(JiraTokenKind::Classic));
        assert_eq!(connection.scopes, ["read:jira-work", "read:jira-user"]);
        assert_eq!(
            f.secrets
                .secrets
                .lock()
                .unwrap()
                .get("jira:dana@example.com"),
            Some(&"ATATT-secret".to_string())
        );
        let printed = format!("{:?}", f.store.jira_connection().unwrap());
        assert!(!printed.contains("ATATT"), "{printed}");

        // Testing again without a token uses the saved one.
        f.transport.push(200, myself());
        let account = f
            .service
            .test_connection(&f.store, "acme.atlassian.net", "dana@example.com", None)
            .unwrap();
        assert_eq!(account.name, "Dana");
        let error = f
            .service
            .test_connection(&f.store, "other.atlassian.net", "dana@example.com", None)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::NotConnected);

        f.service.disconnect(&f.store).unwrap();
        assert!(f.secrets.secrets.lock().unwrap().is_empty());
        assert_eq!(f.service.connection(&f.store).unwrap().account, None);
    }

    #[test]
    fn a_refused_token_is_not_saved() {
        let f = fixture();
        f.transport.push(401, json!({}));
        f.transport.push(404, json!({}));
        let error = f
            .service
            .save_connection(&f.store, "acme", "dana@example.com", "wrong")
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::AuthFailed);
        assert!(f.secrets.secrets.lock().unwrap().is_empty());
        assert_eq!(f.store.jira_connection().unwrap(), None);
    }

    #[test]
    fn the_card_key_comes_from_the_branch_then_the_pull_request_title() {
        let f = fixture();
        assert_eq!(
            f.service
                .find_card(&f.store, &ReviewTarget::WorkingTree, None)
                .unwrap(),
            CardLookup::WorkingTree
        );
        assert_eq!(
            f.service
                .find_card(&f.store, &branch("feature/login"), Some("Adds login"))
                .unwrap(),
            CardLookup::NoKey {
                branch: "feature/login".to_string()
            }
        );
        assert_eq!(
            f.service
                .find_card(
                    &f.store,
                    &branch("feature/login"),
                    Some("ABC-12 Adds login")
                )
                .unwrap(),
            CardLookup::NotConnected {
                key: "ABC-12".to_string()
            }
        );
        // Nothing was asked of Jira without a connection.
        assert!(f.transport.sent().is_empty());

        f.connect();
        f.transport.push(200, issue_json("ABC-200"));
        f.transport.push(200, comments_json(&[], 0));
        let found = f
            .service
            .find_card(&f.store, &branch("feature/ABC-200"), Some("PAY-1 other"))
            .unwrap();
        let card = JiraService::card_of(found).unwrap();
        assert_eq!(card.key, "ABC-200");

        f.transport.push(404, json!({}));
        assert_eq!(
            f.service
                .find_card(&f.store, &branch("feature/ABC-201"), None)
                .unwrap(),
            CardLookup::NotFound {
                key: "ABC-201".to_string(),
                url: "https://acme.atlassian.net/browse/ABC-201".to_string()
            }
        );

        // Any other failure is an error, not a quiet state.
        f.transport.push(401, json!({}));
        let error = f
            .service
            .find_card(&f.store, &branch("feature/ABC-202"), None)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::AuthFailed);
    }
}
