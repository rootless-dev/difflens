//! Signals of a review shown outside their own tabs (the Overview, the Flow
//! and the Diff): static check findings placed on the changed production
//! function that holds their line, how serious a static check finding most
//! likely is, and review comments drafted from a model finding, a static
//! check finding, duplicated code (placed by `duplication::locate`) or a
//! mutant no test killed (placed by `mutation::mutants`).
//!
//! A draft is the signal's own text, put together by the screen, and no
//! model is asked; only a model finding's draft is written by a model, on a
//! click (`insights::comment`), and then kept here like any other. It goes
//! where a comment written from the explanation of a
//! failure goes (`test_diagnosis::placement`): on the line when the review
//! adds or changes it, otherwise on the pull request as a whole, naming the
//! place. It stays linked to its signal (`signal_comments`) until it is
//! deleted, and asking again while it is there returns it.

use std::collections::HashSet;

use serde::Deserialize;

use crate::error::{app_error, AppResult};
use crate::hunks::ChangedLines;
use crate::language::Language;
use crate::model::{
    CheckClass, CheckSeverity, CheckStatus, CommentKind, FunctionRef, LocatedCheck, QualityFunction, ReviewSide,
    SignalDraft, SourceRef, StaticCheck,
};
use crate::review_state::ReviewSession;
use crate::test_diagnosis::{placement, where_section, Placement};

/// Lines of a finding looked at for one the review adds, at most.
const MAX_RANGE: u32 = 400;

/// The kinds of signal a draft is written from, by the prefix of their key.
const DRAFTED_SIGNALS: &[&str] = &["finding:", "check:", "duplicate:", "mutant:", QUALITY_SIGNAL];

/// A function of the Quality tab that crosses a limit:
/// `quality:<path>:<function id>`. Its draft goes on the first line of the
/// function the review adds or changes (its signature for a new one), as a
/// comment about the whole function reads best at its top.
const QUALITY_SIGNAL: &str = "quality:";

/// Whether a finding of `tool` with `rule` and `severity` is most likely a
/// defect or a matter of style. staticcheck's checks are named by a prefix:
/// SA (staticcheck proper) is about correctness; S (simple), ST
/// (stylecheck), QF (quick fixes) and U (unused code) are about style. Of
/// the other checkers, what their own level makes a note or a suggestion
/// (`info`) is about style. Anything else, go vet and a check without a
/// rule included, counts as correctness: a defect ranked too high costs a
/// look, one ranked too low can be missed.
pub fn check_class(tool: &str, rule: Option<&str>, severity: CheckSeverity) -> CheckClass {
    if tool != "staticcheck" {
        return match severity {
            CheckSeverity::Info => CheckClass::Style,
            CheckSeverity::Error | CheckSeverity::Warning => CheckClass::Correctness,
        };
    }
    let prefix: String = rule
        .unwrap_or("")
        .chars()
        .take_while(char::is_ascii_uppercase)
        .collect();
    match prefix.as_str() {
        "S" | "ST" | "QF" | "U" => CheckClass::Style,
        _ => CheckClass::Correctness,
    }
}

/// The findings of `checks` on lines the review adds (`lines`), each with
/// the innermost of `functions` that holds its line; by file, line, tool,
/// rule and message. Findings on lines the review leaves alone are only the
/// Quality tab's.
pub fn locate_checks(
    checks: &[StaticCheck],
    functions: &[QualityFunction],
    lines: &ChangedLines,
) -> Vec<LocatedCheck> {
    let mut located: Vec<LocatedCheck> = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Findings)
        .flat_map(|check| check.findings.iter().map(move |finding| (check, finding)))
        .filter(|(_, finding)| lines.added_within(&finding.file, finding.line, finding.line) > 0)
        .map(|(check, finding)| LocatedCheck {
            tool: check.tool.clone(),
            rule: finding.rule.clone(),
            file: finding.file.clone(),
            line: finding.line,
            message: finding.message.clone(),
            severity: finding.severity,
            class: check_class(&check.tool, finding.rule.as_deref(), finding.severity),
            function: functions
                .iter()
                .filter(|function| {
                    function.file == finding.file
                        && (function.start_line..=function.end_line).contains(&finding.line)
                })
                .min_by_key(|function| function.end_line - function.start_line)
                .map(|function| FunctionRef {
                    id: function.id.clone(),
                    qualified_name: function.qualified_name.clone(),
                }),
        })
        .collect();
    located.sort_by(|left, right| {
        (&left.file, left.line, &left.tool, &left.rule, &left.message).cmp(&(
            &right.file,
            right.line,
            &right.tool,
            &right.rule,
            &right.message,
        ))
    });
    located.dedup();
    located
}

