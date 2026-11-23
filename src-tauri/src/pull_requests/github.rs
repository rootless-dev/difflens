//! GitHub (github.com), through its REST API
//! (<https://docs.github.com/en/rest>), authenticated with a personal access
//! token as a bearer token. Its GraphQL API
//! (<https://docs.github.com/en/graphql>) does only what REST cannot:
//! telling which review threads are resolved, and resolving or reopening
//! them.
//!
//! A pull request has two kinds of comments on GitHub. Review comments sit
//! on a line of one side of a file (`side` LEFT for the old side, RIGHT for
//! the new; `line` in that side's numbering) or on a whole file, in threads
//! one level deep. Issue comments, and the text of a submitted review, are
//! on the pull request as a whole, without threads. Each kind numbers its
//! own, so DiffLens prefixes an id with its kind: `review-comment:12`,
//! `issue-comment:34`, `review:56`.
//!
//! "Publish comments" sends its new threads on lines as one review: created
//! pending (only its author sees it), then submitted as a comment, so they
//! land together. Replies and comments on the whole pull request cannot be
//! part of a review through REST, so they go one by one, as the web does
//! with a reply outside a review. A verdict is a review too, and the latest
//! one replaces the reviewer's previous verdict; taking one back dismisses
//! that review, which GitHub allows only to accounts with write access.
//!
//! Tokens: a classic token needs the `repo` scope; a fine-grained one needs,
//! on the repositories reviewed, Pull requests (read and write), Contents
//! (read) and Metadata (read).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Value};

use super::host::{Anchor, HostProvider, LineComment, NewComment, PullRequestHost, RemoteComment};
use super::remote::RemoteRepo;
use crate::error::{app_error, core_text, escaped, AppError, AppResult, ErrorCode};
use crate::http::{
    self, Auth, Credential, HttpRequest, HttpResponse, Method, Secret, Transport,
};
use crate::model::{Account, Participant, PullRequest, PullRequestState, ReviewSide, Verdict};

pub const API: &str = "https://api.github.com";

/// The REST API version DiffLens is written against.
pub const API_VERSION: &str = "2022-11-28";

const MEDIA_TYPE: &str = "application/vnd.github+json";

/// The scope a classic token needs, shown next to the token field.
pub const SCOPES: &[&str] = &["repo"];

/// Pages followed at most for one listing, so a host that keeps answering
/// with a next page cannot keep DiffLens busy forever.
const MAX_PAGES: usize = 100;

/// The most GitHub gives in one page.
const PER_PAGE: &str = "100";

/// GitHub refuses a request for changes without a text; DiffLens sends the
/// verdict alone, so it says only that.
pub const CHANGES_REQUESTED_BODY: &str = "Changes requested.";

/// GitHub refuses a dismissal without a message.
pub const TAKEN_BACK_MESSAGE: &str = "Taken back by the reviewer.";

/// The host's name in messages.
const HOST: &str = "GitHub";

pub struct GithubProvider;

impl HostProvider for GithubProvider {
    fn key(&self) -> &'static str {
        "github"
    }

    fn name(&self) -> &'static str {
        "GitHub"
    }

    fn scopes(&self) -> &'static [&'static str] {
        SCOPES
    }

    /// `ssh.github.com` is GitHub's SSH on port 443.
    fn serves(&self, host: &str) -> bool {
        matches!(host, "github.com" | "www.github.com" | "ssh.github.com")
    }

    /// The token alone says who it belongs to.
    fn needs_username(&self) -> bool {
        false
    }

    fn connect(
        &self,
        credential: Credential,
        transport: Arc<dyn Transport>,
    ) -> Box<dyn PullRequestHost> {
        Box::new(Github::new(API, &credential.secret, transport))
    }
}

pub struct Github {
    api: Url,
    /// The whole `Authorization` value, `Bearer <token>`.
    authorization: Secret,
    transport: Arc<dyn Transport>,
}

impl Github {
    pub fn new(api: &str, token: &str, transport: Arc<dyn Transport>) -> Self {
        Self {
            api: Url::parse(api).expect("the GitHub API URL is valid"),
            authorization: Secret::new(format!("Bearer {token}")),
            transport,
        }
    }

    /// The API URL of `segments`, each percent-encoded as one path segment.
    fn url(&self, segments: &[&str]) -> Url {
        let mut url = self.api.clone();
        url.path_segments_mut()
            .expect("the GitHub API URL has a path")
            .pop_if_empty()
            .extend(segments);
        url
    }

    fn repo_url(&self, repo: &RemoteRepo, rest: &[&str]) -> Url {
        let mut segments = vec!["repos", repo.owner.as_str(), repo.name.as_str()];
        segments.extend_from_slice(rest);
        self.url(&segments)
    }

    fn pull_url(&self, repo: &RemoteRepo, number: u64, rest: &[&str]) -> Url {
        let number = number.to_string();
        let mut segments = vec!["pulls", number.as_str()];
        segments.extend_from_slice(rest);
        self.repo_url(repo, &segments)
    }

    fn send(
        &self,
        method: Method,
        url: &Url,
        body: Option<Value>,
        missing: &dyn Fn() -> AppError,
    ) -> AppResult<HttpResponse> {
        let mut request = HttpRequest::new(
            method,
            url.to_string(),
            Auth::Header {
                name: "Authorization",
                value: self.authorization.clone(),
            },
        )
        .header("Accept", MEDIA_TYPE)
        .header("X-GitHub-Api-Version", API_VERSION);
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
            Ok(response)
        } else {
            Err(failure(&response, missing))
        }
    }

    fn get<T: for<'de> Deserialize<'de>>(
        &self,
        url: &Url,
        missing: &dyn Fn() -> AppError,
    ) -> AppResult<T> {
        parse(&self.send(Method::Get, url, None, missing)?.body)
    }

    fn post<T: for<'de> Deserialize<'de>>(
        &self,
        url: &Url,
        body: Value,
        missing: &dyn Fn() -> AppError,
    ) -> AppResult<T> {
        parse(&self.send(Method::Post, url, Some(body), missing)?.body)
    }

    /// Every item of a paginated listing, following the `Link` header's
    /// next page only while it stays on the API: the token goes with each
    /// request.
    fn all_pages<T: for<'de> Deserialize<'de>>(
        &self,
        mut first: Url,
        missing: &dyn Fn() -> AppError,
    ) -> AppResult<Vec<T>> {
        first.query_pairs_mut().append_pair("per_page", PER_PAGE);
        let mut values = Vec::new();
        let mut next = Some(first);
        for _ in 0..MAX_PAGES {
            let Some(url) = next.take() else {
                return Ok(values);
            };
            let response = self.send(Method::Get, &url, None, missing)?;
            let page: Vec<T> = parse(&response.body)?;
            values.extend(page);
            next = match response.header("link").and_then(next_link) {
                Some(link) => Some(self.same_api(link)?),
                None => None,
            };
        }
        Err(too_many_pages())
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

    /// The `data` of a GraphQL answer, or why there is none: GitHub answers
    /// a query it could not fully run with `errors`, often with status 200.
    fn graphql<T: for<'de> Deserialize<'de>>(
        &self,
        query: &str,
        variables: Value,
        missing: &dyn Fn() -> AppError,
    ) -> AppResult<T> {
        let answer: GraphqlJson<T> = self.post(
            &self.url(&["graphql"]),
            json!({ "query": query, "variables": variables }),
            missing,
        )?;
        if let Some(error) = answer.errors.into_iter().next() {
            return Err(graphql_failure(error, missing));
        }
        answer.data.ok_or_else(missing)
    }

    /// Every review thread of a pull request, with whether it is resolved.
    fn review_threads(&self, repo: &RemoteRepo, number: u64) -> AppResult<Vec<ThreadJson>> {
        let missing = || pull_request_missing(repo, number);
        let mut threads = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let data: ThreadsData = self.graphql(
                THREADS_QUERY,
                json!({
                    "owner": repo.owner,
                    "name": repo.name,
                    "number": number,
                    "after": after,
                }),
                &missing,
            )?;
            let page = data
                .repository
                .and_then(|repository| repository.pull_request)
                .ok_or_else(missing)?
                .review_threads;
            threads.extend(page.nodes.into_iter().flatten());
            match (page.page_info.has_next_page, page.page_info.end_cursor) {
                (true, Some(cursor)) => after = Some(cursor),
                _ => return Ok(threads),
            }
        }
        Err(too_many_pages())
    }

    /// Every review of a pull request, oldest first, the account's own
    /// pending one included.
    fn reviews(&self, repo: &RemoteRepo, number: u64) -> AppResult<Vec<ReviewJson>> {
        self.all_pages(self.pull_url(repo, number, &["reviews"]), &|| {
            pull_request_missing(repo, number)
        })
    }

    /// The pull request's source commit, which a comment on a line is made
    /// at.
    fn head_commit(&self, repo: &RemoteRepo, number: u64) -> AppResult<String> {
        let missing = || pull_request_missing(repo, number);
        let found: PullJson = self.get(&self.pull_url(repo, number, &[]), &missing)?;
        found.head.sha.ok_or_else(missing)
    }

    /// Creates `comments` as a pending review, reads what GitHub made of
    /// them, then submits it. When any step fails, the pending review is
    /// deleted, so nothing of it lingers on the pull request.
    fn review(
        &self,
        repo: &RemoteRepo,
        number: u64,
        commit: Option<&str>,
        comments: &[LineComment],
    ) -> AppResult<Vec<RemoteComment>> {
        let missing = || pull_request_missing(repo, number);
        let mut body = json!({
            "comments": comments
                .iter()
                .map(|comment| json!({
                    "path": comment.anchor.path,
                    "line": comment.anchor.line,
                    "side": side_name(comment.anchor.side),
                    "body": comment.body,
                }))
                .collect::<Vec<Value>>(),
        });
        if let Some(commit) = commit {
            body["commit_id"] = json!(commit);
        }
        // Without an event, the review stays pending.
        let pending: ReviewJson =
            self.post(&self.pull_url(repo, number, &["reviews"]), body, &missing)?;
        let id = pending.id.to_string();
        let submitted = self
            .all_pages(self.pull_url(repo, number, &["reviews", &id, "comments"]), &missing)
            .and_then(|created| matched(comments, created))
            .and_then(|created| {
                self.send(
                    Method::Post,
                    &self.pull_url(repo, number, &["reviews", &id, "events"]),
                    Some(json!({ "event": "COMMENT" })),
                    &missing,
                )?;
                Ok(created)
            });
        if submitted.is_err() {
            let _ = self.send(
                Method::Delete,
                &self.pull_url(repo, number, &["reviews", &id]),
                None,
                &missing,
            );
        }
        submitted
    }
}

