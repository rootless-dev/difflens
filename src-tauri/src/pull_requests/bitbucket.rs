//! Bitbucket Cloud, through its REST API 2.0
//! (<https://developer.atlassian.com/cloud/bitbucket/rest/>), authenticated
//! with the Atlassian account email and an API token as basic auth.
//!
//! Inline comments carry `inline.to` (a line of the new side) or
//! `inline.from` (a line of the old side); both are line numbers of the pull
//! request's diff, which Bitbucket computes from the merge base of the
//! destination and the source commit, the same sides a review compares.
//!
//! API tokens need three scopes for what DiffLens does, as the API
//! reference lists them per endpoint: `read:user:bitbucket` (who the token
//! belongs to); `read:pullrequest:bitbucket` (list pull requests, list,
//! create, edit and delete comments, resolve and reopen threads); and
//! `write:pullrequest:bitbucket` (approve, request changes, and take either
//! back).

use std::sync::Arc;

use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Value};

use super::host::{Anchor, HostProvider, NewComment, PullRequestHost, RemoteComment};
use super::remote::RemoteRepo;
use crate::error::{app_error, core_text, escaped, AppError, AppResult, ErrorCode};
use crate::http::{self, Auth, Credential, HttpRequest, HttpResponse, Method, Transport};
use crate::model::{Account, Participant, PullRequest, PullRequestState, ReviewSide, Verdict};

pub const API: &str = "https://api.bitbucket.org/2.0";

/// Scopes an API token needs, shown next to the token field.
pub const SCOPES: &[&str] = &[
    "read:user:bitbucket",
    "read:pullrequest:bitbucket",
    "write:pullrequest:bitbucket",
];

/// Pages followed at most for one listing, so a host that keeps answering
/// with a `next` link cannot keep DiffLens busy forever.
const MAX_PAGES: usize = 100;

/// What is shown for a deleted comment that replies still hang from.
pub const DELETED_BODY: &str = "This comment was deleted.";

pub struct BitbucketProvider;

impl HostProvider for BitbucketProvider {
    fn key(&self) -> &'static str {
        "bitbucket"
    }

    fn name(&self) -> &'static str {
        "Bitbucket Cloud"
    }

    fn scopes(&self) -> &'static [&'static str] {
        SCOPES
    }

    /// `altssh` is Bitbucket's SSH on port 443.
    fn serves(&self, host: &str) -> bool {
        matches!(
            host,
            "bitbucket.org" | "www.bitbucket.org" | "altssh.bitbucket.org"
        )
    }

    fn connect(
        &self,
        credential: Credential,
        transport: Arc<dyn Transport>,
    ) -> Box<dyn PullRequestHost> {
        Box::new(BitbucketCloud::new(API, credential, transport))
    }
}

pub struct BitbucketCloud {
    api: Url,
    credential: Credential,
    transport: Arc<dyn Transport>,
}

impl BitbucketCloud {
    pub fn new(api: &str, credential: Credential, transport: Arc<dyn Transport>) -> Self {
        Self {
            api: Url::parse(api).expect("the Bitbucket API URL is valid"),
            credential,
            transport,
        }
    }

    /// The API URL of `segments`, each percent-encoded as one path segment.
    fn url(&self, segments: &[&str]) -> Url {
        let mut url = self.api.clone();
        url.path_segments_mut()
            .expect("the Bitbucket API URL has a path")
            .extend(segments);
        url
    }

    fn pull_request_url(&self, repo: &RemoteRepo, number: u64, rest: &[&str]) -> Url {
        let number = number.to_string();
        let mut segments = vec![
            "repositories",
            repo.owner.as_str(),
            repo.name.as_str(),
            "pullrequests",
            number.as_str(),
        ];
        segments.extend_from_slice(rest);
        self.url(&segments)
    }

    fn send(
        &self,
        method: Method,
        url: &Url,
        body: Option<Value>,
        missing: &dyn Fn() -> AppError,
    ) -> AppResult<String> {
        let mut request = HttpRequest::new(
            method,
            url.to_string(),
            Auth::Basic(self.credential.clone()),
        );
        request.body = body;
        let response = self.transport.send(&request).map_err(|reason| {
            app_error!(
                NetworkFailed,
                HostUnreachable,
                "Could not reach {host}: {detail}.",
                host = HOST,
                detail = reason
            )
        })?;
        if (200..300).contains(&response.status) {
            Ok(response.body)
        } else {
            Err(failure(&response, missing))
        }
    }

    fn get<T: for<'de> Deserialize<'de>>(
        &self,
        url: &Url,
        missing: &dyn Fn() -> AppError,
    ) -> AppResult<T> {
        parse(&self.send(Method::Get, url, None, missing)?)
    }