/// What the screen asks a draft to be written from.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftRequest {
    /// `finding:<id>`, `check:<tool>:<rule>:<path>:<line>:<message>` or
    /// `duplicate:<kind>:<path>:<start line>:<end line>` or `mutant:<id>`
    /// (a mutant that survived, see `mutation`).
    pub signal: String,
    /// Where the signal is on the new side: a file and its lines.
    pub path: String,
    pub line: u32,
    pub end_line: u32,
    /// The comment, from the signal's own text.
    pub body: String,
}

/// Where a draft of the lines `line` to `end_line` of `path` goes: on the
/// last of them the review adds or changes, else on the pull request as a
/// whole, naming `path:line`.
pub fn draft_placement(path: &str, line: u32, end_line: u32, changed: &ChangedLines) -> Placement {
    placement_in(path, line, end_line, changed, false)
}

/// As `draft_placement`, on the first of the lines the review adds or
/// changes rather than the last.
pub fn first_draft_placement(path: &str, line: u32, end_line: u32, changed: &ChangedLines) -> Placement {
    placement_in(path, line, end_line, changed, true)
}

fn placement_in(path: &str, line: u32, end_line: u32, changed: &ChangedLines, first: bool) -> Placement {
    let files: HashSet<String> = changed
        .added_paths()
        .chain(changed.removed_paths())
        .map(str::to_string)
        .collect();
    let last = end_line.max(line).min(line.saturating_add(MAX_RANGE));
    let candidates: Box<dyn Iterator<Item = u32>> = if first {
        Box::new(line..=last)
    } else {
        Box::new((line..=last).rev())
    };
    for candidate in candidates {
        let at = SourceRef {
            file: path.to_string(),
            line: candidate,
        };
        if let found @ Placement::Line(_) = placement(Some(&at), &files, changed) {
            return found;
        }
    }
    let at = SourceRef {
        file: path.to_string(),
        line,
    };
    placement(Some(&at), &files, changed)
}

/// The draft written from `signal` while it is there: not deleted, nor
/// deleted here waiting for a publish.
pub fn existing_draft(session: &ReviewSession<'_>, signal: &str) -> AppResult<Option<SignalDraft>> {
    let store = session.store();
    let Some(review) = session.existing_record()? else {
        return Ok(None);
    };
    let Some(existing) = store.signal_comment(&review.id, signal)? else {
        return Ok(None);
    };
    Ok(store
        .comment(session.project_id(), &existing)?
        .filter(|comment| !comment.delete_pending)
        .map(|comment| SignalDraft {
            signal: signal.to_string(),
            comment_id: comment.id,
            kind: comment.kind,
        }))
}

