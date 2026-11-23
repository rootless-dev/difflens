//! The state of a review against the code it shows: which marked files and
//! functions are still as they were reviewed, where each comment sits now,
//! and what changed since the last review action.
//!
//! Everything is recorded in DiffLens's own database (`store::reviews`), per
//! project and review target, never in the project. Marks and comments are
//! taken against the sides the screen froze (`ReviewRef`), so what is stored
//! is what the user was looking at, even if the branch moved meanwhile.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use uuid::Uuid;

#[cfg(test)]
use crate::error::ErrorCode;
use crate::error::{app_error, escaped, AppError, AppResult};
use crate::fingerprint;
use crate::flow;
use crate::git::Git;
use crate::model::{
    Checkpoint, CommentKind, CommentState, FileSide, FileStatus, FunctionSpan, MarkStatus,
    MarkedFile, MarkedFunction, PendingChange, ReviewComment, ReviewRef, ReviewSide, ReviewState,
    ReviewSummary, SignalDraft, SinceLastReview, SinceUnavailableReason,
};
use crate::pull_requests;
use crate::review::{self, Source};
use crate::store::{CommentRecord, MarkKind, MarkRecord, ReviewRecord, Store};

/// How far up or down, in lines, a comment whose line changed is looked for.
const RELOCATE_WITHIN: usize = 50;

/// Longest comment accepted, in bytes.
const MAX_BODY_BYTES: usize = 64 * 1024;

/// Both sides of each file the review changes, by path.
type Changed = BTreeMap<String, (Source, Source)>;

/// Text of file sides; `None` for a side with no text to show.
type Texts = HashMap<(String, ReviewSide), Option<String>>;

/// A changed file and both of its sides.
#[derive(Debug, Clone)]
pub struct ChangedSides {
    pub path: String,
    /// Before a rename.
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub old: FileSide,
    pub new: FileSide,
}

/// One review of a project, as the screen has it open.
pub struct ReviewSession<'a> {
    store: &'a Store,
    git: &'a Git,
    project_id: &'a str,
    repo: &'a Path,
    review: ReviewRef,
}

impl<'a> ReviewSession<'a> {
    /// Fails with `unknown_branch` when a side is not a commit of `repo`.
    pub fn new(
        store: &'a Store,
        git: &'a Git,
        project_id: &'a str,
        repo: &'a Path,
        review: ReviewRef,
    ) -> AppResult<Self> {
        review::check_commits(git, repo, &review.old_sha, review.new_sha.as_deref())?;
        Ok(Self {
            store,
            git,
            project_id,
            repo,
            review,
        })
    }

    /// Everything recorded in the review, read against its sides. Functions
    /// are only known to the flow graph, so their status is given for the
    /// `functions` asked about.
    pub fn state(&self, functions: &[FunctionSpan]) -> AppResult<ReviewState> {
        let changed = self.changed()?;
        let mut summary = ReviewSummary {
            total_files: count(changed.len()),
            ..ReviewSummary::default()
        };
        let Some(record) = self.store.review(self.project_id, &self.review.target)? else {
            return Ok(ReviewState {
                files: Vec::new(),
                functions: Vec::new(),
                comments: Vec::new(),
                checkpoint: None,
                since: self.since(None)?,
                summary,
                signal_drafts: Vec::new(),
            });
        };
        let function_marks: HashMap<String, MarkRecord> = self
            .store
            .marks(&record.id, MarkKind::Function)?
            .into_iter()
            .map(|mark| (mark.key.clone(), mark))
            .collect();
        let spans: Vec<&FunctionSpan> = functions
            .iter()
            .filter(|span| function_marks.contains_key(&span.id))
            .collect();
        let comments = self.store.comments(&record.id)?;
        let signal_drafts = self
            .store
            .signal_comments(&record.id)?
            .into_iter()
            .filter_map(|(signal, comment_id)| {
                let comment = comments
                    .iter()
                    .find(|comment| comment.id == comment_id && !comment.delete_pending)?;
                Some(SignalDraft {
                    signal,
                    kind: comment.kind,
                    comment_id,
                })
            })
            .collect();

        let wanted = spans
            .iter()
            .filter(|span| !span.deleted)
            .map(|span| (span.file.clone(), ReviewSide::New))
            .chain(
                comments
                    .iter()
                    .filter(|comment| comment.kind == CommentKind::Line)
                    .map(|comment| (comment.path.clone(), comment.side)),
            );
        let texts = self.texts(&changed, wanted.collect())?;

        let mut files = Vec::new();
        for mark in self.store.marks(&record.id, MarkKind::File)? {
            let Some((_, new)) = changed.get(&mark.key) else {
                continue;
            };
            let status = status(&mark, &self.file_fingerprint(&mark.key, new)?);
            files.push(MarkedFile {
                path: mark.key,
                status,
                reviewed_at: mark.reviewed_at,
            });
        }
        let functions: Vec<MarkedFunction> = spans
            .into_iter()
            .map(|span| {
                let mark = &function_marks[&span.id];
                MarkedFunction {
                    id: span.id.clone(),
                    status: status(mark, &function_fingerprint(span, &texts)),
                    reviewed_at: mark.reviewed_at.clone(),
                }
            })
            .collect();
        let accounts = self.store.connected_account_ids()?;
        let comments: Vec<ReviewComment> = comments
            .into_iter()
            .map(|comment| placed(comment, &texts, &accounts))
            .collect();
        let since = self.since(Some(&record))?;

        let reviewed = |status| files.iter().filter(|file| file.status == status).count();
        summary.reviewed_files = count(reviewed(MarkStatus::Reviewed));
        summary.reopened_files = count(reviewed(MarkStatus::Reopened));
        summary.reopened_functions = count(
            functions
                .iter()
                .filter(|function| function.status == MarkStatus::Reopened)
                .count(),
        );
        summary.files_changed_since = match &since {
            SinceLastReview::Available { files, .. } => count(files.len()),
            SinceLastReview::Unavailable { .. } => 0,
        };
        summary.outdated_comments = count(
            comments
                .iter()
                .filter(|comment| {
                    comment.parent_id.is_none() && !comment.resolved && comment.outdated
                })
                .count(),
        );
        let publish = pull_requests::plan(&comments);
        summary.unpublished = count(publish.operations.len());
        summary.stale_drafts = count(publish.stale.len());
        Ok(ReviewState {
            files,
            functions,
            comments,
            checkpoint: record.checkpoint_at.map(|at| Checkpoint {
                sha: record.checkpoint_sha,
                at,
            }),
            since,
            summary,
            signal_drafts,
        })
    }

