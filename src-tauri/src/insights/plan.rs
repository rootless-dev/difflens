//! What an analysis sends: each changed file's diff, and how the files are
//! packed into calls that each fit the input budget.
//!
//! Tokens are estimated as characters / 4, which errs on the high side for
//! code. The whole change goes in one call when it fits; otherwise files are
//! packed by directory, a file's test kept with it when the names make that
//! obvious, and a directory is only split when it does not fit a call alone.

use std::collections::BTreeMap;

use super::diff;
use crate::model::{FileStatus, SideReason, SkipReason, SkippedFile};
use crate::review_state::ChangedSides as ChangedText;

/// Input a call may hold, prompt included.
pub const CALL_BUDGET_TOKENS: u64 = 100_000;

/// Unchanged lines shown around each change.
pub const CONTEXT_LINES: usize = 20;

/// Around each change of a file too large for a call.
pub const TRUNCATED_CONTEXT_LINES: usize = 3;

/// A call holds at least this much code, however long the list of changed
/// files in its prompt is.
const MIN_CONTENT_TOKENS: u64 = 8_000;

pub fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

/// A file as one call sends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInput {
    pub path: String,
    pub text: String,
    pub tokens: u64,
    /// Too large for a call: only its changed lines are sent.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prepared {
    Send(FileInput),
    Skip(SkippedFile),
}

/// How the file list names a change: "modified", "renamed from a.go".
pub fn status_text(file: &ChangedText) -> String {
    match (file.status, &file.old_path) {
        (FileStatus::Renamed, Some(old)) => format!("renamed from {old}"),
        (FileStatus::Added, _) => "added".to_string(),
        (FileStatus::Deleted, _) => "deleted".to_string(),
        (FileStatus::TypeChanged, _) => "type changed".to_string(),
        _ => "modified".to_string(),
    }
}

pub fn skip_text(reason: SkipReason) -> &'static str {
    match reason {
        SkipReason::Deleted => "not shown",
        SkipReason::Binary => "binary, not shown",
        SkipReason::TooLarge => "too large to read, not shown",
        SkipReason::NotText => "not text, not shown",
    }
}

/// Why a changed file is listed but not sent; `None` when it is sent.
pub fn skip_reason(file: &ChangedText) -> Option<SkipReason> {
    if file.status == FileStatus::Deleted {
        return Some(SkipReason::Deleted);
    }
    if file.new.text.is_some() {
        return None;
    }
    Some(match file.new.reason {
        Some(SideReason::Absent) => SkipReason::Deleted,
        Some(SideReason::TooLarge) => SkipReason::TooLarge,
        Some(SideReason::Binary) => SkipReason::Binary,
        _ => SkipReason::NotText,
    })
}

/// The file's section of a prompt, or why it is not sent. `max_tokens` is
/// what a call has room for besides the prompt around the files.
pub fn prepare(file: &ChangedText, max_tokens: u64) -> Prepared {
    let (Some(new_text), None) = (file.new.text.as_deref(), skip_reason(file)) else {
        return Prepared::Skip(SkippedFile {
            path: file.path.clone(),
            reason: skip_reason(file).unwrap_or(SkipReason::NotText),
        });
    };
    // An old side that cannot be read as text is diffed as empty: the whole
    // new side reads as added, which is still what the file now holds.
    let old = diff::lines_of(file.old.text.as_deref().unwrap_or(""));
    let new = diff::lines_of(new_text);
    let lines = diff::diff(&old, &new);
    let header = format!("=== {} ({}) ===\n", file.path, status_text(file));

    let whole = diff::render(&old, &new, &lines, CONTEXT_LINES, usize::MAX);
    let text = format!("{header}{}", whole.text);
    if estimate_tokens(&text) <= max_tokens {
        return send(file, text, false);
    }
    // Room for the hunks once the header and the note are written.
    let room = usize::try_from(max_tokens.saturating_mul(4))
        .unwrap_or(usize::MAX)
        .saturating_sub(header.len() + 400);
    let cut = diff::render(&old, &new, &lines, TRUNCATED_CONTEXT_LINES, room);
    let mut note = format!(
        "Note: this file's diff is too large to send whole, so only its changed lines with \
         {TRUNCATED_CONTEXT_LINES} lines of context are shown"
    );
    if cut.left_out > 0 {
        note.push_str(&format!(
            "; its last {} changed lines were left out",
            cut.left_out
        ));
    }
    send(file, format!("{header}{note}.\n{}", cut.text), true)
}

