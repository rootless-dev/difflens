//! What the commands ask of pull requests, composed from the project's git
//! remote, the saved connections, the Keychain and the host behind its trait.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use super::bitbucket::BitbucketProvider;
use super::github::GithubProvider;
use super::host::{HostProvider, LineComment, NewComment, PullRequestHost, RemoteComment};
use super::publish::{self, BaseRelation, Facts, HeadRelation, Operation, Plan, Target};
use super::remote::{self, RemoteRepo};
use super::sync::{self, PullContext};
use crate::error::{app_error, AppError, AppResult, CoreText, ErrorCode};
use crate::flow;
use crate::git::Git;
use crate::http::{Credential, ReqwestTransport, Transport};
use crate::model::{
    Account, CommentState, Connection, PublishFailure, PublishReadiness, PublishReport,
    PullRequest, PullRequestLink, ReviewComment, ReviewRef, ReviewSide, ReviewTarget, Verdict,
};
use crate::review_state::{self, ReviewSession};
use crate::secrets::{secret_key, KeychainSecrets, SecretStore};
use crate::store::{ConnectionRecord, PublishedAs, Store};

/// The remote reviews are published to.
const REMOTE: &str = "origin";

pub struct PullRequestService {
    providers: Vec<Box<dyn HostProvider>>,
    secrets: Arc<dyn SecretStore>,
    transport: Arc<dyn Transport>,
}

/// A project's remote on a host the user is connected to.
struct Connected<'a> {
    provider: &'a dyn HostProvider,
    host: Box<dyn PullRequestHost>,
    remote: RemoteRepo,
    account: Account,
}

impl PullRequestService {
    pub fn new(
        providers: Vec<Box<dyn HostProvider>>,
        secrets: Arc<dyn SecretStore>,
        transport: Arc<dyn Transport>,
    ) -> Self {
        Self {
            providers,
            secrets,
            transport,
        }
    }

    /// Every host DiffLens supports, tokens in the Keychain, over HTTPS.
    pub fn standard() -> Self {
        Self::new(
            vec![Box::new(BitbucketProvider), Box::new(GithubProvider)],
            Arc::new(KeychainSecrets),
            Arc::new(ReqwestTransport::new()),
        )
    }

    /// One entry per supported host, connected or not.
    pub fn connections(&self, store: &Store) -> AppResult<Vec<Connection>> {
        self.providers
            .iter()
            .map(|provider| connection_of(provider.as_ref(), store))
            .collect()
    }

    /// The account `username` and `token` belong to. Without a token, the
    /// saved one is used, if it was saved for that user name. A host whose
    /// token needs no user name ignores `username`: its saved token is the
    /// one tested.
    pub fn test_connection(
        &self,
        store: &Store,
        host: &str,
        username: &str,
        token: Option<&str>,
    ) -> AppResult<Account> {
        let provider = self.provider(host)?;
        let saved;
        let username = if provider.needs_username() {
            required(username, email_missing())?
        } else {
            saved = store
                .connection(provider.key())?
                .map(|record| record.username)
                .unwrap_or_default();
            saved.as_str()
        };
        let secret = match token.map(str::trim).filter(|token| !token.is_empty()) {
            Some(token) => token.to_string(),
            None => self
                .saved_secret(store, provider, username)?
                .ok_or_else(|| {
                    app_error!(
                        NotConnected,
                        TokenMissingForTest,
                        "Enter the API token to test it."
                    )
                })?,
        };
        provider
            .connect(credential(username, &secret), Arc::clone(&self.transport))
            .account()
    }

    /// Checks the credential with the host, then keeps the token in the
    /// Keychain and who it belongs to in the database. A host whose token
    /// needs no user name ignores `username` and saves the connection under
    /// the account's name.
    pub fn save_connection(
        &self,
        store: &Store,
        host: &str,
        username: &str,
        token: &str,
    ) -> AppResult<Connection> {
        let provider = self.provider(host)?;
        let username = if provider.needs_username() {
            required(username, email_missing())?
        } else {
            ""
        };
        let token = required(
            token,
            app_error!(Internal, TokenMissing, "Enter the API token."),
        )?;
        let account = provider
            .connect(credential(username, token), Arc::clone(&self.transport))
            .account()?;
        let username = if provider.needs_username() {
            username.to_string()
        } else {
            account.name.clone()
        };
        if let Some(previous) = store.connection(provider.key())? {
            if previous.username != username {
                self.secrets
                    .delete(&secret_key(provider.key(), &previous.username))?;
            }
        }
        self.secrets
            .set(&secret_key(provider.key(), &username), token)?;
        store.save_connection(&ConnectionRecord {
            host: provider.key().to_string(),
            username,
            account_id: account.id,
            account_name: account.name,
            connected_at: flow::now()?,
        })?;
        connection_of(provider, store)
    }

    /// Forgets the connection and deletes its token from the Keychain.
    pub fn disconnect(&self, store: &Store, host: &str) -> AppResult<()> {
        let provider = self.provider(host)?;
        if let Some(record) = store.connection(provider.key())? {
            self.secrets
                .delete(&secret_key(provider.key(), &record.username))?;
        }
        store.delete_connection(provider.key())
    }

    /// The pull request of a review, or why there is none. When there is one,
    /// its comments are brought into the review (only read: nothing is sent).
    pub fn link(
        &self,
        store: &Store,
        git: &Git,
        project_id: &str,
        repo: &Path,
        review: &ReviewRef,
    ) -> AppResult<PullRequestLink> {
        let ReviewTarget::Branch { branch, base } = &review.target else {
            return Ok(PullRequestLink::WorkingTree);
        };
        let connected = match self.reach(store, git, repo)? {
            Ok(connected) => connected,
            Err(link) => return Ok(link),
        };
        let host_name = connected.provider.name().to_string();
        let Some(pull_request) = find_pull_request(&connected, branch, base)? else {
            return Ok(PullRequestLink::NoPullRequest {
                host_name,
                branch: branch.clone(),
            });
        };
        let comments = connected
            .host
            .comments(&connected.remote, pull_request.number)?;
        let session = ReviewSession::new(store, git, project_id, repo, review.clone())?;
        pull(&session, store, review, &pull_request, &comments)?;
        let publish = readiness(git, repo, review, base, &pull_request)?;
        let verdict = publish::verdict_readiness(
            &pull_request,
            head_relation(git, repo, review, &pull_request)?,
        );
        Ok(PullRequestLink::Linked {
            host_name,
            pull_request: Box::new(pull_request),
            account: connected.account,
            publish,
            verdict,
        })
    }

    /// The destination commit of the open pull request from `branch` into
    /// `base`, as the host reports it (possibly abbreviated), for a review
    /// to compare with; see `ProjectService::changed_files_against`. Only
    /// read. `None` when there is no such pull request or the host cannot be
    /// asked now (no origin on a host, not connected, offline): the review
    /// then compares with the base branch, and the link, read next, says why.
    pub fn base_commit(
        &self,
        store: &Store,
        git: &Git,
        repo: &Path,
        branch: &str,
        base: &str,
    ) -> Option<String> {
        let Ok(Ok(connected)) = self.reach(store, git, repo) else {
            return None;
        };
        connected
            .host
            .open_pull_requests(&connected.remote, branch)
            .ok()?
            .into_iter()
            .find(|pull_request| pull_request.destination_branch == base)?
            .destination_commit
    }

    /// Approves, requests changes, or takes either back, as the connected
    /// account, then reads the pull request again. Refused before anything
    /// is sent unless the pull request is open at the commit the review
    /// shows. Giving one verdict first takes back the other one, which a
    /// reviewer cannot hold at the same time, unless the host replaces it by
    /// itself.
    #[allow(clippy::too_many_arguments)]
    pub fn set_verdict(
        &self,
        store: &Store,
        git: &Git,
        project_id: &str,
        repo: &Path,
        review: &ReviewRef,
        verdict: Verdict,
        given: bool,
    ) -> AppResult<PullRequestLink> {
        let ReviewTarget::Branch { branch, base } = &review.target else {
            return Err(unavailable_error(&PullRequestLink::WorkingTree));
        };
        let connected = match self.reach(store, git, repo)? {
            Ok(connected) => connected,
            Err(link) => return Err(unavailable_error(&link)),
        };
        let Some(pull_request) = find_pull_request(&connected, branch, base)? else {
            return Err(unavailable_error(&PullRequestLink::NoPullRequest {
                host_name: connected.provider.name().to_string(),
                branch: branch.clone(),
            }));
        };
        let head = head_relation(git, repo, review, &pull_request)?;
        if let Some(reason) = publish::verdict_readiness(&pull_request, head).reason {
            return Err(*reason);
        }
        let (host, remote, number) = (&connected.host, &connected.remote, pull_request.number);
        if given && !host.verdict_replaces_the_other() {
            let current = pull_request
                .participants
                .iter()
                .find(|participant| participant.account.id == connected.account.id)
                .and_then(|participant| participant.verdict);
            if let Some(other) = current.filter(|current| *current != verdict) {
                host.set_verdict(remote, number, other, false)?;
            }
        }
        host.set_verdict(remote, number, verdict, given)?;
        self.link(store, git, project_id, repo, review)
    }