    /// Marks a changed file as reviewed with the fingerprint of its new side
    /// (a deleted file gets a fixed one), or with `reviewed` false forgets
    /// the mark. Marking moves the checkpoint to this head.
    pub fn set_file_reviewed(&self, path: &str, reviewed: bool) -> AppResult<()> {
        review::check_path(path)?;
        if !reviewed {
            return self.unmark(MarkKind::File, path);
        }
        let changed = self.changed()?;
        let Some((_, new)) = changed.get(path) else {
            return Err(not_in_review(path));
        };
        let fingerprint = self.file_fingerprint(path, new)?;
        self.mark(MarkKind::File, path, fingerprint)
    }

    /// Marks a changed function as reviewed with a fingerprint of its source
    /// on the new side, or with `reviewed` false forgets the mark. Marking
    /// moves the checkpoint to this head.
    pub fn set_function_reviewed(&self, span: &FunctionSpan, reviewed: bool) -> AppResult<()> {
        if !reviewed {
            return self.unmark(MarkKind::Function, &span.id);
        }
        review::check_path(&span.file)?;
        let changed = self.changed()?;
        if !changed.contains_key(&span.file) {
            return Err(not_in_review(&span.file));
        }
        let wanted = HashSet::from([(span.file.clone(), ReviewSide::New)]);
        let texts = self.texts(&changed, wanted)?;
        self.mark(
            MarkKind::Function,
            &span.id,
            function_fingerprint(span, &texts),
        )
    }

    /// Comments `line` of one side of a changed file; returns the comment id.
    /// The line's text is kept with it, to find it again once the code moves.
    pub fn add_comment(
        &self,
        path: &str,
        side: ReviewSide,
        line: u32,
        body: &str,
    ) -> AppResult<String> {
        review::check_path(path)?;
        let body = clean_body(body)?;
        let changed = self.changed()?;
        if !changed.contains_key(path) {
            return Err(not_in_review(path));
        }
        let texts = self.texts(&changed, HashSet::from([(path.to_string(), side)]))?;
        let Some(Some(text)) = texts.get(&(path.to_string(), side)) else {
            return Err(app_error!(
                UnknownFile,
                SideHasNoText,
                "That side of \"{path}\" has no text to comment on.",
                path = escaped(path)
            ));
        };
        let lines = doc_lines(text);
        let Some(line_text) = (line as usize)
            .checked_sub(1)
            .and_then(|index| lines.get(index))
        else {
            return Err(app_error!(
                UnknownFile,
                NoSuchLine,
                "\"{path}\" has no line {line} on that side.",
                path = escaped(path),
                line = line
            ));
        };
        let now = flow::now()?;
        let record = self
            .store
            .open_review(self.project_id, &self.review.target, &now)?;
        let comment = CommentRecord {
            id: Uuid::new_v4().to_string(),
            review_id: record.id,
            kind: CommentKind::Line,
            parent_id: None,
            path: path.to_string(),
            side,
            line,
            line_text: line_text.to_string(),
            commit_sha: self.review.new_sha.clone(),
            body,
            state: CommentState::Draft,
            remote_id: None,
            remote_author_id: None,
            remote_author_name: None,
            resolved: false,
            created_at: now.clone(),
            updated_at: now,
            remote_pull_request: None,
            edited_locally: false,
            delete_pending: false,
            publish_error: None,
        };
        self.store.insert_comment(&comment)?;
        Ok(comment.id)
    }

    /// Comments the pull request as a whole, on no file or line; returns the
    /// comment id. Like a line comment it is a draft until published.
    pub fn add_general_comment(&self, body: &str) -> AppResult<String> {
        let body = clean_body(body)?;
        let now = flow::now()?;
        let record = self
            .store
            .open_review(self.project_id, &self.review.target, &now)?;
        let comment = CommentRecord {
            id: Uuid::new_v4().to_string(),
            review_id: record.id,
            kind: CommentKind::General,
            parent_id: None,
            path: String::new(),
            side: ReviewSide::New,
            line: 0,
            line_text: String::new(),
            commit_sha: self.review.new_sha.clone(),
            body,
            state: CommentState::Draft,
            remote_id: None,
            remote_author_id: None,
            remote_author_name: None,
            resolved: false,
            created_at: now.clone(),
            updated_at: now,
            remote_pull_request: None,
            edited_locally: false,
            delete_pending: false,
            publish_error: None,
        };
        self.store.insert_comment(&comment)?;
        Ok(comment.id)
    }

    /// The review's record, created when there is none yet.
    pub fn record(&self) -> AppResult<ReviewRecord> {
        self.store
            .open_review(self.project_id, &self.review.target, &flow::now()?)
    }