fn send(file: &ChangedText, text: String, truncated: bool) -> Prepared {
    Prepared::Send(FileInput {
        path: file.path.clone(),
        tokens: estimate_tokens(&text),
        text,
        truncated,
    })
}

/// What one call holds besides the files: at least `MIN_CONTENT_TOKENS`
/// whatever the prompt around them costs.
pub fn content_budget(overhead_tokens: u64) -> u64 {
    CALL_BUDGET_TOKENS
        .saturating_sub(overhead_tokens)
        .max(MIN_CONTENT_TOKENS)
}

/// One call: its files in prompt order, and the tokens they take.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Batch {
    pub paths: Vec<String>,
    pub content_tokens: u64,
}

/// Packs `files` into calls of at most `capacity` tokens of content each.
pub fn pack(files: &[FileInput], capacity: u64) -> Vec<Batch> {
    let mut packer = Packer {
        capacity,
        done: Vec::new(),
        current: Batch::default(),
    };
    for units in by_directory(files).values() {
        let tokens: u64 = units.iter().flatten().map(|file| file.tokens).sum();
        if packer.fits(tokens) {
            units.iter().flatten().for_each(|file| packer.add(file));
            continue;
        }
        if tokens <= capacity {
            packer.close();
            units.iter().flatten().for_each(|file| packer.add(file));
            continue;
        }
        for unit in units {
            let tokens: u64 = unit.iter().map(|file| file.tokens).sum();
            if !packer.fits(tokens) && tokens <= capacity {
                packer.close();
            }
            for file in unit {
                if !packer.fits(file.tokens) {
                    packer.close();
                }
                packer.add(file);
            }
        }
    }
    packer.close();
    packer.done
}

struct Packer {
    capacity: u64,
    done: Vec<Batch>,
    current: Batch,
}

impl Packer {
    fn fits(&self, tokens: u64) -> bool {
        self.current.content_tokens + tokens <= self.capacity
    }

    fn add(&mut self, file: &FileInput) {
        self.current.paths.push(file.path.clone());
        self.current.content_tokens += file.tokens;
    }

    fn close(&mut self) {
        if !self.current.paths.is_empty() {
            self.done.push(std::mem::take(&mut self.current));
        }
    }
}

/// Files by directory, in path order; within one, the files that go
/// together (a file and its tests), the file first.
fn by_directory(files: &[FileInput]) -> BTreeMap<String, Vec<Vec<&FileInput>>> {
    let mut units: BTreeMap<(String, String), Vec<&FileInput>> = BTreeMap::new();
    for file in files {
        units.entry(unit_key(&file.path)).or_default().push(file);
    }
    let mut directories: BTreeMap<String, Vec<Vec<&FileInput>>> = BTreeMap::new();
    for ((directory, _), mut unit) in units {
        unit.sort_by_key(|file| {
            (
                subject_of(file_name(&file.path)).is_some(),
                file.path.clone(),
            )
        });
        directories.entry(directory).or_default().push(unit);
    }
    directories
}

/// The directory a file belongs with (a `__tests__` folder belongs with the
/// one above it) and the name of what it is about.
pub fn unit_key(path: &str) -> (String, String) {
    let (directory, name) = match path.rsplit_once('/') {
        Some((directory, name)) => (directory, name),
        None => ("", path),
    };
    let directory = match directory.rsplit_once('/') {
        Some((parent, "__tests__")) => parent,
        None if directory == "__tests__" => "",
        _ => directory,
    };
    let subject = subject_of(name).unwrap_or_else(|| stem(name).to_string());
    (directory.to_string(), subject)
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Test code, by the common ways of naming it: a name that says what it
/// tests (see `subject_of`), or a file in a test folder.
pub fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let in_test_directory = lower.split('/').any(|part| {
        matches!(
            part,
            "test" | "tests" | "__tests__" | "spec" | "specs" | "testdata"
        )
    });
    in_test_directory || subject_of(file_name(path)).is_some()
}

