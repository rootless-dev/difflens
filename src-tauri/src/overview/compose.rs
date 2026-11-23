//! The change as one call sends it. The whole diff, with Insights' context,
//! when it fits the room a call has; otherwise a condensed form: every
//! changed file with its lines added and removed, then the hunks with less
//! context, production code before tests, trimmed to fit. What is left out
//! is named, so the model knows what it did not see.

use std::fmt::Write;

use crate::insights::diff::{self, Line};
use crate::insights::plan::{self, CONTEXT_LINES, TRUNCATED_CONTEXT_LINES};
use crate::review_state::ChangedSides;

/// Characters kept back from a condensed change's room for the notes
/// written around the hunks.
const NOTES_ROOM: usize = 600;

/// How every diff line is laid out, told before the diffs.
const DIFF_FORMAT: &str = "Each diff line is a marker (\"+\" added, \"-\" removed, a space for \
    unchanged), the line number on the new side (blank for removed lines), \"|\", then the code. \
    Cite only these new-side line numbers.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composed {
    pub text: String,
    pub tokens: u64,
    /// The whole diff did not fit: the condensed form was sent.
    pub condensed: bool,
    /// Files whose code is shown, whole or in part.
    pub files_shown: u32,
}

/// A changed file, diffed once.
struct Diffed<'a> {
    file: &'a ChangedSides,
    status: String,
    added: usize,
    removed: usize,
    /// Why its code is not sent; `None` when it is.
    skip: Option<&'static str>,
    old: Vec<&'a str>,
    new: Vec<&'a str>,
    lines: Vec<Line>,
}

/// `files` in at most `budget` tokens (a condensed form when the whole diff
/// does not fit).
pub fn compose(files: &[ChangedSides], budget: u64) -> Composed {
    let diffed: Vec<Diffed<'_>> = files.iter().map(diffed).collect();
    let listing = listing(&diffed);

    let mut whole = format!("{listing}\nDiffs of the changed files. {DIFF_FORMAT}\n\n");
    let mut shown = 0;
    for file in diffed.iter().filter(|file| file.skip.is_none()) {
        let hunks = diff::render(&file.old, &file.new, &file.lines, CONTEXT_LINES, usize::MAX);
        whole.push_str(&header(file));
        whole.push_str(&hunks.text);
        whole.push('\n');
        shown += 1;
    }
    let tokens = plan::estimate_tokens(&whole);
    if tokens <= budget {
        return Composed {
            text: whole,
            tokens,
            condensed: false,
            files_shown: shown,
        };
    }
    condensed(&diffed, &listing, budget)
}

fn condensed(diffed: &[Diffed<'_>], listing: &str, budget: u64) -> Composed {
    let mut text = format!(
        "{listing}\nThis change is too large to send whole, so it is condensed: each file's \
         changed lines with {TRUNCATED_CONTEXT_LINES} lines of context, production code before \
         tests, cut where the room ends. Files whose code is not shown, or only in part, are \
         named after the diffs. Do not judge code you were not shown.\n\nDiffs. {DIFF_FORMAT}\n\n"
    );
    let room = usize::try_from(budget.saturating_mul(4))
        .unwrap_or(usize::MAX)
        .saturating_sub(text.len() + NOTES_ROOM);
    let mut order: Vec<&Diffed<'_>> = diffed.iter().filter(|file| file.skip.is_none()).collect();
    order.sort_by_key(|file| (is_test(&file.file.path), file.file.path.clone()));

    let mut used = 0;
    let mut shown = 0;
    let mut partly: Vec<String> = Vec::new();
    let mut hidden: Vec<String> = Vec::new();
    for file in order {
        let header = header(file);
        let left = room.saturating_sub(used + header.len());
        let hunks = diff::render(
            &file.old,
            &file.new,
            &file.lines,
            TRUNCATED_CONTEXT_LINES,
            left,
        );
        if hunks.text.is_empty() {
            hidden.push(file.file.path.clone());
            continue;
        }
        if hunks.left_out > 0 {
            partly.push(format!(
                "{} (its last {} changed lines)",
                file.file.path, hunks.left_out
            ));
        }
        used += header.len() + hunks.text.len() + 1;
        text.push_str(&header);
        text.push_str(&hunks.text);
        text.push('\n');
        shown += 1;
    }
    if !partly.is_empty() {
        let _ = writeln!(
            text,
            "Shown only in part, for lack of room: {}.",
            partly.join(", ")
        );
    }
    if !hidden.is_empty() {
        let _ = writeln!(text, "Not shown, for lack of room: {}.", hidden.join(", "));
    }
    Composed {
        tokens: plan::estimate_tokens(&text),
        text,
        condensed: true,
        files_shown: shown,
    }
}

fn diffed(file: &ChangedSides) -> Diffed<'_> {
    let skip = plan::skip_reason(file).map(plan::skip_text);
    let old = diff::lines_of(file.old.text.as_deref().unwrap_or(""));
    let new = diff::lines_of(file.new.text.as_deref().unwrap_or(""));
    // Without a readable new side there is nothing to show or count.
    let lines = if skip.is_none() {
        diff::diff(&old, &new)
    } else {
        Vec::new()
    };
    let added = lines
        .iter()
        .filter(|line| matches!(line, Line::Added { .. }))
        .count();
    let removed = lines
        .iter()
        .filter(|line| matches!(line, Line::Removed { .. }))
        .count();
    Diffed {
        status: plan::status_text(file),
        file,
        added,
        removed,
        skip,
        old,
        new,
        lines,
    }
}