    /// Every value of a paginated listing, following `next` links only
    /// while they stay on the API: the credential goes with each request.
    fn all_pages<T: for<'de> Deserialize<'de>>(
        &self,
        first: Url,
        missing: &dyn Fn() -> AppError,
    ) -> AppResult<Vec<T>> {
        let mut values = Vec::new();
        let mut next = Some(first);
        for _ in 0..MAX_PAGES {
            let Some(url) = next.take() else {
                return Ok(values);
            };
            let page: Page<T> = self.get(&url, missing)?;
            values.extend(page.values);
            next = match page.next {
                Some(link) => Some(self.same_api(&link)?),
                None => None,
            };
        }
        Err(app_error!(
            RemoteFailed,
            HostTooManyPages,
            "{host} kept sending more pages than DiffLens reads.",
            host = HOST
        ))
    }

    fn same_api(&self, link: &str) -> AppResult<Url> {
        let url = Url::parse(link).ok().filter(|url| {
            url.origin() == self.api.origin() && url.path().starts_with(self.api.path())
        });
        url.ok_or_else(|| {
            app_error!(
                RemoteFailed,
                HostPageOutsideApi,
                "{host} pointed to a next page outside its API; it was not followed.",
                host = HOST
            )
        })
    }
}

impl PullRequestHost for BitbucketCloud {
    fn account(&self) -> AppResult<Account> {
        let user: UserJson = self.get(&self.url(&["user"]), &|| {
            app_error!(
                RemoteNotFound,
                BitbucketAccountNotFound,
                "Bitbucket did not find the account of this token."
            )
        })?;
        Ok(user.account())
    }

    fn open_pull_requests(&self, repo: &RemoteRepo, branch: &str) -> AppResult<Vec<PullRequest>> {
        let mut url = self.url(&["repositories", &repo.owner, &repo.name, "pullrequests"]);
        url.query_pairs_mut()
            .append_pair("state", "OPEN")
            .append_pair("q", &format!("source.branch.name = {}", quoted(branch)))
            .append_pair("pagelen", "50");
        let found: Vec<PullRequestJson> = self.all_pages(url, &|| repository_missing(repo))?;
        Ok(found
            .into_iter()
            .map(PullRequestJson::into_model)
            // The query matched; this guards against a host that ignores it.
            .filter(|pull_request| pull_request.source_branch == branch)
            .collect())
    }

    fn pull_request(&self, repo: &RemoteRepo, number: u64) -> AppResult<PullRequest> {
        let url = self.pull_request_url(repo, number, &[]);
        let found: PullRequestJson = self.get(&url, &|| pull_request_missing(repo, number))?;
        Ok(found.into_model())
    }

    fn comments(&self, repo: &RemoteRepo, number: u64) -> AppResult<Vec<RemoteComment>> {
        let mut url = self.pull_request_url(repo, number, &["comments"]);
        url.query_pairs_mut().append_pair("pagelen", "100");
        let found: Vec<CommentJson> =
            self.all_pages(url, &|| pull_request_missing(repo, number))?;
        Ok(found
            .into_iter()
            // A pending comment belongs to a review its author has not
            // submitted on Bitbucket: nobody else sees it yet.
            .filter(|comment| !comment.pending)
            .map(CommentJson::into_model)
            .collect())
    }

    fn create_comment(
        &self,
        repo: &RemoteRepo,
        number: u64,
        comment: &NewComment,
    ) -> AppResult<RemoteComment> {
        let url = self.pull_request_url(repo, number, &["comments"]);
        let body = match comment {
            NewComment::Inline { anchor, body } => {
                let line = match anchor.side {
                    ReviewSide::New => "to",
                    ReviewSide::Old => "from",
                };
                json!({
                    "content": { "raw": body },
                    "inline": { "path": anchor.path, line: anchor.line },
                })
            }
            // Without `inline`: on the pull request's overview.
            NewComment::General { body } => json!({ "content": { "raw": body } }),
            NewComment::Reply { parent_id, body } => json!({
                "content": { "raw": body },
                "parent": { "id": numeric_id(parent_id)? },
            }),
        };
        let created: CommentJson = parse(&self.send(Method::Post, &url, Some(body), &|| {
            app_error!(
                RemoteNotFound,
                BitbucketReplyTargetMissing,
                "Bitbucket has no pull request #{number} in {repository}, or the comment answered is gone.",
                number = number,
                repository = repository_name(repo)
            )
        })?)?;
        Ok(created.into_model())
    }

    fn update_comment(
        &self,
        repo: &RemoteRepo,
        number: u64,
        id: &str,
        body: &str,
    ) -> AppResult<RemoteComment> {
        let url = self.pull_request_url(repo, number, &["comments", &numeric_id(id)?.to_string()]);
        let updated: CommentJson = parse(&self.send(
            Method::Put,
            &url,
            Some(json!({ "content": { "raw": body } })),
            &comment_missing,
        )?)?;
        Ok(updated.into_model())
    }

