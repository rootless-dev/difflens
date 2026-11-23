//! "Publish comments": when it may send, and what it sends.
//!
//! A comment's line number is only right on the pull request when the review
//! shows exactly the pull request's diff: the same head commit, compared
//! with the same base. Anything else would land comments on other lines, so
//! publishing waits until the two match. A draft whose own line no longer
//! reads as it did is not sent either: its number may point elsewhere now.
//! A general comment, on the pull request as a whole, has no line to move:
//! it goes with the others, in the same batch.

use std::collections::HashMap;

use super::host::Anchor;
use crate::error::{app_error, AppError};
use crate::model::{
    CommentKind, CommentState, PendingChange, PublishReadiness, PullRequest, PullRequestState,
    ReviewComment,
};

/// How the review's head relates to the pull request's source commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadRelation {
    Same,
    /// The local branch has commits the pull request does not.
    LocalAhead,
    /// The pull request has commits the local branch does not.
    LocalBehind,
    Diverged,
    /// The pull request's commit is not in this repository.
    Missing,
}

/// How the review's old side relates to the pull request's destination
/// commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseRelation {
    /// The old side is where the pull request's source left its destination
    /// commit.
    Same,
    /// The destination commit is in this repository but the review was read
    /// against another one: the pull request moved since.
    Moved,
    /// The destination commit is not in this repository (or the host gave
    /// none): only a fetch brings it.
    Missing,
}

/// What is known about the review against its pull request.
pub struct Facts<'a> {
    /// The base the review compares with.
    pub base: &'a str,
    pub pull_request: &'a PullRequest,
    pub head: HeadRelation,
    pub base_relation: BaseRelation,
}

pub fn readiness(facts: &Facts<'_>) -> PublishReadiness {
    let pull_request = facts.pull_request;
    let destination = pull_request.destination_branch.as_str();
    let reason = if pull_request.state != PullRequestState::Open {
        Some(not_open())
    } else if facts.base != destination {
        Some(app_error!(
            PublishBlocked,
            PublishBaseMismatch,
            "This review compares with {base}, but the pull request merges into {destination}: \
             compare with {destination} to publish.",
            base = facts.base,
            destination = destination
        ))
    } else if let Some(reason) = head_reason(facts.head) {
        Some(reason)
    } else {
        match facts.base_relation {
            BaseRelation::Same => None,
            BaseRelation::Missing => Some(app_error!(
                PublishBlocked,
                PublishDestinationMissing,
                "The pull request compares with a {destination} this repository doesn't have \
                 yet: fetch, then Reload.",
                destination = destination
            )),
            BaseRelation::Moved => Some(app_error!(
                PublishBlocked,
                PublishMergeBaseDiffers,
                "The pull request's {destination} moved since this review was read: Reload.",
                destination = destination
            )),
        }
    };
    allowed_unless(reason)
}

/// Whether Approve and Request changes may be used: the verdict is on the
/// pull request's latest commit, so it must be the commit the user reviewed.
/// Line numbers play no part, so the base is not checked.
pub fn verdict_readiness(pull_request: &PullRequest, head: HeadRelation) -> PublishReadiness {
    let reason = if pull_request.state != PullRequestState::Open {
        Some(not_open())
    } else {
        head_reason(head)
    };
    allowed_unless(reason)
}

fn not_open() -> AppError {
    app_error!(
        PublishBlocked,
        PullRequestNotOpen,
        "The pull request is no longer open."
    )
}

fn head_reason(head: HeadRelation) -> Option<AppError> {
    match head {
        HeadRelation::Same => None,
        HeadRelation::LocalAhead => Some(app_error!(
            PublishBlocked,
            PublishLocalAhead,
            "Your local branch has commits the pull request doesn't have yet: push first, then \
             Reload."
        )),
        HeadRelation::LocalBehind => Some(app_error!(
            PublishBlocked,
            PublishLocalBehind,
            "The pull request has commits your local branch doesn't have: pull them, then Reload."
        )),
        HeadRelation::Diverged => Some(app_error!(
            PublishBlocked,
            PublishDiverged,
            "Your local branch and the pull request have diverged: push or pull until they match, \
             then Reload."
        )),
        HeadRelation::Missing => Some(app_error!(
            PublishBlocked,
            PublishCommitMissing,
            "The pull request's latest commit is not in this repository: fetch, then Reload."
        )),
    }
}

