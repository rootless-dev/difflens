//! The lines a review adds and removes, per file, read from a patch without
//! context lines (`git diff -U0`). Added lines are numbered on the new side
//! under the new path, removed lines on the old side under the old path, so a
//! renamed file needs no mapping: each side is looked up under its own name.

use std::collections::HashMap;

use crate::error::{app_error, AppResult};

/// Inclusive range of 1-based line numbers.
type LineRange = (u32, u32);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangedLines {
    added: HashMap<String, Vec<LineRange>>,
    removed: HashMap<String, Vec<LineRange>>,
}

impl ChangedLines {
    /// Every line of a file that only exists on the new side, such as an
    /// untracked file of the working tree.
    pub fn add_whole_file(&mut self, path: &str, lines: u32) {
        if lines > 0 {
            self.added
                .entry(path.to_string())
                .or_default()
                .push((1, lines));
        }
    }

    #[cfg(test)]
    pub fn with_added(mut self, path: &str, start: u32, end: u32) -> Self {
        self.added
            .entry(path.to_string())
            .or_default()
            .push((start, end));
        self
    }

    #[cfg(test)]
    pub fn with_removed(mut self, path: &str, start: u32, end: u32) -> Self {
        self.removed
            .entry(path.to_string())
            .or_default()
            .push((start, end));
        self
    }

    /// Lines added to `path` (new side) between `start` and `end`.
    pub fn added_within(&self, path: &str, start: u32, end: u32) -> u32 {
        count_within(self.added.get(path), start, end)
    }

    /// Lines removed from `path` (old side) between `start` and `end`.
    pub fn removed_within(&self, path: &str, start: u32, end: u32) -> u32 {
        count_within(self.removed.get(path), start, end)
    }

    /// Files with added lines, by their new path.
    pub fn added_paths(&self) -> impl Iterator<Item = &str> {
        self.added.keys().map(String::as_str)
    }

    /// Files with removed lines, by their old path.
    pub fn removed_paths(&self) -> impl Iterator<Item = &str> {
        self.removed.keys().map(String::as_str)
    }

    /// The lines added to each file on the new side, as sorted ranges that
    /// neither overlap nor touch; files by path.
    pub fn added_ranges(&self) -> Vec<(&str, Vec<LineRange>)> {
        let mut files: Vec<(&str, Vec<LineRange>)> = self
            .added
            .iter()
            .map(|(path, ranges)| {
                let mut sorted = ranges.clone();
                sorted.sort_unstable();
                let mut merged: Vec<LineRange> = Vec::with_capacity(sorted.len());
                for (first, last) in sorted {
                    match merged.last_mut() {
                        Some(previous) if first <= previous.1.saturating_add(1) => {
                            previous.1 = previous.1.max(last);
                        }
                        _ => merged.push((first, last)),
                    }
                }
                (path.as_str(), merged)
            })
            .filter(|(_, ranges)| !ranges.is_empty())
            .collect();
        files.sort_unstable_by(|left, right| left.0.cmp(right.0));
        files
    }

    /// Every line added to `path` on the new side, each once.
    pub fn added_lines(&self, path: &str) -> impl Iterator<Item = u32> + '_ {
        let mut ranges: Vec<LineRange> = self.added.get(path).cloned().unwrap_or_default();
        ranges.sort_unstable();
        let mut next = 1u32;
        ranges.into_iter().flat_map(move |(first, last)| {
            let start = first.max(next);
            next = next.max(last.saturating_add(1));
            start..=last
        })
    }
}

fn count_within(ranges: Option<&Vec<LineRange>>, start: u32, end: u32) -> u32 {
    ranges.map_or(0, |ranges| {
        ranges
            .iter()
            .map(|(first, last)| {
                let low = (*first).max(start);
                let high = (*last).min(end);
                if low <= high {
                    high - low + 1
                } else {
                    0
                }
            })
            .fold(0u32, u32::saturating_add)
    })
}

