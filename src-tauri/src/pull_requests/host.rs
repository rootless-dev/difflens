//! The boundary every pull request host sits behind. Nothing outside a
//! host's own module knows its URLs, JSON or quirks: the rest of DiffLens
//! speaks in the host-neutral types here. Supporting another host means
//! writing one `HostProvider` and its `PullRequestHost`, and listing the
//! provider in `PullRequestService::standard`.

use std::sync::Arc;

use super::remote::RemoteRepo;
use crate::error::AppResult;
use crate::http::{Credential, Transport};
use crate::model::{Account, PullRequest, ReviewSide, Verdict};

/// Where an inline comment sits: a line of one side of a file, in the
/// numbering of the pull request's diff (new side: the source commit; old
/// side: where the branch left the destination).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub path: String,
    pub side: ReviewSide,
    pub line: u32,
}

/// A comment as the host has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteComment {
    pub id: String,
    /// The comment it answers, which may itself be a reply.
    pub parent_id: Option<String>,
    pub author: Account,
    pub body: String,
    /// `None` for a comment on the whole pull request or a whole file.
    pub anchor: Option<Anchor>,
    /// On the pull request as a whole: on no file (nor line) of it.
    pub general: bool,
    /// For the first comment of a thread: the thread is resolved.
    pub resolved: bool,
    /// Deleted, but still listed because replies hang from it.
    pub deleted: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// A comment to create: on a line, on the pull request as a whole, or as a
/// reply to an existing comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewComment {
    Inline { anchor: Anchor, body: String },
    General { body: String },
    Reply { parent_id: String, body: String },
}

/// A new thread on a line, as a publish sends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineComment {
    pub anchor: Anchor,
    pub body: String,
}

/// One host as one connected account sees it. Calls block; they run on the
/// commands' blocking threads.
pub trait PullRequestHost: Send + Sync {
    /// The account the credential belongs to.
    fn account(&self) -> AppResult<Account>;

    /// Open pull requests whose source branch is `branch`.
    fn open_pull_requests(&self, repo: &RemoteRepo, branch: &str) -> AppResult<Vec<PullRequest>>;

    /// One pull request with everything about it, description and
    /// participants included.
    fn pull_request(&self, repo: &RemoteRepo, number: u64) -> AppResult<PullRequest>;

    /// Every comment of a pull request, replies included, oldest first.
    fn comments(&self, repo: &RemoteRepo, number: u64) -> AppResult<Vec<RemoteComment>>;

    fn create_comment(
        &self,
        repo: &RemoteRepo,
        number: u64,
        comment: &NewComment,
    ) -> AppResult<RemoteComment>;

    /// A publish's new threads on lines, as one review of the pull request
    /// at `commit` (its source commit, as the review shows it), on a host
    /// whose reviews gather comments and land them together (GitHub):
    /// `Some`, with the created comments in the order given, or why the host
    /// refused the review as a whole. `None`, the default, on a host without
    /// such reviews: each one is then created on its own, by `create_comment`.
    fn create_review(
        &self,
        _repo: &RemoteRepo,
        _number: u64,
        _commit: Option<&str>,
        _comments: &[LineComment],
    ) -> Option<AppResult<Vec<RemoteComment>>> {
        None
    }

    fn update_comment(
        &self,
        repo: &RemoteRepo,
        number: u64,
        id: &str,
        body: &str,
    ) -> AppResult<RemoteComment>;

    /// Succeeds when the comment is already gone.
    fn delete_comment(&self, repo: &RemoteRepo, number: u64, id: &str) -> AppResult<()>;

    /// Resolves or reopens the thread `id` starts.
    fn set_resolved(
        &self,
        repo: &RemoteRepo,
        number: u64,
        id: &str,
        resolved: bool,
    ) -> AppResult<()>;

    /// Gives (`given` true) or takes back the connected account's verdict
    /// on a pull request: an approval, or a request for changes.
    fn set_verdict(
        &self,
        repo: &RemoteRepo,
        number: u64,
        verdict: Verdict,
        given: bool,
    ) -> AppResult<()>;

    /// Whether giving a verdict replaces the account's other one by itself,
    /// as GitHub's latest review does. When it does not (the default), the
    /// other one is taken back first.
    fn verdict_replaces_the_other(&self) -> bool {
        false
    }
}

/// A kind of host: how to recognise its remotes and how to connect to it.
pub trait HostProvider: Send + Sync {
    /// Stable key the connection is saved under, such as "bitbucket".
    fn key(&self) -> &'static str;

    /// Such as "Bitbucket Cloud".
    fn name(&self) -> &'static str;

    /// Permissions the user's token needs, as the host names them; shown
    /// where the token is entered.
    fn scopes(&self) -> &'static [&'static str];

    /// Whether remotes on `host` (a lowercase host name) are this host's.
    fn serves(&self, host: &str) -> bool;

    /// Whether the credential has a user name besides its token, such as
    /// Bitbucket's account email (the default). Without one (GitHub), the
    /// token alone says who it is, and the connection is saved under the
    /// account's name.
    fn needs_username(&self) -> bool {
        true
    }

    fn connect(
        &self,
        credential: Credential,
        transport: Arc<dyn Transport>,
    ) -> Box<dyn PullRequestHost>;
}