/// `service` for `service.go`, `service.test.ts` and `test_service.py`.
fn stem(name: &str) -> &str {
    match name.split_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    }
}

/// What a test file tests, when its name says so in one of the common ways:
/// `service_test.go`, `service.test.ts`, `service.spec.js`,
/// `test_service.py`, `ServiceTest.java`, `ServiceTests.cs`.
fn subject_of(name: &str) -> Option<String> {
    let without_extension = name.rsplit_once('.').map_or(name, |(rest, _)| rest);
    for suffix in [".test", ".spec", "_test", "_spec", "-test", "-spec"] {
        if let Some(subject) = without_extension.strip_suffix(suffix) {
            if !subject.is_empty() {
                return Some(stem(subject).to_string());
            }
        }
    }
    if let Some(subject) = without_extension.strip_prefix("test_") {
        if !subject.is_empty() {
            return Some(subject.to_string());
        }
    }
    for suffix in ["Tests", "Test", "Spec"] {
        if let Some(subject) = without_extension.strip_suffix(suffix) {
            if subject.chars().last().is_some_and(char::is_alphanumeric) {
                return Some(subject.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FileSide;

    fn side(text: &str) -> FileSide {
        FileSide {
            text: Some(text.to_string()),
            reason: None,
            size: text.len() as u64,
        }
    }

    fn without_text(reason: SideReason) -> FileSide {
        FileSide {
            text: None,
            reason: Some(reason),
            size: 0,
        }
    }

    fn changed(path: &str, status: FileStatus, old: FileSide, new: FileSide) -> ChangedText {
        ChangedText {
            path: path.to_string(),
            old_path: None,
            status,
            old,
            new,
        }
    }

    fn input(path: &str, tokens: u64) -> FileInput {
        FileInput {
            path: path.to_string(),
            text: String::new(),
            tokens,
            truncated: false,
        }
    }

    fn paths(batches: &[Batch]) -> Vec<Vec<&str>> {
        batches
            .iter()
            .map(|batch| batch.paths.iter().map(String::as_str).collect())
            .collect()
    }

    #[test]
    fn estimates_a_token_per_four_characters() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        // Characters, not bytes.
        assert_eq!(estimate_tokens("ééééé"), 2);
    }

    #[test]
    fn sends_the_diff_with_context_and_lists_what_it_cannot_send() {
        let old: String = (1..=60).map(|n| format!("line {n}\n")).collect();
        let new = old.replace("line 30\n", "line thirty\n");
        let file = changed("a/b.rs", FileStatus::Modified, side(&old), side(&new));
        let Prepared::Send(input) = prepare(&file, 100_000) else {
            panic!("not sent");
        };
        assert!(input
            .text
            .starts_with("=== a/b.rs (modified) ===\n@@ -10,41 +10,41 @@\n"));
        assert!(input.text.contains("+   30 | line thirty\n"));
        assert!(input.text.contains("  10 | line 10\n"));
        assert!(!input.text.contains("| line 9\n"));
        assert!(!input.truncated);
        assert_eq!(input.tokens, estimate_tokens(&input.text));

        let skipped = |file: ChangedText| match prepare(&file, 100_000) {
            Prepared::Skip(skipped) => skipped.reason,
            Prepared::Send(_) => panic!("sent"),
        };
        assert_eq!(
            skipped(changed(
                "gone.go",
                FileStatus::Deleted,
                side("x\n"),
                without_text(SideReason::Absent)
            )),
            SkipReason::Deleted
        );
        assert_eq!(
            skipped(changed(
                "logo.png",
                FileStatus::Added,
                without_text(SideReason::Absent),
                without_text(SideReason::Binary)
            )),
            SkipReason::Binary
        );
        assert_eq!(
            skipped(changed(
                "dump.sql",
                FileStatus::Modified,
                side("x\n"),
                without_text(SideReason::TooLarge)
            )),
            SkipReason::TooLarge
        );
    }

    #[test]
    fn a_file_over_the_budget_is_sent_as_its_changed_lines_with_a_note() {
        let old: String = (1..=3000)
            .map(|n| format!("unchanged line {n}\n"))
            .collect();
        let new = old
            .replace("line 100\n", "line one hundred\n")
            .replace("line 2900\n", "line two thousand nine hundred\n");
        let file = changed("big.txt", FileStatus::Modified, side(&old), side(&new));

        let Prepared::Send(roomy) = prepare(&file, 100_000) else {
            panic!("not sent");
        };
        assert!(!roomy.truncated);

        let Prepared::Send(cut) = prepare(&file, 500) else {
            panic!("not sent");
        };
        assert!(cut.truncated);
        assert!(cut.text.contains("too large to send whole"));
        assert!(cut.text.contains("one hundred"));
        assert!(cut.text.contains("two thousand nine hundred"));
        assert!(!cut.text.contains("| unchanged line 90\n"));

        let Prepared::Send(tiny) = prepare(&file, 100) else {
            panic!("not sent");
        };
        assert!(tiny.text.contains("its last"), "{}", tiny.text);
        assert!(tiny.tokens <= 100, "{}", tiny.tokens);
    }

    #[test]
    fn an_added_file_reads_as_added_lines() {
        let file = changed(
            "new.py",
            FileStatus::Added,
            without_text(SideReason::Absent),
            side("a = 1\nb = 2\n"),
        );
        let Prepared::Send(input) = prepare(&file, 100_000) else {
            panic!("not sent");
        };
        assert_eq!(
            input.text,
            "=== new.py (added) ===\n@@ -0,0 +1,2 @@\n+    1 | a = 1\n+    2 | b = 2\n"
        );
    }

    #[test]
    fn a_change_that_fits_goes_in_one_call() {
        let files = [input("b/x.go", 10), input("a/y.go", 10), input("c.go", 10)];
        assert_eq!(paths(&pack(&files, 100)), [["c.go", "a/y.go", "b/x.go"]]);
    }

    #[test]
    fn files_are_packed_by_directory_with_their_tests_next_to_them() {
        let files = [
            input("refund/service.go", 30),
            input("payment/repo_test.go", 20),
            input("refund/service_test.go", 30),
            input("payment/repo.go", 20),
            input("refund/validate.go", 30),
            input("web/src/__tests__/cart.test.ts", 10),
            input("web/src/cart.ts", 10),
        ];
        let batches = pack(&files, 100);
        assert_eq!(
            paths(&batches),
            [
                vec!["payment/repo.go", "payment/repo_test.go"],
                vec![
                    "refund/service.go",
                    "refund/service_test.go",
                    "refund/validate.go"
                ],
                vec!["web/src/cart.ts", "web/src/__tests__/cart.test.ts"],
            ]
        );
        assert_eq!(batches[1].content_tokens, 90);
    }

    #[test]
    fn a_directory_too_large_for_a_call_is_split_between_files_that_go_together() {
        let files = [
            input("svc/a.go", 40),
            input("svc/a_test.go", 40),
            input("svc/b.go", 40),
            input("svc/b_test.go", 40),
            input("svc/huge.go", 100),
        ];
        assert_eq!(
            paths(&pack(&files, 100)),
            [
                vec!["svc/a.go", "svc/a_test.go"],
                vec!["svc/b.go", "svc/b_test.go"],
                vec!["svc/huge.go"],
            ]
        );
    }

    #[test]
    fn tells_tests_by_their_names_in_many_languages() {
        let key = |path: &str| unit_key(path);
        assert_eq!(key("a/service.go"), key("a/service_test.go"));
        assert_eq!(key("a/cart.ts"), key("a/cart.test.ts"));
        assert_eq!(key("a/cart.ts"), key("a/cart.spec.ts"));
        assert_eq!(key("a/cart.tsx"), key("a/__tests__/cart.test.tsx"));
        assert_eq!(key("pkg/models.py"), key("pkg/test_models.py"));
        assert_eq!(key("src/Refund.java"), key("src/RefundTest.java"));
        assert_eq!(key("src/Refund.cs"), key("src/RefundTests.cs"));
        assert_eq!(key("lib/refund.rb"), key("lib/refund_spec.rb"));
        assert_ne!(key("a/service.go"), key("b/service_test.go"));
        assert_ne!(key("a/latest.go"), key("a/la.go"));
        assert_eq!(key("Test.java"), ("".to_string(), "Test".to_string()));
        assert_eq!(subject_of("contest.go"), None);
    }
}
