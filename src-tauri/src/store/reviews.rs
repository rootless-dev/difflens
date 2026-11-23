//! Review state: one row per project and review target, the files and
//! functions marked as reviewed in it, and its comments (on lines, or on the
//! pull request as a whole). Only rows; what
//! they mean against the code is worked out in `review_state`.

use rusqlite::{params, OptionalExtension, Row};
use uuid::Uuid;

use super::Store;
use crate::error::{app_error, AppError, AppResult, ErrorCode};
use crate::model::{CommentKind, CommentState, ReviewSide, ReviewTarget};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRecord {
    pub id: String,
    /// Head the most recent review action was taken at; `None` before the
    /// first one, and for the working tree, which has no commit of its own.
    pub checkpoint_sha: Option<String>,
    pub checkpoint_at: Option<String>,
}

/// What a mark is about: a file, keyed by path, or a function, keyed by its
/// analyzer id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    File,
    Function,
}

impl MarkKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Function => "function",
        }
    }
}

/// Something marked as reviewed, with the fingerprint of its content then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkRecord {
    pub key: String,
    pub fingerprint: String,
    pub reviewed_at: String,
}

/// A comment on a line, or a general one on the pull request as a whole
/// (`kind`; no file or line: an empty path and line text, line 0). A reply
/// has a parent and repeats its kind and anchor, so every comment can be
/// placed on its own. The remote fields stay empty until a comment is
/// published to a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentRecord {
    pub id: String,
    pub review_id: String,
    pub kind: CommentKind,
    pub parent_id: Option<String>,
    pub path: String,
    pub side: ReviewSide,
    pub line: u32,
    /// The text of the line when the comment was written, to find it again.
    pub line_text: String,
    /// Head when the comment was written; `None` for the working tree.
    pub commit_sha: Option<String>,
    pub body: String,
    pub state: CommentState,
    pub remote_id: Option<String>,
    pub remote_author_id: Option<String>,
    pub remote_author_name: Option<String>,
    pub resolved: bool,
    pub created_at: String,
    pub updated_at: String,
    /// The pull request a published comment is on, by number.
    pub remote_pull_request: Option<String>,
    /// A published comment's text was changed here and not published yet.
    pub edited_locally: bool,
    /// A published comment was deleted here; the pull request still has it.
    pub delete_pending: bool,
    /// Why the last attempt to publish it failed.
    pub publish_error: Option<AppError>,
}

/// Where a comment now is on the pull request, once published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedAs {
    pub remote_id: String,
    pub author_id: String,
    pub author_name: String,
    pub pull_request: String,
}

/// What a pull of the pull request's comments changes in a comment that is
/// already here. `body` and `resolved` are `None` to keep the local value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulledUpdate {
    pub id: String,
    pub body: Option<String>,
    pub author_id: String,
    pub author_name: String,
    pub resolved: Option<bool>,
    pub updated_at: String,
}

const COMMENT_COLUMNS: &str =
    "c.id, c.parent_id, c.path, c.side, c.line, c.line_text, c.commit_sha, \
     c.body, c.state, c.remote_id, c.remote_author_id, c.remote_author_name, c.resolved, \
     c.created_at, c.updated_at, c.review_id, c.remote_pull_request, c.edited_locally, \
     c.delete_pending, c.publish_error, c.publish_error_code, c.publish_error_reason, \
     c.publish_error_params, c.publish_error_detail, c.kind";

/// The three key columns of a target.
fn target_key(target: &ReviewTarget) -> (&'static str, &str, &str) {
    match target {
        ReviewTarget::Branch { branch, base } => ("branch", branch.as_str(), base.as_str()),
        ReviewTarget::WorkingTree => ("working_tree", "", ""),
    }
}

impl Store {
    /// The review of `target` in a project, `None` until something is
    /// recorded in it.
    pub fn review(
        &self,
        project_id: &str,
        target: &ReviewTarget,
    ) -> AppResult<Option<ReviewRecord>> {
        let (kind, branch, base) = target_key(target);
        let record = self
            .lock()?
            .query_row(
                "SELECT id, checkpoint_sha, checkpoint_at FROM reviews
                 WHERE project_id = ?1 AND target_kind = ?2 AND branch = ?3 AND base = ?4",
                params![project_id, kind, branch, base],
                read_review,
            )
            .optional()?;
        Ok(record)
    }