    /// Sends the review's drafts, replies, edits and deletes to its pull
    /// request. Each change the host refuses stays as it is here, with its
    /// reason; the others are published. Refused as a whole, before anything
    /// is sent, when the review does not show the pull request's diff.
    pub fn publish(
        &self,
        store: &Store,
        git: &Git,
        project_id: &str,
        repo: &Path,
        review: &ReviewRef,
    ) -> AppResult<PublishReport> {
        let ReviewTarget::Branch { branch, base } = &review.target else {
            return Err(unavailable_error(&PullRequestLink::WorkingTree));
        };
        let connected = match self.reach(store, git, repo)? {
            Ok(connected) => connected,
            Err(link) => return Err(unavailable_error(&link)),
        };
        let Some(pull_request) = find_pull_request(&connected, branch, base)? else {
            return Err(unavailable_error(&PullRequestLink::NoPullRequest {
                host_name: connected.provider.name().to_string(),
                branch: branch.clone(),
            }));
        };
        let readiness = readiness(git, repo, review, base, &pull_request)?;
        if let Some(reason) = readiness.reason {
            return Err(*reason);
        }
        let session = ReviewSession::new(store, git, project_id, repo, review.clone())?;
        let comments = session.state(&[])?.comments;
        let plan = publish::plan(&comments);
        execute(store, &connected, &pull_request, &plan, &comments)
    }

    /// Resolves or reopens a thread. One already on the pull request is
    /// resolved there first, at once: resolving is not part of a review's
    /// batch on the hosts, and it does not depend on line numbers.
    pub fn set_resolved(
        &self,
        store: &Store,
        git: &Git,
        project_id: &str,
        repo: &Path,
        comment_id: &str,
        resolved: bool,
    ) -> AppResult<()> {
        let root = review_state::thread_root(store, project_id, comment_id)?;
        if let (CommentState::Published, Some(remote_id), Some(number)) =
            (root.state, &root.remote_id, &root.remote_pull_request)
        {
            let connected = match self.reach(store, git, repo)? {
                Ok(connected) => connected,
                Err(link) => return Err(unavailable_error(&link)),
            };
            let number = number
                .parse()
                .map_err(|_| {
                    app_error!(
                        Internal,
                        CommentPullRequestUnknown,
                        "The comment's pull request is unknown."
                    )
                })?;
            connected
                .host
                .set_resolved(&connected.remote, number, remote_id, resolved)?;
        }
        review_state::set_comment_resolved(store, project_id, comment_id, resolved)
    }

    fn provider(&self, key: &str) -> AppResult<&dyn HostProvider> {
        self.providers
            .iter()
            .map(AsRef::as_ref)
            .find(|provider| provider.key() == key)
            .ok_or_else(|| {
                app_error!(
                    Internal,
                    UnknownIntegration,
                    "DiffLens has no \"{key}\" integration.",
                    key = crate::error::escaped(key)
                )
            })
    }

    fn saved_secret(
        &self,
        store: &Store,
        provider: &dyn HostProvider,
        username: &str,
    ) -> AppResult<Option<String>> {
        match store.connection(provider.key())? {
            Some(record) if record.username == username => {
                self.secrets.get(&secret_key(provider.key(), username))
            }
            _ => Ok(None),
        }
    }

    /// The project's origin on a connected host, or the link that says why
    /// there is none.
    fn reach(
        &self,
        store: &Store,
        git: &Git,
        repo: &Path,
    ) -> AppResult<Result<Connected<'_>, PullRequestLink>> {
        let Some(remote) = git
            .remote_url(repo, REMOTE)?
            .as_deref()
            .and_then(remote::parse)
        else {
            return Ok(Err(PullRequestLink::NoRemote));
        };
        let Some(provider) = self
            .providers
            .iter()
            .map(AsRef::as_ref)
            .find(|provider| provider.serves(&remote.host))
        else {
            return Ok(Err(PullRequestLink::UnsupportedHost { host: remote.host }));
        };
        let not_connected = || {
            Ok(Err(PullRequestLink::NotConnected {
                host_name: provider.name().to_string(),
            }))
        };
        let Some(record) = store.connection(provider.key())? else {
            return not_connected();
        };
        let Some(secret) = self.saved_secret(store, provider, &record.username)? else {
            return not_connected();
        };
        Ok(Ok(Connected {
            provider,
            host: provider.connect(
                credential(&record.username, &secret),
                Arc::clone(&self.transport),
            ),
            remote,
            account: Account {
                id: record.account_id,
                name: record.account_name,
            },
        }))
    }
}

fn connection_of(provider: &dyn HostProvider, store: &Store) -> AppResult<Connection> {
    let record = store.connection(provider.key())?;
    Ok(Connection {
        host: provider.key().to_string(),
        host_name: provider.name().to_string(),
        scopes: provider
            .scopes()
            .iter()
            .map(|scope| scope.to_string())
            .collect(),
        needs_username: provider.needs_username(),
        username: record.as_ref().map(|record| record.username.clone()),
        account: record.as_ref().map(|record| Account {
            id: record.account_id.clone(),
            name: record.account_name.clone(),
        }),
        connected_at: record.map(|record| record.connected_at),
    })
}

fn credential(username: &str, secret: &str) -> Credential {
    Credential {
        username: username.to_string(),
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
        AccountEmailMissing,
        "Enter the email of your account."
    )
}

/// The open pull request from `branch`, preferring one into `base` when the
/// branch has several, read in full (lists leave the description out).
fn find_pull_request(
    connected: &Connected<'_>,
    branch: &str,
    base: &str,
) -> AppResult<Option<PullRequest>> {
    let found = connected
        .host
        .open_pull_requests(&connected.remote, branch)?;
    let chosen = found
        .iter()
        .find(|pull_request| pull_request.destination_branch == base)
        .or_else(|| found.first());
    chosen
        .map(|pull_request| {
            connected
                .host
                .pull_request(&connected.remote, pull_request.number)
        })
        .transpose()
}

/// Brings the pull request's comments into the review. New ones are
/// anchored to the text their line has in the review now.
fn pull(
    session: &ReviewSession<'_>,
    store: &Store,
    review: &ReviewRef,
    pull_request: &PullRequest,
    comments: &[RemoteComment],
) -> AppResult<()> {
    let record = session.record()?;
    let wanted: HashSet<(String, ReviewSide)> = comments
        .iter()
        .filter_map(|comment| comment.anchor.as_ref())
        .map(|anchor| (anchor.path.clone(), anchor.side))
        .collect();
    let lines = session.side_lines(wanted)?;
    let line_text = |path: &str, side: ReviewSide, line: u32| {
        let lines = lines.get(&(path.to_string(), side))?;
        lines.get((line as usize).checked_sub(1)?).cloned()
    };
    let number = pull_request.number.to_string();
    let context = PullContext {
        review_id: &record.id,
        pull_request: &number,
        commit_sha: review.new_sha.as_deref(),
    };
    let existing = store.comments(&record.id)?;
    sync::apply(
        store,
        &sync::plan_pull(&context, &existing, comments, &line_text),
    )
}

/// Whether the review shows the pull request's diff, so its line numbers
/// are the pull request's.
fn readiness(
    git: &Git,
    repo: &Path,
    review: &ReviewRef,
    base: &str,
    pull_request: &PullRequest,
) -> AppResult<PublishReadiness> {
    let local = review.new_sha.as_deref().unwrap_or_default();
    let head = head_relation(git, repo, review, pull_request)?;
    let destination = match pull_request.destination_commit.as_deref() {
        Some(abbreviated) if head == HeadRelation::Same => git.resolve_commit(repo, abbreviated)?,
        _ => None,
    };
    // The review compares with the destination commit whenever this
    // repository has it (`ProjectService::review_base`), so a different old
    // side only means the pull request moved after the review was read.
    let base_relation = match destination {
        None => BaseRelation::Missing,
        Some(destination)
            if git.merge_base(repo, &destination, local)?.as_deref()
                == Some(review.old_sha.as_str()) =>
        {
            BaseRelation::Same
        }
        Some(_) => BaseRelation::Moved,
    };
    Ok(publish::readiness(&Facts {
        base,
        pull_request,
        head,
        base_relation,
    }))
}