impl PullRequestHost for Github {
    fn account(&self) -> AppResult<Account> {
        let user: UserJson = self.get(&self.url(&["user"]), &|| {
            app_error!(
                RemoteNotFound,
                GithubAccountNotFound,
                "GitHub did not find the account of this token."
            )
        })?;
        Ok(user.account())
    }

    fn open_pull_requests(&self, repo: &RemoteRepo, branch: &str) -> AppResult<Vec<PullRequest>> {
        let mut url = self.repo_url(repo, &["pulls"]);
        url.query_pairs_mut()
            .append_pair("state", "open")
            .append_pair("head", &format!("{}:{branch}", repo.owner));
        let found: Vec<PullJson> = self.all_pages(url, &|| repository_missing(repo))?;
        Ok(found
            .into_iter()
            .map(|pull_request| pull_request.into_model(Vec::new()))
            // The filter matched; this guards against a host that ignores it.
            .filter(|pull_request| pull_request.source_branch == branch)
            .collect())
    }

    fn pull_request(&self, repo: &RemoteRepo, number: u64) -> AppResult<PullRequest> {
        let found: PullJson = self.get(&self.pull_url(repo, number, &[]), &|| {
            pull_request_missing(repo, number)
        })?;
        let reviews = self.reviews(repo, number)?;
        Ok(found.into_model(reviews))
    }

    fn comments(&self, repo: &RemoteRepo, number: u64) -> AppResult<Vec<RemoteComment>> {
        let missing = || pull_request_missing(repo, number);
        let on_lines: Vec<ReviewCommentJson> =
            self.all_pages(self.pull_url(repo, number, &["comments"]), &missing)?;
        let on_the_whole: Vec<IssueCommentJson> = self.all_pages(
            self.repo_url(repo, &["issues", &number.to_string(), "comments"]),
            &missing,
        )?;
        let reviews = self.reviews(repo, number)?;
        let resolved: HashSet<u64> = if on_lines.is_empty() {
            HashSet::new()
        } else {
            self.review_threads(repo, number)?
                .into_iter()
                .filter(|thread| thread.is_resolved)
                .filter_map(|thread| thread.first_comment())
                .collect()
        };
        // A pending review is one its author has not submitted: nobody else
        // sees it, nor its comments, yet.
        let pending: HashSet<u64> = reviews
            .iter()
            .filter(|review| review.state == "PENDING")
            .map(|review| review.id)
            .collect();
        let mut comments: Vec<RemoteComment> = on_lines
            .into_iter()
            .filter(|comment| {
                comment
                    .pull_request_review_id
                    .map_or(true, |review| !pending.contains(&review))
            })
            .map(|comment| {
                let resolved = comment.in_reply_to_id.is_none() && resolved.contains(&comment.id);
                comment.into_model(resolved)
            })
            .collect();
        comments.extend(on_the_whole.into_iter().map(IssueCommentJson::into_model));
        comments.extend(
            reviews
                .into_iter()
                .filter(|review| {
                    review.state != "PENDING"
                        && review.body.as_deref().is_some_and(|body| !body.trim().is_empty())
                })
                .map(ReviewJson::into_comment),
        );
        // Oldest first across the kinds; GitHub's times all read alike.
        comments.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(comments)
    }

    fn create_comment(
        &self,
        repo: &RemoteRepo,
        number: u64,
        comment: &NewComment,
    ) -> AppResult<RemoteComment> {
        let missing = || pull_request_missing(repo, number);
        let general = |body: &str| -> AppResult<RemoteComment> {
            let url = self.repo_url(repo, &["issues", &number.to_string(), "comments"]);
            let created: IssueCommentJson = self.post(&url, json!({ "body": body }), &missing)?;
            Ok(created.into_model())
        };
        match comment {
            NewComment::Inline { anchor, body } => {
                let commit = self.head_commit(repo, number)?;
                let created: ReviewCommentJson = self.post(
                    &self.pull_url(repo, number, &["comments"]),
                    json!({
                        "body": body,
                        "commit_id": commit,
                        "path": anchor.path,
                        "line": anchor.line,
                        "side": side_name(anchor.side),
                    }),
                    &missing,
                )?;
                Ok(created.into_model(false))
            }
            NewComment::General { body } => general(body),
            NewComment::Reply { parent_id, body } => match CommentId::parse(parent_id)? {
                CommentId::ReviewComment(parent) => {
                    let created: ReviewCommentJson = self.post(
                        &self.pull_url(repo, number, &["comments", &parent.to_string(), "replies"]),
                        json!({ "body": body }),
                        &|| {
                            app_error!(
                                RemoteNotFound,
                                GithubReplyTargetMissing,
                                "GitHub has no pull request #{number} in {repository}, or the comment answered is gone.",
                                number = number,
                                repository = repository_name(repo)
                            )
                        },
                    )?;
                    Ok(created.into_model(false))
                }
                // No threads on the pull request as a whole: an answer there
                // is a comment of its own.
                CommentId::IssueComment(_) | CommentId::Review(_) => general(body),
            },
        }
    }

    fn update_comment(
        &self,
        repo: &RemoteRepo,
        number: u64,
        id: &str,
        body: &str,
    ) -> AppResult<RemoteComment> {
        let text = json!({ "body": body });
        Ok(match CommentId::parse(id)? {
            CommentId::ReviewComment(id) => {
                let url = self.repo_url(repo, &["pulls", "comments", &id.to_string()]);
                let updated: ReviewCommentJson =
                    parse(&self.send(Method::Patch, &url, Some(text), &comment_missing)?.body)?;
                updated.into_model(false)
            }
            CommentId::IssueComment(id) => {
                let url = self.repo_url(repo, &["issues", "comments", &id.to_string()]);
                let updated: IssueCommentJson =
                    parse(&self.send(Method::Patch, &url, Some(text), &comment_missing)?.body)?;
                updated.into_model()
            }
            CommentId::Review(id) => {
                let url = self.pull_url(repo, number, &["reviews", &id.to_string()]);
                let updated: ReviewJson =
                    parse(&self.send(Method::Put, &url, Some(text), &comment_missing)?.body)?;
                updated.into_comment()
            }
        })
    }

    fn delete_comment(&self, repo: &RemoteRepo, _number: u64, id: &str) -> AppResult<()> {
        let url = match CommentId::parse(id)? {
            CommentId::ReviewComment(id) => {
                self.repo_url(repo, &["pulls", "comments", &id.to_string()])
            }
            CommentId::IssueComment(id) => {
                self.repo_url(repo, &["issues", "comments", &id.to_string()])
            }
            CommentId::Review(_) => {
                return Err(app_error!(
                    RemoteFailed,
                    GithubReviewNotDeletable,
                    "GitHub does not delete a submitted review: edit its text instead."
                ))
            }
        };
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
        let CommentId::ReviewComment(first) = CommentId::parse(id)? else {
            return Err(app_error!(
                RemoteFailed,
                GithubThreadNotResolvable,
                "GitHub resolves only threads on the lines of a file; this one is on the pull request as a whole."
            ));
        };
        let thread = self
            .review_threads(repo, number)?
            .into_iter()
            .find(|thread| thread.first_comment() == Some(first))
            .ok_or_else(comment_missing)?;
        let mutation = if resolved {
            RESOLVE_MUTATION
        } else {
            UNRESOLVE_MUTATION
        };
        let _: Value = self.graphql(mutation, json!({ "thread": thread.id }), &comment_missing)?;
        Ok(())
    }