/// Parses the output of `git diff -U0 --src-prefix=a/ --dst-prefix=b/`.
pub fn parse_patch(patch: &[u8]) -> AppResult<ChangedLines> {
    let bad = || {
        app_error!(
            Internal,
            GitUnexpectedOutput,
            "Unexpected output from git {command}.",
            command = "diff -U0"
        )
    };
    let mut changed = ChangedLines::default();
    let mut old_path: Option<String> = None;
    let mut new_path: Option<String> = None;
    let mut lines = patch.split(|byte| *byte == b'\n');
    while let Some(line) = lines.next() {
        if line.starts_with(b"diff --git ") {
            old_path = None;
            new_path = None;
        } else if let Some(rest) = line.strip_prefix(b"--- ") {
            old_path = header_path(rest, b"a/").ok_or_else(bad)?;
        } else if let Some(rest) = line.strip_prefix(b"+++ ") {
            new_path = header_path(rest, b"b/").ok_or_else(bad)?;
        } else if line.starts_with(b"@@ ") {
            let (old_start, old_count, new_start, new_count) = hunk_header(line).ok_or_else(bad)?;
            if old_count > 0 {
                let path = old_path.as_ref().ok_or_else(bad)?;
                changed
                    .removed
                    .entry(path.clone())
                    .or_default()
                    .push(range(old_start, old_count));
            }
            if new_count > 0 {
                let path = new_path.as_ref().ok_or_else(bad)?;
                changed
                    .added
                    .entry(path.clone())
                    .or_default()
                    .push(range(new_start, new_count));
            }
            // The body is skipped by count: a removed line reading "-- x"
            // must not be taken for a file header.
            let mut remaining = u64::from(old_count) + u64::from(new_count);
            while remaining > 0 {
                match lines.next().ok_or_else(bad)?.first() {
                    Some(b'-' | b'+') => remaining -= 1,
                    Some(b'\\') => {}
                    _ => return Err(bad()),
                }
            }
        }
    }
    Ok(changed)
}

fn range(start: u32, count: u32) -> LineRange {
    (start, start.saturating_add(count - 1))
}

/// `@@ -<start>[,<count>] +<start>[,<count>] @@ ...`
fn hunk_header(line: &[u8]) -> Option<(u32, u32, u32, u32)> {
    let text = std::str::from_utf8(line.get(3..)?).ok()?;
    let mut parts = text.split(' ');
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let side = |part: &str| -> Option<(u32, u32)> {
        match part.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((part.parse().ok()?, 1)),
        }
    };
    let (old_start, old_count) = side(old)?;
    let (new_start, new_count) = side(new)?;
    Some((old_start, old_count, new_start, new_count))
}

/// The path of a `---` or `+++` line: `None` inside for `/dev/null`. git
/// quotes a path with unusual characters, and ends a path that holds a space
/// with a tab.
fn header_path(field: &[u8], prefix: &[u8]) -> Option<Option<String>> {
    if field == b"/dev/null" {
        return Some(None);
    }
    let raw = if field.first() == Some(&b'"') {
        unquote(field)?
    } else {
        field.strip_suffix(b"\t").unwrap_or(field).to_vec()
    };
    let path = raw.strip_prefix(prefix)?;
    Some(Some(String::from_utf8_lossy(path).into_owned()))
}