    /// The review's record, `None` until something is recorded in it.
    pub fn existing_record(&self) -> AppResult<Option<ReviewRecord>> {
        self.store.review(self.project_id, &self.review.target)
    }

    pub fn store(&self) -> &Store {
        self.store
    }

    pub fn project_id(&self) -> &str {
        self.project_id
    }

    /// The sides the screen froze.
    pub fn review(&self) -> &ReviewRef {
        &self.review
    }

    pub fn git(&self) -> &Git {
        self.git
    }

    pub fn repo(&self) -> &Path {
        self.repo
    }

    /// Every changed file, sorted by path, with both of its sides, all read
    /// in one go.
    pub fn changed_sides(&self) -> AppResult<Vec<ChangedSides>> {
        let from_disk = self.review.new_sha.is_none();
        let mut files: Vec<(String, Option<String>, FileStatus, Source, Source)> = Vec::new();
        let changes = self.git.diff_files(
            self.repo,
            &self.review.old_sha,
            self.review.new_sha.as_deref(),
            &[],
        )?;
        for change in &changes {
            let listed = review::changed_file(change);
            let (old, new) = review::sources(change, from_disk);
            files.push((listed.path, listed.old_path, listed.status, old, new));
        }
        if from_disk {
            for untracked in self.git.untracked_files(self.repo, &[])? {
                let path = untracked
                    .to_string_lossy()
                    .trim_end_matches('/')
                    .to_string();
                if !files.iter().any(|file| file.0 == path) {
                    files.push((path, None, FileStatus::Added, Source::Absent, Source::Disk));
                }
            }
        }
        files.sort_by(|left, right| left.0.cmp(&right.0));

        let wanted: Vec<(&str, Source)> = files
            .iter()
            .flat_map(|(path, _, _, old, new)| {
                [(path.as_str(), old.clone()), (path.as_str(), new.clone())]
            })
            .collect();
        let mut sides = review::read_sources(self.git, self.repo, wanted)?.into_iter();
        let mut next = || {
            sides
                .next()
                .ok_or_else(|| {
                    app_error!(
                        Internal,
                        TooFewSidesForFiles,
                        "Reading the files returned too few sides."
                    )
                })
        };
        files
            .into_iter()
            .map(|(path, old_path, status, _, _)| {
                Ok(ChangedSides {
                    path,
                    old_path,
                    status,
                    old: next()?,
                    new: next()?,
                })
            })
            .collect()
    }

    /// The lines of the `wanted` sides of changed files, as the Diff numbers
    /// them. A side with no text, or of a file the review does not change,
    /// is left out.
    pub fn side_lines(
        &self,
        wanted: HashSet<(String, ReviewSide)>,
    ) -> AppResult<HashMap<(String, ReviewSide), Vec<String>>> {
        let changed = self.changed()?;
        Ok(self
            .texts(&changed, wanted)?
            .into_iter()
            .filter_map(|(key, text)| {
                let text = text?;
                Some((
                    key,
                    doc_lines(&text).into_iter().map(str::to_string).collect(),
                ))
            })
            .collect())
    }

    fn mark(&self, kind: MarkKind, key: &str, fingerprint: String) -> AppResult<()> {
        let now = flow::now()?;
        let record = self
            .store
            .open_review(self.project_id, &self.review.target, &now)?;
        self.store.mark(
            &record.id,
            kind,
            &MarkRecord {
                key: key.to_string(),
                fingerprint,
                reviewed_at: now.clone(),
            },
        )?;
        self.store
            .set_checkpoint(&record.id, self.review.new_sha.as_deref(), &now)
    }

    fn unmark(&self, kind: MarkKind, key: &str) -> AppResult<()> {
        match self.store.review(self.project_id, &self.review.target)? {
            Some(record) => self.store.unmark(&record.id, kind, key),
            None => Ok(()),
        }
    }

    /// The files of the frozen sides, listed as the change set lists them.
    fn changed(&self) -> AppResult<Changed> {
        let from_disk = self.review.new_sha.is_none();
        let mut files = Changed::new();
        let changes = self.git.diff_files(
            self.repo,
            &self.review.old_sha,
            self.review.new_sha.as_deref(),
            &[],
        )?;
        for change in &changes {
            let path = String::from_utf8_lossy(&change.path).into_owned();
            files.insert(path, review::sources(change, from_disk));
        }
        if from_disk {
            for untracked in self.git.untracked_files(self.repo, &[])? {
                let path = untracked.to_string_lossy();
                files
                    .entry(path.trim_end_matches('/').to_string())
                    .or_insert((Source::Absent, Source::Disk));
            }
        }
        Ok(files)
    }

    /// The texts of the `wanted` sides of changed files, in one read.
    fn texts(&self, changed: &Changed, wanted: HashSet<(String, ReviewSide)>) -> AppResult<Texts> {
        let wanted: Vec<(String, ReviewSide)> = wanted
            .into_iter()
            .filter(|(path, _)| changed.contains_key(path))
            .collect();
        let sources = wanted
            .iter()
            .map(|(path, side)| {
                let (old, new) = &changed[path];
                let source = match side {
                    ReviewSide::Old => old,
                    ReviewSide::New => new,
                };
                (path.as_str(), source.clone())
            })
            .collect();
        let sides = review::read_sources(self.git, self.repo, sources)?;
        Ok(wanted
            .into_iter()
            .zip(sides)
            .map(|(key, side)| (key, side.text))
            .collect())
    }

