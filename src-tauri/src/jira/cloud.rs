//! Jira Cloud, through its REST API v3
//! (<https://developer.atlassian.com/cloud/jira/platform/rest/v3/>),
//! authenticated with the Atlassian account email and an API token as basic
//! auth (<https://developer.atlassian.com/cloud/jira/platform/basic-auth-for-rest-apis/>).
//!
//! A classic API token is used on the site itself,
//! `https://<site>/rest/api/3/...`. A token with scopes only works through
//! the Atlassian API gateway, `https://api.atlassian.com/ex/jira/<cloudId>/
//! rest/api/3/...` (<https://support.atlassian.com/atlassian-account/docs/
//! manage-api-tokens-for-your-atlassian-account/>), where the cloud id is
//! the one the site's `/_edge/tenant_info` gives (<https://support.atlassian.
//! com/jira/kb/retrieve-my-atlassian-sites-cloud-id/>). Connecting tries the
//! site first and the gateway when the site refuses the token.
//!
//! Scopes, as the API reference lists them per endpoint: reading an issue
//! and its comments needs `read:jira-work`; who the token belongs to
//! (`/myself`) needs `read:jira-user`.

use std::sync::Arc;

use reqwest::Url;
use serde::Deserialize;
use serde_json::Value;

use super::adf;
use super::tracker::IssueTracker;
use crate::error::{app_error, core_text, AppError, AppResult, ErrorCode};
use crate::http::{self, Auth, Credential, HttpRequest, HttpResponse, Method, Transport};
use crate::model::{Account, Card, CardComment, JiraTokenKind};

/// The Atlassian API gateway's Jira routes; the cloud id follows.
pub const GATEWAY: &str = "https://api.atlassian.com/ex/jira";

/// Scopes a token with scopes needs, shown next to the token field.
pub const SCOPES: &[&str] = &["read:jira-work", "read:jira-user"];

/// Comments read at most for one card, so a card with thousands of them
/// cannot keep DiffLens busy.
const MAX_COMMENTS: usize = 500;
const COMMENTS_PAGE: usize = 100;

/// Where the REST API is reached with a token, and which kind it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub api_base: String,
    pub kind: JiraTokenKind,
}

/// A site as it is typed, `https://acme.atlassian.net/jira` or `acme`, as
/// the host name it stands for: `acme.atlassian.net`.
pub fn normalize_site(typed: &str) -> AppResult<String> {
    let mut site = typed.trim().to_ascii_lowercase();
    for scheme in ["https://", "http://"] {
        if let Some(rest) = site.strip_prefix(scheme) {
            site = rest.to_string();
        }
    }
    if let Some(end) = site.find(['/', '?', '#']) {
        site.truncate(end);
    }
    let site = site.trim_end_matches('.').to_string();
    if site.is_empty() {
        return Err(app_error!(
            Internal,
            JiraSiteMissing,
            "Enter your Jira site, such as acme.atlassian.net."
        ));
    }
    let site = if site.contains('.') {
        site
    } else {
        format!("{site}.atlassian.net")
    };
    let valid = site.split('.').all(|label| {
        !label.is_empty()
            && !label.starts_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    });
    if !valid {
        return Err(app_error!(
            Internal,
            JiraSiteInvalid,
            "\"{site}\" is not a Jira site. Enter it as acme.atlassian.net.",
            site = crate::error::escaped(typed)
        ));
    }
    Ok(site)
}

/// The page of card `key` on `site`.
pub fn card_url(site: &str, key: &str) -> String {
    format!("https://{site}/browse/{key}")
}

/// Finds how `credential` reaches `site`: on the site itself (a classic
/// token), else through the gateway (a token with scopes). Answers the route
/// and the account the token belongs to.
pub fn connect(
    site: &str,
    credential: &Credential,
    transport: &Arc<dyn Transport>,
) -> AppResult<(Route, Account)> {
    let classic = Route {
        api_base: format!("https://{site}"),
        kind: JiraTokenKind::Classic,
    };
    let refused =
        match JiraCloud::new(site, &classic, credential.clone(), Arc::clone(transport)).account() {
            Ok(account) => return Ok((classic, account)),
            Err(error) if error.code == ErrorCode::AuthFailed => error,
            Err(error) => return Err(error),
        };
    // A token with scopes is refused by the site: try the gateway, unless the
    // site's cloud id cannot be found, which leaves the site's own refusal.
    let Some(cloud_id) = cloud_id(site, transport) else {
        return Err(refused);
    };
    let scoped = Route {
        api_base: format!("{GATEWAY}/{cloud_id}"),
        kind: JiraTokenKind::Scoped,
    };
    let account =
        JiraCloud::new(site, &scoped, credential.clone(), Arc::clone(transport)).account()?;
    Ok((scoped, account))
}