    fn set_verdict(
        &self,
        repo: &RemoteRepo,
        number: u64,
        verdict: Verdict,
        given: bool,
    ) -> AppResult<()> {
        let nothing_to_take_back = || {
            app_error!(
                RemoteNotFound,
                GithubVerdictTargetMissing,
                "GitHub has no pull request #{number} in {repository}, or nothing of yours to take back.",
                number = number,
                repository = repository_name(repo)
            )
        };
        if given {
            let body = match verdict {
                Verdict::Approved => json!({ "event": "APPROVE" }),
                Verdict::ChangesRequested => {
                    json!({ "event": "REQUEST_CHANGES", "body": CHANGES_REQUESTED_BODY })
                }
            };
            return self
                .send(
                    Method::Post,
                    &self.pull_url(repo, number, &["reviews"]),
                    Some(body),
                    &|| pull_request_missing(repo, number),
                )
                .map(|_| ());
        }
        // The verdict is the account's latest review that gave or dismissed
        // one; taking it back dismisses that review.
        let account = self.account()?;
        let wanted = match verdict {
            Verdict::Approved => "APPROVED",
            Verdict::ChangesRequested => "CHANGES_REQUESTED",
        };
        let reviews = self.reviews(repo, number)?;
        let latest = reviews
            .iter()
            .rev()
            .filter(|review| review.author_id().as_deref() == Some(account.id.as_str()))
            .find(|review| {
                matches!(
                    review.state.as_str(),
                    "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED"
                )
            });
        let Some(review) = latest.filter(|review| review.state == wanted) else {
            return Err(nothing_to_take_back());
        };
        let url = self.pull_url(
            repo,
            number,
            &["reviews", &review.id.to_string(), "dismissals"],
        );
        let body = json!({ "message": TAKEN_BACK_MESSAGE, "event": "DISMISS" });
        match self.send(Method::Put, &url, Some(body), &nothing_to_take_back) {
            Ok(_) => Ok(()),
            Err(error) if error.code == ErrorCode::MissingScope => Err(app_error!(
                MissingScope,
                GithubTakeBackRefused,
                "GitHub did not let this account take back its review. Taking it back dismisses it, which needs write access to {repository} (on a protected branch, the right to dismiss reviews) and a token with write access to Pull requests.",
                repository = repository_name(repo)
            )),
            Err(error) => Err(error),
        }
    }

    fn create_review(
        &self,
        repo: &RemoteRepo,
        number: u64,
        commit: Option<&str>,
        comments: &[LineComment],
    ) -> Option<AppResult<Vec<RemoteComment>>> {
        Some(self.review(repo, number, commit, comments))
    }

    fn verdict_replaces_the_other(&self) -> bool {
        true
    }
}

const THREADS_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!, $after: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100, after: $after) {
        nodes { id isResolved comments(first: 1) { nodes { databaseId } } }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}";

const RESOLVE_MUTATION: &str =
    "mutation($thread: ID!) { resolveReviewThread(input: { threadId: $thread }) { thread { id } } }";

const UNRESOLVE_MUTATION: &str =
    "mutation($thread: ID!) { unresolveReviewThread(input: { threadId: $thread }) { thread { id } } }";

/// Which kind of comment an id names, and its number among that kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommentId {
    /// On a line or a file.
    ReviewComment(u64),
    /// On the pull request as a whole.
    IssueComment(u64),
    /// The text of a submitted review, on the pull request as a whole.
    Review(u64),
}

const REVIEW_COMMENT: &str = "review-comment:";
const ISSUE_COMMENT: &str = "issue-comment:";
const REVIEW: &str = "review:";

impl CommentId {
    fn parse(id: &str) -> AppResult<Self> {
        let number = |rest: &str| rest.parse::<u64>().ok();
        let parsed = if let Some(rest) = id.strip_prefix(REVIEW_COMMENT) {
            number(rest).map(Self::ReviewComment)
        } else if let Some(rest) = id.strip_prefix(ISSUE_COMMENT) {
            number(rest).map(Self::IssueComment)
        } else if let Some(rest) = id.strip_prefix(REVIEW) {
            number(rest).map(Self::Review)
        } else {
            None
        };
        parsed.ok_or_else(|| {
            app_error!(
                Internal,
                NotAGithubCommentId,
                "\"{id}\" is not a GitHub comment id.",
                id = escaped(id)
            )
        })
    }

    fn key(self) -> String {
        match self {
            Self::ReviewComment(id) => format!("{REVIEW_COMMENT}{id}"),
            Self::IssueComment(id) => format!("{ISSUE_COMMENT}{id}"),
            Self::Review(id) => format!("{REVIEW}{id}"),
        }
    }
}

/// The URL of the `rel="next"` entry of a `Link` header.
fn next_link(header: &str) -> Option<&str> {
    header.split(',').find_map(|entry| {
        let (target, params) = entry.trim().split_once(';')?;
        let next = params
            .split(';')
            .any(|param| matches!(param.trim(), "rel=\"next\"" | "rel=next"));
        let target = target.trim().strip_prefix('<')?.strip_suffix('>')?;
        next.then_some(target)
    })
}

/// Matches the comments GitHub made of a review to the ones sent, by place
/// and text: one created comment per comment sent, in their order.
fn matched(sent: &[LineComment], created: Vec<ReviewCommentJson>) -> AppResult<Vec<RemoteComment>> {
    let mut left: Vec<Option<ReviewCommentJson>> = created.into_iter().map(Some).collect();
    sent.iter()
        .map(|comment| {
            let same_place = |created: &ReviewCommentJson| {
                created.path == comment.anchor.path
                    && created.line.or(created.original_line) == Some(comment.anchor.line)
                    && created.side() == comment.anchor.side
            };
            // The same text first: two comments may share a line.
            let index = left
                .iter()
                .position(|created| {
                    created
                        .as_ref()
                        .is_some_and(|created| same_place(created) && created.body == comment.body)
                })
                .or_else(|| {
                    left.iter()
                        .position(|created| created.as_ref().is_some_and(same_place))
                });
            index
                .and_then(|index| left[index].take())
                .map(|created| created.into_model(false))
                .ok_or_else(|| {
                    app_error!(
                        RemoteFailed,
                        GithubReviewIncomplete,
                        "GitHub did not keep every comment of the review."
                    )
                })
        })
        .collect()
}

fn side_name(side: ReviewSide) -> &'static str {
    match side {
        ReviewSide::New => "RIGHT",
        ReviewSide::Old => "LEFT",
    }
}

/// A readable error for an answer that is not a success.
fn failure(response: &HttpResponse, missing: &dyn Fn() -> AppError) -> AppError {
    let detail = error_message(&response.body);
    match response.status {
        401 => app_error!(
            AuthFailed,
            GithubTokenRefused,
            "GitHub did not accept the token. It may have expired or been revoked: create a new one and save it in Preferences."
        ),
        429 => http::host_rate_limited(HOST, retry_after(response).as_deref()),
        403 if rate_limited(response, detail.as_deref()) => {
            http::host_rate_limited(HOST, retry_after(response).as_deref())
        }
        403 => missing_permission(detail),
        404 => missing(),
        422 if detail
            .as_deref()
            .is_some_and(|detail| detail.to_lowercase().contains("pending review")) =>
        {
            app_error!(
                PublishBlocked,
                GithubPendingReviewExists,
                "You have a review in progress on GitHub on this pull request: submit or discard it there, then publish again."
            )
        }
        status if status >= 500 => http::host_server_error(HOST, status),
        status => http::host_refused(HOST, status, detail),
    }
}

/// GitHub answers 403 both to a token without a permission and to one that
/// used up its requests: the latter says so in its headers or its message.
fn rate_limited(response: &HttpResponse, detail: Option<&str>) -> bool {
    response.retry_after.is_some()
        || response.header("x-ratelimit-remaining").map(str::trim) == Some("0")
        || detail.is_some_and(|detail| detail.to_lowercase().contains("rate limit"))
}

/// Seconds to wait: `Retry-After`, or until `X-RateLimit-Reset` (a Unix
/// time) when the requests are used up.
fn retry_after(response: &HttpResponse) -> Option<String> {
    if let Some(after) = &response.retry_after {
        return Some(after.clone());
    }
    if response.header("x-ratelimit-remaining").map(str::trim) != Some("0") {
        return None;
    }
    let reset: u64 = response.header("x-ratelimit-reset")?.trim().parse().ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(reset.saturating_sub(now).max(1).to_string())
}