    fn file_fingerprint(&self, path: &str, new: &Source) -> AppResult<String> {
        Ok(match new {
            Source::Absent => fingerprint::ABSENT.to_string(),
            Source::Object(entry) => fingerprint::of_object(&entry.oid),
            Source::Disk => review::disk_fingerprint(self.git, self.repo, path)?
                .unwrap_or_else(|| fingerprint::ABSENT.to_string()),
        })
    }

    fn since(&self, record: Option<&ReviewRecord>) -> AppResult<SinceLastReview> {
        let unavailable = |reason| Ok(SinceLastReview::Unavailable { reason });
        let Some(head) = self.review.new_sha.as_deref() else {
            return unavailable(SinceUnavailableReason::WorkingTree);
        };
        let Some(checkpoint) = record.and_then(|record| record.checkpoint_sha.as_deref()) else {
            return unavailable(SinceUnavailableReason::NoCheckpoint);
        };
        if checkpoint == head {
            return unavailable(SinceUnavailableReason::UpToDate);
        }
        if !self.git.are_commits(self.repo, &[checkpoint])?[0] {
            return unavailable(SinceUnavailableReason::CheckpointMissing);
        }
        Ok(SinceLastReview::Available {
            checkpoint_sha: checkpoint.to_string(),
            commits: self.git.count_commits(self.repo, checkpoint, head)?,
            files: review::between_commits(self.git, self.repo, checkpoint, head)?,
        })
    }
}

/// Answers `body` in the thread of comment `parent_id` (a reply to a reply
/// joins the same thread); returns the new comment's id.
pub fn reply(store: &Store, project_id: &str, parent_id: &str, body: &str) -> AppResult<String> {
    let body = clean_body(body)?;
    let root = thread_root(store, project_id, parent_id)?;
    let now = flow::now()?;
    let comment = CommentRecord {
        id: Uuid::new_v4().to_string(),
        parent_id: Some(root.id.clone()),
        body,
        state: CommentState::Draft,
        remote_id: None,
        remote_author_id: None,
        remote_author_name: None,
        resolved: false,
        created_at: now.clone(),
        updated_at: now,
        remote_pull_request: None,
        edited_locally: false,
        delete_pending: false,
        publish_error: None,
        ..root
    };
    store.insert_comment(&comment)?;
    Ok(comment.id)
}

/// Changes the text of one of the user's comments. A published one keeps
/// its old text on the pull request until the next publish.
pub fn edit_comment(store: &Store, project_id: &str, id: &str, body: &str) -> AppResult<()> {
    let body = clean_body(body)?;
    let comment = own_comment(store, project_id, id)?;
    if body == comment.body {
        return Ok(());
    }
    let published = comment.state == CommentState::Published;
    store.set_comment_body(&comment.id, &body, published, &flow::now()?)
}

/// Deletes one of the user's comments, and the replies of a thread's first
/// comment. A published one is only marked: it leaves the pull request, and
/// then this review, on the next publish.
pub fn delete_comment(store: &Store, project_id: &str, id: &str) -> AppResult<()> {
    let comment = own_comment(store, project_id, id)?;
    match comment.state {
        CommentState::Published => store.set_delete_pending(&comment.id, true),
        CommentState::Draft => store.delete_comment(&comment.id),
    }
}

/// Takes back the delete of a published comment that was not published yet.
pub fn restore_comment(store: &Store, project_id: &str, id: &str) -> AppResult<()> {
    let comment = own_comment(store, project_id, id)?;
    store.set_delete_pending(&comment.id, false)
}

/// Resolves or reopens the thread the comment is part of.
pub fn set_comment_resolved(
    store: &Store,
    project_id: &str,
    id: &str,
    resolved: bool,
) -> AppResult<()> {
    let root = thread_root(store, project_id, id)?;
    store.set_comment_resolved(&root.id, resolved, &flow::now()?)
}

fn find_comment(store: &Store, project_id: &str, id: &str) -> AppResult<CommentRecord> {
    store
        .comment(project_id, id)?
        .ok_or_else(|| {
            app_error!(
                UnknownComment,
                CommentGone,
                "That comment no longer exists."
            )
        })
}

/// The first comment of the thread `id` is in.
pub fn thread_root(store: &Store, project_id: &str, id: &str) -> AppResult<CommentRecord> {
    let comment = find_comment(store, project_id, id)?;
    match &comment.parent_id {
        Some(parent) => find_comment(store, project_id, parent),
        None => Ok(comment),
    }
}

/// A comment of the user's: written here, or on the pull request by a
/// connected account.
fn own_comment(store: &Store, project_id: &str, id: &str) -> AppResult<CommentRecord> {
    let comment = find_comment(store, project_id, id)?;
    if !is_mine(&comment, &store.connected_account_ids()?) {
        return Err(app_error!(
            Internal,
            CommentNotYours,
            "Only your own comments can be changed here."
        ));
    }
    Ok(comment)
}

fn is_mine(comment: &CommentRecord, accounts: &[String]) -> bool {
    match &comment.remote_author_id {
        None => true,
        Some(author) => accounts.contains(author),
    }
}

fn clean_body(body: &str) -> AppResult<String> {
    let body = body.trim();
    if body.is_empty() {
        return Err(app_error!(
            Internal,
            CommentEmpty,
            "A comment needs some text."
        ));
    }
    if body.len() > MAX_BODY_BYTES {
        return Err(app_error!(
            Internal,
            CommentTooLong,
            "This comment is too long to keep."
        ));
    }
    Ok(body.to_string())
}

fn not_in_review(path: &str) -> AppError {
    app_error!(
        UnknownFile,
        NotAChangedFile,
        "\"{path}\" is not a changed file of this review.",
        path = escaped(path)
    )
}

