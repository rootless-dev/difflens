//! Bringing a pull request's comments into a review: one local row per
//! remote comment, found again by its remote id on every pull. What the user
//! changed here and has not published yet (an edit, a delete) wins over the
//! remote text until it is published.
//!
//! Threads are one level deep here, as in the Diff: a reply to a reply joins
//! the thread of its first comment, and takes that comment's line (or, for a
//! comment on the pull request as a whole, its kind: general, on no line).
//! Comments on a whole file have no line in the Diff and are left out,
//! replies included.

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use super::host::{Anchor, RemoteComment};
use crate::error::AppResult;
use crate::model::{CommentKind, CommentState, ReviewSide};
use crate::store::{CommentRecord, PulledUpdate, Store};

/// Longest chain of replies followed to find a thread's first comment.
const MAX_DEPTH: usize = 64;

/// What a pull is done against.
pub struct PullContext<'a> {
    pub review_id: &'a str,
    /// The pull request's number.
    pub pull_request: &'a str,
    /// The review's head, recorded as the commit new comments were read at.
    pub commit_sha: Option<&'a str>,
}

/// What a pull changes in the review's comments.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PullPlan {
    /// First comments before their replies.
    pub inserts: Vec<CommentRecord>,
    pub updates: Vec<PulledUpdate>,
    /// Published comments of this pull request it no longer has.
    pub removals: Vec<String>,
}

/// Works out the changes. `line_text` reads a line of the review's sides, to
/// anchor a new comment to the text it is on now.
pub fn plan_pull(
    context: &PullContext<'_>,
    existing: &[CommentRecord],
    remote: &[RemoteComment],
    line_text: &dyn Fn(&str, ReviewSide, u32) -> Option<String>,
) -> PullPlan {
    let by_id: HashMap<&str, &RemoteComment> = remote
        .iter()
        .map(|comment| (comment.id.as_str(), comment))
        .collect();
    let root_of = |comment: &RemoteComment| -> Option<String> {
        let mut current = comment;
        for _ in 0..MAX_DEPTH {
            match &current.parent_id {
                None => return Some(current.id.clone()),
                Some(parent) => current = by_id.get(parent.as_str())?,
            }
        }
        None
    };
    let mut live_replies: HashSet<String> = HashSet::new();
    for comment in remote {
        if comment.parent_id.is_some() && !comment.deleted {
            if let Some(root) = root_of(comment) {
                live_replies.insert(root);
            }
        }
    }
    let local: HashMap<&str, &CommentRecord> = existing
        .iter()
        .filter_map(|record| record.remote_id.as_deref().map(|id| (id, record)))
        .collect();

    let mut plan = PullPlan::default();
    let mut kept: HashSet<&str> = HashSet::new();
    // Each kept first comment's local row, which its replies copy the line of.
    let mut threads: HashMap<String, CommentRecord> = HashMap::new();

    for comment in remote.iter().filter(|comment| comment.parent_id.is_none()) {
        let (kind, anchor) = match &comment.anchor {
            Some(anchor) => (CommentKind::Line, anchor.clone()),
            None if comment.general => (
                CommentKind::General,
                Anchor {
                    path: String::new(),
                    side: ReviewSide::New,
                    line: 0,
                },
            ),
            None => continue,
        };
        if comment.deleted && !live_replies.contains(&comment.id) {
            continue;
        }
        kept.insert(&comment.id);
        let record = match local.get(comment.id.as_str()) {
            Some(record) => {
                plan.updates
                    .push(update(record, comment, Some(comment.resolved)));
                (*record).clone()
            }
            None => {
                let line_text = match kind {
                    CommentKind::Line => {
                        line_text(&anchor.path, anchor.side, anchor.line).unwrap_or_default()
                    }
                    CommentKind::General => String::new(),
                };
                let record = CommentRecord {
                    id: Uuid::new_v4().to_string(),
                    review_id: context.review_id.to_string(),
                    kind,
                    parent_id: None,
                    path: anchor.path,
                    side: anchor.side,
                    line: anchor.line,
                    line_text,
                    commit_sha: context.commit_sha.map(str::to_string),
                    body: comment.body.clone(),
                    state: CommentState::Published,
                    remote_id: Some(comment.id.clone()),
                    remote_author_id: Some(comment.author.id.clone()),
                    remote_author_name: Some(comment.author.name.clone()),
                    resolved: comment.resolved,
                    created_at: comment.created_at.clone(),
                    updated_at: comment.updated_at.clone(),
                    remote_pull_request: Some(context.pull_request.to_string()),
                    edited_locally: false,
                    delete_pending: false,
                    publish_error: None,
                };
                plan.inserts.push(record.clone());
                record
            }
        };
        threads.insert(comment.id.clone(), record);
    }

    for comment in remote.iter().filter(|comment| comment.parent_id.is_some()) {
        if comment.deleted {
            continue;
        }
        let Some(thread) = root_of(comment).and_then(|root| threads.get(&root)) else {
            continue;
        };
        kept.insert(&comment.id);
        match local.get(comment.id.as_str()) {
            Some(record) => plan.updates.push(update(record, comment, None)),
            None => plan.inserts.push(CommentRecord {
                id: Uuid::new_v4().to_string(),
                parent_id: Some(thread.id.clone()),
                body: comment.body.clone(),
                state: CommentState::Published,
                remote_id: Some(comment.id.clone()),
                remote_author_id: Some(comment.author.id.clone()),
                remote_author_name: Some(comment.author.name.clone()),
                resolved: false,
                created_at: comment.created_at.clone(),
                updated_at: comment.updated_at.clone(),
                remote_pull_request: Some(context.pull_request.to_string()),
                edited_locally: false,
                delete_pending: false,
                publish_error: None,
                ..thread.clone()
            }),
        }
    }

    plan.removals = existing
        .iter()
        .filter(|record| {
            record.remote_pull_request.as_deref() == Some(context.pull_request)
                && record
                    .remote_id
                    .as_deref()
                    .is_some_and(|id| !kept.contains(id))
        })
        .map(|record| record.id.clone())
        .collect();
    plan
}