fn missing_permission(detail: Option<String>) -> AppError {
    let needs = core_text!(
        GithubPermissionsNeeded,
        "a classic token needs the {scope} scope; a fine-grained one needs read and write access to Pull requests and read access to Contents and Metadata",
        scope = SCOPES.join(", ")
    );
    match detail {
        Some(detail) => app_error!(
            MissingScope,
            GithubMissingPermissionSaid,
            "GitHub refused this ({detail}). The token may be missing a permission: {needs}.",
            detail = detail,
            needs = needs
        ),
        None => app_error!(
            MissingScope,
            GithubMissingPermission,
            "GitHub refused this. The token may be missing a permission: {needs}.",
            needs = needs
        ),
    }
}

/// GitHub's own explanation, `{"message": ..., "errors": [...]}`, kept short.
fn error_message(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty());
    let details: Vec<&str> = value
        .get("errors")
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .filter_map(|error| {
                    error
                        .as_str()
                        .or_else(|| error.get("message").and_then(Value::as_str))
                })
                .map(str::trim)
                .filter(|detail| !detail.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let text = match (message, details.is_empty()) {
        (Some(message), true) => message.to_string(),
        (Some(message), false) => format!("{message}: {}", details.join("; ")),
        (None, false) => details.join("; "),
        (None, true) => return None,
    };
    Some(text.chars().take(300).collect())
}

fn graphql_failure(error: GraphqlErrorJson, missing: &dyn Fn() -> AppError) -> AppError {
    let message = Some(error.message.trim().chars().take(300).collect::<String>())
        .filter(|message| !message.is_empty());
    match error.kind.as_deref() {
        Some("NOT_FOUND") => missing(),
        Some("FORBIDDEN" | "INSUFFICIENT_SCOPES") => missing_permission(message),
        Some("RATE_LIMITED") => http::host_rate_limited(HOST, None),
        _ => http::host_refused(HOST, 200, message),
    }
}

fn too_many_pages() -> AppError {
    app_error!(
        RemoteFailed,
        HostTooManyPages,
        "{host} kept sending more pages than DiffLens reads.",
        host = HOST
    )
}

fn repository_name(repo: &RemoteRepo) -> String {
    format!("{}/{}", repo.owner, repo.name)
}

fn repository_missing(repo: &RemoteRepo) -> AppError {
    app_error!(
        RemoteNotFound,
        GithubRepositoryMissing,
        "GitHub has no repository {repository}, or the token cannot see it.",
        repository = repository_name(repo)
    )
}