fn status(mark: &MarkRecord, current: &str) -> MarkStatus {
    if mark.fingerprint == current {
        MarkStatus::Reviewed
    } else {
        MarkStatus::Reopened
    }
}

fn function_fingerprint(span: &FunctionSpan, texts: &Texts) -> String {
    if span.deleted {
        return fingerprint::ABSENT.to_string();
    }
    match texts.get(&(span.file.clone(), ReviewSide::New)) {
        Some(Some(text)) => fingerprint::of_lines(text, span.start_line, span.end_line),
        _ => fingerprint::ABSENT.to_string(),
    }
}

fn placed(comment: CommentRecord, texts: &Texts, accounts: &[String]) -> ReviewComment {
    let text = texts
        .get(&(comment.path.clone(), comment.side))
        .and_then(Option::as_deref);
    let (current_line, outdated) = match (comment.kind, text) {
        // On no line: never moved, never outdated.
        (CommentKind::General, _) => (None, false),
        (CommentKind::Line, Some(text)) => {
            let placement = place(&doc_lines(text), comment.line, &comment.line_text);
            (Some(placement.line), placement.outdated)
        }
        (CommentKind::Line, None) => (None, true),
    };
    let mine = is_mine(&comment, accounts);
    let pending_change = if comment.delete_pending {
        Some(PendingChange::Delete)
    } else if comment.edited_locally {
        Some(PendingChange::Edit)
    } else {
        None
    };
    ReviewComment {
        mine,
        pending_change,
        publish_error: comment.publish_error,
        id: comment.id,
        kind: comment.kind,
        parent_id: comment.parent_id,
        path: comment.path,
        side: comment.side,
        line: comment.line,
        line_text: comment.line_text,
        commit_sha: comment.commit_sha,
        body: comment.body,
        state: comment.state,
        remote_id: comment.remote_id,
        remote_author_id: comment.remote_author_id,
        remote_author_name: comment.remote_author_name,
        resolved: comment.resolved,
        created_at: comment.created_at,
        updated_at: comment.updated_at,
        current_line,
        outdated,
    }
}