fn header(file: &Diffed<'_>) -> String {
    format!("=== {} ({}) ===\n", file.file.path, file.status)
}

/// "Files this change touches (3), with the lines each adds and removes:"
/// and one line per file.
fn listing(files: &[Diffed<'_>]) -> String {
    let mut text = format!(
        "Files this change touches ({}), with the lines each adds and removes:\n",
        files.len()
    );
    for file in files {
        let _ = match file.skip {
            Some(skip) => writeln!(text, "- {} ({}, {skip})", file.file.path, file.status),
            None => writeln!(
                text,
                "- {} ({}, +{} -{})",
                file.file.path, file.status, file.added, file.removed
            ),
        };
    }
    text
}

/// Test code, as Insights tells it when it packs a file with its tests.
fn is_test(path: &str) -> bool {
    plan::is_test_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileSide, FileStatus, SideReason};

    fn side(text: &str) -> FileSide {
        FileSide {
            text: Some(text.to_string()),
            reason: None,
            size: text.len() as u64,
        }
    }

    fn changed(path: &str, old: &str, new: &str) -> ChangedSides {
        ChangedSides {
            path: path.to_string(),
            old_path: None,
            status: FileStatus::Modified,
            old: side(old),
            new: side(new),
        }
    }

    /// `count` numbered lines, line `changed` reading differently when set.
    fn numbered(count: usize, changed: Option<usize>) -> String {
        (1..=count)
            .map(|n| {
                if Some(n) == changed {
                    format!("changed line {n}\n")
                } else {
                    format!("line {n} of a reasonably long file\n")
                }
            })
            .collect()
    }

    #[test]
    fn a_change_that_fits_is_sent_whole_with_its_stats() {
        let files = [
            changed("a.go", "x\ny\n", "x\nz\nw\n"),
            ChangedSides {
                path: "logo.png".to_string(),
                old_path: None,
                status: FileStatus::Added,
                old: FileSide {
                    text: None,
                    reason: Some(SideReason::Absent),
                    size: 0,
                },
                new: FileSide {
                    text: None,
                    reason: Some(SideReason::Binary),
                    size: 10,
                },
            },
        ];
        let composed = compose(&files, 10_000);
        assert!(!composed.condensed);
        assert_eq!(composed.files_shown, 1);
        assert!(
            composed.text.contains("- a.go (modified, +2 -1)"),
            "{}",
            composed.text
        );
        assert!(composed
            .text
            .contains("- logo.png (added, binary, not shown)"));
        assert!(composed
            .text
            .contains("=== a.go (modified) ===\n@@ -1,2 +1,3 @@\n"));
        assert!(composed.text.contains("+    2 | z"));
        assert_eq!(composed.tokens, plan::estimate_tokens(&composed.text));
    }

    #[test]
    fn a_change_too_large_is_condensed_production_first_and_trimmed_to_fit() {
        let files = [
            changed(
                "a/service_test.go",
                &numbered(400, None),
                &numbered(400, Some(200)),
            ),
            changed(
                "a/service.go",
                &numbered(400, None),
                &numbered(400, Some(100)),
            ),
            changed(
                "b/big.go",
                &numbered(3000, None),
                &numbered(3000, Some(1500)),
            ),
        ];
        let whole = compose(&files, 1_000_000);
        assert!(!whole.condensed);
        let budget = 1_500;
        let composed = compose(&files, budget);
        assert!(composed.condensed);
        assert!(composed.tokens <= budget, "{} tokens", composed.tokens);
        assert!(composed
            .text
            .contains("This change is too large to send whole"));
        // Production code comes before its test, with 3 lines of context.
        let service = composed.text.find("=== a/service.go").unwrap();
        let test = composed.text.find("=== a/service_test.go").unwrap();
        assert!(service < test);
        assert!(composed.text.contains("@@ -97,7 +97,7 @@"));
        assert!(composed.text.contains("- b/big.go (modified, +1 -1)"));
    }

    #[test]
    fn what_does_not_fit_a_condensed_change_is_named() {
        let files = [
            changed("a.go", &numbered(50, None), &numbered(50, Some(10))),
            changed("b.go", &numbered(50, None), &numbered(50, Some(10))),
        ];
        // Room for the listing and one file's hunk only.
        let composed = compose(&files, 450);
        assert!(composed.condensed);
        assert_eq!(composed.files_shown, 1, "{}", composed.text);
        assert!(composed.text.contains("Not shown, for lack of room: b.go."));
    }

    #[test]
    fn tells_test_code_by_its_name() {
        for path in [
            "refund/service_test.go",
            "src/a.test.ts",
            "src/a.spec.js",
            "tests/test_refund.py",
            "src/__tests__/a.ts",
            "src/test/java/RefundTest.java",
        ] {
            assert!(is_test(path), "{path}");
        }
        for path in ["refund/service.go", "src/contest.ts", "src/attestation.go"] {
            assert!(!is_test(path), "{path}");
        }
    }
}