/// The cloud id of `site`, from its public tenant info.
fn cloud_id(site: &str, transport: &Arc<dyn Transport>) -> Option<String> {
    let request = HttpRequest::new(
        Method::Get,
        format!("https://{site}/_edge/tenant_info"),
        Auth::None,
    );
    let response = transport.send(&request).ok()?;
    if !(200..300).contains(&response.status) {
        return None;
    }
    let value: Value = serde_json::from_str(&response.body).ok()?;
    let id = value["cloudId"].as_str()?.trim();
    // It goes into a URL path: only what a cloud id (a UUID) holds.
    (!id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .then(|| id.to_string())
}

pub struct JiraCloud {
    site: String,
    api: Url,
    credential: Credential,
    transport: Arc<dyn Transport>,
}

impl JiraCloud {
    pub fn new(
        site: &str,
        route: &Route,
        credential: Credential,
        transport: Arc<dyn Transport>,
    ) -> Self {
        Self {
            site: site.to_string(),
            api: Url::parse(&route.api_base).expect("the Jira API base is a URL"),
            credential,
            transport,
        }
    }

    /// The REST API v3 URL of `segments`, each percent-encoded as one path
    /// segment.
    fn url(&self, segments: &[&str]) -> Url {
        let mut url = self.api.clone();
        url.path_segments_mut()
            .expect("the Jira API base has a path")
            .pop_if_empty()
            .extend(["rest", "api", "3"])
            .extend(segments);
        url
    }

    fn get<T: for<'de> Deserialize<'de>>(
        &self,
        url: &Url,
        missing: &dyn Fn() -> AppError,
    ) -> AppResult<T> {
        let request = HttpRequest::new(
            Method::Get,
            url.to_string(),
            Auth::Basic(self.credential.clone()),
        );
        let response = self.transport.send(&request).map_err(|reason| {
            app_error!(
                NetworkFailed,
                HostUnreachable,
                "Could not reach {host}: {detail}.",
                host = HOST,
                detail = reason
            )
        })?;
        if !(200..300).contains(&response.status) {
            return Err(failure(&response, missing));
        }
        serde_json::from_str(&response.body).map_err(|error| http::host_unreadable(HOST, &error))
    }

    fn comments(&self, key: &str, missing: &dyn Fn() -> AppError) -> AppResult<Vec<CardComment>> {
        let mut comments = Vec::new();
        while comments.len() < MAX_COMMENTS {
            let mut url = self.url(&["issue", key, "comment"]);
            url.query_pairs_mut()
                .append_pair("orderBy", "created")
                .append_pair("startAt", &comments.len().to_string())
                .append_pair("maxResults", &COMMENTS_PAGE.to_string());
            let page: CommentPage = self.get(&url, missing)?;
            let read = page.comments.len();
            comments.extend(page.comments.into_iter().map(CommentJson::into_model));
            if read == 0 || page.total.is_some_and(|total| comments.len() >= total) {
                break;
            }
        }
        comments.truncate(MAX_COMMENTS);
        Ok(comments)
    }
}

impl IssueTracker for JiraCloud {
    fn account(&self) -> AppResult<Account> {
        let user: UserJson = self.get(&self.url(&["myself"]), &|| {
            app_error!(
                RemoteNotFound,
                JiraSiteNotFound,
                "Jira has no site at {site}.",
                site = &self.site
            )
        })?;
        Ok(Account {
            id: user.account_id.unwrap_or_default(),
            name: user
                .display_name
                .or(user.email_address)
                .unwrap_or_else(|| "Unknown user".to_string()),
        })
    }

    fn card(&self, key: &str) -> AppResult<Option<Card>> {
        let missing = || {
            app_error!(
                RemoteNotFound,
                JiraCardMissing,
                "Jira has no card {key}, or the connected account cannot see it.",
                key = key
            )
        };
        let mut url = self.url(&["issue", key]);
        url.query_pairs_mut()
            .append_pair("fields", "summary,status,issuetype,description");
        let issue: IssueJson = match self.get(&url, &missing) {
            Ok(issue) => issue,
            Err(error) if error.code == ErrorCode::RemoteNotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        // The key as Jira has it: a moved card answers under its new key.
        let key = issue.key.unwrap_or_else(|| key.to_string());
        let comments = self.comments(&key, &missing)?;
        let fields = issue.fields;
        Ok(Some(Card {
            url: card_url(&self.site, &key),
            key,
            summary: fields.summary.unwrap_or_default(),
            status: fields
                .status
                .and_then(|status| status.name)
                .unwrap_or_default(),
            issue_type: fields
                .issuetype
                .and_then(|issue_type| issue_type.name)
                .unwrap_or_default(),
            description: adf::plain_text(&fields.description),
            comments,
        }))
    }
}

/// A readable error for an answer that is not a success.
fn failure(response: &HttpResponse, missing: &dyn Fn() -> AppError) -> AppError {
    let detail = error_message(&response.body);
    let needs = || {
        core_text!(
            JiraScopesNeeded,
            "DiffLens needs {work} and {user}",
            work = SCOPES[0],
            user = SCOPES[1]
        )
    };
    match response.status {
        // The gateway says "Unauthorized; scope does not match" for a token
        // with scopes that lacks one.
        401 if detail
            .as_deref()
            .is_some_and(|detail| detail.to_ascii_lowercase().contains("scope")) =>
        {
            app_error!(
                MissingScope,
                JiraTokenMissingScope,
                "Jira refused this: the API token is missing a scope. {needs}.",
                needs = needs()
            )
        }
        401 => http::host_credential_refused(HOST),
        403 => match detail {
            Some(detail) => app_error!(
                MissingScope,
                JiraForbiddenSaid,
                "Jira refused this ({detail}). The API token may be missing a scope ({needs}), \
                 or the account may not be allowed to browse the project.",
                detail = detail,
                needs = needs()
            ),
            None => app_error!(
                MissingScope,
                JiraForbidden,
                "Jira refused this. The API token may be missing a scope ({needs}), or the \
                 account may not be allowed to browse the project.",
                needs = needs()
            ),
        },
        404 => missing(),
        429 => http::host_rate_limited(HOST, response.retry_after.as_deref()),
        status if status >= 500 => http::host_server_error(HOST, status),
        status => http::host_refused(HOST, status, detail),
    }
}

/// The service's name in messages.
const HOST: &str = "Jira";

/// Jira's own explanation, `{"errorMessages": [...]}`, or the gateway's
/// `{"message": ...}`, kept short.
fn error_message(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let message = value["errorMessages"]
        .as_array()
        .and_then(|messages| messages.iter().find_map(Value::as_str))
        .or_else(|| value["message"].as_str())?
        .trim();
    if message.is_empty() {
        return None;
    }
    Some(message.chars().take(300).collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserJson {
    account_id: Option<String>,
    display_name: Option<String>,
    email_address: Option<String>,
}

#[derive(Deserialize)]
struct IssueJson {
    key: Option<String>,
    #[serde(default)]
    fields: FieldsJson,
}

#[derive(Deserialize, Default)]
struct FieldsJson {
    summary: Option<String>,
    status: Option<NamedJson>,
    issuetype: Option<NamedJson>,
    /// A document, or null when the card has no description.
    #[serde(default)]
    description: Value,
}

#[derive(Deserialize)]
struct NamedJson {
    name: Option<String>,
}

#[derive(Deserialize)]
struct CommentPage {
    #[serde(default)]
    comments: Vec<CommentJson>,
    total: Option<usize>,
}

#[derive(Deserialize)]
struct CommentJson {
    author: Option<UserJson>,
    #[serde(default)]
    body: Value,
    #[serde(default)]
    created: String,
}

impl CommentJson {
    fn into_model(self) -> CardComment {
        CardComment {
            author: self
                .author
                .and_then(|author| author.display_name)
                .unwrap_or_else(|| "Former user".to_string()),
            created_at: self.created,
            text: adf::plain_text(&self.body),
        }
    }
}

#[cfg(test)]
pub mod tests {
    use serde_json::json;

    use super::*;
    use crate::http::tests::CannedTransport;

    pub fn credential() -> Credential {
        Credential {
            username: "dana@example.com".to_string(),
            secret: "ATATT-secret".to_string(),
        }
    }

    pub fn classic() -> Route {
        Route {
            api_base: "https://acme.atlassian.net".to_string(),
            kind: JiraTokenKind::Classic,
        }
    }

    fn client(transport: &Arc<CannedTransport>, route: &Route) -> JiraCloud {
        JiraCloud::new(
            "acme.atlassian.net",
            route,
            credential(),
            Arc::clone(transport) as Arc<dyn Transport>,
        )
    }

    pub fn myself() -> Value {
        json!({ "accountId": "5b10a28", "displayName": "Dana", "emailAddress": "dana@example.com" })
    }

    pub fn issue_json(key: &str) -> Value {
        json!({
            "key": key,
            "fields": {
                "summary": "Allow refunds for settled payments",
                "status": { "name": "In Progress" },
                "issuetype": { "name": "Story" },
                "description": {
                    "type": "doc", "version": 1,
                    "content": [{ "type": "paragraph", "content": [
                        { "type": "text", "text": "Refund a settled payment." }
                    ]}]
                }
            }
        })
    }

    pub fn comments_json(texts: &[&str], total: usize) -> Value {
        let comments: Vec<Value> = texts
            .iter()
            .map(|text| {
                json!({
                    "author": { "displayName": "Ana" },
                    "created": "2026-11-12T09:30:00.000+0000",
                    "body": { "type": "doc", "version": 1, "content": [
                        { "type": "paragraph", "content": [{ "type": "text", "text": text }] }
                    ]}
                })
            })
            .collect();
        json!({ "startAt": 0, "maxResults": 100, "total": total, "comments": comments })
    }

    #[test]
    fn normalizes_the_site_as_it_is_typed() {
        for typed in [
            "acme.atlassian.net",
            " https://acme.atlassian.net/jira/software ",
            "ACME",
            "http://acme.atlassian.net.",
        ] {
            assert_eq!(
                normalize_site(typed).unwrap(),
                "acme.atlassian.net",
                "{typed}"
            );
        }
        assert!(normalize_site("").is_err());
        assert!(normalize_site("acme atlassian").is_err());
        assert!(normalize_site("acme..net").is_err());
        assert!(normalize_site("user@acme.atlassian.net").is_err());
    }

    #[test]
    fn reads_a_card_with_its_comments_across_pages() {
        let transport = CannedTransport::answering(vec![
            (200, issue_json("PAY-482")),
            (200, comments_json(&["first"], 2)),
            (200, comments_json(&["second"], 2)),
        ]);
        let card = client(&transport, &classic())
            .card("PAY-482")
            .unwrap()
            .unwrap();
        assert_eq!(card.key, "PAY-482");
        assert_eq!(card.url, "https://acme.atlassian.net/browse/PAY-482");
        assert_eq!(card.summary, "Allow refunds for settled payments");
        assert_eq!(card.status, "In Progress");
        assert_eq!(card.issue_type, "Story");
        assert_eq!(card.description, "Refund a settled payment.");
        let texts: Vec<&str> = card.comments.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["first", "second"]);
        assert_eq!(card.comments[0].author, "Ana");

        let sent = transport.sent();
        assert_eq!(
            sent[0].url,
            "https://acme.atlassian.net/rest/api/3/issue/PAY-482?fields=summary%2Cstatus%2Cissuetype%2Cdescription"
        );
        assert_eq!(
            sent[1].url,
            "https://acme.atlassian.net/rest/api/3/issue/PAY-482/comment?orderBy=created&startAt=0&maxResults=100"
        );
        assert!(sent[2].url.contains("startAt=1"));
        assert!(sent.iter().all(
            |request| matches!(&request.auth, Auth::Basic(c) if c.username == "dana@example.com")
        ));
    }

    #[test]
    fn a_card_jira_does_not_have_is_none_and_scoped_tokens_use_the_gateway() {
        let scoped = Route {
            api_base: format!("{GATEWAY}/1324a887-45db-1bf4-1e99-ef0ff456d421"),
            kind: JiraTokenKind::Scoped,
        };
        let transport = CannedTransport::answering(vec![(
            404,
            json!({ "errorMessages": ["Issue does not exist or you do not have permission to see it."] }),
        )]);
        assert_eq!(client(&transport, &scoped).card("PAY-9").unwrap(), None);
        assert_eq!(
            transport.sent()[0].url,
            "https://api.atlassian.com/ex/jira/1324a887-45db-1bf4-1e99-ef0ff456d421/rest/api/3/issue/PAY-9?fields=summary%2Cstatus%2Cissuetype%2Cdescription"
        );
    }

    #[test]
    fn failures_read_as_what_went_wrong() {
        let cases = [
            (401, json!({}), ErrorCode::AuthFailed),
            (
                401,
                json!({ "code": 401, "message": "Unauthorized; scope does not match" }),
                ErrorCode::MissingScope,
            ),
            (
                403,
                json!({ "errorMessages": ["No permission"] }),
                ErrorCode::MissingScope,
            ),
            (429, json!({}), ErrorCode::RateLimited),
            (503, json!({}), ErrorCode::RemoteFailed),
            (
                400,
                json!({ "errorMessages": ["Bad"] }),
                ErrorCode::RemoteFailed,
            ),
        ];
        for (status, body, code) in cases {
            let transport = CannedTransport::answering(vec![(status, body)]);
            let error = client(&transport, &classic()).card("PAY-1").unwrap_err();
            assert_eq!(error.code, code, "HTTP {status}: {}", error.message);
        }
        let transport = CannedTransport::answering(vec![]);
        let error = client(&transport, &classic()).account().unwrap_err();
        assert_eq!(error.code, ErrorCode::NetworkFailed);
        let scope = failure(
            &HttpResponse {
                status: 401,
                body: json!({ "message": "Unauthorized; scope does not match" }).to_string(),
                retry_after: None,
                headers: Vec::new(),
            },
            &|| AppError::untranslated(ErrorCode::RemoteNotFound, ""),
        );
        assert!(
            scope.message.contains("read:jira-work and read:jira-user"),
            "{}",
            scope.message
        );
    }

    #[test]
    fn a_classic_token_connects_on_the_site() {
        let transport = CannedTransport::answering(vec![(200, myself())]);
        let shared = Arc::clone(&transport) as Arc<dyn Transport>;
        let (route, account) = connect("acme.atlassian.net", &credential(), &shared).unwrap();
        assert_eq!(route, classic());
        assert_eq!(account.name, "Dana");
        assert_eq!(account.id, "5b10a28");
        assert_eq!(
            transport.sent()[0].url,
            "https://acme.atlassian.net/rest/api/3/myself"
        );
    }

    #[test]
    fn a_token_with_scopes_connects_through_the_gateway() {
        let transport = CannedTransport::answering(vec![
            (401, json!({})),
            (200, json!({ "cloudId": "1324a887-45db" })),
            (200, myself()),
        ]);
        let shared = Arc::clone(&transport) as Arc<dyn Transport>;
        let (route, _) = connect("acme.atlassian.net", &credential(), &shared).unwrap();
        assert_eq!(route.kind, JiraTokenKind::Scoped);
        assert_eq!(
            route.api_base,
            "https://api.atlassian.com/ex/jira/1324a887-45db"
        );
        let sent = transport.sent();
        assert_eq!(sent[1].url, "https://acme.atlassian.net/_edge/tenant_info");
        assert_eq!(sent[1].auth, Auth::None);
        assert_eq!(
            sent[2].url,
            "https://api.atlassian.com/ex/jira/1324a887-45db/rest/api/3/myself"
        );
    }

    #[test]
    fn a_wrong_token_fails_on_both_routes_and_an_odd_cloud_id_is_not_used() {
        let transport = CannedTransport::answering(vec![
            (401, json!({})),
            (200, json!({ "cloudId": "1324a887-45db" })),
            (401, json!({})),
        ]);
        let shared = Arc::clone(&transport) as Arc<dyn Transport>;
        let error = connect("acme.atlassian.net", &credential(), &shared).unwrap_err();
        assert_eq!(error.code, ErrorCode::AuthFailed);

        let transport = CannedTransport::answering(vec![
            (401, json!({})),
            (200, json!({ "cloudId": "../../evil" })),
        ]);
        let shared = Arc::clone(&transport) as Arc<dyn Transport>;
        let error = connect("acme.atlassian.net", &credential(), &shared).unwrap_err();
        assert_eq!(error.code, ErrorCode::AuthFailed);
        assert_eq!(transport.sent().len(), 2);
    }
}