    /// The review of `target`, created when there is none yet.
    pub fn open_review(
        &self,
        project_id: &str,
        target: &ReviewTarget,
        now: &str,
    ) -> AppResult<ReviewRecord> {
        let (kind, branch, base) = target_key(target);
        self.lock()?.execute(
            "INSERT INTO reviews (id, project_id, target_kind, branch, base, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (project_id, target_kind, branch, base) DO NOTHING",
            params![
                Uuid::new_v4().to_string(),
                project_id,
                kind,
                branch,
                base,
                now
            ],
        )?;
        self.review(project_id, target)?
            .ok_or_else(|| {
                app_error!(
                    Internal,
                    ReviewNotRecorded,
                    "The review could not be recorded."
                )
            })
    }

    pub fn set_checkpoint(&self, review_id: &str, sha: Option<&str>, at: &str) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE reviews SET checkpoint_sha = ?2, checkpoint_at = ?3 WHERE id = ?1",
            params![review_id, sha, at],
        )?;
        Ok(())
    }

    pub fn marks(&self, review_id: &str, kind: MarkKind) -> AppResult<Vec<MarkRecord>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT key, fingerprint, reviewed_at FROM review_marks
             WHERE review_id = ?1 AND kind = ?2 ORDER BY key",
        )?;
        let marks = statement
            .query_map(params![review_id, kind.as_str()], |row| {
                Ok(MarkRecord {
                    key: row.get(0)?,
                    fingerprint: row.get(1)?,
                    reviewed_at: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(marks)
    }

    /// Marks `key` as reviewed, replacing an earlier mark.
    pub fn mark(&self, review_id: &str, kind: MarkKind, mark: &MarkRecord) -> AppResult<()> {
        self.lock()?.execute(
            "INSERT INTO review_marks (review_id, kind, key, fingerprint, reviewed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (review_id, kind, key) DO UPDATE SET
                 fingerprint = excluded.fingerprint, reviewed_at = excluded.reviewed_at",
            params![
                review_id,
                kind.as_str(),
                mark.key,
                mark.fingerprint,
                mark.reviewed_at
            ],
        )?;
        Ok(())
    }

    pub fn unmark(&self, review_id: &str, kind: MarkKind, key: &str) -> AppResult<()> {
        self.lock()?.execute(
            "DELETE FROM review_marks WHERE review_id = ?1 AND kind = ?2 AND key = ?3",
            params![review_id, kind.as_str(), key],
        )?;
        Ok(())
    }

    /// Every comment of a review, oldest first, replies included.
    pub fn comments(&self, review_id: &str) -> AppResult<Vec<CommentRecord>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(&format!(
            "SELECT {COMMENT_COLUMNS} FROM review_comments c
             WHERE c.review_id = ?1 ORDER BY c.created_at, c.rowid"
        ))?;
        let comments = statement
            .query_map(params![review_id], read_comment)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(comments)
    }

    /// A comment of any review of the project, `None` when the project has
    /// no comment with that id.
    pub fn comment(&self, project_id: &str, id: &str) -> AppResult<Option<CommentRecord>> {
        let record = self
            .lock()?
            .query_row(
                &format!(
                    "SELECT {COMMENT_COLUMNS} FROM review_comments c
                     JOIN reviews r ON r.id = c.review_id
                     WHERE r.project_id = ?1 AND c.id = ?2"
                ),
                params![project_id, id],
                read_comment,
            )
            .optional()?;
        Ok(record)
    }

    pub fn insert_comment(&self, comment: &CommentRecord) -> AppResult<()> {
        let error = comment.publish_error.as_ref();
        self.lock()?.execute(
            "INSERT INTO review_comments (id, review_id, parent_id, path, side, line, line_text,
                 commit_sha, body, state, remote_id, remote_author_id, remote_author_name,
                 resolved, created_at, updated_at, remote_pull_request, edited_locally,
                 delete_pending, publish_error, publish_error_code, publish_error_reason,
                 publish_error_params, publish_error_detail, kind)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                 ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
            params![
                comment.id,
                comment.review_id,
                comment.parent_id,
                comment.path,
                side_text(comment.side),
                comment.line,
                comment.line_text,
                comment.commit_sha,
                comment.body,
                state_text(comment.state),
                comment.remote_id,
                comment.remote_author_id,
                comment.remote_author_name,
                comment.resolved,
                comment.created_at,
                comment.updated_at,
                comment.remote_pull_request,
                comment.edited_locally,
                comment.delete_pending,
                error.map(|error| error.message.as_str()),
                error.map(|error| error.code.key()),
                error.and_then(|error| error.reason).map(|reason| reason.key()),
                error.and_then(AppError::params_json),
                error.and_then(|error| error.detail.as_deref()),
                kind_text(comment.kind),
            ],
        )?;
        Ok(())
    }

    /// Sets a comment's text. `edited_locally` marks a published comment
    /// whose new text the pull request has not received yet.
    pub fn set_comment_body(
        &self,
        id: &str,
        body: &str,
        edited_locally: bool,
        at: &str,
    ) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE review_comments SET body = ?2, edited_locally = ?3, updated_at = ?4
             WHERE id = ?1",
            params![id, body, edited_locally, at],
        )?;
        Ok(())
    }

    /// Marks a published comment as deleted here, to delete it from the pull
    /// request on the next publish; `false` takes that back.
    pub fn set_delete_pending(&self, id: &str, pending: bool) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE review_comments SET delete_pending = ?2 WHERE id = ?1",
            params![id, pending],
        )?;
        Ok(())
    }

    /// Records that the pull request now has the comment as it is here.
    pub fn mark_published(&self, id: &str, remote: &PublishedAs, at: &str) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE review_comments SET state = 'published', remote_id = ?2,
                 remote_author_id = ?3, remote_author_name = ?4, remote_pull_request = ?5,
                 edited_locally = 0, publish_error = NULL, publish_error_code = NULL,
                 publish_error_reason = NULL, publish_error_params = NULL,
                 publish_error_detail = NULL, updated_at = ?6
             WHERE id = ?1",
            params![
                id,
                remote.remote_id,
                remote.author_id,
                remote.author_name,
                remote.pull_request,
                at
            ],
        )?;
        Ok(())
    }

    /// Keeps why publishing the comment failed: its English message (what a
    /// row written before errors had reasons holds too) and what words it.
    pub fn set_publish_error(&self, id: &str, error: Option<&AppError>) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE review_comments SET publish_error = ?2, publish_error_code = ?3,
                 publish_error_reason = ?4, publish_error_params = ?5, publish_error_detail = ?6
             WHERE id = ?1",
            params![
                id,
                error.map(|error| error.message.as_str()),
                error.map(|error| error.code.key()),
                error.and_then(|error| error.reason).map(|reason| reason.key()),
                error.and_then(AppError::params_json),
                error.and_then(|error| error.detail.as_deref()),
            ],
        )?;
        Ok(())
    }

    /// Brings a comment in line with the pull request after a pull.
    pub fn apply_pulled_update(&self, update: &PulledUpdate) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE review_comments SET body = COALESCE(?2, body), remote_author_id = ?3,
                 remote_author_name = ?4, resolved = COALESCE(?5, resolved), updated_at = ?6
             WHERE id = ?1",
            params![
                update.id,
                update.body,
                update.author_id,
                update.author_name,
                update.resolved,
                update.updated_at
            ],
        )?;
        Ok(())
    }

    pub fn set_comment_resolved(&self, id: &str, resolved: bool, at: &str) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE review_comments SET resolved = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, resolved, at],
        )?;
        Ok(())
    }

    /// Deletes a comment and its replies.
    pub fn delete_comment(&self, id: &str) -> AppResult<()> {
        self.lock()?
            .execute("DELETE FROM review_comments WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// The comments written from a signal in a review, by signal key. A
    /// deleted comment takes its link with it.
    pub fn signal_comments(&self, review_id: &str) -> AppResult<Vec<(String, String)>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT signal, comment_id FROM signal_comments WHERE review_id = ?1 ORDER BY signal",
        )?;
        let links = statement
            .query_map(params![review_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(links)
    }

    /// The comment written from the signal `signal`, while it is there.
    pub fn signal_comment(&self, review_id: &str, signal: &str) -> AppResult<Option<String>> {
        let comment = self
            .lock()?
            .query_row(
                "SELECT comment_id FROM signal_comments WHERE review_id = ?1 AND signal = ?2",
                params![review_id, signal],
                |row| row.get(0),
            )
            .optional()?;
        Ok(comment)
    }

    /// Links the review comment `comment_id` to the signal `signal`,
    /// replacing an earlier link.
    pub fn link_signal_comment(&self, review_id: &str, signal: &str, comment_id: &str) -> AppResult<()> {
        self.lock()?.execute(
            "INSERT INTO signal_comments (review_id, signal, comment_id) VALUES (?1, ?2, ?3)
             ON CONFLICT (review_id, signal) DO UPDATE SET comment_id = excluded.comment_id",
            params![review_id, signal, comment_id],
        )?;
        Ok(())
    }
}

fn read_review(row: &Row<'_>) -> rusqlite::Result<ReviewRecord> {
    Ok(ReviewRecord {
        id: row.get(0)?,
        checkpoint_sha: row.get(1)?,
        checkpoint_at: row.get(2)?,
    })
}

fn read_comment(row: &Row<'_>) -> rusqlite::Result<CommentRecord> {
    let side: String = row.get(3)?;
    let state: String = row.get(8)?;
    let kind: String = row.get(24)?;
    Ok(CommentRecord {
        id: row.get(0)?,
        // A kind a later version wrote reads as a line comment, as every
        // comment was before general ones.
        kind: if kind == "general" {
            CommentKind::General
        } else {
            CommentKind::Line
        },
        parent_id: row.get(1)?,
        path: row.get(2)?,
        side: if side == "old" {
            ReviewSide::Old
        } else {
            ReviewSide::New
        },
        line: row.get(4)?,
        line_text: row.get(5)?,
        commit_sha: row.get(6)?,
        body: row.get(7)?,
        state: if state == "published" {
            CommentState::Published
        } else {
            CommentState::Draft
        },
        remote_id: row.get(9)?,
        remote_author_id: row.get(10)?,
        remote_author_name: row.get(11)?,
        resolved: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        review_id: row.get(15)?,
        remote_pull_request: row.get(16)?,
        edited_locally: row.get(17)?,
        delete_pending: row.get(18)?,
        publish_error: row.get::<_, Option<String>>(19)?.map(|message| {
            Ok::<_, rusqlite::Error>(AppError::restore(
                row.get::<_, Option<String>>(20)?
                    .as_deref()
                    .and_then(ErrorCode::from_key)
                    .unwrap_or(ErrorCode::Internal),
                message,
                row.get::<_, Option<String>>(21)?.as_deref(),
                row.get::<_, Option<String>>(22)?.as_deref(),
                row.get(23)?,
            ))
        })
        .transpose()?,
    })
}

fn side_text(side: ReviewSide) -> &'static str {
    match side {
        ReviewSide::Old => "old",
        ReviewSide::New => "new",
    }
}