    fn delete_comment(&self, repo: &RemoteRepo, number: u64, id: &str) -> AppResult<()> {
        let url = self.pull_request_url(repo, number, &["comments", &numeric_id(id)?.to_string()]);
        match self.send(Method::Delete, &url, None, &comment_missing) {
            Ok(_) => Ok(()),
            Err(error) if error.code == ErrorCode::RemoteNotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn set_resolved(
        &self,
        repo: &RemoteRepo,
        number: u64,
        id: &str,
        resolved: bool,
    ) -> AppResult<()> {
        let id = numeric_id(id)?.to_string();
        let url = self.pull_request_url(repo, number, &["comments", &id, "resolve"]);
        let method = if resolved {
            Method::Post
        } else {
            Method::Delete
        };
        self.send(method, &url, None, &comment_missing).map(|_| ())
    }

    fn set_verdict(
        &self,
        repo: &RemoteRepo,
        number: u64,
        verdict: Verdict,
        given: bool,
    ) -> AppResult<()> {
        let action = match verdict {
            Verdict::Approved => "approve",
            Verdict::ChangesRequested => "request-changes",
        };
        let url = self.pull_request_url(repo, number, &[action]);
        let method = if given { Method::Post } else { Method::Delete };
        self.send(method, &url, None, &|| {
            app_error!(
                RemoteNotFound,
                BitbucketVerdictTargetMissing,
                "Bitbucket has no pull request #{number} in {repository}, or nothing of yours to take back.",
                number = number,
                repository = repository_name(repo)
            )
        })
        .map(|_| ())
    }
}

/// The host's name in messages.
const HOST: &str = "Bitbucket";

/// A readable error for an answer that is not a success.
fn failure(response: &HttpResponse, missing: &dyn Fn() -> AppError) -> AppError {
    let detail = error_message(&response.body);
    match response.status {
        401 => http::host_credential_refused(HOST),
        403 => {
            let required = required_scopes(&response.body);
            let needs = if required.is_empty() {
                core_text!(
                    ScopesDiffLensNeeds,
                    "DiffLens needs {scopes}",
                    scopes = SCOPES.join(", ")
                )
            } else {
                core_text!(
                    ScopesThisNeeds,
                    "this needs {scopes}",
                    scopes = required.join(", ")
                )
            };
            match detail {
                Some(detail) => app_error!(
                    MissingScope,
                    BitbucketMissingScopeSaid,
                    "Bitbucket refused this ({detail}). The API token may be missing a scope: {needs}.",
                    detail = detail,
                    needs = needs
                ),
                None => app_error!(
                    MissingScope,
                    BitbucketMissingScope,
                    "Bitbucket refused this. The API token may be missing a scope: {needs}.",
                    needs = needs
                ),
            }
        }
        404 => missing(),
        429 => http::host_rate_limited(HOST, response.retry_after.as_deref()),
        status if status >= 500 => http::host_server_error(HOST, status),
        status => http::host_refused(HOST, status, detail),
    }
}

/// Bitbucket's own explanation, `{"error": {"message": ...}}`, kept short.
fn error_message(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let message = value.get("error")?.get("message")?.as_str()?.trim();
    if message.is_empty() {
        return None;
    }
    Some(message.chars().take(300).collect())
}

/// The scopes a refused request needed, when Bitbucket lists them in
/// `error.detail.required`; only names that look like scopes are kept.
fn required_scopes(body: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    value
        .pointer("/error/detail/required")
        .and_then(Value::as_array)
        .map(|scopes| {
            scopes
                .iter()
                .filter_map(Value::as_str)
                .filter(|scope| {
                    scope.len() <= 64
                        && scope
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == ':' || c == '_')
                })
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn repository_name(repo: &RemoteRepo) -> String {
    format!("{}/{}", repo.owner, repo.name)
}

fn repository_missing(repo: &RemoteRepo) -> AppError {
    app_error!(
        RemoteNotFound,
        BitbucketRepositoryMissing,
        "Bitbucket has no repository {repository}, or the connected account cannot see it.",
        repository = repository_name(repo)
    )
}

fn pull_request_missing(repo: &RemoteRepo, number: u64) -> AppError {
    app_error!(
        RemoteNotFound,
        BitbucketPullRequestMissing,
        "Bitbucket has no pull request #{number} in {repository}.",
        number = number,
        repository = repository_name(repo)
    )
}

fn comment_missing() -> AppError {
    app_error!(
        RemoteNotFound,
        RemoteCommentGone,
        "That comment is no longer on the pull request."
    )
}

fn parse<T: for<'de> Deserialize<'de>>(body: &str) -> AppResult<T> {
    serde_json::from_str(body).map_err(|error| http::host_unreadable(HOST, &error))
}

/// Comment ids are numbers on Bitbucket.
fn numeric_id(id: &str) -> AppResult<u64> {
    id.parse().map_err(|_| {
        app_error!(
            Internal,
            NotABitbucketCommentId,
            "\"{id}\" is not a Bitbucket comment id.",
            id = escaped(id)
        )
    })
}

/// A string literal of Bitbucket's query language.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

#[derive(Deserialize)]
struct Page<T> {
    #[serde(default = "Vec::new")]
    values: Vec<T>,
    next: Option<String>,
}

#[derive(Deserialize, Default)]
struct UserJson {
    uuid: Option<String>,
    account_id: Option<String>,
    display_name: Option<String>,
    nickname: Option<String>,
}

impl UserJson {
    fn account(self) -> Account {
        Account {
            id: self.uuid.or(self.account_id).unwrap_or_default(),
            name: self
                .display_name
                .or(self.nickname)
                .unwrap_or_else(|| "Former user".to_string()),
        }
    }
}

#[derive(Deserialize)]
struct PullRequestJson {
    id: u64,
    #[serde(default)]
    title: String,
    description: Option<String>,
    #[serde(default)]
    state: String,
    links: Option<LinksJson>,
    author: Option<UserJson>,
    source: EndpointJson,
    destination: EndpointJson,
    /// Listings leave it out; a single pull request has it.
    #[serde(default)]
    participants: Vec<ParticipantJson>,
}

#[derive(Deserialize)]
struct ParticipantJson {
    user: Option<UserJson>,
    role: Option<String>,
    #[serde(default)]
    approved: bool,
    state: Option<String>,
}

impl ParticipantJson {
    fn into_model(self) -> Participant {
        // `state` is the newer field; `approved` alone still says approved.
        let verdict = match self.state.as_deref() {
            Some("approved") => Some(Verdict::Approved),
            Some("changes_requested") => Some(Verdict::ChangesRequested),
            _ if self.approved => Some(Verdict::Approved),
            _ => None,
        };
        Participant {
            account: self.user.unwrap_or_default().account(),
            reviewer: self.role.as_deref() == Some("REVIEWER"),
            verdict,
        }
    }
}

#[derive(Deserialize)]
struct LinksJson {
    html: Option<HrefJson>,
}

#[derive(Deserialize)]
struct HrefJson {
    href: String,
}

#[derive(Deserialize)]
struct EndpointJson {
    branch: Option<NameJson>,
    commit: Option<HashJson>,
}

#[derive(Deserialize)]
struct NameJson {
    name: String,
}

#[derive(Deserialize)]
struct HashJson {
    hash: String,
}

impl PullRequestJson {
    fn into_model(self) -> PullRequest {
        PullRequest {
            number: self.id,
            title: self.title,
            description: self.description.unwrap_or_default(),
            state: match self.state.as_str() {
                "OPEN" => PullRequestState::Open,
                "MERGED" => PullRequestState::Merged,
                "DECLINED" => PullRequestState::Declined,
                // Anything else is not open, so nothing is published to it.
                _ => PullRequestState::Superseded,
            },
            url: self
                .links
                .and_then(|links| links.html)
                .map(|html| html.href)
                .unwrap_or_default(),
            author: self.author.map(|author| author.account().name),
            source_branch: self
                .source
                .branch
                .map(|branch| branch.name)
                .unwrap_or_default(),
            source_commit: self.source.commit.map(|commit| commit.hash),
            destination_branch: self
                .destination
                .branch
                .map(|branch| branch.name)
                .unwrap_or_default(),
            destination_commit: self.destination.commit.map(|commit| commit.hash),
            participants: self
                .participants
                .into_iter()
                .map(ParticipantJson::into_model)
                .collect(),
        }
    }
}

#[derive(Deserialize)]
struct CommentJson {
    id: u64,
    content: Option<ContentJson>,
    user: Option<UserJson>,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    pending: bool,
    parent: Option<IdJson>,
    inline: Option<InlineJson>,
    resolution: Option<Value>,
    created_on: Option<String>,
    updated_on: Option<String>,
}

#[derive(Deserialize)]
struct ContentJson {
    raw: Option<String>,
}

#[derive(Deserialize)]
struct IdJson {
    id: u64,
}

#[derive(Deserialize)]
struct InlineJson {
    path: String,
    from: Option<u32>,
    to: Option<u32>,
}

impl CommentJson {
    fn into_model(self) -> RemoteComment {
        // Without `inline`: on the pull request as a whole (its overview).
        let general = self.inline.is_none();
        // A line of the new side wins: a comment on an unchanged line has
        // both numbers, and the review shows unchanged lines on the new side.
        let anchor = self.inline.and_then(|inline| {
            let (side, line) = match (inline.to, inline.from) {
                (Some(to), _) => (ReviewSide::New, to),
                (None, Some(from)) => (ReviewSide::Old, from),
                // A comment on the whole file.
                (None, None) => return None,
            };
            Some(Anchor {
                path: inline.path,
                side,
                line,
            })
        });
        let created_at = self.created_on.unwrap_or_default();
        RemoteComment {
            id: self.id.to_string(),
            parent_id: self.parent.map(|parent| parent.id.to_string()),
            author: self.user.unwrap_or_default().account(),
            body: if self.deleted {
                DELETED_BODY.to_string()
            } else {
                self.content
                    .and_then(|content| content.raw)
                    .unwrap_or_default()
            },
            anchor,
            general,
            resolved: self.resolution.is_some(),
            deleted: self.deleted,
            updated_at: self.updated_on.unwrap_or_else(|| created_at.clone()),
            created_at,
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    pub use crate::http::tests::CannedTransport;

    pub fn credential() -> Credential {
        Credential {
            username: "dana@example.com".to_string(),
            secret: "token".to_string(),
        }
    }

    fn client(transport: &Arc<CannedTransport>) -> BitbucketCloud {
        BitbucketCloud::new(
            API,
            credential(),
            Arc::clone(transport) as Arc<dyn Transport>,
        )
    }

    fn repo() -> RemoteRepo {
        RemoteRepo {
            host: "bitbucket.org".to_string(),
            owner: "acme".to_string(),
            name: "payments-api".to_string(),
        }
    }

    pub fn pull_request_json(id: u64, branch: &str) -> Value {
        json!({
            "id": id,
            "title": "Adds the refund flow",
            "description": "Refunds settled payments.",
            "state": "OPEN",
            "links": { "html": { "href": format!("https://bitbucket.org/acme/payments-api/pull-requests/{id}") } },
            "author": { "uuid": "{a1}", "display_name": "Ana" },
            "source": { "branch": { "name": branch }, "commit": { "hash": "e7f3b10c9d8e" } },
            "destination": { "branch": { "name": "main" }, "commit": { "hash": "0123456789ab" } },
        })
    }

    #[test]
    fn finds_the_open_pull_request_of_a_branch_across_pages() {
        let transport = CannedTransport::answering(vec![
            (
                200,
                json!({
                    "values": [pull_request_json(7, "feat/refund-flow-v0")],
                    "next": "https://api.bitbucket.org/2.0/repositories/acme/payments-api/pullrequests?page=2",
                }),
            ),
            (
                200,
                json!({ "values": [pull_request_json(318, "feat/refund \"flow\"")] }),
            ),
        ]);
        let found = client(&transport)
            .open_pull_requests(&repo(), "feat/refund \"flow\"")
            .unwrap();

        assert_eq!(found.len(), 1);
        let pull_request = &found[0];
        assert_eq!(pull_request.number, 318);
        assert_eq!(pull_request.state, PullRequestState::Open);
        assert_eq!(pull_request.description, "Refunds settled payments.");
        assert_eq!(
            pull_request.url,
            "https://bitbucket.org/acme/payments-api/pull-requests/318"
        );
        assert_eq!(pull_request.source_commit.as_deref(), Some("e7f3b10c9d8e"));
        assert_eq!(pull_request.destination_branch, "main");

        let sent = transport.sent();
        assert_eq!(sent.len(), 2);
        let first = Url::parse(&sent[0].url).unwrap();
        assert_eq!(
            first.path(),
            "/2.0/repositories/acme/payments-api/pullrequests"
        );
        let query: Vec<(String, String)> = first.query_pairs().into_owned().collect();
        assert!(query.contains(&("state".to_string(), "OPEN".to_string())));
        assert!(query.contains(&(
            "q".to_string(),
            r#"source.branch.name = "feat/refund \"flow\"""#.to_string()
        )));
        assert_eq!(sent[0].method, Method::Get);
        assert_eq!(sent[0].auth, Auth::Basic(credential()));
    }

    #[test]
    fn a_next_link_off_the_api_is_not_followed() {
        let transport = CannedTransport::answering(vec![(
            200,
            json!({ "values": [], "next": "https://evil.example.com/2.0/steal" }),
        )]);
        let error = client(&transport)
            .open_pull_requests(&repo(), "feat/x")
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::RemoteFailed);
        assert_eq!(transport.sent().len(), 1);
    }

    #[test]
    fn maps_inline_comments_replies_and_resolution() {
        let transport = CannedTransport::answering(vec![(
            200,
            json!({ "values": [
                {
                    "id": 1, "content": { "raw": "Why here?" },
                    "user": { "uuid": "{b2}", "display_name": "Bruno" },
                    "inline": { "path": "internal/refund/service.go", "from": null, "to": 44 },
                    "resolution": { "type": "resolved" },
                    "created_on": "2026-11-18T10:00:00Z", "updated_on": "2026-11-18T11:00:00Z",
                },
                {
                    "id": 2, "content": { "raw": "Was this removed on purpose?" },
                    "user": { "uuid": "{b2}", "display_name": "Bruno" },
                    "inline": { "path": "internal/legacy/old.go", "from": 12, "to": null },
                    "created_on": "2026-11-18T10:01:00Z",
                },
                {
                    "id": 3, "content": { "raw": "Agreed" }, "parent": { "id": 1 },
                    "user": { "uuid": "{c3}", "display_name": "Carla" },
                    "inline": { "path": "internal/refund/service.go", "to": 44 },
                    "created_on": "2026-11-18T10:02:00Z",
                },
                {
                    "id": 4, "content": { "raw": "" }, "deleted": true,
                    "user": { "uuid": "{c3}", "display_name": "Carla" },
                    "created_on": "2026-11-18T10:03:00Z",
                },
                {
                    "id": 5, "content": { "raw": "Not submitted yet" }, "pending": true,
                    "user": { "uuid": "{c3}", "display_name": "Carla" },
                    "created_on": "2026-11-18T10:04:00Z",
                },
            ] }),
        )]);
        let comments = client(&transport).comments(&repo(), 318).unwrap();

        assert_eq!(comments.len(), 4, "the pending comment is left out");
        assert_eq!(
            comments[0].anchor,
            Some(Anchor {
                path: "internal/refund/service.go".to_string(),
                side: ReviewSide::New,
                line: 44
            })
        );
        assert!(comments[0].resolved);
        assert_eq!(comments[0].author.id, "{b2}");
        assert_eq!(comments[0].updated_at, "2026-11-18T11:00:00Z");
        assert_eq!(
            comments[1]
                .anchor
                .as_ref()
                .map(|anchor| (anchor.side, anchor.line)),
            Some((ReviewSide::Old, 12))
        );
        assert!(!comments[1].resolved);
        assert_eq!(comments[2].parent_id.as_deref(), Some("1"));
        assert!(comments[3].deleted);
        assert_eq!(comments[3].anchor, None);
        assert_eq!(comments[3].body, DELETED_BODY);
        let url = Url::parse(&transport.sent()[0].url).unwrap();
        assert_eq!(
            url.path(),
            "/2.0/repositories/acme/payments-api/pullrequests/318/comments"
        );
    }

    #[test]
    fn creates_inline_comments_on_either_side_and_replies() {
        let created = |id: u64| {
            (
                201,
                json!({ "id": id, "content": { "raw": "x" }, "user": { "uuid": "{me}", "display_name": "Me" } }),
            )
        };
        let transport = CannedTransport::answering(vec![created(10), created(11), created(12)]);
        let bitbucket = client(&transport);
        let anchor = |side, line| Anchor {
            path: "a.go".to_string(),
            side,
            line,
        };

        let new_side = bitbucket
            .create_comment(
                &repo(),
                318,
                &NewComment::Inline {
                    anchor: anchor(ReviewSide::New, 8),
                    body: "On the new line".to_string(),
                },
            )
            .unwrap();
        bitbucket
            .create_comment(
                &repo(),
                318,
                &NewComment::Inline {
                    anchor: anchor(ReviewSide::Old, 3),
                    body: "On the old line".to_string(),
                },
            )
            .unwrap();
        bitbucket
            .create_comment(
                &repo(),
                318,
                &NewComment::Reply {
                    parent_id: "10".to_string(),
                    body: "A reply".to_string(),
                },
            )
            .unwrap();

        assert_eq!(new_side.id, "10");
        assert_eq!(new_side.author.id, "{me}");
        let bodies: Vec<Value> = transport
            .sent()
            .into_iter()
            .map(|request| {
                assert_eq!(request.method, Method::Post);
                request.body.unwrap()
            })
            .collect();
        assert_eq!(
            bodies[0],
            json!({ "content": { "raw": "On the new line" }, "inline": { "path": "a.go", "to": 8 } })
        );
        assert_eq!(
            bodies[1],
            json!({ "content": { "raw": "On the old line" }, "inline": { "path": "a.go", "from": 3 } })
        );
        assert_eq!(
            bodies[2],
            json!({ "content": { "raw": "A reply" }, "parent": { "id": 10 } })
        );
    }

    #[test]
    fn general_comments_go_without_inline_and_are_read_back_apart_from_file_comments() {
        let transport = CannedTransport::answering(vec![
            (
                201,
                json!({
                    "id": 20, "content": { "raw": "The race in `recordLastLogin`" },
                    "user": { "uuid": "{me}", "display_name": "Me" },
                    "created_on": "2026-11-21T10:00:00Z",
                }),
            ),
            (
                201,
                json!({
                    "id": 21, "content": { "raw": "Fixed" }, "parent": { "id": 20 },
                    "user": { "uuid": "{a1}", "display_name": "Ana" },
                    "created_on": "2026-11-21T10:05:00Z",
                }),
            ),
            (
                200,
                json!({ "values": [
                    {
                        "id": 20, "content": { "raw": "The race in `recordLastLogin`" },
                        "user": { "uuid": "{me}", "display_name": "Me" },
                        "created_on": "2026-11-21T10:00:00Z",
                    },
                    {
                        "id": 21, "content": { "raw": "Fixed" }, "parent": { "id": 20 },
                        "user": { "uuid": "{a1}", "display_name": "Ana" },
                        "created_on": "2026-11-21T10:05:00Z",
                    },
                    {
                        "id": 22, "content": { "raw": "On the whole file" },
                        "user": { "uuid": "{a1}", "display_name": "Ana" },
                        "inline": { "path": "a.go" },
                        "created_on": "2026-11-21T10:06:00Z",
                    },
                ] }),
            ),
        ]);
        let bitbucket = client(&transport);

        let created = bitbucket
            .create_comment(
                &repo(),
                318,
                &NewComment::General {
                    body: "The race in `recordLastLogin`".to_string(),
                },
            )
            .unwrap();
        assert_eq!(created.id, "20");
        assert!(created.general);
        assert_eq!(created.anchor, None);
        let reply = bitbucket
            .create_comment(
                &repo(),
                318,
                &NewComment::Reply {
                    parent_id: "20".to_string(),
                    body: "Fixed".to_string(),
                },
            )
            .unwrap();
        assert_eq!(reply.parent_id.as_deref(), Some("20"));
        let read = bitbucket.comments(&repo(), 318).unwrap();

        let sent = transport.sent();
        assert_eq!(
            sent[0].body,
            Some(json!({ "content": { "raw": "The race in `recordLastLogin`" } })),
            "no inline: on the pull request's overview"
        );
        assert_eq!(sent[1].body, Some(json!({ "content": { "raw": "Fixed" }, "parent": { "id": 20 } })));
        for request in &sent[..2] {
            assert_eq!(request.method, Method::Post);
            assert_eq!(
                Url::parse(&request.url).unwrap().path(),
                "/2.0/repositories/acme/payments-api/pullrequests/318/comments"
            );
        }
        let kinds: Vec<(&str, bool, bool)> = read
            .iter()
            .map(|comment| (comment.id.as_str(), comment.general, comment.anchor.is_some()))
            .collect();
        // A comment on a whole file has `inline` without a line: neither.
        assert_eq!(kinds, vec![("20", true, false), ("21", true, false), ("22", false, false)]);
        assert_eq!(read[1].author.name, "Ana");
    }

    #[test]
    fn edits_deletes_resolves_and_reopens() {
        let transport = CannedTransport::answering(vec![
            (200, json!({ "id": 10, "content": { "raw": "Edited" } })),
            (204, Value::Null),
            (
                404,
                json!({ "type": "error", "error": { "message": "No such comment" } }),
            ),
            (200, json!({})),
            (204, Value::Null),
        ]);
        let bitbucket = client(&transport);
        let edited = bitbucket
            .update_comment(&repo(), 318, "10", "Edited")
            .unwrap();
        assert_eq!(edited.body, "Edited");
        bitbucket.delete_comment(&repo(), 318, "10").unwrap();
        // Already gone counts as deleted.
        bitbucket.delete_comment(&repo(), 318, "11").unwrap();
        bitbucket.set_resolved(&repo(), 318, "12", true).unwrap();
        bitbucket.set_resolved(&repo(), 318, "12", false).unwrap();

        let calls: Vec<(Method, String)> = transport
            .sent()
            .into_iter()
            .map(|request| {
                (
                    request.method,
                    Url::parse(&request.url).unwrap().path().to_string(),
                )
            })
            .collect();
        let base = "/2.0/repositories/acme/payments-api/pullrequests/318/comments";
        assert_eq!(
            calls,
            vec![
                (Method::Put, format!("{base}/10")),
                (Method::Delete, format!("{base}/10")),
                (Method::Delete, format!("{base}/11")),
                (Method::Post, format!("{base}/12/resolve")),
                (Method::Delete, format!("{base}/12/resolve")),
            ]
        );
    }

    #[test]
    fn failures_read_as_what_went_wrong() {
        let transport = CannedTransport::default();
        transport.push(401, json!({}));
        transport.push(
            403,
            json!({ "type": "error", "error": { "message": "Your credentials lack one or more required privilege scopes." } }),
        );
        transport.push(
            404,
            json!({ "type": "error", "error": { "message": "Repository not found" } }),
        );
        transport.push_raw(Ok(HttpResponse {
            status: 429,
            body: String::new(),
            retry_after: Some("30".to_string()),
            headers: Vec::new(),
        }));
        transport.push(503, json!({}));
        transport.push(
            400,
            json!({ "type": "error", "error": { "message": "Invalid inline anchor" } }),
        );
        transport.push_raw(Err("it did not answer in time".to_string()));
        let transport = Arc::new(transport);
        let bitbucket = client(&transport);
        let next = || bitbucket.open_pull_requests(&repo(), "feat/x").unwrap_err();

        let expired = next();
        assert_eq!(expired.code, ErrorCode::AuthFailed);
        assert!(expired.message.contains("expired or been revoked"));
        let scope = next();
        assert_eq!(scope.code, ErrorCode::MissingScope);
        assert!(scope
            .message
            .contains("lack one or more required privilege scopes"));
        assert!(scope.message.contains("read:pullrequest:bitbucket"));
        let missing = next();
        assert_eq!(missing.code, ErrorCode::RemoteNotFound);
        assert!(missing.message.contains("acme/payments-api"));
        let limited = next();
        assert_eq!(limited.code, ErrorCode::RateLimited);
        assert!(limited.message.contains("30 seconds"));
        assert_eq!(next().code, ErrorCode::RemoteFailed);
        let refused = next();
        assert_eq!(refused.code, ErrorCode::RemoteFailed);
        assert!(refused.message.contains("Invalid inline anchor"));
        let offline = next();
        assert_eq!(offline.code, ErrorCode::NetworkFailed);
        assert_eq!(
            offline.message,
            "Could not reach Bitbucket: it did not answer in time."
        );
    }

    #[test]
    fn reads_participants_and_their_verdicts() {
        let mut pull_request = pull_request_json(318, "feat/refund-flow");
        pull_request["participants"] = json!([
            { "user": { "uuid": "{b2}", "display_name": "Bruno" }, "role": "REVIEWER",
              "approved": true, "state": "approved" },
            { "user": { "uuid": "{c3}", "display_name": "Carla" }, "role": "REVIEWER",
              "approved": false, "state": "changes_requested" },
            { "user": { "uuid": "{d4}", "display_name": "Dana" }, "role": "PARTICIPANT",
              "approved": false, "state": null },
            // Older answers carry only `approved`.
            { "user": { "uuid": "{e5}", "display_name": "Edu" }, "role": "PARTICIPANT",
              "approved": true },
        ]);
        let transport = CannedTransport::answering(vec![(200, pull_request)]);
        let found = client(&transport).pull_request(&repo(), 318).unwrap();

        let participants: Vec<(&str, bool, Option<Verdict>)> = found
            .participants
            .iter()
            .map(|participant| {
                (
                    participant.account.name.as_str(),
                    participant.reviewer,
                    participant.verdict,
                )
            })
            .collect();
        assert_eq!(
            participants,
            vec![
                ("Bruno", true, Some(Verdict::Approved)),
                ("Carla", true, Some(Verdict::ChangesRequested)),
                ("Dana", false, None),
                ("Edu", false, Some(Verdict::Approved)),
            ]
        );
        assert_eq!(found.participants[0].account.id, "{b2}");
    }

    #[test]
    fn approves_requests_changes_and_takes_either_back() {
        let transport = CannedTransport::answering(vec![
            (200, json!({ "approved": true })),
            (204, Value::Null),
            (200, json!({ "state": "changes_requested" })),
            (204, Value::Null),
        ]);
        let bitbucket = client(&transport);
        bitbucket
            .set_verdict(&repo(), 318, Verdict::Approved, true)
            .unwrap();
        bitbucket
            .set_verdict(&repo(), 318, Verdict::Approved, false)
            .unwrap();
        bitbucket
            .set_verdict(&repo(), 318, Verdict::ChangesRequested, true)
            .unwrap();
        bitbucket
            .set_verdict(&repo(), 318, Verdict::ChangesRequested, false)
            .unwrap();

        let base = "/2.0/repositories/acme/payments-api/pullrequests/318";
        let calls: Vec<(Method, String, bool)> = transport
            .sent()
            .into_iter()
            .map(|request| {
                (
                    request.method,
                    Url::parse(&request.url).unwrap().path().to_string(),
                    request.body.is_none(),
                )
            })
            .collect();
        assert_eq!(
            calls,
            vec![
                (Method::Post, format!("{base}/approve"), true),
                (Method::Delete, format!("{base}/approve"), true),
                (Method::Post, format!("{base}/request-changes"), true),
                (Method::Delete, format!("{base}/request-changes"), true),
            ]
        );
    }

    #[test]
    fn approving_without_the_write_scope_says_which_scope_is_missing() {
        let transport = CannedTransport::answering(vec![(
            403,
            json!({ "type": "error", "error": { "message": "Your credentials lack one or more required privilege scopes.", "detail": { "required": ["write:pullrequest:bitbucket"] } } }),
        )]);
        let error = client(&transport)
            .set_verdict(&repo(), 318, Verdict::Approved, true)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::MissingScope);
        assert!(error.message.contains("write:pullrequest:bitbucket"));
        assert!(error
            .message
            .contains("lack one or more required privilege scopes"));
    }

    #[test]
    fn reads_the_account_of_the_token() {
        let transport = CannedTransport::answering(vec![(
            200,
            json!({ "uuid": "{d4}", "account_id": "557058:x", "display_name": "Dana", "nickname": "dana" }),
        )]);
        let account = client(&transport).account().unwrap();
        assert_eq!(
            account,
            Account {
                id: "{d4}".to_string(),
                name: "Dana".to_string()
            }
        );
        assert_eq!(
            Url::parse(&transport.sent()[0].url).unwrap().path(),
            "/2.0/user"
        );
    }

    #[test]
    fn serves_bitbucket_cloud_only() {
        let provider = BitbucketProvider;
        assert!(provider.serves("bitbucket.org"));
        assert!(provider.serves("altssh.bitbucket.org"));
        assert!(!provider.serves("bitbucket.example.com"));
        assert!(!provider.serves("github.com"));
    }
}