fn pull_request_missing(repo: &RemoteRepo, number: u64) -> AppError {
    app_error!(
        RemoteNotFound,
        GithubPullRequestMissing,
        "GitHub has no pull request #{number} in {repository}.",
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

#[derive(Deserialize, Default)]
struct UserJson {
    id: Option<u64>,
    login: Option<String>,
}

impl UserJson {
    /// Named by the login, as GitHub shows people.
    fn account(self) -> Account {
        Account {
            id: self.id.map(|id| id.to_string()).unwrap_or_default(),
            // What GitHub shows for a deleted account.
            name: self.login.unwrap_or_else(|| "ghost".to_string()),
        }
    }
}

#[derive(Deserialize)]
struct PullJson {
    number: u64,
    #[serde(default)]
    title: String,
    body: Option<String>,
    #[serde(default)]
    state: String,
    merged_at: Option<String>,
    #[serde(default)]
    html_url: String,
    user: Option<UserJson>,
    head: RefJson,
    base: RefJson,
    /// Asked to review and not reviewed since; teams are left out.
    #[serde(default)]
    requested_reviewers: Vec<UserJson>,
}

#[derive(Deserialize)]
struct RefJson {
    #[serde(rename = "ref", default)]
    name: String,
    sha: Option<String>,
}

impl PullJson {
    fn into_model(self, reviews: Vec<ReviewJson>) -> PullRequest {
        let author = self.user.map(UserJson::account);
        let participants = participants(
            author.as_ref().map(|author| author.id.as_str()),
            reviews,
            self.requested_reviewers,
        );
        PullRequest {
            number: self.number,
            title: self.title,
            description: self.body.unwrap_or_default(),
            state: match self.state.as_str() {
                "open" => PullRequestState::Open,
                "closed" if self.merged_at.is_some() => PullRequestState::Merged,
                "closed" => PullRequestState::Declined,
                // Anything else is not open, so nothing is published to it.
                _ => PullRequestState::Superseded,
            },
            url: self.html_url,
            author: author.map(|author| author.name),
            source_branch: self.head.name,
            source_commit: self.head.sha,
            destination_branch: self.base.name,
            destination_commit: self.base.sha,
            participants,
        }
    }
}

/// Everyone who reviewed (the author aside) or is asked to, with their
/// verdict: their latest review that approved, requested changes or was
/// dismissed. A review that only comments leaves the verdict as it was.
fn participants(
    author: Option<&str>,
    reviews: Vec<ReviewJson>,
    requested: Vec<UserJson>,
) -> Vec<Participant> {
    let mut participants: Vec<Participant> = Vec::new();
    let place = |participants: &mut Vec<Participant>, account: Account| {
        match participants
            .iter()
            .position(|participant| participant.account.id == account.id)
        {
            Some(index) => index,
            None => {
                participants.push(Participant {
                    account,
                    reviewer: true,
                    verdict: None,
                });
                participants.len() - 1
            }
        }
    };
    for review in reviews {
        if review.state == "PENDING" {
            continue;
        }
        let Some(user) = review.user else {
            continue;
        };
        let account = user.account();
        if Some(account.id.as_str()) == author {
            continue;
        }
        let index = place(&mut participants, account);
        match review.state.as_str() {
            "APPROVED" => participants[index].verdict = Some(Verdict::Approved),
            "CHANGES_REQUESTED" => participants[index].verdict = Some(Verdict::ChangesRequested),
            "DISMISSED" => participants[index].verdict = None,
            _ => {}
        }
    }
    for user in requested {
        place(&mut participants, user.account());
    }
    participants
}

#[derive(Deserialize)]
struct ReviewJson {
    id: u64,
    user: Option<UserJson>,
    body: Option<String>,
    /// APPROVED, CHANGES_REQUESTED, COMMENTED, DISMISSED or PENDING.
    #[serde(default)]
    state: String,
    submitted_at: Option<String>,
}

impl ReviewJson {
    fn author_id(&self) -> Option<String> {
        self.user.as_ref()?.id.map(|id| id.to_string())
    }

    /// Its text, as a comment on the pull request as a whole.
    fn into_comment(self) -> RemoteComment {
        let at = self.submitted_at.unwrap_or_default();
        RemoteComment {
            id: CommentId::Review(self.id).key(),
            parent_id: None,
            author: self.user.unwrap_or_default().account(),
            body: self.body.unwrap_or_default(),
            anchor: None,
            general: true,
            resolved: false,
            deleted: false,
            created_at: at.clone(),
            updated_at: at,
        }
    }
}

#[derive(Deserialize)]
struct ReviewCommentJson {
    id: u64,
    pull_request_review_id: Option<u64>,
    /// The thread's first comment: GitHub's threads are one level deep.
    in_reply_to_id: Option<u64>,
    user: Option<UserJson>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    path: String,
    /// `None` once the line is no longer in the pull request's diff.
    line: Option<u32>,
    original_line: Option<u32>,
    side: Option<String>,
    /// "line", or "file" for a comment on the whole file.
    subject_type: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

impl ReviewCommentJson {
    fn side(&self) -> ReviewSide {
        match self.side.as_deref() {
            Some("LEFT") => ReviewSide::Old,
            _ => ReviewSide::New,
        }
    }

    fn into_model(self, resolved: bool) -> RemoteComment {
        let side = self.side();
        // A comment GitHub calls outdated has lost its `line`: it is read at
        // the line it was written on, as Bitbucket keeps those of its own.
        let anchor = match self.subject_type.as_deref() {
            Some("file") => None,
            _ => self.line.or(self.original_line).map(|line| Anchor {
                path: self.path,
                side,
                line,
            }),
        };
        let created_at = self.created_at.unwrap_or_default();
        RemoteComment {
            id: CommentId::ReviewComment(self.id).key(),
            parent_id: self
                .in_reply_to_id
                .map(|parent| CommentId::ReviewComment(parent).key()),
            author: self.user.unwrap_or_default().account(),
            body: self.body,
            anchor,
            general: false,
            resolved,
            deleted: false,
            updated_at: self.updated_at.unwrap_or_else(|| created_at.clone()),
            created_at,
        }
    }
}

#[derive(Deserialize)]
struct IssueCommentJson {
    id: u64,
    user: Option<UserJson>,
    body: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

impl IssueCommentJson {
    fn into_model(self) -> RemoteComment {
        let created_at = self.created_at.unwrap_or_default();
        RemoteComment {
            id: CommentId::IssueComment(self.id).key(),
            parent_id: None,
            author: self.user.unwrap_or_default().account(),
            body: self.body.unwrap_or_default(),
            anchor: None,
            general: true,
            resolved: false,
            deleted: false,
            updated_at: self.updated_at.unwrap_or_else(|| created_at.clone()),
            created_at,
        }
    }
}

#[derive(Deserialize)]
struct GraphqlJson<T> {
    data: Option<T>,
    #[serde(default = "Vec::new")]
    errors: Vec<GraphqlErrorJson>,
}

#[derive(Deserialize)]
struct GraphqlErrorJson {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct ThreadsData {
    repository: Option<RepositoryThreadsJson>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryThreadsJson {
    pull_request: Option<PullThreadsJson>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullThreadsJson {
    review_threads: ThreadPageJson,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadPageJson {
    #[serde(default = "Vec::new")]
    nodes: Vec<Option<ThreadJson>>,
    page_info: PageInfoJson,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfoJson {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadJson {
    /// The node id the mutations take.
    id: String,
    is_resolved: bool,
    comments: FirstCommentsJson,
}

#[derive(Deserialize)]
struct FirstCommentsJson {
    #[serde(default = "Vec::new")]
    nodes: Vec<Option<DatabaseIdJson>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DatabaseIdJson {
    database_id: Option<u64>,
}

impl ThreadJson {
    /// The REST id of the thread's first comment.
    fn first_comment(&self) -> Option<u64> {
        self.comments.nodes.first()?.as_ref()?.database_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Reason;
    use crate::http::tests::CannedTransport;

    const HEAD: &str = "e7f3b10c9d8e4f5a6b7c8d9e0f1a2b3c4d5e6f7a";

    type Answer = Result<HttpResponse, String>;

    fn answer(status: u16, body: Value) -> Answer {
        Ok(HttpResponse {
            status,
            body: body.to_string(),
            ..HttpResponse::default()
        })
    }

    /// A page of a listing, with a `Link` header to `next` when there is one.
    fn page(body: Value, next: Option<&str>) -> Answer {
        Ok(HttpResponse {
            status: 200,
            body: body.to_string(),
            retry_after: None,
            headers: next
                .map(|next| {
                    vec![(
                        "link",
                        format!(
                            "<{next}>; rel=\"next\", <https://api.github.com/repositories/1/pulls?page=9>; rel=\"last\""
                        ),
                    )]
                })
                .unwrap_or_default(),
        })
    }

    fn canned(answers: Vec<Answer>) -> Arc<CannedTransport> {
        let transport = CannedTransport::default();
        for answer in answers {
            transport.push_raw(answer);
        }
        Arc::new(transport)
    }

    fn client(transport: &Arc<CannedTransport>) -> Github {
        Github::new(API, "ghp-token", Arc::clone(transport) as Arc<dyn Transport>)
    }

    fn repo() -> RemoteRepo {
        RemoteRepo {
            host: "github.com".to_string(),
            owner: "acme".to_string(),
            name: "payments-api".to_string(),
        }
    }

    /// Method and path of every request sent.
    fn calls(transport: &CannedTransport) -> Vec<(Method, String)> {
        transport
            .sent()
            .into_iter()
            .map(|request| {
                (
                    request.method,
                    Url::parse(&request.url).unwrap().path().to_string(),
                )
            })
            .collect()
    }

    fn user(id: u64, login: &str) -> Value {
        json!({ "id": id, "login": login })
    }

    fn pull_json(number: u64, branch: &str) -> Value {
        json!({
            "number": number,
            "title": "Adds the refund flow",
            "body": "Refunds settled payments.",
            "state": "open",
            "merged_at": null,
            "html_url": format!("https://github.com/acme/payments-api/pull/{number}"),
            "user": user(1, "ana"),
            "head": { "ref": branch, "sha": HEAD },
            "base": { "ref": "main", "sha": "0123456789ab0123456789ab0123456789ab0123" },
            "requested_reviewers": [],
        })
    }

    fn review_comment(id: u64, line: Option<u32>, side: &str) -> Value {
        json!({
            "id": id,
            "pull_request_review_id": 900,
            "user": user(2, "bruno"),
            "body": format!("comment {id}"),
            "path": "internal/refund/service.go",
            "line": line,
            "original_line": line,
            "side": side,
            "subject_type": "line",
            "created_at": "2026-11-18T10:00:00Z",
            "updated_at": "2026-11-18T10:00:00Z",
        })
    }

    fn threads(nodes: Value, next: Option<&str>) -> Answer {
        answer(
            200,
            json!({ "data": { "repository": { "pullRequest": { "reviewThreads": {
                "nodes": nodes,
                "pageInfo": { "hasNextPage": next.is_some(), "endCursor": next },
            } } } } }),
        )
    }

    fn thread(id: &str, resolved: bool, first: u64) -> Value {
        json!({ "id": id, "isResolved": resolved, "comments": { "nodes": [{ "databaseId": first }] } })
    }

    #[test]
    fn finds_the_open_pull_request_of_a_branch_across_pages() {
        let next = "https://api.github.com/repositories/1/pulls?state=open&per_page=100&page=2";
        let transport = canned(vec![
            page(json!([pull_json(7, "feat/refund-flow-v0")]), Some(next)),
            page(json!([pull_json(318, "feat/refund-flow")]), None),
        ]);
        let found = client(&transport)
            .open_pull_requests(&repo(), "feat/refund-flow")
            .unwrap();

        assert_eq!(found.len(), 1);
        let pull_request = &found[0];
        assert_eq!(pull_request.number, 318);
        assert_eq!(pull_request.state, PullRequestState::Open);
        assert_eq!(pull_request.description, "Refunds settled payments.");
        assert_eq!(pull_request.url, "https://github.com/acme/payments-api/pull/318");
        assert_eq!(pull_request.author.as_deref(), Some("ana"));
        assert_eq!(pull_request.source_commit.as_deref(), Some(HEAD));
        assert_eq!(pull_request.destination_branch, "main");

        let sent = transport.sent();
        assert_eq!(sent.len(), 2);
        let first = Url::parse(&sent[0].url).unwrap();
        assert_eq!(first.path(), "/repos/acme/payments-api/pulls");
        let query: Vec<(String, String)> = first.query_pairs().into_owned().collect();
        assert!(query.contains(&("state".to_string(), "open".to_string())));
        assert!(query.contains(&("head".to_string(), "acme:feat/refund-flow".to_string())));
        assert!(query.contains(&("per_page".to_string(), "100".to_string())));
        assert_eq!(sent[1].url, next);
        assert_eq!(sent[0].method, Method::Get);
        assert_eq!(
            sent[0].auth,
            Auth::Header {
                name: "Authorization",
                value: Secret::new("Bearer ghp-token")
            }
        );
        assert!(sent[0]
            .headers
            .contains(&("Accept", "application/vnd.github+json".to_string())));
        assert!(sent[0]
            .headers
            .contains(&("X-GitHub-Api-Version", API_VERSION.to_string())));
    }

    #[test]
    fn a_next_link_off_the_api_is_not_followed() {
        let transport = canned(vec![page(json!([]), Some("https://evil.example.com/steal"))]);
        let error = client(&transport)
            .open_pull_requests(&repo(), "feat/x")
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::RemoteFailed);
        assert_eq!(error.reason, Some(Reason::HostPageOutsideApi));
        assert_eq!(transport.sent().len(), 1);
    }

    #[test]
    fn reads_reviewers_and_their_latest_verdicts() {
        let mut pull_request = pull_json(318, "feat/refund-flow");
        pull_request["requested_reviewers"] = json!([user(5, "edu"), user(3, "carla")]);
        let review = |id: u64, by: Value, state: &str| json!({ "id": id, "user": by, "state": state, "body": "" });
        let transport = canned(vec![
            answer(200, pull_request),
            page(
                json!([
                    review(1, user(2, "bruno"), "APPROVED"),
                    review(2, user(3, "carla"), "CHANGES_REQUESTED"),
                    // Commenting afterwards keeps the request for changes.
                    review(3, user(3, "carla"), "COMMENTED"),
                    review(4, user(4, "dana"), "APPROVED"),
                    review(5, user(4, "dana"), "DISMISSED"),
                    // The author answering is no reviewer.
                    review(6, user(1, "ana"), "COMMENTED"),
                    review(7, user(9, "me"), "PENDING"),
                ]),
                None,
            ),
        ]);
        let found = client(&transport).pull_request(&repo(), 318).unwrap();

        let participants: Vec<(&str, &str, bool, Option<Verdict>)> = found
            .participants
            .iter()
            .map(|participant| {
                (
                    participant.account.id.as_str(),
                    participant.account.name.as_str(),
                    participant.reviewer,
                    participant.verdict,
                )
            })
            .collect();
        assert_eq!(
            participants,
            vec![
                ("2", "bruno", true, Some(Verdict::Approved)),
                ("3", "carla", true, Some(Verdict::ChangesRequested)),
                ("4", "dana", true, None),
                ("5", "edu", true, None),
            ]
        );
        assert_eq!(
            calls(&transport),
            vec![
                (Method::Get, "/repos/acme/payments-api/pulls/318".to_string()),
                (Method::Get, "/repos/acme/payments-api/pulls/318/reviews".to_string()),
            ]
        );
    }

    #[test]
    fn a_closed_pull_request_is_merged_or_declined() {
        let state = |state: &str, merged_at: Value| {
            let mut found = pull_json(318, "feat/x");
            found["state"] = json!(state);
            found["merged_at"] = merged_at;
            serde_json::from_value::<PullJson>(found)
                .unwrap()
                .into_model(Vec::new())
                .state
        };
        assert_eq!(state("open", Value::Null), PullRequestState::Open);
        assert_eq!(
            state("closed", json!("2026-11-20T10:00:00Z")),
            PullRequestState::Merged
        );
        assert_eq!(state("closed", Value::Null), PullRequestState::Declined);
    }

    #[test]
    fn maps_comments_of_every_kind_with_replies_resolution_and_outdated_lines() {
        let mut reply = review_comment(2, Some(44), "RIGHT");
        reply["in_reply_to_id"] = json!(1);
        reply["user"] = user(3, "carla");
        reply["created_at"] = json!("2026-11-18T10:02:00Z");
        let mut old_side = review_comment(3, Some(12), "LEFT");
        old_side["created_at"] = json!("2026-11-18T10:01:00Z");
        // Its line left the diff: read where it was written.
        let mut outdated = review_comment(4, None, "RIGHT");
        outdated["original_line"] = json!(30);
        outdated["created_at"] = json!("2026-11-18T09:00:00Z");
        let mut on_the_file = review_comment(5, None, "RIGHT");
        on_the_file["subject_type"] = json!("file");
        on_the_file["created_at"] = json!("2026-11-18T10:03:00Z");
        let mut pending = review_comment(6, Some(50), "RIGHT");
        pending["pull_request_review_id"] = json!(901);
        let review = |id: u64, state: &str, body: &str, at: &str| {
            json!({ "id": id, "user": user(3, "carla"), "state": state, "body": body, "submitted_at": at })
        };
        let transport = canned(vec![
            page(
                json!([
                    review_comment(1, Some(44), "RIGHT"),
                    reply,
                    old_side,
                    outdated,
                    on_the_file,
                    pending,
                ]),
                None,
            ),
            page(
                json!([{
                    "id": 60, "user": user(3, "carla"), "body": "On the whole change",
                    "created_at": "2026-11-18T09:30:00Z", "updated_at": "2026-11-18T09:40:00Z",
                }]),
                None,
            ),
            page(
                json!([
                    review(900, "COMMENTED", "", "2026-11-18T10:00:00Z"),
                    review(902, "CHANGES_REQUESTED", "Please split this", "2026-11-18T09:45:00Z"),
                    review(901, "PENDING", "Not sent yet", ""),
                ]),
                None,
            ),
            threads(
                json!([thread("PRRT_1", true, 1), thread("PRRT_3", false, 3)]),
                None,
            ),
        ]);
        let comments = client(&transport).comments(&repo(), 318).unwrap();

        let ids: Vec<&str> = comments.iter().map(|comment| comment.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "review-comment:4",
                "issue-comment:60",
                "review:902",
                "review-comment:1",
                "review-comment:3",
                "review-comment:2",
                "review-comment:5",
            ],
            "oldest first; the pending review and its comment are left out"
        );
        let by_id = |id: &str| comments.iter().find(|comment| comment.id == id).unwrap();
        let root = by_id("review-comment:1");
        assert_eq!(
            root.anchor,
            Some(Anchor {
                path: "internal/refund/service.go".to_string(),
                side: ReviewSide::New,
                line: 44
            })
        );
        assert!(root.resolved && !root.general);
        assert_eq!(root.author, Account { id: "2".to_string(), name: "bruno".to_string() });
        let answer = by_id("review-comment:2");
        assert_eq!(answer.parent_id.as_deref(), Some("review-comment:1"));
        assert!(!answer.resolved);
        let old = by_id("review-comment:3");
        assert_eq!(old.anchor.as_ref().map(|anchor| (anchor.side, anchor.line)), Some((ReviewSide::Old, 12)));
        assert!(!old.resolved);
        assert_eq!(
            by_id("review-comment:4").anchor.as_ref().map(|anchor| anchor.line),
            Some(30)
        );
        let file = by_id("review-comment:5");
        assert_eq!((file.anchor.as_ref(), file.general), (None, false));
        let general = by_id("issue-comment:60");
        assert!(general.general && general.anchor.is_none() && general.parent_id.is_none());
        assert_eq!(general.updated_at, "2026-11-18T09:40:00Z");
        let verdict = by_id("review:902");
        assert!(verdict.general);
        assert_eq!(verdict.body, "Please split this");

        assert_eq!(
            calls(&transport),
            vec![
                (Method::Get, "/repos/acme/payments-api/pulls/318/comments".to_string()),
                (Method::Get, "/repos/acme/payments-api/issues/318/comments".to_string()),
                (Method::Get, "/repos/acme/payments-api/pulls/318/reviews".to_string()),
                (Method::Post, "/graphql".to_string()),
            ]
        );
        let query = transport.sent()[3].body.clone().unwrap();
        assert_eq!(
            query["variables"],
            json!({ "owner": "acme", "name": "payments-api", "number": 318, "after": null })
        );
        assert!(query["query"].as_str().unwrap().contains("reviewThreads"));
    }

    #[test]
    fn without_comments_on_lines_no_thread_is_asked_for() {
        let transport = canned(vec![
            page(json!([]), None),
            page(json!([]), None),
            page(json!([]), None),
        ]);
        assert!(client(&transport).comments(&repo(), 318).unwrap().is_empty());
        assert_eq!(transport.sent().len(), 3);
    }

    #[test]
    fn creates_comments_on_lines_on_the_whole_pull_request_and_replies() {
        let created_on_line = |id: u64| answer(201, review_comment(id, Some(8), "RIGHT"));
        let mut reply = review_comment(12, Some(8), "RIGHT");
        reply["in_reply_to_id"] = json!(10);
        let general = |id: u64| {
            answer(
                201,
                json!({ "id": id, "user": user(9, "me"), "body": "x", "created_at": "2026-11-21T10:00:00Z" }),
            )
        };
        let transport = canned(vec![
            answer(200, pull_json(318, "feat/refund-flow")),
            created_on_line(10),
            answer(200, pull_json(318, "feat/refund-flow")),
            created_on_line(11),
            general(60),
            answer(201, reply),
            general(61),
        ]);
        let github = client(&transport);
        let anchor = |side, line| Anchor {
            path: "a.go".to_string(),
            side,
            line,
        };
        let create = |comment: NewComment| github.create_comment(&repo(), 318, &comment).unwrap();

        let new_side = create(NewComment::Inline {
            anchor: anchor(ReviewSide::New, 8),
            body: "On the new line".to_string(),
        });
        create(NewComment::Inline {
            anchor: anchor(ReviewSide::Old, 3),
            body: "On the old line".to_string(),
        });
        let on_the_whole = create(NewComment::General {
            body: "On the whole change".to_string(),
        });
        let answer_on_line = create(NewComment::Reply {
            parent_id: "review-comment:10".to_string(),
            body: "A reply".to_string(),
        });
        let answer_on_the_whole = create(NewComment::Reply {
            parent_id: "issue-comment:60".to_string(),
            body: "Agreed".to_string(),
        });

        assert_eq!(new_side.id, "review-comment:10");
        assert!(on_the_whole.general);
        assert_eq!(on_the_whole.id, "issue-comment:60");
        assert_eq!(answer_on_line.parent_id.as_deref(), Some("review-comment:10"));
        assert_eq!(answer_on_the_whole.id, "issue-comment:61");
        let base = "/repos/acme/payments-api";
        assert_eq!(
            calls(&transport),
            vec![
                (Method::Get, format!("{base}/pulls/318")),
                (Method::Post, format!("{base}/pulls/318/comments")),
                (Method::Get, format!("{base}/pulls/318")),
                (Method::Post, format!("{base}/pulls/318/comments")),
                (Method::Post, format!("{base}/issues/318/comments")),
                (Method::Post, format!("{base}/pulls/318/comments/10/replies")),
                (Method::Post, format!("{base}/issues/318/comments")),
            ]
        );
        let bodies: Vec<Option<Value>> = transport.sent().into_iter().map(|request| request.body).collect();
        assert_eq!(
            bodies[1],
            Some(json!({ "body": "On the new line", "commit_id": HEAD, "path": "a.go", "line": 8, "side": "RIGHT" }))
        );
        assert_eq!(
            bodies[3],
            Some(json!({ "body": "On the old line", "commit_id": HEAD, "path": "a.go", "line": 3, "side": "LEFT" }))
        );
        assert_eq!(bodies[4], Some(json!({ "body": "On the whole change" })));
        assert_eq!(bodies[5], Some(json!({ "body": "A reply" })));
        assert_eq!(bodies[6], Some(json!({ "body": "Agreed" })));
    }

    #[test]
    fn edits_and_deletes_each_kind_of_comment() {
        let transport = canned(vec![
            answer(200, review_comment(10, Some(8), "RIGHT")),
            answer(200, json!({ "id": 60, "user": user(9, "me"), "body": "Edited" })),
            answer(200, json!({ "id": 902, "user": user(9, "me"), "body": "Edited", "state": "CHANGES_REQUESTED" })),
            answer(204, Value::Null),
            answer(404, json!({ "message": "Not Found" })),
        ]);
        let github = client(&transport);
        assert_eq!(
            github
                .update_comment(&repo(), 318, "review-comment:10", "Edited")
                .unwrap()
                .id,
            "review-comment:10"
        );
        assert_eq!(
            github
                .update_comment(&repo(), 318, "issue-comment:60", "Edited")
                .unwrap()
                .body,
            "Edited"
        );
        let review = github.update_comment(&repo(), 318, "review:902", "Edited").unwrap();
        assert!(review.general);
        github.delete_comment(&repo(), 318, "review-comment:10").unwrap();
        // Already gone counts as deleted.
        github.delete_comment(&repo(), 318, "issue-comment:61").unwrap();
        let submitted = github.delete_comment(&repo(), 318, "review:902").unwrap_err();
        assert_eq!(submitted.reason, Some(Reason::GithubReviewNotDeletable));
        let unknown = github.delete_comment(&repo(), 318, "10").unwrap_err();
        assert_eq!(unknown.reason, Some(Reason::NotAGithubCommentId));

        let base = "/repos/acme/payments-api";
        assert_eq!(
            calls(&transport),
            vec![
                (Method::Patch, format!("{base}/pulls/comments/10")),
                (Method::Patch, format!("{base}/issues/comments/60")),
                (Method::Put, format!("{base}/pulls/318/reviews/902")),
                (Method::Delete, format!("{base}/pulls/comments/10")),
                (Method::Delete, format!("{base}/issues/comments/61")),
            ]
        );
        assert!(transport.sent()[..3]
            .iter()
            .all(|request| request.body == Some(json!({ "body": "Edited" }))));
    }

    #[test]
    fn resolves_and_reopens_threads_through_graphql() {
        let transport = canned(vec![
            threads(json!([thread("PRRT_3", false, 3)]), Some("cursor-1")),
            threads(json!([thread("PRRT_1", false, 1)]), None),
            answer(200, json!({ "data": { "resolveReviewThread": { "thread": { "id": "PRRT_1" } } } })),
            threads(json!([thread("PRRT_1", true, 1)]), None),
            answer(200, json!({ "data": { "unresolveReviewThread": { "thread": { "id": "PRRT_1" } } } })),
            threads(json!([]), None),
            answer(200, json!({ "data": null, "errors": [{ "type": "NOT_FOUND", "message": "Could not resolve to a PullRequest with the number of 318." }] })),
        ]);
        let github = client(&transport);
        github.set_resolved(&repo(), 318, "review-comment:1", true).unwrap();
        github.set_resolved(&repo(), 318, "review-comment:1", false).unwrap();
        let gone = github.set_resolved(&repo(), 318, "review-comment:7", true).unwrap_err();
        assert_eq!(gone.code, ErrorCode::RemoteNotFound);
        let missing = github.set_resolved(&repo(), 318, "review-comment:1", true).unwrap_err();
        assert_eq!(missing.reason, Some(Reason::GithubPullRequestMissing));
        // Comments on the pull request as a whole have no thread to resolve.
        let general = github.set_resolved(&repo(), 318, "issue-comment:60", true).unwrap_err();
        assert_eq!(general.reason, Some(Reason::GithubThreadNotResolvable));

        let bodies: Vec<Value> = transport
            .sent()
            .into_iter()
            .map(|request| {
                assert_eq!(request.method, Method::Post);
                assert_eq!(Url::parse(&request.url).unwrap().path(), "/graphql");
                request.body.unwrap()
            })
            .collect();
        assert_eq!(bodies.len(), 7);
        assert_eq!(bodies[0]["variables"]["after"], Value::Null);
        assert_eq!(bodies[1]["variables"]["after"], "cursor-1");
        assert!(bodies[2]["query"].as_str().unwrap().contains("resolveReviewThread"));
        assert_eq!(bodies[2]["variables"], json!({ "thread": "PRRT_1" }));
        assert!(bodies[4]["query"].as_str().unwrap().contains("unresolveReviewThread"));
        assert_eq!(bodies[4]["variables"], json!({ "thread": "PRRT_1" }));
    }

    fn line(path: &str, side: ReviewSide, line: u32, body: &str) -> LineComment {
        LineComment {
            anchor: Anchor {
                path: path.to_string(),
                side,
                line,
            },
            body: body.to_string(),
        }
    }

    fn created_in_review(id: u64, path: &str, side: &str, line: u32, body: &str) -> Value {
        json!({
            "id": id, "pull_request_review_id": 77, "user": user(9, "me"), "body": body,
            "path": path, "line": line, "original_line": line, "side": side,
            "created_at": "2026-11-21T10:00:00Z",
        })
    }

    #[test]
    fn publishes_line_comments_as_one_pending_review_then_submits_it() {
        let transport = canned(vec![
            answer(200, json!({ "id": 77, "user": user(9, "me"), "state": "PENDING" })),
            // Not in the order sent: they are matched by place and text.
            page(
                json!([
                    created_in_review(23, "b.go", "RIGHT", 8, "Third"),
                    created_in_review(21, "a.go", "RIGHT", 8, "First"),
                    created_in_review(22, "a.go", "LEFT", 3, "Second"),
                ]),
                None,
            ),
            answer(200, json!({ "id": 77, "user": user(9, "me"), "state": "COMMENTED" })),
        ]);
        let sent = [
            line("a.go", ReviewSide::New, 8, "First"),
            line("a.go", ReviewSide::Old, 3, "Second"),
            line("b.go", ReviewSide::New, 8, "Third"),
        ];
        let created = client(&transport)
            .create_review(&repo(), 318, Some(HEAD), &sent)
            .expect("GitHub publishes reviews")
            .unwrap();

        let ids: Vec<&str> = created.iter().map(|comment| comment.id.as_str()).collect();
        assert_eq!(ids, vec!["review-comment:21", "review-comment:22", "review-comment:23"]);
        assert_eq!(created[1].anchor.as_ref().map(|anchor| anchor.side), Some(ReviewSide::Old));
        let base = "/repos/acme/payments-api/pulls/318/reviews";
        assert_eq!(
            calls(&transport),
            vec![
                (Method::Post, base.to_string()),
                (Method::Get, format!("{base}/77/comments")),
                (Method::Post, format!("{base}/77/events")),
            ]
        );
        let requests = transport.sent();
        assert_eq!(
            requests[0].body,
            Some(json!({
                "commit_id": HEAD,
                "comments": [
                    { "path": "a.go", "line": 8, "side": "RIGHT", "body": "First" },
                    { "path": "a.go", "line": 3, "side": "LEFT", "body": "Second" },
                    { "path": "b.go", "line": 8, "side": "RIGHT", "body": "Third" },
                ],
            })),
            "no event: the review stays pending until it is submitted"
        );
        assert_eq!(requests[2].body, Some(json!({ "event": "COMMENT" })));
        assert!(client(&transport).verdict_replaces_the_other());
    }

    #[test]
    fn a_review_that_cannot_be_submitted_is_deleted_and_says_why() {
        let transport = canned(vec![
            answer(200, json!({ "id": 77, "state": "PENDING" })),
            page(json!([created_in_review(21, "a.go", "RIGHT", 8, "First")]), None),
            answer(422, json!({ "message": "Unprocessable Entity", "errors": ["Something went wrong"] })),
            answer(200, json!({ "id": 77, "state": "PENDING" })),
            answer(
                422,
                json!({ "message": "Unprocessable Entity", "errors": ["User can only have one pending review per pull request"] }),
            ),
        ]);
        let github = client(&transport);
        let sent = [line("a.go", ReviewSide::New, 8, "First")];
        let error = github
            .create_review(&repo(), 318, Some(HEAD), &sent)
            .unwrap()
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::RemoteFailed);
        assert!(error.message.contains("Unprocessable Entity: Something went wrong"));
        assert_eq!(
            calls(&transport).last(),
            Some(&(Method::Delete, "/repos/acme/payments-api/pulls/318/reviews/77".to_string()))
        );

        let in_progress = github
            .create_review(&repo(), 318, Some(HEAD), &sent)
            .unwrap()
            .unwrap_err();
        assert_eq!(in_progress.code, ErrorCode::PublishBlocked);
        assert_eq!(in_progress.reason, Some(Reason::GithubPendingReviewExists));
        assert_eq!(transport.sent().len(), 5, "nothing was created to delete");
    }

    #[test]
    fn a_review_missing_a_comment_is_not_taken_as_published() {
        let transport = canned(vec![
            answer(200, json!({ "id": 77, "state": "PENDING" })),
            page(json!([created_in_review(21, "a.go", "RIGHT", 8, "First")]), None),
            answer(204, Value::Null),
        ]);
        let sent = [
            line("a.go", ReviewSide::New, 8, "First"),
            line("a.go", ReviewSide::New, 9, "Second"),
        ];
        let error = client(&transport)
            .create_review(&repo(), 318, None, &sent)
            .unwrap()
            .unwrap_err();
        assert_eq!(error.reason, Some(Reason::GithubReviewIncomplete));
        assert_eq!(
            calls(&transport)[2],
            (Method::Delete, "/repos/acme/payments-api/pulls/318/reviews/77".to_string())
        );
        assert!(transport.sent()[0].body.as_ref().unwrap().get("commit_id").is_none());
    }

    #[test]
    fn approves_requests_changes_and_takes_the_latest_back() {
        let review = |id: u64, by: Value, state: &str| json!({ "id": id, "user": by, "state": state });
        let reviews = json!([
            review(80, user(4, "dana"), "APPROVED"),
            review(81, user(2, "bruno"), "CHANGES_REQUESTED"),
            review(82, user(4, "dana"), "CHANGES_REQUESTED"),
            // Only commenting keeps the request for changes.
            review(83, user(4, "dana"), "COMMENTED"),
        ]);
        let me = || answer(200, user(4, "dana"));
        let transport = canned(vec![
            answer(200, review(90, user(4, "dana"), "APPROVED")),
            answer(200, review(91, user(4, "dana"), "CHANGES_REQUESTED")),
            me(),
            page(reviews.clone(), None),
            answer(200, review(82, user(4, "dana"), "DISMISSED")),
            me(),
            page(reviews, None),
        ]);
        let github = client(&transport);
        github.set_verdict(&repo(), 318, Verdict::Approved, true).unwrap();
        github
            .set_verdict(&repo(), 318, Verdict::ChangesRequested, true)
            .unwrap();
        github
            .set_verdict(&repo(), 318, Verdict::ChangesRequested, false)
            .unwrap();
        // The latest verdict is a request for changes: no approval to take back.
        let nothing = github
            .set_verdict(&repo(), 318, Verdict::Approved, false)
            .unwrap_err();
        assert_eq!(nothing.code, ErrorCode::RemoteNotFound);
        assert_eq!(nothing.reason, Some(Reason::GithubVerdictTargetMissing));

        let base = "/repos/acme/payments-api/pulls/318";
        assert_eq!(
            calls(&transport),
            vec![
                (Method::Post, format!("{base}/reviews")),
                (Method::Post, format!("{base}/reviews")),
                (Method::Get, "/user".to_string()),
                (Method::Get, format!("{base}/reviews")),
                (Method::Put, format!("{base}/reviews/82/dismissals")),
                (Method::Get, "/user".to_string()),
                (Method::Get, format!("{base}/reviews")),
            ]
        );
        let bodies: Vec<Option<Value>> = transport.sent().into_iter().map(|request| request.body).collect();
        assert_eq!(bodies[0], Some(json!({ "event": "APPROVE" })));
        assert_eq!(
            bodies[1],
            Some(json!({ "event": "REQUEST_CHANGES", "body": CHANGES_REQUESTED_BODY }))
        );
        assert_eq!(
            bodies[4],
            Some(json!({ "message": TAKEN_BACK_MESSAGE, "event": "DISMISS" }))
        );
    }

    #[test]
    fn taking_back_without_the_right_to_dismiss_says_what_it_needs() {
        let transport = canned(vec![
            answer(200, user(4, "dana")),
            page(json!([{ "id": 80, "user": user(4, "dana"), "state": "APPROVED" }]), None),
            answer(403, json!({ "message": "Resource not accessible by personal access token" })),
        ]);
        let error = client(&transport)
            .set_verdict(&repo(), 318, Verdict::Approved, false)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::MissingScope);
        assert_eq!(error.reason, Some(Reason::GithubTakeBackRefused));
        assert!(error.message.contains("write access to acme/payments-api"));
    }

    #[test]
    fn failures_read_as_what_went_wrong() {
        let in_two_minutes = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 120;
        let transport = canned(vec![
            answer(401, json!({ "message": "Bad credentials" })),
            answer(403, json!({ "message": "Resource not accessible by personal access token" })),
            answer(404, json!({ "message": "Not Found" })),
            Ok(HttpResponse {
                status: 403,
                body: json!({ "message": "API rate limit exceeded for user ID 4." }).to_string(),
                retry_after: None,
                headers: vec![
                    ("x-ratelimit-remaining", "0".to_string()),
                    ("x-ratelimit-reset", in_two_minutes.to_string()),
                ],
            }),
            Ok(HttpResponse {
                status: 429,
                body: String::new(),
                retry_after: Some("30".to_string()),
                headers: Vec::new(),
            }),
            answer(403, json!({ "message": "You have exceeded a secondary rate limit." })),
            answer(503, json!({})),
            answer(
                422,
                json!({ "message": "Validation Failed", "errors": [{ "resource": "PullRequest", "message": "head is invalid" }] }),
            ),
            Err("it did not answer in time".to_string()),
        ]);
        let github = client(&transport);
        let next = || github.open_pull_requests(&repo(), "feat/x").unwrap_err();

        let expired = next();
        assert_eq!(expired.code, ErrorCode::AuthFailed);
        assert_eq!(expired.reason, Some(Reason::GithubTokenRefused));
        assert!(expired.message.contains("expired or been revoked"));
        let permission = next();
        assert_eq!(permission.code, ErrorCode::MissingScope);
        assert!(permission.message.contains("Resource not accessible by personal access token"));
        assert!(permission.message.contains("needs the repo scope"));
        assert!(permission.message.contains("fine-grained"));
        let missing = next();
        assert_eq!(missing.code, ErrorCode::RemoteNotFound);
        assert!(missing.message.contains("acme/payments-api"));
        let used_up = next();
        assert_eq!(used_up.code, ErrorCode::RateLimited);
        assert_eq!(used_up.reason, Some(Reason::HostRateLimitedRetryAfter));
        let limited = next();
        assert_eq!(limited.code, ErrorCode::RateLimited);
        assert!(limited.message.contains("30 seconds"));
        let secondary = next();
        assert_eq!(secondary.code, ErrorCode::RateLimited);
        assert_eq!(secondary.reason, Some(Reason::HostRateLimited));
        assert_eq!(next().code, ErrorCode::RemoteFailed);
        let refused = next();
        assert_eq!(refused.code, ErrorCode::RemoteFailed);
        assert!(refused.message.contains("Validation Failed: head is invalid"));
        let offline = next();
        assert_eq!(offline.code, ErrorCode::NetworkFailed);
        assert_eq!(offline.message, "Could not reach GitHub: it did not answer in time.");
    }

    #[test]
    fn reads_the_account_of_the_token() {
        let transport = canned(vec![answer(
            200,
            json!({ "id": 4, "login": "dana", "name": "Dana Silva" }),
        )]);
        let account = client(&transport).account().unwrap();
        assert_eq!(
            account,
            Account {
                id: "4".to_string(),
                name: "dana".to_string()
            }
        );
        assert_eq!(calls(&transport), vec![(Method::Get, "/user".to_string())]);
    }

    #[test]
    fn serves_github_com_with_the_token_alone() {
        let provider = GithubProvider;
        assert_eq!(provider.key(), "github");
        assert!(provider.serves("github.com"));
        assert!(provider.serves("ssh.github.com"));
        assert!(!provider.serves("github.example.com"));
        assert!(!provider.serves("bitbucket.org"));
        assert!(!provider.needs_username());
        assert_eq!(provider.scopes(), &["repo"]);
    }
}