/// Writes the draft of `request` in the review, or returns the one written
/// before while it is there. On the pull request as a whole the body starts
/// with the place it is about, in `language`.
pub fn draft(
    session: &ReviewSession<'_>,
    changed: &ChangedLines,
    request: &DraftRequest,
    language: Language,
) -> AppResult<SignalDraft> {
    if !DRAFTED_SIGNALS
        .iter()
        .any(|prefix| request.signal.starts_with(prefix))
    {
        return Err(app_error!(
            Internal,
            SignalUnknown,
            "That is not a finding or a check a comment can be written from."
        ));
    }
    if let Some(existing) = existing_draft(session, &request.signal)? {
        return Ok(existing);
    }
    let store = session.store();
    let placed = if request.signal.starts_with(QUALITY_SIGNAL) {
        first_draft_placement(&request.path, request.line, request.end_line, changed)
    } else {
        draft_placement(&request.path, request.line, request.end_line, changed)
    };
    let (comment_id, kind) = match &placed {
        Placement::Line(location) => (
            session.add_comment(&location.file, ReviewSide::New, location.line, &request.body)?,
            CommentKind::Line,
        ),
        Placement::General(_) => {
            let body = match where_section(&placed, Some(&session.review().target), language) {
                Some(place) => format!("{place}\n\n{}", request.body.trim()),
                None => request.body.clone(),
            };
            (session.add_general_comment(&body)?, CommentKind::General)
        }
    };
    let review = session.record()?;
    store.link_signal_comment(&review.id, &request.signal, &comment_id)?;
    Ok(SignalDraft {
        signal: request.signal.clone(),
        comment_id,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChangedState, CheckFinding, ChangeSet, CommentState, ReviewRef, ReviewTarget};
    use crate::projects::ProjectService;
    use crate::review_state;
    use crate::store::Store;
    use crate::test_support::{isolated_git, TestRepo};

    fn function(id: &str, file: &str, start_line: u32, end_line: u32) -> QualityFunction {
        QualityFunction {
            id: id.to_string(),
            qualified_name: format!("p.{id}"),
            name: id.to_string(),
            container: "p".to_string(),
            file: file.to_string(),
            start_line,
            end_line,
            state: ChangedState::Modified,
            complexity: Some(1),
            previous_complexity: None,
            changed_lines: None,
            crap: None,
            untested: false,
            code: Vec::new(),
        }
    }

    fn finding(file: &str, line: u32, rule: Option<&str>, message: &str) -> CheckFinding {
        CheckFinding {
            file: file.to_string(),
            line,
            message: message.to_string(),
            rule: rule.map(str::to_string),
            severity: CheckSeverity::Warning,
        }
    }

    fn check(tool: &str, status: CheckStatus, findings: Vec<CheckFinding>) -> StaticCheck {
        StaticCheck {
            tool: tool.to_string(),
            status,
            message: None,
            findings,
        }
    }

    #[test]
    fn go_vet_and_staticcheck_sa_are_correctness_the_other_staticcheck_checks_style() {
        use CheckSeverity::{Error, Info, Warning};
        assert_eq!(check_class("go vet", Some("printf"), Error), CheckClass::Correctness);
        assert_eq!(check_class("go vet", None, Warning), CheckClass::Correctness);
        assert_eq!(check_class("staticcheck", Some("SA4006"), Error), CheckClass::Correctness);
        for rule in ["S1000", "ST1005", "QF1001", "U1000"] {
            assert_eq!(check_class("staticcheck", Some(rule), Info), CheckClass::Style, "{rule}");
            // staticcheck's own names win over the level.
            assert_eq!(check_class("staticcheck", Some(rule), Error), CheckClass::Style, "{rule}");
        }
        // Unknown or missing: ranked as a defect.
        assert_eq!(check_class("staticcheck", Some("XY1"), Info), CheckClass::Correctness);
        assert_eq!(check_class("staticcheck", None, Warning), CheckClass::Correctness);
        assert_eq!(check_class("gosec", Some("G101"), Warning), CheckClass::Correctness);
        // Other checkers: a note or a suggestion is style.
        assert_eq!(check_class("tsc", Some("TS6133"), Info), CheckClass::Style);
        assert_eq!(check_class("clippy", Some("clippy::needless_return"), Warning), CheckClass::Correctness);
        assert_eq!(check_class("eslint", Some("no-undef"), Error), CheckClass::Correctness);
    }

    #[test]
    fn checks_on_added_lines_are_placed_on_the_innermost_function_holding_them() {
        let functions = vec![
            function("Outer", "a.go", 1, 40),
            function("Inner", "a.go", 10, 20),
            function("Other", "b.go", 1, 9),
        ];
        let lines = ChangedLines::default()
            .with_added("a.go", 12, 12)
            .with_added("a.go", 30, 30)
            .with_added("c.go", 1, 5);
        let checks = vec![
            check(
                "staticcheck",
                CheckStatus::Findings,
                vec![
                    finding("a.go", 30, Some("ST1005"), "error strings should not be capitalized"),
                    finding("a.go", 12, Some("SA4006"), "this value of err is never used"),
                    // An unchanged line: only the Quality tab's.
                    finding("a.go", 2, Some("SA1000"), "unchanged"),
                ],
            ),
            check(
                "go vet",
                CheckStatus::Findings,
                vec![CheckFinding {
                    severity: CheckSeverity::Error,
                    ..finding("c.go", 3, Some("printf"), "wrong type")
                }],
            ),
            check(
                "other",
                CheckStatus::Failed,
                vec![finding("a.go", 12, None, "not a result")],
            ),
        ];

        let located = locate_checks(&checks, &functions, &lines);

        let summary: Vec<_> = located
            .iter()
            .map(|check| {
                (
                    check.file.as_str(),
                    check.line,
                    check.tool.as_str(),
                    check.class,
                    check.function.as_ref().map(|function| function.id.as_str()),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("a.go", 12, "staticcheck", CheckClass::Correctness, Some("Inner")),
                ("a.go", 30, "staticcheck", CheckClass::Style, Some("Outer")),
                // In no changed function: kept by file and line.
                ("c.go", 3, "go vet", CheckClass::Correctness, None),
            ]
        );
        assert_eq!(located[0].rule.as_deref(), Some("SA4006"));
        assert_eq!(located[0].message, "this value of err is never used");
        let severities: Vec<_> = located.iter().map(|check| check.severity).collect();
        assert_eq!(severities, [CheckSeverity::Warning, CheckSeverity::Warning, CheckSeverity::Error]);
        assert_eq!(
            located[0].function.as_ref().map(|function| function.qualified_name.as_str()),
            Some("p.Inner")
        );
    }

    #[test]
    fn a_draft_goes_on_the_last_added_line_of_its_lines_else_on_the_pull_request() {
        let changed = ChangedLines::default()
            .with_added("a.go", 5, 6)
            .with_removed("b.go", 3, 3);
        let line = |path: &str, line: u32| {
            Placement::Line(SourceRef {
                file: path.to_string(),
                line,
            })
        };
        assert_eq!(draft_placement("a.go", 4, 9, &changed), line("a.go", 6));
        assert_eq!(draft_placement("a.go", 5, 5, &changed), line("a.go", 5));
        // An end before the start reads as the start alone.
        assert_eq!(draft_placement("a.go", 6, 1, &changed), line("a.go", 6));
        assert!(matches!(
            draft_placement("a.go", 8, 9, &changed),
            Placement::General(crate::test_diagnosis::GeneralReason::LineNotChanged(at)) if at.line == 8
        ));
        assert!(matches!(
            draft_placement("b.go", 3, 3, &changed),
            Placement::General(crate::test_diagnosis::GeneralReason::LineNotChanged(_))
        ));
        assert!(matches!(
            draft_placement("z.go", 1, 1, &changed),
            Placement::General(crate::test_diagnosis::GeneralReason::FileNotInChange(_))
        ));
    }

    #[test]
    fn a_function_draft_goes_on_its_first_added_line() {
        let changed = ChangedLines::default().with_added("a.go", 5, 6).with_added("a.go", 9, 9);
        assert_eq!(
            first_draft_placement("a.go", 1, 10, &changed),
            Placement::Line(SourceRef { file: "a.go".to_string(), line: 5 })
        );
        assert!(matches!(first_draft_placement("a.go", 10, 12, &changed), Placement::General(_)));
        assert!(DRAFTED_SIGNALS.iter().any(|prefix| "quality:a.go:p.A".starts_with(prefix)));
    }

    struct Fixture {
        repo: TestRepo,
        service: ProjectService,
        id: String,
    }

    impl Fixture {
        /// `feature` adds lines 7 to 9 of app.go on top of `main`.
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
            let service =
                ProjectService::new(Store::open_in_memory().unwrap(), isolated_git(), None);
            let id = service.add_local(repo.path().to_str().unwrap()).unwrap().id;
            Self { repo, service, id }
        }

        fn session(&self) -> ReviewSession<'_> {
            let target = ReviewTarget::Branch {
                branch: "feature".to_string(),
                base: "main".to_string(),
            };
            let ChangeSet {
                target,
                old_sha,
                new_sha,
                ..
            } = self.service.changed_files(&self.id, target).unwrap();
            ReviewSession::new(
                self.service.store(),
                self.service.git(),
                &self.id,
                self.repo.path(),
                ReviewRef {
                    target,
                    old_sha,
                    new_sha,
                },
            )
            .unwrap()
        }
    }

    fn request(signal: &str, line: u32, end_line: u32) -> DraftRequest {
        DraftRequest {
            signal: signal.to_string(),
            path: "app.go".to_string(),
            line,
            end_line,
            body: "Log the error before the fallback.".to_string(),
        }
    }

    #[test]
    fn a_draft_from_a_finding_is_a_line_draft_linked_to_it_until_it_is_deleted() {
        let fixture = Fixture::new();
        let session = fixture.session();
        let changed = ChangedLines::default().with_added("app.go", 6, 9);

        let written = draft(&session, &changed, &request("finding:f-1", 7, 8), Language::English).unwrap();

        assert_eq!(written.kind, CommentKind::Line);
        let state = session.state(&[]).unwrap();
        assert_eq!(state.signal_drafts, vec![written.clone()]);
        let comment = &state.comments[0];
        assert_eq!((comment.path.as_str(), comment.line), ("app.go", 8));
        assert_eq!(comment.body, "Log the error before the fallback.");
        assert_eq!(comment.state, CommentState::Draft);

        // Asked again: the same draft, nothing written.
        let again = draft(&session, &changed, &request("finding:f-1", 7, 8), Language::English).unwrap();
        assert_eq!(again, written);
        assert_eq!(session.state(&[]).unwrap().comments.len(), 1);

        // Deleted: the link goes with it, and a new draft can be written.
        review_state::delete_comment(fixture.service.store(), &fixture.id, &written.comment_id).unwrap();
        assert!(session.state(&[]).unwrap().signal_drafts.is_empty());
        let fresh = draft(&session, &changed, &request("finding:f-1", 7, 8), Language::English).unwrap();
        assert_ne!(fresh.comment_id, written.comment_id);
    }

    #[test]
    fn a_draft_on_an_unchanged_line_is_a_general_draft_citing_the_place() {
        let fixture = Fixture::new();
        let session = fixture.session();
        let changed = ChangedLines::default().with_added("app.go", 6, 9);

        let written = draft(
            &session,
            &changed,
            &request("check:staticcheck:SA4006:app.go:4:this value is never used", 4, 4),
            Language::BrazilianPortuguese,
        )
        .unwrap();

        assert_eq!(written.kind, CommentKind::General);
        let state = session.state(&[]).unwrap();
        assert_eq!(state.signal_drafts, vec![written]);
        assert_eq!(state.comments[0].kind, CommentKind::General);
        assert_eq!(
            state.comments[0].body,
            "**Onde:** `app.go:4`\nEsse arquivo foi alterado nesta PR, mas não essa linha.\n\n\
             Log the error before the fallback."
        );
    }

    #[test]
    fn a_draft_from_duplicated_code_goes_on_the_last_added_line_of_its_block() {
        let fixture = Fixture::new();
        let session = fixture.session();
        let changed = ChangedLines::default().with_added("app.go", 6, 9);
        let request = DraftRequest {
            signal: "duplicate:copies_existing:app.go:7:9".to_string(),
            path: "app.go".to_string(),
            line: 7,
            end_line: 9,
            body: "This already exists in `app.go:3-5`; reuse it instead of duplicating it.".to_string(),
        };

        let written = draft(&session, &changed, &request, Language::English).unwrap();

        assert_eq!(written.kind, CommentKind::Line);
        let state = session.state(&[]).unwrap();
        assert_eq!(state.signal_drafts, vec![written]);
        assert_eq!((state.comments[0].path.as_str(), state.comments[0].line), ("app.go", 9));
        assert_eq!(state.comments[0].body, request.body);
    }

    #[test]
    fn a_draft_from_a_surviving_mutant_goes_on_its_line_and_is_found_again() {
        let fixture = Fixture::new();
        let session = fixture.session();
        let changed = ChangedLines::default().with_added("app.go", 3, 8);
        let request = DraftRequest {
            signal: "mutant:5f2a9c".to_string(),
            path: "app.go".to_string(),
            line: 4,
            end_line: 4,
            body: "No test fails if `>=` becomes `>` here; a test for this boundary is missing.".to_string(),
        };

        let written = draft(&session, &changed, &request, Language::English).unwrap();
        let again = draft(&session, &changed, &request, Language::English).unwrap();

        assert_eq!(written, again, "asking again returns the draft");
        assert_eq!(written.kind, CommentKind::Line);
        let state = session.state(&[]).unwrap();
        assert_eq!(state.comments.len(), 1);
        assert_eq!((state.comments[0].path.as_str(), state.comments[0].line), ("app.go", 4));
    }

    #[test]
    fn a_draft_needs_a_known_kind_of_signal() {
        let fixture = Fixture::new();
        let session = fixture.session();
        let error = draft(
            &session,
            &ChangedLines::default(),
            &request("diagnosis:x", 1, 1),
            Language::English,
        )
        .unwrap_err();
        assert!(error.to_string().contains("not a finding or a check"), "{error}");
    }
}