/// Lines of `text` as the diff editor numbers them: split on `\n` (so a
/// final newline is followed by an empty last line), without a `\r` before it.
pub fn doc_lines(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Placement {
    line: u32,
    outdated: bool,
}

/// Where a comment written on `line`, which then read `line_text`, goes in
/// `lines`: on that line while it still reads the same; otherwise on the
/// nearest line within `RELOCATE_WITHIN` that reads exactly the same (blank
/// lines are too common to be followed); otherwise on its line, kept inside
/// the file. Outdated whenever its own line no longer reads the same.
fn place(lines: &[&str], line: u32, line_text: &str) -> Placement {
    let index = (line as usize).saturating_sub(1);
    if lines.get(index) == Some(&line_text) {
        return Placement {
            line,
            outdated: false,
        };
    }
    if !line_text.trim().is_empty() {
        for distance in 1..=RELOCATE_WITHIN {
            let candidates = [index.checked_sub(distance), index.checked_add(distance)];
            if let Some(found) = candidates
                .into_iter()
                .flatten()
                .find(|&candidate| lines.get(candidate) == Some(&line_text))
            {
                return Placement {
                    line: count(found + 1),
                    outdated: true,
                };
            }
        }
    }
    Placement {
        line: line.clamp(1, count(lines.len().max(1))),
        outdated: true,
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::model::{ChangeSet, ReviewTarget};
    use crate::projects::ProjectService;
    use crate::test_support::{isolated_git, TestRepo};

    fn service() -> ProjectService {
        ProjectService::new(Store::open_in_memory().unwrap(), isolated_git(), None)
    }

    fn branch(name: &str) -> ReviewTarget {
        ReviewTarget::Branch {
            branch: name.to_string(),
            base: "main".to_string(),
        }
    }

    /// The review as the screen would open it now.
    fn open(service: &ProjectService, id: &str, target: ReviewTarget) -> ReviewRef {
        let ChangeSet {
            target,
            old_sha,
            new_sha,
            ..
        } = service.changed_files(id, target).unwrap();
        ReviewRef {
            target,
            old_sha,
            new_sha,
        }
    }

    struct Fixture {
        repo: TestRepo,
        service: ProjectService,
        id: String,
    }

    impl Fixture {
        /// `feature` changes app.go and gone.txt on top of `main`.
        fn new() -> Self {
            let repo = TestRepo::init();
            repo.write("app.go", "package app\n\nfunc A() {\n\treturn\n}\n");
            repo.write("gone.txt", "bye\n");
            repo.write("same.txt", "same\n");
            repo.commit_all("initial commit");
            repo.git(&["checkout", "--quiet", "-b", "feature"]);
            repo.write(
                "app.go",
                "package app\n\nfunc A() {\n\treturn\n}\n\nfunc B() {\n\tA()\n}\n",
            );
            fs::remove_file(repo.path().join("gone.txt")).unwrap();
            repo.commit_all("add B");
            let service = service();
            let id = service.add_local(repo.path().to_str().unwrap()).unwrap().id;
            Self { repo, service, id }
        }

        fn review(&self) -> ReviewRef {
            open(&self.service, &self.id, branch("feature"))
        }

        fn session(&self, review: ReviewRef) -> ReviewSession<'_> {
            ReviewSession::new(
                self.service.store(),
                self.service.git(),
                &self.id,
                self.repo.path(),
                review,
            )
            .unwrap()
        }

        fn state(&self, functions: &[FunctionSpan]) -> ReviewState {
            self.session(self.review()).state(functions).unwrap()
        }

        fn files(&self) -> Vec<(String, MarkStatus)> {
            self.state(&[])
                .files
                .into_iter()
                .map(|file| (file.path, file.status))
                .collect()
        }
    }

    fn span(start: u32, end: u32) -> FunctionSpan {
        FunctionSpan {
            id: "example.com/app.B".to_string(),
            file: "app.go".to_string(),
            start_line: start,
            end_line: end,
            deleted: false,
        }
    }

    fn lines(text: &str) -> Vec<&str> {
        text.split('\n').collect()
    }

    #[test]
    fn a_comment_stays_on_its_line_follows_it_nearby_or_stays_put_outdated() {
        let fresh = |line| Placement {
            line,
            outdated: false,
        };
        let outdated = |line| Placement {
            line,
            outdated: true,
        };
        let file = lines("a\nb\nc\nd");
        assert_eq!(place(&file, 2, "b"), fresh(2));
        // Two lines were added above it: found below, the nearest copy wins.
        let moved = lines("x\ny\na\nb\nc\nd\nb");
        assert_eq!(place(&moved, 2, "b"), outdated(4));
        // Its text changed: kept where it was.
        assert_eq!(place(&lines("a\nB\nc"), 2, "b"), outdated(2));
        // Beyond the end of a shorter file: kept on the last line.
        assert_eq!(place(&lines("a"), 9, "zzz"), outdated(1));
        assert_eq!(place(&[], 3, "zzz"), outdated(1));
        // A blank line is not followed.
        assert_eq!(place(&lines("x\n\na\n"), 1, ""), outdated(1));
        // Too far away to be the same line.
        let mut far = vec!["-"; 60];
        far.push("b");
        assert_eq!(place(&far, 2, "b"), outdated(2));
    }

    #[test]
    fn doc_lines_number_lines_as_the_editor_does() {
        assert_eq!(doc_lines("a\r\nb\n"), vec!["a", "b", ""]);
        assert_eq!(doc_lines(""), vec![""]);
    }

    #[test]
    fn a_marked_file_is_reviewed_until_its_content_changes() {
        let fixture = Fixture::new();
        let session = fixture.session(fixture.review());

        session.set_file_reviewed("app.go", true).unwrap();
        session.set_file_reviewed("gone.txt", true).unwrap();
        assert_eq!(
            fixture.files(),
            vec![
                ("app.go".to_string(), MarkStatus::Reviewed),
                ("gone.txt".to_string(), MarkStatus::Reviewed),
            ]
        );
        assert_eq!(fixture.state(&[]).summary.reviewed_files, 2);

        fixture
            .repo
            .commit_file("app.go", "package app\n", "rewrite app");
        assert_eq!(
            fixture.files(),
            vec![
                ("app.go".to_string(), MarkStatus::Reopened),
                ("gone.txt".to_string(), MarkStatus::Reviewed),
            ]
        );
        let state = fixture.state(&[]);
        assert_eq!(
            (state.summary.reviewed_files, state.summary.reopened_files),
            (1, 1)
        );

        fixture
            .session(fixture.review())
            .set_file_reviewed("app.go", true)
            .unwrap();
        assert_eq!(fixture.files()[0].1, MarkStatus::Reviewed);
        fixture
            .session(fixture.review())
            .set_file_reviewed("app.go", false)
            .unwrap();
        assert_eq!(
            fixture.files(),
            vec![("gone.txt".to_string(), MarkStatus::Reviewed)]
        );
    }

    #[test]
    fn marks_are_judged_against_the_frozen_sides_and_kept_per_target() {
        let fixture = Fixture::new();
        let before = fixture.review();
        fixture
            .session(before.clone())
            .set_file_reviewed("app.go", true)
            .unwrap();
        fixture
            .repo
            .commit_file("app.go", "package app\n", "rewrite app");

        // The screen still shows the old head: what was reviewed is unchanged.
        let state = fixture.session(before).state(&[]).unwrap();
        assert_eq!(state.files[0].status, MarkStatus::Reviewed);

        let other_base = ReviewRef {
            target: ReviewTarget::Branch {
                branch: "feature".to_string(),
                base: "develop".to_string(),
            },
            ..fixture.review()
        };
        assert!(fixture
            .session(other_base)
            .state(&[])
            .unwrap()
            .files
            .is_empty());
    }

    #[test]
    fn only_changed_files_can_be_marked_or_commented() {
        let fixture = Fixture::new();
        let session = fixture.session(fixture.review());
        for path in ["same.txt", "missing.go", "../app.go", ""] {
            assert_eq!(
                session.set_file_reviewed(path, true).unwrap_err().code,
                ErrorCode::UnknownFile,
                "{path:?}"
            );
            assert_eq!(
                session
                    .add_comment(path, ReviewSide::New, 1, "x")
                    .unwrap_err()
                    .code,
                ErrorCode::UnknownFile,
                "{path:?}"
            );
        }
        // The new side of a deleted file has no line to comment on.
        assert_eq!(
            session
                .add_comment("gone.txt", ReviewSide::New, 1, "x")
                .unwrap_err()
                .code,
            ErrorCode::UnknownFile
        );
        for line in [0, 12] {
            assert_eq!(
                session
                    .add_comment("app.go", ReviewSide::New, line, "x")
                    .unwrap_err()
                    .code,
                ErrorCode::UnknownFile,
                "line {line}"
            );
        }
        assert!(session
            .add_comment("app.go", ReviewSide::New, 1, "  ")
            .is_err());
        // Nothing was recorded by the failures.
        assert!(fixture.state(&[]).comments.is_empty());
        assert_eq!(fixture.state(&[]).checkpoint, None);
    }

    #[test]
    fn the_working_tree_fingerprints_files_on_disk() {
        let fixture = Fixture::new();
        fixture.repo.write("app.go", "package app\n// edited\n");
        fixture.repo.write("new.txt", "untracked\n");
        let review = open(&fixture.service, &fixture.id, ReviewTarget::WorkingTree);
        let session = fixture.session(review.clone());
        session.set_file_reviewed("app.go", true).unwrap();
        session.set_file_reviewed("new.txt", true).unwrap();
        let statuses = |review: ReviewRef| -> Vec<MarkStatus> {
            fixture
                .session(review)
                .state(&[])
                .unwrap()
                .files
                .into_iter()
                .map(|file| file.status)
                .collect()
        };
        assert_eq!(
            statuses(review.clone()),
            vec![MarkStatus::Reviewed, MarkStatus::Reviewed]
        );

        fixture.repo.write("new.txt", "untracked, edited\n");
        assert_eq!(
            statuses(review.clone()),
            vec![MarkStatus::Reviewed, MarkStatus::Reopened]
        );
        let state = fixture.session(review).state(&[]).unwrap();
        assert_eq!(
            state.since,
            SinceLastReview::Unavailable {
                reason: SinceUnavailableReason::WorkingTree
            }
        );
        assert_eq!(state.checkpoint.unwrap().sha, None);
        // The branch review knows nothing of these marks.
        assert!(fixture.files().is_empty());
    }

    #[test]
    fn a_marked_function_reopens_only_when_its_own_lines_change() {
        let fixture = Fixture::new();
        fixture
            .session(fixture.review())
            .set_function_reviewed(&span(7, 9), true)
            .unwrap();
        let status = |span: FunctionSpan| fixture.state(&[span]).functions[0].status;
        assert_eq!(status(span(7, 9)), MarkStatus::Reviewed);
        // Not asked about, so not reported.
        assert!(fixture.state(&[]).functions.is_empty());

        // Lines added above move the function without changing it.
        fixture.repo.commit_file(
            "app.go",
            "package app\n\n// A returns.\nfunc A() {\n\treturn\n}\n\nfunc B() {\n\tA()\n}\n",
            "document A",
        );
        assert_eq!(status(span(8, 10)), MarkStatus::Reviewed);

        fixture.repo.commit_file(
            "app.go",
            "package app\n\n// A returns.\nfunc A() {\n\treturn\n}\n\nfunc B() {\n\tA()\n\tA()\n}\n",
            "call A twice",
        );
        assert_eq!(status(span(8, 11)), MarkStatus::Reopened);
        assert_eq!(fixture.state(&[span(8, 11)]).summary.reopened_functions, 1);

        let deleted = FunctionSpan {
            deleted: true,
            ..span(8, 11)
        };
        fixture
            .session(fixture.review())
            .set_function_reviewed(&deleted, true)
            .unwrap();
        assert_eq!(status(deleted.clone()), MarkStatus::Reviewed);
        fixture
            .session(fixture.review())
            .set_function_reviewed(&deleted, false)
            .unwrap();
        assert!(fixture.state(&[deleted]).functions.is_empty());
    }

    #[test]
    fn comments_are_anchored_moved_with_their_line_and_flagged_when_it_changes() {
        let fixture = Fixture::new();
        let session = fixture.session(fixture.review());
        let new_id = session
            .add_comment("app.go", ReviewSide::New, 8, "  Why call A here?  ")
            .unwrap();
        let old_id = session
            .add_comment("gone.txt", ReviewSide::Old, 1, "Still needed?")
            .unwrap();
        let state = fixture.state(&[]);
        let new = &state.comments[0];
        assert_eq!(
            (
                new.id.as_str(),
                new.line,
                new.line_text.as_str(),
                new.body.as_str()
            ),
            (new_id.as_str(), 8, "\tA()", "Why call A here?")
        );
        assert_eq!(new.commit_sha, fixture.review().new_sha);
        assert_eq!((new.current_line, new.outdated), (Some(8), false));
        assert_eq!(new.state, CommentState::Draft);
        assert_eq!(state.comments[1].id, old_id);
        assert_eq!(state.comments[1].side, ReviewSide::Old);
        // Comments are not review actions: no checkpoint yet.
        assert_eq!(state.checkpoint, None);

        // Two lines above it: it follows its line.
        fixture.repo.commit_file(
            "app.go",
            "package app\n\n// A returns.\n//\nfunc A() {\n\treturn\n}\n\nfunc B() {\n\tA()\n}\n",
            "document A",
        );
        let moved = &fixture.state(&[]).comments[0];
        assert_eq!((moved.current_line, moved.outdated), (Some(10), true));
        assert_eq!(fixture.state(&[]).summary.outdated_comments, 1);

        // Its line is rewritten: it stays where it was written.
        fixture.repo.commit_file(
            "app.go",
            "package app\n\nfunc A() {\n\treturn\n}\n\nfunc B() {\n\tA(1)\n}\n",
            "pass an argument",
        );
        let changed = &fixture.state(&[]).comments[0];
        assert_eq!((changed.current_line, changed.outdated), (Some(8), true));
    }

    #[test]
    fn threads_reply_resolve_edit_and_delete() {
        let fixture = Fixture::new();
        let store = fixture.service.store();
        let id = fixture.id.as_str();
        let root = fixture
            .session(fixture.review())
            .add_comment("app.go", ReviewSide::New, 8, "First")
            .unwrap();
        let reply_id = reply(store, id, &root, "Second").unwrap();
        // A reply to a reply joins the same thread.
        let nested = reply(store, id, &reply_id, "Third").unwrap();
        let state = fixture.state(&[]);
        let parents: Vec<Option<&str>> = state
            .comments
            .iter()
            .map(|comment| comment.parent_id.as_deref())
            .collect();
        assert_eq!(
            parents,
            vec![None, Some(root.as_str()), Some(root.as_str())]
        );
        assert!(state
            .comments
            .iter()
            .all(|comment| comment.current_line == Some(8) && comment.path == "app.go"));

        set_comment_resolved(store, id, &nested, true).unwrap();
        assert!(fixture.state(&[]).comments[0].resolved);
        set_comment_resolved(store, id, &root, false).unwrap();
        assert!(!fixture.state(&[]).comments[0].resolved);

        edit_comment(store, id, &reply_id, " Edited ").unwrap();
        assert_eq!(fixture.state(&[]).comments[1].body, "Edited");
        assert!(edit_comment(store, id, &reply_id, "").is_err());

        delete_comment(store, id, &nested).unwrap();
        assert_eq!(fixture.state(&[]).comments.len(), 2);
        delete_comment(store, id, &root).unwrap();
        assert!(fixture.state(&[]).comments.is_empty());

        for result in [
            edit_comment(store, id, &root, "x"),
            delete_comment(store, id, &root),
            set_comment_resolved(store, id, &root, true),
            reply(store, id, &root, "x").map(|_| ()),
            edit_comment(store, "other project", &reply_id, "x"),
        ] {
            assert_eq!(result.unwrap_err().code, ErrorCode::UnknownComment);
        }
    }

    #[test]
    fn since_the_last_review_lists_what_changed_after_the_checkpoint() {
        let fixture = Fixture::new();
        let since = || fixture.state(&[]).since;
        assert_eq!(
            since(),
            SinceLastReview::Unavailable {
                reason: SinceUnavailableReason::NoCheckpoint
            }
        );

        let reviewed_at = fixture.review().new_sha.unwrap();
        fixture
            .session(fixture.review())
            .set_file_reviewed("app.go", true)
            .unwrap();
        let state = fixture.state(&[]);
        assert_eq!(
            state.checkpoint.unwrap().sha.as_deref(),
            Some(reviewed_at.as_str())
        );
        assert_eq!(
            state.since,
            SinceLastReview::Unavailable {
                reason: SinceUnavailableReason::UpToDate
            }
        );

        fixture.repo.commit_file("app.go", "package app\n", "one");
        fixture.repo.commit_file("more.txt", "more\n", "two");
        let state = fixture.state(&[]);
        let SinceLastReview::Available {
            checkpoint_sha,
            commits,
            files,
        } = &state.since
        else {
            panic!("expected changes since the checkpoint: {:?}", state.since);
        };
        assert_eq!(checkpoint_sha, &reviewed_at);
        assert_eq!(*commits, 2);
        let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, vec!["app.go", "more.txt"]);
        assert_eq!(state.summary.files_changed_since, 2);
        // Those files diff from the checkpoint to the head.
        let versions = fixture
            .service
            .file_versions(
                &fixture.id,
                checkpoint_sha,
                fixture.review().new_sha.as_deref(),
                "app.go",
                None,
            )
            .unwrap();
        assert_eq!(versions.new.text.as_deref(), Some("package app\n"));

        // Unmarking is not a review action: the checkpoint stays.
        fixture
            .session(fixture.review())
            .set_file_reviewed("app.go", false)
            .unwrap();
        assert!(matches!(since(), SinceLastReview::Available { .. }));

        let record = fixture
            .service
            .store()
            .review(&fixture.id, &branch("feature"))
            .unwrap()
            .unwrap();
        fixture
            .service
            .store()
            .set_checkpoint(
                &record.id,
                Some("0123456789abcdef0123456789abcdef01234567"),
                "t",
            )
            .unwrap();
        assert_eq!(
            since(),
            SinceLastReview::Unavailable {
                reason: SinceUnavailableReason::CheckpointMissing
            }
        );
    }

    #[test]
    fn a_side_that_is_not_a_commit_is_refused() {
        let fixture = Fixture::new();
        let review = ReviewRef {
            new_sha: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            ..fixture.review()
        };
        let error = ReviewSession::new(
            fixture.service.store(),
            fixture.service.git(),
            &fixture.id,
            fixture.repo.path(),
            review,
        )
        .err()
        .unwrap();
        assert_eq!(error.code, ErrorCode::UnknownBranch);
    }

    #[test]
    fn the_state_has_the_shape_of_the_contract() {
        let fixture = Fixture::new();
        let state = fixture.state(&[]);
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(
            json["since"],
            serde_json::json!({ "kind": "unavailable", "reason": "no_checkpoint" })
        );
        assert_eq!(json["summary"]["totalFiles"], 2);
        let available = SinceLastReview::Available {
            checkpoint_sha: "abc".to_string(),
            commits: 1,
            files: Vec::new(),
        };
        assert_eq!(
            serde_json::to_value(available).unwrap(),
            serde_json::json!({ "kind": "available", "checkpointSha": "abc", "commits": 1, "files": [] })
        );
        let review: ReviewRef = serde_json::from_value(serde_json::json!({
            "target": { "kind": "working_tree" }, "oldSha": "abc", "newSha": null
        }))
        .unwrap();
        assert_eq!(review.target, ReviewTarget::WorkingTree);
        let span: FunctionSpan = serde_json::from_value(serde_json::json!({
            "id": "f", "file": "a.go", "startLine": 1, "endLine": 2, "deleted": false
        }))
        .unwrap();
        assert_eq!(span.end_line, 2);
    }
}