/// How the review's head relates to the pull request's source commit.
fn head_relation(
    git: &Git,
    repo: &Path,
    review: &ReviewRef,
    pull_request: &PullRequest,
) -> AppResult<HeadRelation> {
    let local = review.new_sha.as_deref().unwrap_or_default();
    let source = match pull_request.source_commit.as_deref() {
        Some(abbreviated) => git.resolve_commit(repo, abbreviated)?,
        None => None,
    };
    Ok(match source {
        None => HeadRelation::Missing,
        Some(source) if source == local => HeadRelation::Same,
        Some(source) if git.is_ancestor(repo, &source, local)? => HeadRelation::LocalAhead,
        Some(source) if git.is_ancestor(repo, local, &source)? => HeadRelation::LocalBehind,
        Some(_) => HeadRelation::Diverged,
    })
}

/// Failures after which nothing else would get through either.
fn stops_the_batch(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::AuthFailed
            | ErrorCode::MissingScope
            | ErrorCode::RateLimited
            | ErrorCode::NetworkFailed
    )
}

/// What a publish has done so far.
struct Progress<'a> {
    report: PublishReport,
    /// Remote ids of the threads this publish created, by local id.
    threads: HashMap<String, String>,
    /// Why nothing more is sent, after a failure nothing would get past.
    stopped: Option<CoreText>,
    by_id: HashMap<&'a str, &'a ReviewComment>,
}

impl Progress<'_> {
    fn published(&mut self, comment_id: &str, created: Option<String>) {
        self.report.published += 1;
        if let Some(remote_id) = created {
            self.threads.insert(comment_id.to_string(), remote_id);
        }
    }

    /// Keeps the change as it is here, with its reason.
    fn failed(&mut self, store: &Store, comment_id: &str, error: AppError) -> AppResult<()> {
        if self.stopped.is_none() && stops_the_batch(error.code) {
            self.stopped = Some(error.text());
        }
        store.set_publish_error(comment_id, Some(&error))?;
        let comment = self.by_id.get(comment_id);
        self.report.failed.push(PublishFailure {
            comment_id: comment_id.to_string(),
            path: comment
                .map(|comment| comment.path.clone())
                .unwrap_or_default(),
            line: comment
                .map(|comment| comment.current_line.unwrap_or(comment.line))
                .unwrap_or_default(),
            error,
        });
        Ok(())
    }
}

fn execute(
    store: &Store,
    connected: &Connected<'_>,
    pull_request: &PullRequest,
    plan: &Plan,
    comments: &[ReviewComment],
) -> AppResult<PublishReport> {
    let number = pull_request.number;
    let mut progress = Progress {
        report: PublishReport {
            published: 0,
            failed: Vec::new(),
            stale: u32::try_from(plan.stale.len()).unwrap_or(u32::MAX),
        },
        threads: HashMap::new(),
        stopped: None,
        by_id: comments
            .iter()
            .map(|comment| (comment.id.as_str(), comment))
            .collect(),
    };
    let reviewed = send_review(store, connected, pull_request, plan, &mut progress)?;
    for operation in &plan.operations {
        if reviewed.contains(operation.comment_id()) {
            continue;
        }
        let result = match &progress.stopped {
            Some(reason) => Err(not_sent(reason)),
            None => send(store, connected, number, operation, &progress.threads),
        };
        match result {
            Ok(created) => progress.published(operation.comment_id(), created),
            Err(error) => progress.failed(store, operation.comment_id(), error)?,
        }
    }
    Ok(progress.report)
}

fn not_sent(reason: &CoreText) -> AppError {
    app_error!(
        PublishBlocked,
        NotSent,
        "Not sent: {cause}",
        cause = reason.clone()
    )
}

/// On a host whose reviews gather comments, sends the plan's new threads on
/// lines first, together, as one review; returns the local ids of the
/// comments it sent, which the rest of the publish skips. On another host it
/// sends nothing, and each thread goes on its own with the others.
fn send_review<'a>(
    store: &Store,
    connected: &Connected<'_>,
    pull_request: &PullRequest,
    plan: &'a Plan,
    progress: &mut Progress<'_>,
) -> AppResult<HashSet<&'a str>> {
    let lines: Vec<(&str, bool, LineComment)> = plan
        .operations
        .iter()
        .filter_map(|operation| match operation {
            Operation::Create {
                comment_id,
                body,
                target: Target::Line(anchor),
                resolve,
            } => Some((
                comment_id.as_str(),
                *resolve,
                LineComment {
                    anchor: anchor.clone(),
                    body: body.clone(),
                },
            )),
            _ => None,
        })
        .collect();
    if lines.is_empty() {
        return Ok(HashSet::new());
    }
    let new: Vec<LineComment> = lines.iter().map(|(_, _, comment)| comment.clone()).collect();
    let number = pull_request.number;
    let Some(result) = connected.host.create_review(
        &connected.remote,
        number,
        pull_request.source_commit.as_deref(),
        &new,
    ) else {
        return Ok(HashSet::new());
    };
    match result {
        Ok(created) => {
            let now = flow::now()?;
            // The host gives one comment per line comment, in their order.
            for ((comment_id, resolve, _), created) in lines.iter().zip(&created) {
                record_created(store, connected, number, comment_id, created, *resolve, &now)?;
                progress.published(comment_id, Some(created.id.clone()));
            }
        }
        Err(error) => {
            for (comment_id, _, _) in &lines {
                progress.failed(store, comment_id, error.clone())?;
            }
        }
    }
    Ok(lines.into_iter().map(|(comment_id, _, _)| comment_id).collect())
}

/// Records a created comment as published here, and resolves the thread it
/// starts when it was resolved before it was ever published.
fn record_created(
    store: &Store,
    connected: &Connected<'_>,
    number: u64,
    comment_id: &str,
    created: &RemoteComment,
    resolve: bool,
    now: &str,
) -> AppResult<()> {
    store.mark_published(comment_id, &published_as(connected, number, created, &created.id), now)?;
    if resolve {
        // The comment is on the pull request either way; if resolving
        // fails, the next pull shows the thread open, as it then is.
        let _ = connected
            .host
            .set_resolved(&connected.remote, number, &created.id, true);
    }
    Ok(())
}

fn published_as(
    connected: &Connected<'_>,
    number: u64,
    comment: &RemoteComment,
    remote_id: &str,
) -> PublishedAs {
    PublishedAs {
        remote_id: remote_id.to_string(),
        author_id: non_empty(&comment.author.id, &connected.account.id),
        author_name: non_empty(&comment.author.name, &connected.account.name),
        pull_request: number.to_string(),
    }
}

/// Sends one change and records it here; returns the remote id of a
/// created comment.
fn send(
    store: &Store,
    connected: &Connected<'_>,
    number: u64,
    operation: &Operation,
    threads: &HashMap<String, String>,
) -> AppResult<Option<String>> {
    let (host, remote) = (&connected.host, &connected.remote);
    let now = flow::now()?;
    match operation {
        Operation::Create {
            comment_id,
            body,
            target,
            resolve,
        } => {
            let new = match target {
                Target::Line(anchor) => NewComment::Inline {
                    anchor: anchor.clone(),
                    body: body.clone(),
                },
                Target::General => NewComment::General { body: body.clone() },
                Target::Thread(parent_id) => NewComment::Reply {
                    parent_id: parent_id.clone(),
                    body: body.clone(),
                },
                Target::NewThread(root) => match threads.get(root) {
                    Some(parent_id) => NewComment::Reply {
                        parent_id: parent_id.clone(),
                        body: body.clone(),
                    },
                    None => {
                        return Err(app_error!(
                            PublishBlocked,
                            NotSentParentFailed,
                            "Not sent: the comment it answers could not be published."
                        ))
                    }
                },
            };
            let created = host.create_comment(remote, number, &new)?;
            record_created(store, connected, number, comment_id, &created, *resolve, &now)?;
            Ok(Some(created.id))
        }
        Operation::Edit {
            comment_id,
            remote_id,
            body,
        } => {
            let updated = host.update_comment(remote, number, remote_id, body)?;
            store.mark_published(
                comment_id,
                &published_as(connected, number, &updated, remote_id),
                &now,
            )?;
            Ok(None)
        }
        Operation::Delete {
            comment_id,
            remote_id,
        } => {
            host.delete_comment(remote, number, remote_id)?;
            store.delete_comment(comment_id)?;
            Ok(None)
        }
    }
}

fn non_empty(value: &str, fallback: &str) -> String {
    if value.is_empty() { fallback } else { value }.to_string()
}