fn allowed_unless(reason: Option<AppError>) -> PublishReadiness {
    PublishReadiness {
        allowed: reason.is_none(),
        reason: reason.map(Box::new),
    }
}

/// Where a new comment goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Line(Anchor),
    /// On the pull request as a whole.
    General,
    /// Into a thread already on the pull request, by its remote id.
    Thread(String),
    /// Into a thread whose first comment this same publish creates, by its
    /// local id.
    NewThread(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Create {
        comment_id: String,
        body: String,
        target: Target,
        /// The thread was resolved here before it was ever published.
        resolve: bool,
    },
    Edit {
        comment_id: String,
        remote_id: String,
        body: String,
    },
    Delete {
        comment_id: String,
        remote_id: String,
    },
}

impl Operation {
    pub fn comment_id(&self) -> &str {
        match self {
            Self::Create { comment_id, .. }
            | Self::Edit { comment_id, .. }
            | Self::Delete { comment_id, .. } => comment_id,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Plan {
    /// New comments first (a thread's first comment before its replies),
    /// then edits, then deletes, so a reply never goes to a thread already
    /// deleted by the same publish.
    pub operations: Vec<Operation>,
    /// First comments of draft threads left alone: their line changed.
    pub stale: Vec<String>,
}

/// What publishing would do with the review's comments (oldest first, as
/// `ReviewSession::state` gives them).
pub fn plan(comments: &[ReviewComment]) -> Plan {
    let mut replies: HashMap<&str, Vec<&ReviewComment>> = HashMap::new();
    for comment in comments {
        if let Some(parent) = &comment.parent_id {
            replies.entry(parent.as_str()).or_default().push(comment);
        }
    }
    let mut plan = Plan::default();
    let mut edits = Vec::new();
    let mut deletes = Vec::new();
    let mut changes = |comment: &ReviewComment| {
        let (Some(remote_id), true) = (&comment.remote_id, comment.mine) else {
            return;
        };
        match comment.pending_change {
            Some(PendingChange::Delete) => deletes.push(Operation::Delete {
                comment_id: comment.id.clone(),
                remote_id: remote_id.clone(),
            }),
            Some(PendingChange::Edit) => edits.push(Operation::Edit {
                comment_id: comment.id.clone(),
                remote_id: remote_id.clone(),
                body: comment.body.clone(),
            }),
            None => {}
        }
    };
    for root in comments
        .iter()
        .filter(|comment| comment.parent_id.is_none())
    {
        let thread_replies = replies.remove(root.id.as_str()).unwrap_or_default();
        let thread = match (root.state, &root.remote_id) {
            (CommentState::Draft, _) => {
                let target = match root.kind {
                    CommentKind::General => Target::General,
                    CommentKind::Line => {
                        if root.outdated || root.current_line.is_none() {
                            plan.stale.push(root.id.clone());
                            continue;
                        }
                        Target::Line(Anchor {
                            path: root.path.clone(),
                            side: root.side,
                            line: root.line,
                        })
                    }
                };
                plan.operations.push(Operation::Create {
                    comment_id: root.id.clone(),
                    body: root.body.clone(),
                    target,
                    resolve: root.resolved,
                });
                Target::NewThread(root.id.clone())
            }
            (CommentState::Published, Some(remote_id)) => {
                changes(root);
                Target::Thread(remote_id.clone())
            }
            (CommentState::Published, None) => continue,
        };
        for reply in thread_replies {
            match reply.state {
                CommentState::Draft => plan.operations.push(Operation::Create {
                    comment_id: reply.id.clone(),
                    body: reply.body.clone(),
                    target: thread.clone(),
                    resolve: false,
                }),
                CommentState::Published => changes(reply),
            }
        }
    }
    plan.operations.extend(edits);
    plan.operations.extend(deletes);
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ReviewSide;

    fn pull_request(destination: &str, state: PullRequestState) -> PullRequest {
        PullRequest {
            number: 318,
            title: "Adds the refund flow".to_string(),
            description: String::new(),
            state,
            url: String::new(),
            author: None,
            source_branch: "feat/refund-flow".to_string(),
            source_commit: Some("e7f3b10c9d8e".to_string()),
            destination_branch: destination.to_string(),
            destination_commit: Some("0123456789ab".to_string()),
            participants: Vec::new(),
        }
    }

    #[test]
    fn a_verdict_needs_an_open_pull_request_at_the_reviewed_commit() {
        let open = pull_request("main", PullRequestState::Open);
        assert_eq!(
            verdict_readiness(&open, HeadRelation::Same),
            PublishReadiness {
                allowed: true,
                reason: None
            }
        );
        for (head, expected) in [
            (HeadRelation::LocalAhead, "push first, then Reload"),
            (HeadRelation::LocalBehind, "pull them, then Reload"),
            (HeadRelation::Diverged, "have diverged"),
            (HeadRelation::Missing, "fetch, then Reload"),
        ] {
            let readiness = verdict_readiness(&open, head);
            assert!(!readiness.allowed);
            assert!(
                readiness.reason.unwrap().message.contains(expected),
                "{head:?}"
            );
        }
        let declined = pull_request("main", PullRequestState::Declined);
        assert_eq!(
            verdict_readiness(&declined, HeadRelation::Same)
                .reason
                .map(|reason| reason.message)
                .as_deref(),
            Some("The pull request is no longer open.")
        );
        // Unlike publishing, the base the review compares with does not matter.
        let into_develop = pull_request("develop", PullRequestState::Open);
        assert!(verdict_readiness(&into_develop, HeadRelation::Same).allowed);
    }

    fn reason(
        base: &str,
        pull_request: &PullRequest,
        head: HeadRelation,
        base_relation: BaseRelation,
    ) -> Option<String> {
        let readiness = readiness(&Facts {
            base,
            pull_request,
            head,
            base_relation,
        });
        assert_eq!(readiness.allowed, readiness.reason.is_none());
        readiness.reason.map(|reason| reason.message)
    }

    #[test]
    fn publishing_needs_the_same_head_and_base_as_the_pull_request() {
        let open = pull_request("main", PullRequestState::Open);
        let same = BaseRelation::Same;
        assert_eq!(reason("main", &open, HeadRelation::Same, same), None);

        let cases = [
            (HeadRelation::LocalAhead, same, "push first, then Reload"),
            (HeadRelation::LocalBehind, same, "pull them, then Reload"),
            (HeadRelation::Diverged, same, "have diverged"),
            (HeadRelation::Missing, same, "fetch, then Reload"),
            (
                HeadRelation::Same,
                BaseRelation::Missing,
                "a main this repository doesn't have yet: fetch, then Reload",
            ),
            (
                HeadRelation::Same,
                BaseRelation::Moved,
                "main moved since this review was read: Reload",
            ),
        ];
        for (head, base_relation, expected) in cases {
            let reason = reason("main", &open, head, base_relation).unwrap();
            assert!(reason.contains(expected), "{head:?}: {reason}");
        }
        // Only a missing destination commit asks for a fetch.
        assert!(!reason("main", &open, HeadRelation::Same, BaseRelation::Moved)
            .unwrap()
            .contains("fetch"));
        assert!(reason("develop", &open, HeadRelation::Same, same)
            .unwrap()
            .contains("compares with develop, but the pull request merges into main"));
        let merged = pull_request("main", PullRequestState::Merged);
        assert!(reason("main", &merged, HeadRelation::Same, same).is_some());
    }

    fn comment(id: &str, parent: Option<&str>) -> ReviewComment {
        ReviewComment {
            id: id.to_string(),
            kind: CommentKind::Line,
            parent_id: parent.map(str::to_string),
            path: "a.go".to_string(),
            side: ReviewSide::Old,
            line: 3,
            line_text: "x".to_string(),
            commit_sha: None,
            body: format!("body {id}"),
            state: CommentState::Draft,
            remote_id: None,
            remote_author_id: None,
            remote_author_name: None,
            resolved: false,
            created_at: "t".to_string(),
            updated_at: "t".to_string(),
            current_line: Some(3),
            outdated: false,
            mine: true,
            pending_change: None,
            publish_error: None,
        }
    }

    fn published(id: &str, parent: Option<&str>, mine: bool) -> ReviewComment {
        ReviewComment {
            state: CommentState::Published,
            remote_id: Some(format!("r-{id}")),
            mine,
            ..comment(id, parent)
        }
    }

    #[test]
    fn plans_new_threads_replies_edits_and_deletes_in_order() {
        let mut resolved_draft = comment("d1", None);
        resolved_draft.resolved = true;
        let mut edited = published("p2", Some("p1"), true);
        edited.pending_change = Some(PendingChange::Edit);
        let mut deleted = published("p3", None, true);
        deleted.pending_change = Some(PendingChange::Delete);
        let comments = vec![
            published("p1", None, false),
            resolved_draft,
            comment("d1-reply", Some("d1")),
            edited,
            comment("p1-reply", Some("p1")),
            deleted,
            comment("p3-reply", Some("p3")),
        ];
        let plan = plan(&comments);

        assert!(plan.stale.is_empty());
        let create = |id: &str, target: Target, resolve: bool| Operation::Create {
            comment_id: id.to_string(),
            body: format!("body {id}"),
            target,
            resolve,
        };
        assert_eq!(
            plan.operations,
            vec![
                create("p1-reply", Target::Thread("r-p1".to_string()), false),
                create(
                    "d1",
                    Target::Line(Anchor {
                        path: "a.go".to_string(),
                        side: ReviewSide::Old,
                        line: 3
                    }),
                    true
                ),
                create("d1-reply", Target::NewThread("d1".to_string()), false),
                create("p3-reply", Target::Thread("r-p3".to_string()), false),
                Operation::Edit {
                    comment_id: "p2".to_string(),
                    remote_id: "r-p2".to_string(),
                    body: "body p2".to_string()
                },
                Operation::Delete {
                    comment_id: "p3".to_string(),
                    remote_id: "r-p3".to_string()
                },
            ]
        );
    }

    #[test]
    fn a_draft_thread_whose_line_changed_is_held_back_with_its_replies() {
        let mut moved = comment("d1", None);
        moved.outdated = true;
        let mut gone = comment("d2", None);
        gone.current_line = None;
        let comments = vec![moved, comment("d1-reply", Some("d1")), gone];
        let plan = plan(&comments);
        assert!(plan.operations.is_empty());
        assert_eq!(plan.stale, vec!["d1".to_string(), "d2".to_string()]);
    }

    fn general(id: &str, parent: Option<&str>) -> ReviewComment {
        ReviewComment {
            kind: CommentKind::General,
            path: String::new(),
            line: 0,
            line_text: String::new(),
            current_line: None,
            ..comment(id, parent)
        }
    }

    #[test]
    fn general_comments_go_in_the_same_batch_with_their_replies_and_changes() {
        let mut edited = published("g2", None, true);
        edited.kind = CommentKind::General;
        edited.pending_change = Some(PendingChange::Edit);
        let mut resolved = general("g1", None);
        resolved.resolved = true;
        let comments = vec![
            comment("d1", None),
            resolved,
            general("g1-reply", Some("g1")),
            edited,
            general("g2-reply", Some("g2")),
        ];
        let plan = plan(&comments);

        // No line to have moved: never held back.
        assert!(plan.stale.is_empty());
        let create = |id: &str, target: Target, resolve: bool| Operation::Create {
            comment_id: id.to_string(),
            body: format!("body {id}"),
            target,
            resolve,
        };
        assert_eq!(
            plan.operations,
            vec![
                create(
                    "d1",
                    Target::Line(Anchor {
                        path: "a.go".to_string(),
                        side: ReviewSide::Old,
                        line: 3
                    }),
                    false
                ),
                create("g1", Target::General, true),
                create("g1-reply", Target::NewThread("g1".to_string()), false),
                create("g2-reply", Target::Thread("r-g2".to_string()), false),
                Operation::Edit {
                    comment_id: "g2".to_string(),
                    remote_id: "r-g2".to_string(),
                    body: "body g2".to_string()
                },
            ]
        );
    }

    #[test]
    fn other_peoples_comments_are_never_changed() {
        let mut theirs = published("p1", None, false);
        theirs.pending_change = Some(PendingChange::Edit);
        assert!(plan(&[theirs]).operations.is_empty());
    }
}