/// The remote text, unless the user changed the comment here and has not
/// published that yet.
fn update(record: &CommentRecord, comment: &RemoteComment, resolved: Option<bool>) -> PulledUpdate {
    let changed_here = record.edited_locally || record.delete_pending;
    PulledUpdate {
        id: record.id.clone(),
        body: (!changed_here).then(|| comment.body.clone()),
        author_id: comment.author.id.clone(),
        author_name: comment.author.name.clone(),
        resolved,
        updated_at: comment.updated_at.clone(),
    }
}

pub fn apply(store: &Store, plan: &PullPlan) -> AppResult<()> {
    for id in &plan.removals {
        store.delete_comment(id)?;
    }
    for update in &plan.updates {
        store.apply_pulled_update(update)?;
    }
    for record in &plan.inserts {
        store.insert_comment(record)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Account;
    use crate::pull_requests::host::Anchor;

    const CONTEXT: PullContext<'static> = PullContext {
        review_id: "r",
        pull_request: "318",
        commit_sha: Some("head"),
    };

    fn remote(id: &str, parent: Option<&str>, line: Option<(ReviewSide, u32)>) -> RemoteComment {
        RemoteComment {
            id: id.to_string(),
            parent_id: parent.map(str::to_string),
            author: Account {
                id: "{b2}".to_string(),
                name: "Bruno".to_string(),
            },
            body: format!("remote {id}"),
            anchor: line.map(|(side, line)| Anchor {
                path: "a.go".to_string(),
                side,
                line,
            }),
            // Without a line: on the whole file, unless said otherwise.
            general: false,
            resolved: false,
            deleted: false,
            created_at: "t1".to_string(),
            updated_at: "t2".to_string(),
        }
    }

    fn text(_path: &str, side: ReviewSide, line: u32) -> Option<String> {
        Some(format!("{side:?} line {line}"))
    }

    fn published(id: &str, remote_id: &str, parent: Option<&str>) -> CommentRecord {
        CommentRecord {
            id: id.to_string(),
            review_id: "r".to_string(),
            kind: CommentKind::Line,
            parent_id: parent.map(str::to_string),
            path: "a.go".to_string(),
            side: ReviewSide::New,
            line: 8,
            line_text: "kept".to_string(),
            commit_sha: Some("old head".to_string()),
            body: "local".to_string(),
            state: CommentState::Published,
            remote_id: Some(remote_id.to_string()),
            remote_author_id: Some("{me}".to_string()),
            remote_author_name: Some("Me".to_string()),
            resolved: false,
            created_at: "t0".to_string(),
            updated_at: "t0".to_string(),
            remote_pull_request: Some("318".to_string()),
            edited_locally: false,
            delete_pending: false,
            publish_error: None,
        }
    }

    #[test]
    fn new_threads_land_on_their_side_and_line_with_replies_flattened() {
        let mut resolved_root = remote("1", None, Some((ReviewSide::New, 44)));
        resolved_root.resolved = true;
        let comments = vec![
            resolved_root,
            remote("2", None, Some((ReviewSide::Old, 12))),
            remote("3", Some("1"), None),
            // A reply to a reply joins the first comment's thread.
            remote("4", Some("3"), None),
            // On a whole file: no line in the Diff.
            remote("5", None, None),
            remote("6", Some("5"), None),
        ];
        let plan = plan_pull(&CONTEXT, &[], &comments, &text);

        /// Remote id, parent, side, line, line text, resolved.
        type Row<'a> = (
            Option<&'a str>,
            Option<&'a str>,
            ReviewSide,
            u32,
            &'a str,
            bool,
        );
        let rows: Vec<Row<'_>> = plan
            .inserts
            .iter()
            .map(|record| {
                (
                    record.remote_id.as_deref(),
                    record.parent_id.as_deref(),
                    record.side,
                    record.line,
                    record.line_text.as_str(),
                    record.resolved,
                )
            })
            .collect();
        let root_one = plan.inserts[0].id.as_str();
        assert_eq!(
            rows,
            vec![
                (Some("1"), None, ReviewSide::New, 44, "New line 44", true),
                (Some("2"), None, ReviewSide::Old, 12, "Old line 12", false),
                (
                    Some("3"),
                    Some(root_one),
                    ReviewSide::New,
                    44,
                    "New line 44",
                    false
                ),
                (
                    Some("4"),
                    Some(root_one),
                    ReviewSide::New,
                    44,
                    "New line 44",
                    false
                ),
            ]
        );
        assert!(plan.inserts.iter().all(|record| {
            record.state == CommentState::Published
                && record.remote_pull_request.as_deref() == Some("318")
                && record.remote_author_name.as_deref() == Some("Bruno")
                && record.commit_sha.as_deref() == Some("head")
        }));
        assert!(plan.updates.is_empty() && plan.removals.is_empty());
    }

    #[test]
    fn comments_on_the_whole_pull_request_come_in_as_general_threads_from_anyone() {
        let general = |id: &str, parent: Option<&str>| RemoteComment {
            general: true,
            ..remote(id, parent, None)
        };
        let mut known = published("local-7", "7", None);
        known.kind = CommentKind::General;
        known.path = String::new();
        known.line = 0;
        known.line_text = String::new();
        let comments = vec![
            general("5", None),
            general("6", Some("5")),
            general("7", None),
            general("8", Some("7")),
        ];
        let plan = plan_pull(&CONTEXT, &[known], &comments, &text);

        let rows: Vec<(Option<&str>, CommentKind, &str, u32, &str)> = plan
            .inserts
            .iter()
            .map(|record| {
                (
                    record.remote_id.as_deref(),
                    record.kind,
                    record.path.as_str(),
                    record.line,
                    record.line_text.as_str(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                (Some("5"), CommentKind::General, "", 0, ""),
                (Some("6"), CommentKind::General, "", 0, ""),
                (Some("8"), CommentKind::General, "", 0, ""),
            ]
        );
        assert_eq!(plan.inserts[1].parent_id.as_deref(), Some(plan.inserts[0].id.as_str()));
        assert_eq!(plan.inserts[2].parent_id.as_deref(), Some("local-7"));
        assert!(plan
            .inserts
            .iter()
            .all(|record| record.remote_author_name.as_deref() == Some("Bruno")));
        assert_eq!(plan.updates.len(), 1);
        assert_eq!(plan.updates[0].id, "local-7");
        assert!(plan.removals.is_empty());
    }

    #[test]
    fn known_comments_are_updated_in_place_by_remote_id() {
        let existing = vec![
            published("local-1", "1", None),
            published("local-3", "3", Some("local-1")),
        ];
        let mut root = remote("1", None, Some((ReviewSide::New, 44)));
        root.resolved = true;
        let comments = vec![
            root,
            remote("3", Some("1"), None),
            remote("7", Some("1"), None),
        ];
        let plan = plan_pull(&CONTEXT, &existing, &comments, &text);

        assert_eq!(
            plan.updates,
            vec![
                PulledUpdate {
                    id: "local-1".to_string(),
                    body: Some("remote 1".to_string()),
                    author_id: "{b2}".to_string(),
                    author_name: "Bruno".to_string(),
                    resolved: Some(true),
                    updated_at: "t2".to_string(),
                },
                PulledUpdate {
                    id: "local-3".to_string(),
                    body: Some("remote 3".to_string()),
                    author_id: "{b2}".to_string(),
                    author_name: "Bruno".to_string(),
                    resolved: None,
                    updated_at: "t2".to_string(),
                },
            ]
        );
        // A new reply joins the known thread and keeps its line.
        assert_eq!(plan.inserts.len(), 1);
        let reply = &plan.inserts[0];
        assert_eq!(reply.parent_id.as_deref(), Some("local-1"));
        assert_eq!((reply.line, reply.line_text.as_str()), (8, "kept"));
    }

    #[test]
    fn local_edits_and_deletes_not_published_win_over_the_remote_text() {
        let mut edited = published("local-1", "1", None);
        edited.edited_locally = true;
        let mut deleting = published("local-2", "2", None);
        deleting.delete_pending = true;
        let comments = vec![
            remote("1", None, Some((ReviewSide::New, 8))),
            remote("2", None, Some((ReviewSide::New, 9))),
        ];
        let plan = plan_pull(&CONTEXT, &[edited, deleting], &comments, &text);
        assert!(plan.updates.iter().all(|update| update.body.is_none()));
    }

    #[test]
    fn gone_and_deleted_comments_leave_unless_replies_hang_from_them() {
        let mut other_pull_request = published("local-9", "9", None);
        other_pull_request.remote_pull_request = Some("200".to_string());
        let existing = vec![
            published("local-1", "1", None),
            published("local-2", "2", None),
            published("local-3", "3", None),
            other_pull_request,
        ];
        let mut deleted_alone = remote("2", None, Some((ReviewSide::New, 9)));
        deleted_alone.deleted = true;
        let mut deleted_with_reply = remote("3", None, Some((ReviewSide::New, 10)));
        deleted_with_reply.deleted = true;
        let comments = vec![
            deleted_alone,
            deleted_with_reply,
            remote("4", Some("3"), None),
        ];
        let plan = plan_pull(&CONTEXT, &existing, &comments, &text);

        // 1 is gone from the pull request, 2 was deleted with nothing under it.
        assert_eq!(
            plan.removals,
            vec!["local-1".to_string(), "local-2".to_string()]
        );
        assert_eq!(plan.updates.len(), 1);
        assert_eq!(plan.updates[0].id, "local-3");
        assert_eq!(plan.inserts[0].parent_id.as_deref(), Some("local-3"));
    }

    #[test]
    fn applying_a_plan_twice_keeps_one_row_per_remote_comment() {
        use crate::model::ReviewTarget;
        use crate::store::ProjectRecord;

        let store = Store::open_in_memory().unwrap();
        store
            .insert(&ProjectRecord {
                id: "p".to_string(),
                path: "/repos/p".to_string(),
                integration_branches: vec!["main".to_string()],
                last_fetched_at: None,
                added_at: "t".to_string(),
            })
            .unwrap();
        let target = ReviewTarget::Branch {
            branch: "feature".to_string(),
            base: "main".to_string(),
        };
        let review = store.open_review("p", &target, "t").unwrap();
        let context = PullContext {
            review_id: &review.id,
            ..CONTEXT
        };
        let comments = vec![
            remote("1", None, Some((ReviewSide::New, 8))),
            remote("2", Some("1"), None),
        ];

        for _ in 0..2 {
            let existing = store.comments(&review.id).unwrap();
            apply(&store, &plan_pull(&context, &existing, &comments, &text)).unwrap();
        }
        let stored = store.comments(&review.id).unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[1].parent_id.as_deref(), Some(stored[0].id.as_str()));
    }
}