/// Why there is no pull request to send to: `not_connected` when the host
/// lacks a credential, `publish_blocked` otherwise.
fn unavailable_error(link: &PullRequestLink) -> AppError {
    match link {
        PullRequestLink::WorkingTree => app_error!(
            PublishBlocked,
            NoPullRequestForWorkingTree,
            "Uncommitted changes have no pull request."
        ),
        PullRequestLink::NoRemote => app_error!(
            PublishBlocked,
            NoPullRequestWithoutOrigin,
            "This project has no origin on a server, so it has no pull request."
        ),
        PullRequestLink::UnsupportedHost { host } => app_error!(
            PublishBlocked,
            UnsupportedHost,
            "No pull request integration for {host} yet.",
            host = host
        ),
        PullRequestLink::NotConnected { host_name } => app_error!(
            NotConnected,
            HostNotConnected,
            "Connect {host} in Preferences first.",
            host = host_name
        ),
        PullRequestLink::NoPullRequest { host_name, branch } => app_error!(
            PublishBlocked,
            NoOpenPullRequest,
            "{host} has no open pull request from {branch}.",
            host = host_name,
            branch = branch
        ),
        PullRequestLink::Linked { .. } => AppError::untranslated(ErrorCode::PublishBlocked, ""),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::error::Reason;
    use crate::http::{HttpRequest, HttpResponse};
    use crate::model::{
        ChangeSet, CommentKind, Participant, PendingChange, PullRequestState, ReviewBase,
        ReviewBaseKind,
    };
    use crate::projects::ProjectService;
    use crate::pull_requests::host::Anchor;
    use crate::secrets::tests::MemorySecrets;
    use crate::test_support::{isolated_git, TestRepo};

    /// A host kept in memory: its pull requests and comments, the calls it
    /// was given, and the comment bodies it refuses.
    #[derive(Default)]
    struct FakeHost {
        pull_requests: Mutex<Vec<PullRequest>>,
        comments: Mutex<Vec<RemoteComment>>,
        calls: Mutex<Vec<String>>,
        refuse: Mutex<Vec<(String, AppError)>>,
        next_id: Mutex<u64>,
        /// Behaves as GitHub does: new line comments go as one review, a
        /// verdict replaces the other, and the token alone says who it is.
        like_github: Mutex<bool>,
    }

    impl FakeHost {
        fn call(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn refuse(&self, body: &str, error: AppError) {
            self.refuse.lock().unwrap().push((body.to_string(), error));
        }

        fn refused(&self, body: &str) -> AppResult<()> {
            match self
                .refuse
                .lock()
                .unwrap()
                .iter()
                .find(|(text, _)| text == body)
            {
                Some((_, error)) => Err(error.clone()),
                None => Ok(()),
            }
        }

        fn like_github(&self) -> bool {
            *self.like_github.lock().unwrap()
        }

        fn created(&self, comment: &NewComment) -> RemoteComment {
            let (body, parent_id, anchor) = match comment {
                NewComment::Inline { anchor, body } => (body, None, Some(anchor.clone())),
                NewComment::General { body } => (body, None, None),
                NewComment::Reply { parent_id, body } => (body, Some(parent_id.clone()), None),
            };
            let mut next = self.next_id.lock().unwrap();
            *next += 1;
            let created = RemoteComment {
                id: format!("{}", 100 + *next),
                parent_id,
                author: me(),
                body: body.clone(),
                anchor,
                general: matches!(comment, NewComment::General { .. }),
                resolved: false,
                deleted: false,
                created_at: "t".to_string(),
                updated_at: "t".to_string(),
            };
            self.comments.lock().unwrap().push(created.clone());
            created
        }
    }

    fn other(name: &str) -> Account {
        Account {
            id: format!("{{{name}}}"),
            name: name.to_string(),
        }
    }

    fn me() -> Account {
        other("me")
    }

    struct SharedHost(Arc<FakeHost>);

    impl PullRequestHost for SharedHost {
        fn account(&self) -> AppResult<Account> {
            self.0.call("account".to_string());
            Ok(me())
        }

        fn open_pull_requests(
            &self,
            repo: &RemoteRepo,
            branch: &str,
        ) -> AppResult<Vec<PullRequest>> {
            self.0
                .call(format!("list {}/{} {branch}", repo.owner, repo.name));
            Ok(self
                .0
                .pull_requests
                .lock()
                .unwrap()
                .iter()
                .filter(|pull_request| pull_request.source_branch == branch)
                .cloned()
                .collect())
        }

        fn pull_request(&self, _: &RemoteRepo, number: u64) -> AppResult<PullRequest> {
            let found = self.0.pull_requests.lock().unwrap().clone();
            Ok(found
                .into_iter()
                .find(|pull_request| pull_request.number == number)
                .unwrap())
        }

        fn comments(&self, _: &RemoteRepo, _: u64) -> AppResult<Vec<RemoteComment>> {
            Ok(self.0.comments.lock().unwrap().clone())
        }

        fn create_comment(
            &self,
            _: &RemoteRepo,
            _: u64,
            comment: &NewComment,
        ) -> AppResult<RemoteComment> {
            let (body, parent_id) = match comment {
                NewComment::Inline { body, .. } | NewComment::General { body } => (body, None),
                NewComment::Reply { parent_id, body } => (body, Some(parent_id)),
            };
            if matches!(comment, NewComment::General { .. }) {
                self.0.call(format!("create {body:?} on the pull request"));
            } else {
                self.0.call(format!("create {body:?} parent {parent_id:?}"));
            }
            self.0.refused(body)?;
            Ok(self.0.created(comment))
        }

        fn create_review(
            &self,
            _: &RemoteRepo,
            _: u64,
            commit: Option<&str>,
            comments: &[LineComment],
        ) -> Option<AppResult<Vec<RemoteComment>>> {
            if !self.0.like_github() {
                return None;
            }
            let bodies: Vec<&str> = comments.iter().map(|comment| comment.body.as_str()).collect();
            self.0
                .call(format!("review {bodies:?} at {}", commit.map_or(0, str::len)));
            if let Some(error) = bodies.iter().find_map(|body| self.0.refused(body).err()) {
                return Some(Err(error));
            }
            Some(Ok(comments
                .iter()
                .map(|comment| {
                    self.0.created(&NewComment::Inline {
                        anchor: comment.anchor.clone(),
                        body: comment.body.clone(),
                    })
                })
                .collect()))
        }

        fn verdict_replaces_the_other(&self) -> bool {
            self.0.like_github()
        }

        fn update_comment(
            &self,
            _: &RemoteRepo,
            _: u64,
            id: &str,
            body: &str,
        ) -> AppResult<RemoteComment> {
            self.0.call(format!("update {id} {body:?}"));
            self.0.refused(body)?;
            let mut comments = self.0.comments.lock().unwrap();
            let comment = comments
                .iter_mut()
                .find(|comment| comment.id == id)
                .unwrap();
            comment.body = body.to_string();
            Ok(comment.clone())
        }

        fn delete_comment(&self, _: &RemoteRepo, _: u64, id: &str) -> AppResult<()> {
            self.0.call(format!("delete {id}"));
            self.0
                .comments
                .lock()
                .unwrap()
                .retain(|comment| comment.id != id);
            Ok(())
        }

        fn set_resolved(&self, _: &RemoteRepo, _: u64, id: &str, resolved: bool) -> AppResult<()> {
            self.0.call(format!("resolve {id} {resolved}"));
            for comment in self.0.comments.lock().unwrap().iter_mut() {
                if comment.id == id {
                    comment.resolved = resolved;
                }
            }
            Ok(())
        }

        fn set_verdict(
            &self,
            _: &RemoteRepo,
            number: u64,
            verdict: Verdict,
            given: bool,
        ) -> AppResult<()> {
            self.0.call(format!("verdict {verdict:?} {given}"));
            let mut pull_requests = self.0.pull_requests.lock().unwrap();
            let pull_request = pull_requests
                .iter_mut()
                .find(|pull_request| pull_request.number == number)
                .unwrap();
            pull_request
                .participants
                .retain(|participant| participant.account != me());
            if given {
                pull_request.participants.push(Participant {
                    account: me(),
                    reviewer: true,
                    verdict: Some(verdict),
                });
            }
            Ok(())
        }
    }

    struct FakeProvider(Arc<FakeHost>);

    impl HostProvider for FakeProvider {
        fn key(&self) -> &'static str {
            "bitbucket"
        }

        fn name(&self) -> &'static str {
            "Bitbucket Cloud"
        }

        fn scopes(&self) -> &'static [&'static str] {
            &["read:user:bitbucket"]
        }

        fn serves(&self, host: &str) -> bool {
            host == "bitbucket.org"
        }

        fn needs_username(&self) -> bool {
            !self.0.like_github()
        }

        fn connect(&self, _: Credential, _: Arc<dyn Transport>) -> Box<dyn PullRequestHost> {
            Box::new(SharedHost(Arc::clone(&self.0)))
        }
    }

    /// Never asked anything: the fake host stands in for the network.
    struct NoTransport;

    impl Transport for NoTransport {
        fn send(&self, _: &HttpRequest) -> Result<HttpResponse, CoreText> {
            panic!("a test reached for the network")
        }
    }

    struct Fixture {
        repo: TestRepo,
        projects: ProjectService,
        id: String,
        host: Arc<FakeHost>,
        secrets: Arc<MemorySecrets>,
        service: PullRequestService,
    }

    impl Fixture {
        /// `feature` changes app.go on top of `main`; origin is on Bitbucket
        /// and the user is connected.
        fn new() -> Self {
            let repo = TestRepo::init();
            repo.write("app.go", "package app\n\nfunc A() {\n\treturn\n}\n");
            repo.commit_all("initial commit");
            repo.git(&["checkout", "--quiet", "-b", "feature"]);
            repo.write(
                "app.go",
                "package app\n\nfunc A() {\n\treturn\n}\n\nfunc B() {\n\tA()\n}\n",
            );
            repo.commit_all("add B");
            repo.git(&[
                "remote",
                "add",
                "origin",
                "git@bitbucket.org:acme/payments-api.git",
            ]);
            let projects =
                ProjectService::new(Store::open_in_memory().unwrap(), isolated_git(), None);
            let id = projects
                .add_local(repo.path().to_str().unwrap())
                .unwrap()
                .id;
            let host = Arc::new(FakeHost::default());
            let secrets = Arc::new(MemorySecrets::default());
            let service = PullRequestService::new(
                vec![Box::new(FakeProvider(Arc::clone(&host)))],
                Arc::clone(&secrets) as Arc<dyn SecretStore>,
                Arc::new(NoTransport),
            );
            let fixture = Self {
                repo,
                projects,
                id,
                host,
                secrets,
                service,
            };
            fixture
                .service
                .save_connection(fixture.store(), "bitbucket", "me@example.com", "token")
                .unwrap();
            fixture
        }

        fn store(&self) -> &Store {
            self.projects.store()
        }

        fn review(&self) -> ReviewRef {
            let ChangeSet {
                target,
                old_sha,
                new_sha,
                ..
            } = self
                .projects
                .changed_files(
                    &self.id,
                    ReviewTarget::Branch {
                        branch: "feature".to_string(),
                        base: "main".to_string(),
                    },
                )
                .unwrap();
            ReviewRef {
                target,
                old_sha,
                new_sha,
            }
        }

        /// The review as the screen opens it: compared with what the host
        /// says the pull request's destination commit is, if any.
        fn review_with_pull_request(&self) -> (ChangeSet, ReviewRef) {
            let pull_request_base = self.service.base_commit(
                self.store(),
                self.projects.git(),
                self.repo.path(),
                "feature",
                "main",
            );
            let change_set = self
                .projects
                .changed_files_against(
                    &self.id,
                    ReviewTarget::Branch {
                        branch: "feature".to_string(),
                        base: "main".to_string(),
                    },
                    pull_request_base.as_deref(),
                )
                .unwrap();
            let review = ReviewRef {
                target: change_set.target.clone(),
                old_sha: change_set.old_sha.clone(),
                new_sha: change_set.new_sha.clone(),
            };
            (change_set, review)
        }

        fn publish_readiness(&self, review: &ReviewRef) -> PublishReadiness {
            let link = self
                .service
                .link(
                    self.store(),
                    self.projects.git(),
                    &self.id,
                    self.repo.path(),
                    review,
                )
                .unwrap();
            let PullRequestLink::Linked { publish, .. } = link else {
                panic!("expected a linked pull request: {link:?}");
            };
            publish
        }

        /// An open pull request from `feature` into main, at their heads now.
        fn open_pull_request(&self) {
            let main = self.repo.git(&["rev-parse", "main"]);
            self.open_pull_request_into(&main);
        }

        /// An open pull request from `feature`, at its head now, into main
        /// at `destination` as the host reports it (abbreviated, as
        /// Bitbucket does).
        fn open_pull_request_into(&self, destination: &str) {
            let head = self.repo.git(&["rev-parse", "feature"]);
            let main = destination;
            self.host.pull_requests.lock().unwrap().push(PullRequest {
                number: 318,
                title: "Adds B".to_string(),
                description: "B calls A.".to_string(),
                state: PullRequestState::Open,
                url: "https://bitbucket.org/acme/payments-api/pull-requests/318".to_string(),
                author: Some("Ana".to_string()),
                source_branch: "feature".to_string(),
                source_commit: Some(head[..12].to_string()),
                destination_branch: "main".to_string(),
                destination_commit: Some(main[..12].to_string()),
                participants: vec![Participant {
                    account: other("Bruno"),
                    reviewer: true,
                    verdict: Some(Verdict::Approved),
                }],
            });
        }

        fn link(&self) -> PullRequestLink {
            self.service
                .link(
                    self.store(),
                    self.projects.git(),
                    &self.id,
                    self.repo.path(),
                    &self.review(),
                )
                .unwrap()
        }

        fn publish(&self) -> AppResult<PublishReport> {
            self.service.publish(
                self.store(),
                self.projects.git(),
                &self.id,
                self.repo.path(),
                &self.review(),
            )
        }

        fn session(&self) -> ReviewSession<'_> {
            ReviewSession::new(
                self.store(),
                self.projects.git(),
                &self.id,
                self.repo.path(),
                self.review(),
            )
            .unwrap()
        }

        fn comments(&self) -> Vec<ReviewComment> {
            self.session().state(&[]).unwrap().comments
        }

        fn draft(&self, line: u32, body: &str) -> String {
            self.session()
                .add_comment("app.go", ReviewSide::New, line, body)
                .unwrap()
        }
    }

    fn remote_comment(id: &str, parent: Option<&str>, author: Account, line: u32) -> RemoteComment {
        RemoteComment {
            id: id.to_string(),
            parent_id: parent.map(str::to_string),
            author,
            body: format!("remote {id}"),
            anchor: Some(Anchor {
                path: "app.go".to_string(),
                side: ReviewSide::New,
                line,
            }),
            general: false,
            resolved: false,
            deleted: false,
            // Before anything written in the tests, which are dated now.
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn a_review_without_a_reachable_pull_request_says_why() {
        let fixture = Fixture::new();
        let working_tree = ReviewRef {
            target: ReviewTarget::WorkingTree,
            new_sha: None,
            ..fixture.review()
        };
        assert_eq!(
            fixture
                .service
                .link(
                    fixture.store(),
                    fixture.projects.git(),
                    &fixture.id,
                    fixture.repo.path(),
                    &working_tree
                )
                .unwrap(),
            PullRequestLink::WorkingTree
        );
        assert_eq!(
            fixture.link(),
            PullRequestLink::NoPullRequest {
                host_name: "Bitbucket Cloud".to_string(),
                branch: "feature".to_string()
            }
        );

        fixture
            .service
            .disconnect(fixture.store(), "bitbucket")
            .unwrap();
        assert_eq!(
            fixture.link(),
            PullRequestLink::NotConnected {
                host_name: "Bitbucket Cloud".to_string()
            }
        );

        fixture.repo.git(&[
            "remote",
            "set-url",
            "origin",
            "https://github.com/acme/payments-api.git",
        ]);
        assert_eq!(
            fixture.link(),
            PullRequestLink::UnsupportedHost {
                host: "github.com".to_string()
            }
        );
        fixture.repo.git(&["remote", "remove", "origin"]);
        assert_eq!(fixture.link(), PullRequestLink::NoRemote);
        // Nothing was asked of the host but the account and the listing.
        assert_eq!(
            fixture.host.calls(),
            vec!["account", "list acme/payments-api feature"]
        );
    }

    #[test]
    fn linking_pulls_others_comments_as_read_only_and_marks_the_users_own() {
        let fixture = Fixture::new();
        fixture.open_pull_request();
        fixture.host.comments.lock().unwrap().extend([
            remote_comment("1", None, other("Bruno"), 8),
            remote_comment("2", Some("1"), me(), 8),
        ]);

        let PullRequestLink::Linked {
            pull_request,
            account,
            publish,
            ..
        } = fixture.link()
        else {
            panic!("expected a linked pull request");
        };
        assert_eq!(pull_request.number, 318);
        assert_eq!(pull_request.description, "B calls A.");
        assert_eq!(account, me());
        assert_eq!(
            publish,
            PublishReadiness {
                allowed: true,
                reason: None
            }
        );

        let comments = fixture.comments();
        assert_eq!(comments.len(), 2);
        let (theirs, mine) = (&comments[0], &comments[1]);
        assert_eq!(theirs.remote_author_name.as_deref(), Some("Bruno"));
        assert!(!theirs.mine);
        assert_eq!((theirs.current_line, theirs.outdated), (Some(8), false));
        assert!(mine.mine);
        assert_eq!(mine.parent_id.as_deref(), Some(theirs.id.as_str()));
        let store = fixture.store();
        assert!(review_state::edit_comment(store, &fixture.id, &theirs.id, "x").is_err());
        assert!(review_state::delete_comment(store, &fixture.id, &theirs.id).is_err());

        // Linking again finds the same comments instead of adding them twice.
        fixture.link();
        assert_eq!(fixture.comments().len(), 2);
    }

    #[test]
    fn of_several_open_pull_requests_the_one_into_the_reviews_base_is_linked() {
        let fixture = Fixture::new();
        fixture.open_pull_request();
        let mut into_develop = fixture.host.pull_requests.lock().unwrap()[0].clone();
        into_develop.number = 300;
        into_develop.destination_branch = "develop".to_string();
        fixture
            .host
            .pull_requests
            .lock()
            .unwrap()
            .insert(0, into_develop);

        let link = fixture.link();
        let PullRequestLink::Linked { pull_request, .. } = &link else {
            panic!("expected a linked pull request");
        };
        assert_eq!(pull_request.number, 318);
        let json = serde_json::to_value(&link).unwrap();
        assert_eq!(json["kind"], "linked");
        assert_eq!(json["hostName"], "Bitbucket Cloud");
        assert_eq!(json["pullRequest"]["destinationBranch"], "main");
        assert_eq!(json["publish"]["allowed"], true);
        assert_eq!(
            serde_json::to_value(PullRequestLink::UnsupportedHost {
                host: "github.com".to_string()
            })
            .unwrap(),
            serde_json::json!({ "kind": "unsupported_host", "host": "github.com" })
        );
    }

    #[test]
    fn publishing_waits_until_the_review_shows_the_pull_requests_diff() {
        let fixture = Fixture::new();
        fixture.open_pull_request();
        fixture.draft(8, "Why call A?");
        fixture
            .repo
            .commit_file("more.go", "package app\n", "not pushed yet");

        let PullRequestLink::Linked { publish, .. } = fixture.link() else {
            panic!("expected a linked pull request");
        };
        assert!(!publish.allowed);
        assert!(publish
            .reason
            .unwrap()
            .message
            .contains("push first, then Reload"));
        let error = fixture.publish().unwrap_err();
        assert_eq!(error.code, ErrorCode::PublishBlocked);
        assert!(error.message.contains("push first"));
        assert!(!fixture
            .host
            .calls()
            .iter()
            .any(|call| call.starts_with("create")));
    }

    /// `feature` moved onto a main the local `main` is behind; returns that
    /// newer main.
    fn rebuild_feature_on_a_newer_main(repo: &TestRepo) -> String {
        repo.git(&["checkout", "--quiet", "-b", "newer", "main"]);
        repo.commit_file("lib.go", "package app\n", "main moves on");
        let newer = repo.git(&["rev-parse", "HEAD"]);
        repo.git(&["checkout", "--quiet", "-B", "feature", "newer"]);
        repo.commit_file("app.go", "package app\n\nfunc B() {}\n", "add B");
        repo.git(&["branch", "--quiet", "-D", "newer"]);
        newer
    }

    fn paths(change_set: &ChangeSet) -> Vec<&str> {
        change_set
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect()
    }

    #[test]
    fn a_review_compares_with_the_pull_requests_destination_commit_when_it_is_here() {
        let fixture = Fixture::new();
        let newer = rebuild_feature_on_a_newer_main(&fixture.repo);
        fixture.open_pull_request_into(&newer[..12]);

        let (change_set, review) = fixture.review_with_pull_request();

        assert_eq!(
            change_set.base,
            Some(ReviewBase {
                name: "main".to_string(),
                kind: ReviewBaseKind::PullRequest,
                ref_name: None,
                sha: newer.clone(),
            })
        );
        assert_eq!(change_set.old_sha, newer);
        // Only what the pull request changes; the stale local main would
        // add lib.go.
        assert_eq!(paths(&change_set), vec!["app.go"]);
        assert_eq!(
            fixture.publish_readiness(&review),
            PublishReadiness {
                allowed: true,
                reason: None
            }
        );

        // A review read against the stale local main (as before a Reload)
        // only needs a Reload: a fetch would bring nothing.
        let stale = fixture.review();
        assert_ne!(stale.old_sha, newer);
        let reason = fixture.publish_readiness(&stale).reason.unwrap();
        assert_eq!(reason.reason, Some(Reason::PublishMergeBaseDiffers));
        assert!(reason.message.ends_with(": Reload."), "{}", reason.message);
        assert!(!reason.message.contains("fetch"));
    }

    #[test]
    fn a_destination_commit_missing_here_falls_back_to_the_base_and_asks_for_a_fetch() {
        let fixture = Fixture::new();
        let main = fixture.repo.git(&["rev-parse", "main"]);
        // A commit only the host has.
        fixture.open_pull_request_into("0123456789abcdef0123");

        let (change_set, review) = fixture.review_with_pull_request();

        // origin has no main here, so the local one is the base.
        assert_eq!(
            change_set.base,
            Some(ReviewBase {
                name: "main".to_string(),
                kind: ReviewBaseKind::Local,
                ref_name: Some("main".to_string()),
                sha: main.clone(),
            })
        );
        assert_eq!(change_set.old_sha, main);
        assert_eq!(paths(&change_set), vec!["app.go"]);
        let readiness = fixture.publish_readiness(&review);
        assert!(!readiness.allowed);
        let reason = readiness.reason.unwrap();
        assert_eq!(reason.reason, Some(Reason::PublishDestinationMissing));
        assert!(reason.message.contains("fetch, then Reload"), "{}", reason.message);
    }

    #[test]
    fn the_base_commit_is_only_the_destination_of_a_pull_request_into_the_base() {
        let fixture = Fixture::new();
        let base_commit = |base: &str| {
            fixture.service.base_commit(
                fixture.store(),
                fixture.projects.git(),
                fixture.repo.path(),
                "feature",
                base,
            )
        };
        assert_eq!(base_commit("main"), None);

        let main = fixture.repo.git(&["rev-parse", "main"]);
        fixture.open_pull_request();
        assert_eq!(base_commit("main"), Some(main[..12].to_string()));
        // A pull request into another branch says nothing of this base.
        assert_eq!(base_commit("develop"), None);

        fixture
            .service
            .disconnect(fixture.store(), "bitbucket")
            .unwrap();
        assert_eq!(base_commit("main"), None);
    }

    #[test]
    fn publishes_drafts_replies_edits_and_deletes_then_shows_them_published() {
        let fixture = Fixture::new();
        fixture.open_pull_request();
        fixture.host.comments.lock().unwrap().extend([
            remote_comment("1", None, other("Bruno"), 8),
            remote_comment("2", None, me(), 9),
            remote_comment("3", None, me(), 1),
        ]);
        fixture.link();
        let store = fixture.store();
        let pulled = fixture.comments();
        let id_of = |remote: &str| {
            pulled
                .iter()
                .find(|comment| comment.remote_id.as_deref() == Some(remote))
                .unwrap()
                .id
                .clone()
        };
        let root = fixture.draft(7, "New thread");
        review_state::reply(store, &fixture.id, &root, "Its reply").unwrap();
        review_state::reply(store, &fixture.id, &id_of("1"), "Answering Bruno").unwrap();
        review_state::edit_comment(store, &fixture.id, &id_of("2"), "Edited").unwrap();
        review_state::delete_comment(store, &fixture.id, &id_of("3")).unwrap();
        let state = fixture.session().state(&[]).unwrap();
        assert_eq!(state.summary.unpublished, 5);
        let pending: Vec<Option<PendingChange>> = state
            .comments
            .iter()
            .map(|comment| comment.pending_change)
            .collect();
        assert!(pending.contains(&Some(PendingChange::Edit)));
        assert!(pending.contains(&Some(PendingChange::Delete)));

        let report = fixture.publish().unwrap();
        assert_eq!((report.published, report.failed.len()), (5, 0));
        assert_eq!(
            fixture.host.calls()[3..],
            [
                "create \"Answering Bruno\" parent Some(\"1\")".to_string(),
                "create \"New thread\" parent None".to_string(),
                "create \"Its reply\" parent Some(\"102\")".to_string(),
                "update 2 \"Edited\"".to_string(),
                "delete 3".to_string(),
            ]
        );
        let state = fixture.session().state(&[]).unwrap();
        assert_eq!(state.summary.unpublished, 0);
        assert!(state
            .comments
            .iter()
            .all(|comment| comment.state == CommentState::Published
                && comment.pending_change.is_none()));
        assert!(!state
            .comments
            .iter()
            .any(|comment| comment.remote_id.as_deref() == Some("3")));

        // A pull afterwards finds everything where it is.
        fixture.link();
        assert_eq!(fixture.comments().len(), state.comments.len());
    }

    #[test]
    fn general_comments_are_pulled_from_anyone_and_published_with_line_comments() {
        let fixture = Fixture::new();
        fixture.open_pull_request();
        let on_the_pull_request = |id: &str, parent: Option<&str>, author: Account| RemoteComment {
            anchor: None,
            general: true,
            ..remote_comment(id, parent, author, 0)
        };
        fixture.host.comments.lock().unwrap().extend([
            on_the_pull_request("1", None, other("Bruno")),
            on_the_pull_request("2", Some("1"), other("Carla")),
        ]);
        fixture.link();
        let store = fixture.store();
        let pulled = fixture.comments();
        assert_eq!(pulled.len(), 2);
        assert!(pulled.iter().all(|comment| comment.kind == CommentKind::General
            && comment.current_line.is_none()
            && !comment.outdated
            && !comment.mine));
        assert_eq!(pulled[1].parent_id.as_deref(), Some(pulled[0].id.as_str()));

        let line = fixture.draft(7, "On a line");
        let general = fixture.session().add_general_comment("  On the whole change  ").unwrap();
        review_state::reply(store, &fixture.id, &general, "More on it").unwrap();
        review_state::reply(store, &fixture.id, &pulled[0].id, "Answering Bruno").unwrap();
        let state = fixture.session().state(&[]).unwrap();
        assert_eq!(state.summary.unpublished, 4, "general drafts count as unpublished");
        assert_eq!(state.summary.stale_drafts, 0);
        let mine = state.comments.iter().find(|comment| comment.id == general).unwrap();
        assert_eq!(
            (mine.kind, mine.path.as_str(), mine.line, mine.body.as_str()),
            (CommentKind::General, "", 0, "On the whole change")
        );

        let report = fixture.publish().unwrap();
        assert_eq!((report.published, report.failed.len()), (4, 0));
        assert_eq!(
            fixture.host.calls()[3..],
            [
                "create \"Answering Bruno\" parent Some(\"1\")".to_string(),
                "create \"On a line\" parent None".to_string(),
                "create \"On the whole change\" on the pull request".to_string(),
                "create \"More on it\" parent Some(\"103\")".to_string(),
            ]
        );
        let created = fixture.host.comments.lock().unwrap().clone();
        let general_remote = created.iter().find(|comment| comment.body == "On the whole change").unwrap();
        assert!(general_remote.general && general_remote.anchor.is_none());
        let state = fixture.session().state(&[]).unwrap();
        assert_eq!(state.summary.unpublished, 0);
        assert!(state.comments.iter().any(|comment| comment.id == line));

        // Edited and deleted like a line comment; a pull finds them where they are.
        review_state::edit_comment(store, &fixture.id, &general, "Reworded").unwrap();
        let report = fixture.publish().unwrap();
        assert_eq!(report.published, 1);
        assert_eq!(fixture.host.calls().last().unwrap(), "update 103 \"Reworded\"");
        fixture.link();
        let after = fixture.comments();
        assert_eq!(after.len(), state.comments.len());
        // Bruno's and Carla's, the reply to Bruno, and the user's thread of two.
        assert_eq!(after.iter().filter(|comment| comment.kind == CommentKind::General).count(), 5);
        review_state::delete_comment(store, &fixture.id, &general).unwrap();
        fixture.publish().unwrap();
        assert!(fixture.host.calls().contains(&"delete 103".to_string()));
        assert!(!fixture.comments().iter().any(|comment| comment.id == general));
    }

    #[test]
    fn a_partial_failure_keeps_what_was_refused_as_it_was_and_says_why() {
        let fixture = Fixture::new();
        fixture.open_pull_request();
        let refused = fixture.draft(7, "Refused");
        let accepted = fixture.draft(8, "Accepted");
        fixture.host.refuse(
            "Refused",
            AppError::untranslated(
                ErrorCode::RemoteFailed,
                "Bitbucket refused this: bad anchor",
            ),
        );
        let report = fixture.publish().unwrap();

        assert_eq!(report.published, 1);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].comment_id, refused);
        assert_eq!(report.failed[0].line, 7);
        assert!(report.failed[0].error.message.contains("bad anchor"));
        let comments = fixture.comments();
        let state_of = |id: &str| comments.iter().find(|comment| comment.id == id).unwrap();
        assert_eq!(state_of(&refused).state, CommentState::Draft);
        assert_eq!(
            state_of(&refused)
                .publish_error
                .as_ref()
                .map(|error| error.message.as_str()),
            Some("Bitbucket refused this: bad anchor")
        );
        assert_eq!(state_of(&accepted).state, CommentState::Published);
    }

    #[test]
    fn an_expired_token_stops_the_batch_and_keeps_every_draft() {
        let fixture = Fixture::new();
        fixture.open_pull_request();
        fixture.draft(7, "First");
        fixture.draft(8, "Second");
        fixture.host.refuse(
            "First",
            AppError::untranslated(ErrorCode::AuthFailed, "The token has expired."),
        );
        let report = fixture.publish().unwrap();

        assert_eq!(report.published, 0);
        assert_eq!(report.failed.len(), 2);
        assert_eq!(
            report.failed[1].error.message,
            "Not sent: The token has expired."
        );
        assert_eq!(report.failed[1].error.reason, Some(Reason::NotSent));
        // The second one was never sent.
        assert_eq!(
            fixture
                .host
                .calls()
                .iter()
                .filter(|call| call.starts_with("create"))
                .count(),
            1
        );
        assert!(fixture
            .comments()
            .iter()
            .all(|comment| comment.state == CommentState::Draft));
    }

    #[test]
    fn a_draft_whose_line_changed_is_not_published() {
        let fixture = Fixture::new();
        fixture.open_pull_request();
        fixture.draft(8, "On A()");
        // The branch moves on and the pull request with it: line 8 now reads
        // differently.
        fixture.repo.commit_file(
            "app.go",
            "package app\n\nfunc A() {\n\treturn\n}\n\nfunc B() {\n\tA(1)\n}\n",
            "pass an argument",
        );
        fixture.host.pull_requests.lock().unwrap().clear();
        fixture.open_pull_request();

        let report = fixture.publish().unwrap();
        assert_eq!((report.published, report.stale), (0, 1));
        let comments = fixture.comments();
        assert_eq!(comments[0].state, CommentState::Draft);
        assert!(comments[0].outdated);
        assert_eq!(
            fixture.session().state(&[]).unwrap().summary.stale_drafts,
            1
        );
    }

    #[test]
    fn resolving_a_published_thread_goes_to_the_pull_request_at_once() {
        let fixture = Fixture::new();
        fixture.open_pull_request();
        fixture
            .host
            .comments
            .lock()
            .unwrap()
            .push(remote_comment("1", None, other("Bruno"), 8));
        fixture.link();
        let published = fixture.comments()[0].id.clone();
        let draft = fixture.draft(7, "Local only");
        let resolve = |id: &str, resolved: bool| {
            fixture
                .service
                .set_resolved(
                    fixture.store(),
                    fixture.projects.git(),
                    &fixture.id,
                    fixture.repo.path(),
                    id,
                    resolved,
                )
                .unwrap()
        };

        resolve(&published, true);
        resolve(&draft, true);
        resolve(&published, false);
        let calls: Vec<String> = fixture
            .host
            .calls()
            .into_iter()
            .filter(|call| call.starts_with("resolve"))
            .collect();
        assert_eq!(calls, vec!["resolve 1 true", "resolve 1 false"]);
        let comments = fixture.comments();
        assert!(!comments[0].resolved);
        assert!(comments[1].resolved);
    }

    impl Fixture {
        fn verdict(&self, verdict: Verdict, given: bool) -> AppResult<PullRequestLink> {
            self.service.set_verdict(
                self.store(),
                self.projects.git(),
                &self.id,
                self.repo.path(),
                &self.review(),
                verdict,
                given,
            )
        }
    }

    fn verdict_of(link: &PullRequestLink, account: &Account) -> Option<Verdict> {
        let PullRequestLink::Linked { pull_request, .. } = link else {
            panic!("expected a linked pull request");
        };
        pull_request
            .participants
            .iter()
            .find(|participant| &participant.account == account)
            .and_then(|participant| participant.verdict)
    }

    fn verdict_calls(host: &FakeHost) -> Vec<String> {
        host.calls()
            .into_iter()
            .filter(|call| call.starts_with("verdict"))
            .collect()
    }

    #[test]
    fn approves_and_takes_the_approval_back_then_shows_the_pull_request_as_it_is() {
        let fixture = Fixture::new();
        fixture.open_pull_request();
        let PullRequestLink::Linked { verdict, .. } = fixture.link() else {
            panic!("expected a linked pull request");
        };
        assert!(verdict.allowed);

        let approved = fixture.verdict(Verdict::Approved, true).unwrap();
        assert_eq!(verdict_of(&approved, &me()), Some(Verdict::Approved));
        // Others keep theirs.
        assert_eq!(
            verdict_of(&approved, &other("Bruno")),
            Some(Verdict::Approved)
        );
        let withdrawn = fixture.verdict(Verdict::Approved, false).unwrap();
        assert_eq!(verdict_of(&withdrawn, &me()), None);
        assert_eq!(
            verdict_calls(&fixture.host),
            vec!["verdict Approved true", "verdict Approved false"]
        );
    }

    #[test]
    fn requesting_changes_takes_back_an_approval_first() {
        let fixture = Fixture::new();
        fixture.open_pull_request();
        fixture.verdict(Verdict::Approved, true).unwrap();

        let link = fixture.verdict(Verdict::ChangesRequested, true).unwrap();
        assert_eq!(verdict_of(&link, &me()), Some(Verdict::ChangesRequested));
        assert_eq!(
            verdict_calls(&fixture.host),
            vec![
                "verdict Approved true",
                "verdict Approved false",
                "verdict ChangesRequested true"
            ]
        );
        fixture.verdict(Verdict::ChangesRequested, false).unwrap();
        assert_eq!(
            verdict_calls(&fixture.host).last().map(String::as_str),
            Some("verdict ChangesRequested false")
        );
    }

    #[test]
    fn no_verdict_is_given_on_a_commit_the_review_does_not_show() {
        let fixture = Fixture::new();
        fixture.open_pull_request();
        fixture
            .repo
            .commit_file("more.go", "package app\n", "not pushed yet");

        let PullRequestLink::Linked { verdict, .. } = fixture.link() else {
            panic!("expected a linked pull request");
        };
        assert!(!verdict.allowed);
        assert!(verdict
            .reason
            .unwrap()
            .message
            .contains("push first, then Reload"));
        let error = fixture.verdict(Verdict::Approved, true).unwrap_err();
        assert_eq!(error.code, ErrorCode::PublishBlocked);
        assert!(error.message.contains("push first"));

        fixture.host.pull_requests.lock().unwrap()[0].state = PullRequestState::Merged;
        let error = fixture.verdict(Verdict::Approved, true).unwrap_err();
        assert_eq!(error.message, "The pull request is no longer open.");
        assert!(verdict_calls(&fixture.host).is_empty());
    }

    #[test]
    fn a_connection_keeps_its_token_in_the_secret_store_only() {
        let fixture = Fixture::new();
        let connections = fixture.service.connections(fixture.store()).unwrap();
        assert_eq!(connections.len(), 1);
        let connection = &connections[0];
        assert_eq!(connection.username.as_deref(), Some("me@example.com"));
        assert_eq!(connection.account, Some(me()));
        assert_eq!(
            fixture
                .secrets
                .secrets
                .lock()
                .unwrap()
                .get("bitbucket:me@example.com"),
            Some(&"token".to_string())
        );
        assert!(!serde_json::to_string(connection)
            .unwrap()
            .contains("\"token\""));

        // Testing without a token uses the saved one.
        assert_eq!(
            fixture
                .service
                .test_connection(fixture.store(), "bitbucket", "me@example.com", None)
                .unwrap(),
            me()
        );
        assert_eq!(
            fixture
                .service
                .test_connection(fixture.store(), "bitbucket", "someone@example.com", None)
                .unwrap_err()
                .code,
            ErrorCode::NotConnected
        );

        // Another account replaces the first one and its token.
        fixture
            .service
            .save_connection(fixture.store(), "bitbucket", "other@example.com", "token-2")
            .unwrap();
        assert_eq!(
            fixture
                .secrets
                .secrets
                .lock()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["bitbucket:other@example.com".to_string()]
        );
        fixture
            .service
            .disconnect(fixture.store(), "bitbucket")
            .unwrap();
        assert!(fixture.secrets.secrets.lock().unwrap().is_empty());
        assert_eq!(
            fixture.service.connections(fixture.store()).unwrap()[0].username,
            None
        );
    }

    #[test]
    fn a_token_that_says_who_it_is_is_saved_under_the_accounts_name() {
        let fixture = Fixture::new();
        *fixture.host.like_github.lock().unwrap() = true;
        let store = fixture.store();
        let connection = fixture
            .service
            .save_connection(store, "bitbucket", "", "token-2")
            .unwrap();
        assert!(!connection.needs_username);
        assert_eq!(connection.username.as_deref(), Some("me"));
        assert_eq!(
            fixture
                .secrets
                .secrets
                .lock()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["bitbucket:me".to_string()],
            "the token saved under the email before is gone"
        );
        // Testing without a token uses the saved one, whatever user name comes.
        assert_eq!(
            fixture
                .service
                .test_connection(store, "bitbucket", "", None)
                .unwrap(),
            me()
        );
        fixture.service.disconnect(store, "bitbucket").unwrap();
        assert_eq!(
            fixture
                .service
                .test_connection(store, "bitbucket", "", None)
                .unwrap_err()
                .reason,
            Some(Reason::TokenMissingForTest)
        );
    }

    #[test]
    fn new_line_threads_go_as_one_review_where_the_host_gathers_them() {
        let fixture = Fixture::new();
        fixture.open_pull_request();
        fixture
            .host
            .comments
            .lock()
            .unwrap()
            .push(remote_comment("1", None, other("Bruno"), 8));
        fixture.link();
        *fixture.host.like_github.lock().unwrap() = true;
        let store = fixture.store();
        let bruno = fixture.comments()[0].id.clone();
        let first = fixture.draft(7, "First");
        review_state::set_comment_resolved(store, &fixture.id, &first, true).unwrap();
        fixture.draft(8, "Second");
        review_state::reply(store, &fixture.id, &first, "Its reply").unwrap();
        review_state::reply(store, &fixture.id, &bruno, "Answering Bruno").unwrap();
        fixture.session().add_general_comment("On the whole change").unwrap();

        let report = fixture.publish().unwrap();
        assert_eq!((report.published, report.failed.len()), (5, 0));
        assert_eq!(
            fixture.host.calls()[3..],
            [
                "review [\"First\", \"Second\"] at 12".to_string(),
                "resolve 101 true".to_string(),
                "create \"Answering Bruno\" parent Some(\"1\")".to_string(),
                "create \"Its reply\" parent Some(\"101\")".to_string(),
                "create \"On the whole change\" on the pull request".to_string(),
            ]
        );
        let state = fixture.session().state(&[]).unwrap();
        assert_eq!(state.summary.unpublished, 0);
        let second = state.comments.iter().find(|comment| comment.body == "Second").unwrap();
        assert_eq!(second.remote_id.as_deref(), Some("102"));
        assert_eq!(second.remote_author_name.as_deref(), Some("me"));
        fixture.link();
        assert_eq!(fixture.comments().len(), state.comments.len());
    }

    #[test]
    fn a_refused_review_keeps_its_line_drafts_and_their_replies() {
        let fixture = Fixture::new();
        fixture.open_pull_request();
        *fixture.host.like_github.lock().unwrap() = true;
        let store = fixture.store();
        let first = fixture.draft(7, "First");
        let refused = fixture.draft(8, "Refused");
        let reply = review_state::reply(store, &fixture.id, &first, "Its reply").unwrap();
        fixture.session().add_general_comment("On the whole change").unwrap();
        fixture.host.refuse(
            "Refused",
            AppError::untranslated(
                ErrorCode::RemoteFailed,
                "GitHub refused this: Line could not be resolved",
            ),
        );
        let report = fixture.publish().unwrap();

        assert_eq!(report.published, 1, "the general comment is not in the review");
        let failed: Vec<(&str, &str)> = report
            .failed
            .iter()
            .map(|failure| (failure.comment_id.as_str(), failure.error.message.as_str()))
            .collect();
        assert_eq!(
            failed,
            vec![
                (first.as_str(), "GitHub refused this: Line could not be resolved"),
                (refused.as_str(), "GitHub refused this: Line could not be resolved"),
                (reply.as_str(), "Not sent: the comment it answers could not be published."),
            ]
        );
        let comments = fixture.comments();
        assert!(comments
            .iter()
            .filter(|comment| comment.kind == CommentKind::Line)
            .all(|comment| comment.state == CommentState::Draft && comment.publish_error.is_some()));
    }

    #[test]
    fn where_a_verdict_replaces_the_other_nothing_is_taken_back_first() {
        let fixture = Fixture::new();
        fixture.open_pull_request();
        *fixture.host.like_github.lock().unwrap() = true;
        fixture.verdict(Verdict::Approved, true).unwrap();

        let link = fixture.verdict(Verdict::ChangesRequested, true).unwrap();
        assert_eq!(verdict_of(&link, &me()), Some(Verdict::ChangesRequested));
        assert_eq!(
            verdict_calls(&fixture.host),
            vec!["verdict Approved true", "verdict ChangesRequested true"]
        );
    }
}