/// Undoes git's C-style quoting: `"a\tb\303\251"`.
fn unquote(field: &[u8]) -> Option<Vec<u8>> {
    let inner = field.strip_prefix(b"\"")?.strip_suffix(b"\"")?;
    let mut bytes = Vec::with_capacity(inner.len());
    let mut rest = inner.iter().copied();
    while let Some(byte) = rest.next() {
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        let escaped = rest.next()?;
        let plain = match escaped {
            b'a' => 0x07,
            b'b' => 0x08,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 0x0b,
            b'f' => 0x0c,
            b'r' => b'\r',
            b'"' | b'\\' => escaped,
            b'0'..=b'7' => {
                let digits = [escaped, rest.next()?, rest.next()?];
                let mut value: u32 = 0;
                for digit in digits {
                    if !(b'0'..=b'7').contains(&digit) {
                        return None;
                    }
                    value = value * 8 + u32::from(digit - b'0');
                }
                u8::try_from(value).ok()?
            }
            _ => return None,
        };
        bytes.push(plain);
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{isolated_git, TestRepo};

    #[test]
    fn hunks_become_line_ranges_per_side() {
        let patch = b"diff --git a/a.go b/a.go\n\
index 1..2 100644\n\
--- a/a.go\n\
+++ b/a.go\n\
@@ -3,2 +3 @@ func A() {\n\
-old one\n\
--- a/looks like a header\n\
+new\n\
@@ -10,0 +10,2 @@\n\
+x\n\
+y\n\
\\ No newline at end of file\n\
@@ -20 +21,0 @@\n\
-gone\n";
        let changed = parse_patch(patch).unwrap();
        assert_eq!(changed.removed["a.go"], vec![(3, 4), (20, 20)]);
        assert_eq!(changed.added["a.go"], vec![(3, 3), (10, 11)]);
        assert_eq!(changed.added_within("a.go", 1, 10), 2);
        assert_eq!(changed.added_within("a.go", 11, 100), 1);
        assert_eq!(changed.removed_within("a.go", 4, 20), 2);
        assert_eq!(changed.added_within("other.go", 1, 100), 0);
    }

    #[test]
    fn added_and_deleted_files_have_one_side() {
        let patch = b"diff --git a/new.go b/new.go\n\
new file mode 100644\n\
--- /dev/null\n\
+++ b/new.go\n\
@@ -0,0 +1,3 @@\n\
+a\n\
+b\n\
+c\n\
diff --git a/old.go b/old.go\n\
deleted file mode 100644\n\
--- a/old.go\n\
+++ /dev/null\n\
@@ -1,2 +0,0 @@\n\
-a\n\
-b\n";
        let changed = parse_patch(patch).unwrap();
        assert_eq!(changed.added["new.go"], vec![(1, 3)]);
        assert_eq!(changed.removed["old.go"], vec![(1, 2)]);
        assert!(!changed.added.contains_key("old.go"));
    }

    #[test]
    fn quoted_and_spaced_paths_are_read_back() {
        let patch = b"diff --git \"a/t\\tb\" \"b/t\\tb\"\n\
--- \"a/t\\tb\"\n\
+++ \"b/caf\\303\\251\"\n\
@@ -1 +1 @@\n\
-x\n\
+y\n\
diff --git a/x y.go b/z y.go\n\
--- a/x y.go\t\n\
+++ b/z y.go\t\n\
@@ -2 +2 @@\n\
-b\n\
+c\n";
        let changed = parse_patch(patch).unwrap();
        assert_eq!(changed.removed["t\tb"], vec![(1, 1)]);
        assert_eq!(changed.added["caf\u{e9}"], vec![(1, 1)]);
        assert_eq!(changed.removed["x y.go"], vec![(2, 2)]);
        assert_eq!(changed.added["z y.go"], vec![(2, 2)]);
    }

    #[test]
    fn malformed_patches_are_errors() {
        assert!(parse_patch(b"@@ -1 +1 @@\n-x\n+y\n").is_err());
        assert!(parse_patch(b"--- a/x\n+++ b/x\n@@ -1 +1 @@\n-x\n").is_err());
        assert!(parse_patch(b"--- a/x\n+++ b/x\n@@ nonsense @@\n").is_err());
        assert!(parse_patch(b"").unwrap() == ChangedLines::default());
    }

    #[test]
    fn added_lines_are_listed_once_and_paths_per_side() {
        let changed = ChangedLines::default()
            .with_added("a.go", 5, 7)
            .with_added("a.go", 1, 2)
            .with_added("a.go", 6, 9)
            .with_removed("old.go", 1, 1);
        assert_eq!(
            changed.added_lines("a.go").collect::<Vec<_>>(),
            vec![1, 2, 5, 6, 7, 8, 9]
        );
        assert_eq!(changed.added_lines("none.go").count(), 0);
        assert_eq!(changed.added_paths().collect::<Vec<_>>(), vec!["a.go"]);
        assert_eq!(changed.removed_paths().collect::<Vec<_>>(), vec!["old.go"]);
    }

    #[test]
    fn added_ranges_are_sorted_and_merged_per_file() {
        let changed = ChangedLines::default()
            .with_added("b.go", 9, 12)
            .with_added("b.go", 1, 3)
            .with_added("b.go", 4, 5)
            .with_added("b.go", 10, 11)
            .with_added("a.go", 2, 2)
            .with_removed("c.go", 1, 1);
        assert_eq!(
            changed.added_ranges(),
            vec![("a.go", vec![(2, 2)]), ("b.go", vec![(1, 5), (9, 12)])]
        );
    }

    #[test]
    fn whole_files_count_every_line() {
        let mut changed = ChangedLines::default();
        changed.add_whole_file("u.go", 5);
        changed.add_whole_file("empty.go", 0);
        assert_eq!(changed.added_within("u.go", 2, 9), 4);
        assert!(!changed.added.contains_key("empty.go"));
    }

    #[test]
    fn reads_a_real_rename_with_edits() {
        let repo = TestRepo::init();
        let body: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        repo.commit_file("old name.go", &body, "first");
        let first = repo.git(&["rev-parse", "HEAD"]);
        repo.git(&["mv", "old name.go", "new name.go"]);
        repo.write(
            "new name.go",
            body.replace("line 5\n", "line five\nextra\n"),
        );
        repo.commit_all("rename");
        let second = repo.git(&["rev-parse", "HEAD"]);

        let patch = isolated_git()
            .diff_patch(repo.path(), &first, Some(&second))
            .unwrap();
        let changed = parse_patch(&patch).unwrap();

        assert_eq!(changed.removed["old name.go"], vec![(5, 5)]);
        assert_eq!(changed.added["new name.go"], vec![(5, 6)]);
    }
}