fn kind_text(kind: CommentKind) -> &'static str {
    match kind {
        CommentKind::Line => "line",
        CommentKind::General => "general",
    }
}

fn state_text(state: CommentState) -> &'static str {
    match state {
        CommentState::Draft => "draft",
        CommentState::Published => "published",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ProjectRecord;

    fn store_with_project(id: &str) -> Store {
        let store = Store::open_in_memory().unwrap();
        store
            .insert(&ProjectRecord {
                id: id.to_string(),
                path: format!("/repos/{id}"),
                integration_branches: vec!["main".to_string()],
                last_fetched_at: None,
                added_at: "2026-01-01T00:00:00Z".to_string(),
            })
            .unwrap();
        store
    }

    fn branch(name: &str, base: &str) -> ReviewTarget {
        ReviewTarget::Branch {
            branch: name.to_string(),
            base: base.to_string(),
        }
    }

    fn comment(review_id: &str, id: &str, parent: Option<&str>, created_at: &str) -> CommentRecord {
        CommentRecord {
            id: id.to_string(),
            review_id: review_id.to_string(),
            kind: CommentKind::Line,
            parent_id: parent.map(str::to_string),
            path: "src/app.go".to_string(),
            side: ReviewSide::New,
            line: 3,
            line_text: "func A() {}".to_string(),
            commit_sha: Some("abc".to_string()),
            body: format!("body of {id}"),
            state: CommentState::Draft,
            remote_id: None,
            remote_author_id: None,
            remote_author_name: None,
            resolved: false,
            created_at: created_at.to_string(),
            updated_at: created_at.to_string(),
            remote_pull_request: None,
            edited_locally: false,
            delete_pending: false,
            publish_error: None,
        }
    }

    #[test]
    fn a_review_is_one_per_project_and_target() {
        let store = store_with_project("p");
        let target = branch("feature/x", "main");
        assert_eq!(store.review("p", &target).unwrap(), None);

        let first = store.open_review("p", &target, "t1").unwrap();
        let again = store.open_review("p", &target, "t2").unwrap();
        let other_base = store
            .open_review("p", &branch("feature/x", "develop"), "t3")
            .unwrap();
        let working = store
            .open_review("p", &ReviewTarget::WorkingTree, "t4")
            .unwrap();

        assert_eq!(first, again);
        assert_ne!(first.id, other_base.id);
        assert_ne!(first.id, working.id);
        assert_eq!(
            store
                .open_review("p", &ReviewTarget::WorkingTree, "t5")
                .unwrap(),
            working
        );
        assert_eq!(first.checkpoint_sha, None);
    }

    #[test]
    fn records_the_checkpoint() {
        let store = store_with_project("p");
        let review = store
            .open_review("p", &branch("feature/x", "main"), "t1")
            .unwrap();

        store
            .set_checkpoint(&review.id, Some("abc"), "2026-02-01T00:00:00Z")
            .unwrap();

        let stored = store.review("p", &branch("feature/x", "main")).unwrap();
        assert_eq!(
            stored.map(|review| (review.checkpoint_sha, review.checkpoint_at)),
            Some((
                Some("abc".to_string()),
                Some("2026-02-01T00:00:00Z".to_string())
            ))
        );
    }

    #[test]
    fn marks_replace_and_remove_per_kind() {
        let store = store_with_project("p");
        let review = store
            .open_review("p", &branch("feature/x", "main"), "t1")
            .unwrap();
        let mark = |key: &str, fingerprint: &str| MarkRecord {
            key: key.to_string(),
            fingerprint: fingerprint.to_string(),
            reviewed_at: "t".to_string(),
        };

        store
            .mark(&review.id, MarkKind::File, &mark("a.go", "one"))
            .unwrap();
        store
            .mark(&review.id, MarkKind::File, &mark("a.go", "two"))
            .unwrap();
        store
            .mark(&review.id, MarkKind::Function, &mark("a.go", "fn"))
            .unwrap();

        assert_eq!(
            store.marks(&review.id, MarkKind::File).unwrap(),
            vec![mark("a.go", "two")]
        );
        store.unmark(&review.id, MarkKind::File, "a.go").unwrap();
        assert!(store.marks(&review.id, MarkKind::File).unwrap().is_empty());
        assert_eq!(
            store.marks(&review.id, MarkKind::Function).unwrap().len(),
            1
        );
    }

    #[test]
    fn comments_round_trip_and_go_with_their_parent_and_project() {
        let store = store_with_project("p");
        let review = store
            .open_review("p", &branch("feature/x", "main"), "t1")
            .unwrap();
        let mut root = comment(&review.id, "root", None, "2026-02-01T00:00:00Z");
        root.side = ReviewSide::Old;
        root.state = CommentState::Published;
        root.remote_id = Some("r-1".to_string());
        root.remote_author_name = Some("Dana".to_string());
        store.insert_comment(&root).unwrap();
        let reply = comment(&review.id, "reply", Some("root"), "2026-02-01T00:00:01Z");
        store.insert_comment(&reply).unwrap();

        assert_eq!(
            store.comments(&review.id).unwrap(),
            vec![root.clone(), reply.clone()]
        );
        assert_eq!(store.comment("p", "reply").unwrap(), Some(reply));
        assert_eq!(store.comment("other", "reply").unwrap(), None);

        store
            .set_comment_body("root", "edited", true, "2026-02-02T00:00:00Z")
            .unwrap();
        store
            .set_comment_resolved("root", true, "2026-02-03T00:00:00Z")
            .unwrap();
        let stored = store.comment("p", "root").unwrap().unwrap();
        assert_eq!(
            (
                stored.body.as_str(),
                stored.edited_locally,
                stored.resolved,
                stored.updated_at.as_str()
            ),
            ("edited", true, true, "2026-02-03T00:00:00Z")
        );

        store.delete_comment("root").unwrap();
        assert!(store.comments(&review.id).unwrap().is_empty());

        store.insert_comment(&root).unwrap();
        store.delete("p").unwrap();
        assert_eq!(
            store.review("p", &branch("feature/x", "main")).unwrap(),
            None
        );
        assert_eq!(store.comment("p", "root").unwrap(), None);
    }

    #[test]
    fn a_publish_error_keeps_its_code_reason_params_and_detail() {
        let store = store_with_project("p");
        let review = store
            .open_review("p", &branch("feature/x", "main"), "t1")
            .unwrap();
        store
            .insert_comment(&comment(&review.id, "draft", None, "t1"))
            .unwrap();
        let refused = app_error!(
            RemoteFailed,
            HostRefusedSaid,
            "{host} refused this: {detail}",
            host = "Bitbucket",
            detail = "bad anchor"
        );
        store.set_publish_error("draft", Some(&refused)).unwrap();
        assert_eq!(
            store.comment("p", "draft").unwrap().unwrap().publish_error,
            Some(refused.clone())
        );

        // A comment inserted with an error keeps it the same way.
        let mut copy = comment(&review.id, "copy", None, "t1");
        copy.publish_error = Some(refused.clone());
        store.insert_comment(&copy).unwrap();
        assert_eq!(
            store.comment("p", "copy").unwrap().unwrap().publish_error,
            Some(refused)
        );

        store.set_publish_error("draft", None).unwrap();
        assert_eq!(
            store.comment("p", "draft").unwrap().unwrap().publish_error,
            None
        );
    }

    #[test]
    fn a_publish_error_stored_before_errors_had_reasons_reads_back_by_its_message() {
        let store = store_with_project("p");
        let review = store
            .open_review("p", &branch("feature/x", "main"), "t1")
            .unwrap();
        store
            .insert_comment(&comment(&review.id, "draft", None, "t1"))
            .unwrap();
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE review_comments SET publish_error = 'Bitbucket said no' WHERE id = 'draft'",
                [],
            )
            .unwrap();
        let error = store
            .comment("p", "draft")
            .unwrap()
            .unwrap()
            .publish_error
            .unwrap();
        assert_eq!(error.message, "Bitbucket said no");
        assert_eq!(error.reason, None);
        assert!(error.params.is_empty() && error.detail.is_none());
    }

    #[test]
    fn publishing_and_pulling_keep_one_row_per_remote_comment() {
        let store = store_with_project("p");
        let review = store
            .open_review("p", &branch("feature/x", "main"), "t1")
            .unwrap();
        let draft = comment(&review.id, "draft", None, "t1");
        store.insert_comment(&draft).unwrap();
        store
            .set_publish_error(
                "draft",
                Some(&AppError::untranslated(
                    ErrorCode::RemoteFailed,
                    "Bitbucket said no",
                )),
            )
            .unwrap();

        let published = PublishedAs {
            remote_id: "101".to_string(),
            author_id: "{me}".to_string(),
            author_name: "Me".to_string(),
            pull_request: "318".to_string(),
        };
        store.mark_published("draft", &published, "t2").unwrap();
        let stored = store.comment("p", "draft").unwrap().unwrap();
        assert_eq!(stored.state, CommentState::Published);
        assert_eq!(stored.remote_id.as_deref(), Some("101"));
        assert_eq!(stored.remote_pull_request.as_deref(), Some("318"));
        assert_eq!(stored.publish_error, None);

        // A second row with the same remote id in the review is refused.
        let mut twin = comment(&review.id, "twin", None, "t3");
        twin.remote_id = Some("101".to_string());
        assert!(store.insert_comment(&twin).is_err());

        // A pull updates it in place; `None` keeps the local value.
        store
            .apply_pulled_update(&PulledUpdate {
                id: "draft".to_string(),
                body: None,
                author_id: "{me}".to_string(),
                author_name: "Me Renamed".to_string(),
                resolved: Some(true),
                updated_at: "t4".to_string(),
            })
            .unwrap();
        let stored = store.comment("p", "draft").unwrap().unwrap();
        assert_eq!(
            (
                stored.body.as_str(),
                stored.remote_author_name.as_deref(),
                stored.resolved
            ),
            ("body of draft", Some("Me Renamed"), true)
        );

        store.set_delete_pending("draft", true).unwrap();
        assert!(store.comment("p", "draft").unwrap().unwrap().delete_pending);
        store.set_delete_pending("draft", false).unwrap();
        assert!(!store.comment("p", "draft").unwrap().unwrap().delete_pending);
    }
}
