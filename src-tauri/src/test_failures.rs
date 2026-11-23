//! Why a test or a package failed, read from what it printed, so the Tests
//! tab can show the key part of a failure without the whole output. Pure.
//!
//! Recognised, in this order: a data race (the race detector's report: for
//! each access, the first frame in the project's own code and where its
//! goroutine was created), a test that timed out (the tests still running
//! and where), a panic (its message and the first frame in the project's
//! code), an assertion (testify's "Error Trace / Error / expected / actual"
//! block, else the first "file.go:12: message" line a test printed), and for
//! packages a build that failed (the compiler's "file.go:47:31: message"
//! lines). Anything else keeps its last meaningful lines.
//!
//! Other languages are read after Go, from the text alone, so that Go's
//! output reads as it always did: Rust (libtest's "thread '…' panicked at"
//! with `assert_eq!`'s left and right, rustc's "error[E…]" over " --> "),
//! Python (pytest's short traceback with its "E   " lines, unittest's
//! "Traceback (most recent call last):", and for modules that did not
//! import the error and where), then the JavaScript runners (jest's and
//! vitest's "Expected" / "Received" and diffs, node:assert's errors as mocha
//! and node:test print them, node:test's TAP, the first stack frame in the
//! project, and for files that did not load the error and where).
//!
//! Stacks and compilers print absolute paths of the run's own copy of the
//! tree, which is gone by then; `Locator` finds the project file they name
//! by the longest suffix that is a file of the project.

use std::borrow::Cow;
use std::collections::HashSet;

use crate::model::{
    BuildError, FailureFrame, PackageOutcomeKind, RaceAccess, RaceAccessKind, RaceSummary,
    SourceRef, TestFailure,
};

/// Lines kept when nothing else is recognised.
const FALLBACK_LINES: usize = 6;
/// Most of anything kept from one failure: errors, races, running tests.
const MAX_ITEMS: usize = 20;
/// Lines of code kept under a build error of another language than Go.
const SNIPPET_LINES: usize = 8;
/// Most "where" lines of pytest kept with an assertion's message.
const WHERE_LINES: usize = 3;

const RACE_START: &str = "WARNING: DATA RACE";
const RACE_RULE: &str = "==================";
const TIMED_OUT: &str = "panic: test timed out after";
const TRACEBACK: &str = "Traceback (most recent call last):";
const JEST_SUITE_FAILED: &str = "● Test suite failed to run";

/// Finds the project's files that paths printed by a run name.
pub struct Locator {
    files: HashSet<String>,
}

impl Locator {
    /// `files`: the project's files, relative to the tree with "/".
    pub fn new(files: impl IntoIterator<Item = String>) -> Self {
        Self {
            files: files.into_iter().collect(),
        }
    }

    /// The project file `path` names, as a stack or a compiler prints it
    /// (a "file://" URL too); None for a file of a toolchain or of a
    /// dependency. Relative paths are the compiler's or the runner's,
    /// relative to the module, the crate or the folder it ran in: one that
    /// is not a project file is the only project file ending with it (a
    /// project in a folder of the tree), else kept as printed.
    pub fn relative(&self, path: &str) -> Option<String> {
        let path = path.trim();
        let decoded;
        let path = match path.strip_prefix("file://") {
            Some(rest) => {
                decoded = percent_decoded(rest);
                decoded.as_str()
            }
            None => path,
        };
        let path = path.trim_start_matches("./");
        if foreign(path) {
            return None;
        }
        if self.files.contains(path) {
            return Some(path.to_string());
        }
        for (index, _) in path.match_indices('/') {
            let suffix = &path[index + 1..];
            if self.files.contains(suffix) {
                return Some(suffix.to_string());
            }
        }
        if !path.starts_with('/') {
            if path.contains('/') {
                if let Some(file) = self.ending_with(path) {
                    return Some(file);
                }
            }
            return (!path.is_empty()).then(|| path.to_string());
        }
        if outside_project(path) {
            return None;
        }
        // Unknown files of the run's copy: "<run folder>/tree/<file>".
        path.rsplit_once("/tree/").map(|(_, file)| file.to_string())
    }

    /// The file a test printed by its base name, such as "service_test.go":
    /// `hint` (the test's own file) when it has that name, else the only
    /// project file with it, else the name as printed.
    fn short(&self, name: &str, hint: Option<&str>) -> String {
        if let Some(hint) = hint.filter(|hint| base_name(hint) == name) {
            return hint.to_string();
        }
        if name.contains('/') {
            return self.relative(name).unwrap_or_else(|| name.to_string());
        }
        let mut matches = self.files.iter().filter(|file| base_name(file) == name);
        match (matches.next(), matches.next()) {
            (Some(only), None) => only.clone(),
            _ => name.to_string(),
        }
    }

    /// The only project file whose path ends with "/`path`".
    fn ending_with(&self, path: &str) -> Option<String> {
        let suffix = format!("/{path}");
        let mut matches = self.files.iter().filter(|file| file.ends_with(&suffix));
        match (matches.next(), matches.next()) {
            (Some(only), None) => Some(only.clone()),
            _ => None,
        }
    }
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The toolchain's sources and the module cache.
fn outside_project(path: &str) -> bool {
    ["/pkg/mod/", "/go/src/", "/libexec/src/", "/toolchain@", "/goroot/src/"]
        .iter()
        .any(|marker| path.contains(marker))
}

/// What other languages' stacks name outside the project, absolute or
/// relative: cargo's registry and git checkouts, the Rust toolchain,
/// node_modules and node's own modules, Python's installed packages and
/// standard library, and what is not a file ("<anonymous>", "<frozen …>").
fn foreign(path: &str) -> bool {
    if path.starts_with("node:") || path.starts_with('<') {
        return true;
    }
    let path = format!("/{}", path.trim_start_matches('/'));
    let markers = [
        "/.cargo/registry/",
        "/.cargo/git/",
        "/.rustup/toolchains/",
        "/node_modules/",
        "/site-packages/",
        "/dist-packages/",
        "/lib/python3.",
    ];
    markers.iter().any(|marker| path.contains(marker)) || rust_sources(&path)
}

/// "/rustc/<commit>/library/…": the sources of the Rust toolchain.
fn rust_sources(path: &str) -> bool {
    path.match_indices("/rustc/").any(|(index, marker)| {
        let rest = &path[index + marker.len()..];
        let (commit, _) = rest.split_once('/').unwrap_or((rest, ""));
        commit.len() == 40 && commit.chars().all(|c| c.is_ascii_hexdigit())
    })
}

/// A "file://" URL's path with its "%20"s and such decoded.
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let digit = |byte: u8| (byte as char).to_digit(16).map(|digit| digit as u8);
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (digit(bytes[index + 1]), digit(bytes[index + 2])) {
                decoded.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Whether a project file is test code: Go's "_test.go"; for the others a
/// file in a test folder ("tests/", "test/", "__tests__/", "spec/"),
/// "test_*.py", "*_test.py", "*.test.*" or "*.spec.*".
fn is_test_file(file: &str) -> bool {
    let name = base_name(file);
    if name.ends_with(".go") {
        return name.ends_with("_test.go");
    }
    let in_tests = file
        .split('/')
        .rev()
        .skip(1)
        .any(|folder| matches!(folder, "tests" | "test" | "__tests__" | "spec"));
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    in_tests
        || name.starts_with("test_")
        || stem.ends_with("_test")
        || name.contains(".test.")
        || name.contains(".spec.")
}

/// Why a failed test failed. `races` are reports the runner attributed to
/// it or its subtests, used when its output lost them; `test_file` is where
/// its function is, to place a message printed with a base name.
pub fn test_failure(
    output: &str,
    races: &[&str],
    test_file: Option<&str>,
    locator: &Locator,
) -> TestFailure {
    let lines: Vec<&str> = output.lines().collect();
    let mut summaries = race_summaries(&lines, locator);
    if summaries.is_empty() && (output.contains("race detected") || output.contains(RACE_START)) {
        for race in races {
            let lines: Vec<&str> = race.lines().collect();
            for summary in race_summaries(&lines, locator) {
                add_race(&mut summaries, summary);
            }
        }
    }
    if !summaries.is_empty() {
        summaries.truncate(MAX_ITEMS);
        return TestFailure::Race { races: summaries };
    }
    if let Some(timeout) = timeout(&lines, locator) {
        return timeout;
    }
    if let Some(panic) = panic(&lines, locator) {
        return panic;
    }
    if let Some(assertion) = assertion(&lines, test_file, locator) {
        return assertion;
    }
    let text = without_colours(output);
    let lines: Vec<&str> = text.lines().collect();
    rust_failure(&lines, locator)
        .or_else(|| python_failure(&lines, locator))
        .or_else(|| js_failure(&lines, locator))
        .unwrap_or_else(|| other(&lines))
}

/// Why a package failed outside its tests: the build errors when it did not
/// build (Go's, else rustc's, a Python module's that did not import, a
/// JavaScript file's that did not load), else as a test's output reads.
pub fn package_failure(output: &str, outcome: PackageOutcomeKind, locator: &Locator) -> TestFailure {
    let lines: Vec<&str> = output.lines().collect();
    if outcome == PackageOutcomeKind::BuildFailed {
        let errors = build_errors(&lines, locator);
        if !errors.is_empty() {
            return TestFailure::Build { errors };
        }
        let text = without_colours(output);
        let lines: Vec<&str> = text.lines().collect();
        let mut errors = rust_build_errors(&lines, locator);
        if errors.is_empty() {
            errors = python_build_errors(&lines, locator);
        }
        if errors.is_empty() {
            errors = js_build_errors(&lines, locator);
        }
        if !errors.is_empty() {
            errors.truncate(MAX_ITEMS);
            return TestFailure::Build { errors };
        }
        return other(&lines);
    }
    test_failure(output, &[], None, locator)
}

/// A frame as a stack prints it: the function, then its file and line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RawFrame {
    function: String,
    path: String,
    line: u32,
}

/// "      /work/app/service.go:661 +0x84" as a path and a line.
fn location(line: &str) -> Option<(String, u32)> {
    let text = line.trim();
    let text = match text.rsplit_once(" +0x") {
        Some((before, after)) if after.chars().all(|c| c.is_ascii_hexdigit()) => before,
        _ => text,
    };
    let (path, number) = text.rsplit_once(':')?;
    let number = number.parse().ok()?;
    (path.ends_with(".go") || path.ends_with(".s")).then(|| (path.to_string(), number))
}

/// "app/services/auth_service.(*Service).recordLastLogin(0xc000…)" as
/// "(*Service).recordLastLogin".
fn short_function(line: &str) -> String {
    let mut text = line.trim();
    if let Some(rest) = text.strip_prefix("created by ") {
        text = rest.split(" in goroutine ").next().unwrap_or(rest);
    }
    let text = strip_arguments(text);
    let last = text.rsplit('/').next().unwrap_or(text);
    match last.split_once('.') {
        Some((_, name)) if !name.is_empty() => name.to_string(),
        _ => last.to_string(),
    }
}

/// Drops the argument list a stack prints after a function: "f(0x1, …)",
/// "f()" or "f(...)".
fn strip_arguments(text: &str) -> &str {
    if !text.ends_with(')') {
        return text;
    }
    let mut depth = 0usize;
    for (index, c) in text.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    return &text[..index];
                }
            }
            _ => {}
        }
    }
    text
}

/// The frames of `lines`: each function line followed by its location.
fn frames(lines: &[&str]) -> Vec<RawFrame> {
    let mut frames = Vec::new();
    for pair in lines.windows(2) {
        let function = pair[0].trim();
        if function.is_empty() || location(function).is_some() {
            continue;
        }
        if let Some((path, line)) = location(pair[1]) {
            frames.push(RawFrame {
                function: short_function(function),
                path,
                line,
            });
        }
    }
    frames
}

/// The first of `frames` in the project's own code.
fn project_frame(frames: &[RawFrame], locator: &Locator) -> Option<FailureFrame> {
    frames.iter().find_map(|frame| {
        let file = locator.relative(&frame.path)?;
        Some(FailureFrame {
            function: frame.function.clone(),
            test: is_test_file(&file),
            file,
            line: frame.line,
        })
    })
}

/// The header of an access in a race report: "Write at 0x… by goroutine
/// 214:", "Previous read of size 8 at 0x… by main goroutine:".
fn access_header(line: &str) -> Option<(RaceAccessKind, bool, Option<u32>)> {
    let text = line.trim().to_ascii_lowercase();
    let text = text.strip_suffix(':')?;
    let (what, who) = text.split_once(" by ")?;
    if !what.contains(" at 0x") {
        return None;
    }
    let (previous, what) = match what.strip_prefix("previous ") {
        Some(rest) => (true, rest),
        None => (false, what),
    };
    let what = what.strip_prefix("atomic ").unwrap_or(what);
    let kind = if what.starts_with("write") {
        RaceAccessKind::Write
    } else if what.starts_with("read") {
        RaceAccessKind::Read
    } else {
        return None;
    };
    let goroutine = who.strip_prefix("goroutine ").and_then(|n| n.trim().parse().ok());
    if goroutine.is_none() && who != "main goroutine" {
        return None;
    }
    Some((kind, previous, goroutine))
}

/// "Goroutine 214 (running) created at:" as 214.
fn creation_header(line: &str) -> Option<u32> {
    let text = line.trim();
    let rest = text.strip_prefix("Goroutine ")?;
    if !text.ends_with("created at:") {
        return None;
    }
    rest.split_whitespace().next()?.parse().ok()
}

/// Every race report in `lines`, summarised, without repeats.
fn race_summaries(lines: &[&str], locator: &Locator) -> Vec<RaceSummary> {
    let mut summaries = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if !lines[index].trim_start().starts_with(RACE_START) {
            index += 1;
            continue;
        }
        let start = index + 1;
        let mut end = start;
        while end < lines.len()
            && !lines[end].trim_start().starts_with(RACE_RULE)
            && !lines[end].trim_start().starts_with(RACE_START)
        {
            end += 1;
        }
        if let Some(summary) = race_summary(&lines[start..end], locator) {
            add_race(&mut summaries, summary);
        }
        index = end.max(start);
    }
    summaries
}

/// Adds `summary` unless the same race is there: the same accesses at the
/// same places, whatever the goroutines' numbers (a race repeated in each
/// subtest).
fn add_race(summaries: &mut Vec<RaceSummary>, summary: RaceSummary) {
    let places = |summary: &RaceSummary| -> Vec<_> {
        summary
            .accesses
            .iter()
            .map(|access| (access.kind, access.previous, access.frame.clone(), access.created_at.clone()))
            .collect()
    };
    let key = places(&summary);
    if !summaries.iter().any(|seen| places(seen) == key) {
        summaries.push(summary);
    }
}

enum Section {
    Access(RaceAccessKind, bool, Option<u32>),
    Created(u32),
}

fn race_summary(lines: &[&str], locator: &Locator) -> Option<RaceSummary> {
    let mut sections: Vec<(Section, Vec<&str>)> = Vec::new();
    for line in lines {
        if let Some((kind, previous, goroutine)) = access_header(line) {
            sections.push((Section::Access(kind, previous, goroutine), Vec::new()));
        } else if let Some(goroutine) = creation_header(line) {
            sections.push((Section::Created(goroutine), Vec::new()));
        } else if let Some((_, body)) = sections.last_mut() {
            body.push(line);
        }
    }
    let created = |wanted: u32| {
        sections.iter().find_map(|(section, body)| match section {
            Section::Created(goroutine) if *goroutine == wanted => {
                Some(project_frame(&frames(body), locator))
            }
            _ => None,
        })
    };
    let accesses: Vec<RaceAccess> = sections
        .iter()
        .filter_map(|(section, body)| match section {
            Section::Access(kind, previous, goroutine) => Some(RaceAccess {
                kind: *kind,
                previous: *previous,
                goroutine: *goroutine,
                frame: project_frame(&frames(body), locator),
                created_at: goroutine.and_then(|goroutine| created(goroutine).flatten()),
            }),
            Section::Created(_) => None,
        })
        .collect();
    (!accesses.is_empty()).then_some(RaceSummary { accesses })
}

/// The test a line of a parent's output belongs to, from the "=== RUN"
/// lines before it.
fn context_at(lines: &[&str], index: usize) -> Option<String> {
    lines[..index].iter().rev().find_map(|line| {
        ["=== RUN", "=== CONT", "=== NAME"].iter().find_map(|marker| {
            let name = line.trim_start().strip_prefix(marker)?.trim();
            (!name.is_empty()).then(|| name.to_string())
        })
    })
}

fn timeout(lines: &[&str], locator: &Locator) -> Option<TestFailure> {
    let start = lines.iter().position(|line| line.trim_start().starts_with(TIMED_OUT))?;
    let message = lines[start].trim().trim_start_matches("panic: ").to_string();
    let mut running = Vec::new();
    let mut index = start + 1;
    if lines.get(index).is_some_and(|line| line.trim() == "running tests:") {
        index += 1;
        while let Some(line) = lines.get(index) {
            let name = line.trim();
            if name.is_empty() || !line.starts_with(char::is_whitespace) {
                break;
            }
            let name = name.split(" (").next().unwrap_or(name);
            if running.len() < MAX_ITEMS {
                running.push(name.to_string());
            }
            index += 1;
        }
    }
    let frame = project_frame(&frames(&lines[start..]), locator);
    Some(TestFailure::Timeout {
        message,
        frame,
        running,
    })
}

fn panic(lines: &[&str], locator: &Locator) -> Option<TestFailure> {
    let start = lines.iter().position(|line| {
        let line = line.trim_start();
        line.starts_with("panic: ") || line.starts_with("fatal error: ")
    })?;
    let first = lines[start].trim();
    let mut message = first
        .strip_prefix("panic: ")
        .or_else(|| first.strip_prefix("fatal error: "))
        .unwrap_or(first)
        .trim_end_matches(" [recovered]")
        .to_string();
    // A recovered and repanicked value prints the original next.
    if let Some(original) = lines
        .get(start + 1)
        .and_then(|line| line.trim().strip_prefix("panic: "))
    {
        message = original.trim_end_matches(" [recovered]").to_string();
    }
    let frame = project_frame(&frames(&lines[start..]), locator);
    Some(TestFailure::Panic {
        message,
        frame,
        test: context_at(lines, start),
    })
}

/// "    service_test.go:88: message" as the file, the line and the message.
fn located_message(line: &str) -> Option<(&str, u32, &str)> {
    if !line.starts_with(char::is_whitespace) {
        return None;
    }
    let text = line.trim_start();
    let (file, rest) = text.split_once(':')?;
    if !file.ends_with(".go") || file.contains(' ') || file == "testing.go" {
        return None;
    }
    let (number, message) = rest.split_once(':')?;
    let number = number.parse().ok()?;
    Some((file, number, message.trim()))
}

const TESTIFY_FIELDS: [&str; 6] = ["Error Trace", "Error", "Test", "Messages", "Diff", "Stack"];

fn testify_field(line: &str) -> Option<(&'static str, &str)> {
    let text = line.trim_start();
    TESTIFY_FIELDS.iter().find_map(|field| {
        let rest = text.strip_prefix(field)?.strip_prefix(':')?;
        Some((*field, rest.trim()))
    })
}

fn assertion(lines: &[&str], test_file: Option<&str>, locator: &Locator) -> Option<TestFailure> {
    if let Some(start) = lines.iter().position(|line| line.trim_start().starts_with("Error Trace:")) {
        return Some(testify(lines, start, test_file, locator));
    }
    let (index, (file, line, message)) = lines
        .iter()
        .enumerate()
        .find_map(|(index, line)| located_message(line).map(|found| (index, found)))?;
    let mut message = message.to_string();
    // A message of several lines goes on more indented.
    let indent = lines[index].len() - lines[index].trim_start().len();
    for next in &lines[index + 1..] {
        let deeper = next.len() - next.trim_start().len() > indent && !next.trim().is_empty();
        if !deeper || located_message(next).is_some() {
            break;
        }
        message.push('\n');
        message.push_str(next.trim());
    }
    Some(TestFailure::Assertion {
        message: message.trim().to_string(),
        expected: None,
        actual: None,
        at: Some(SourceRef {
            file: locator.short(file, test_file),
            line,
        }),
        test: context_at(lines, index),
    })
}

/// testify's block, from its "Error Trace:" line.
fn testify(lines: &[&str], start: usize, test_file: Option<&str>, locator: &Locator) -> TestFailure {
    let mut fields: Vec<(&str, Vec<String>)> = Vec::new();
    for line in &lines[start..] {
        if !line.starts_with(char::is_whitespace) || line.trim_start().starts_with("--- ") {
            break;
        }
        if let Some((field, value)) = testify_field(line) {
            if fields.iter().any(|(seen, _)| *seen == field) {
                break; // the next assertion
            }
            fields.push((field, if value.is_empty() { Vec::new() } else { vec![value.to_string()] }));
        } else if let Some((_, values)) = fields.last_mut() {
            let value = line.trim();
            if !value.is_empty() {
                values.push(value.to_string());
            }
        }
    }
    let field = |name: &str| fields.iter().find(|(field, _)| *field == name).map(|(_, values)| values);
    let error = field("Error").cloned().unwrap_or_default();
    let mut message = error.first().cloned().unwrap_or_default();
    let mut expected = None;
    let mut actual = None;
    let mut rest = Vec::new();
    for value in error.iter().skip(1) {
        if let Some(value) = value.strip_prefix("expected:") {
            expected = Some(value.trim().to_string());
        } else if let Some(value) = value.strip_prefix("actual") {
            actual = Some(value.trim_start().trim_start_matches(':').trim().to_string());
        } else if expected.is_none() && actual.is_none() {
            rest.push(value.clone());
        }
    }
    // "Received unexpected error:" says the error on the lines after it.
    if expected.is_none() && actual.is_none() && !rest.is_empty() {
        actual = Some(rest.join("\n"));
    }
    message = message.trim_end_matches(':').trim().to_string();
    let at = field("Error Trace")
        .and_then(|values| values.first())
        .and_then(|trace| {
            let (path, line) = trace.rsplit_once(':')?;
            Some(SourceRef {
                file: locator.short(path, test_file),
                line: line.trim().parse().ok()?,
            })
        })
        .or_else(|| {
            let (file, line, _) = lines[..start].iter().rev().find_map(|line| located_message(line))?;
            Some(SourceRef {
                file: locator.short(file, test_file),
                line,
            })
        });
    let test = field("Test")
        .and_then(|values| values.first().cloned())
        .or_else(|| context_at(lines, start));
    TestFailure::Assertion {
        message,
        expected,
        actual,
        at,
        test,
    }
}

/// "app/containers/auth_container.go:47:31: message" and the indented lines
/// after it.
fn build_errors(lines: &[&str], locator: &Locator) -> Vec<BuildError> {
    let mut errors: Vec<BuildError> = Vec::new();
    for line in lines {
        if line.starts_with('\t') || line.starts_with("    ") {
            if let Some(last) = errors.last_mut() {
                if last.detail.len() < MAX_ITEMS {
                    last.detail.push(line.trim().to_string());
                }
            }
            continue;
        }
        if let Some(error) = build_error(line, locator) {
            if errors.len() < MAX_ITEMS {
                errors.push(error);
            }
        }
    }
    errors
}

fn build_error(line: &str, locator: &Locator) -> Option<BuildError> {
    let text = line.trim();
    if text.starts_with("# ") || text.starts_with("FAIL") || text.starts_with("ok ") {
        return None;
    }
    let at = text.find(".go:")? + 3;
    let path = &text[..at];
    let mut rest = text[at + 1..].splitn(3, ':');
    let line_number = rest.next()?.parse().ok()?;
    let (column, message) = match (rest.next(), rest.next()) {
        (Some(column), Some(message)) => match column.parse() {
            Ok(column) => (Some(column), message.trim().to_string()),
            Err(_) => (None, format!("{column}:{message}").trim().to_string()),
        },
        (Some(message), None) => (None, message.trim().to_string()),
        _ => return None,
    };
    Some(BuildError {
        file: locator.relative(path).or_else(|| Some(path.to_string())),
        line: Some(line_number),
        column,
        message,
        detail: Vec::new(),
    })
}

/// The last lines that say something: not the framework's own.
fn other(lines: &[&str]) -> TestFailure {
    let noise = |line: &str| {
        let text = line.trim();
        text.is_empty()
            || text == "FAIL"
            || text == "PASS"
            || text.starts_with("=== ")
            || text.starts_with("--- ")
            || text.starts_with("FAIL\t")
            || text.starts_with("ok \t")
            || text.starts_with("ok  \t")
            || text.starts_with("exit status ")
            || text.starts_with("# ")
            || text == RACE_RULE
    };
    let mut kept: Vec<String> = lines
        .iter()
        .rev()
        .filter(|line| !noise(line))
        .take(FALLBACK_LINES)
        .map(|line| line.trim_end().to_string())
        .collect();
    kept.reverse();
    TestFailure::Other { lines: kept }
}

/// `text` without the terminal's colour codes some tools print anyway.
fn without_colours(text: &str) -> Cow<'_, str> {
    if !text.contains('\u{1b}') {
        return Cow::Borrowed(text);
    }
    let mut kept = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            kept.push(c);
            continue;
        }
        // "ESC [ parameters final": the final is one of '@' to '~'.
        if chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    Cow::Owned(kept)
}

/// The spaces and tabs `line` starts with.
fn indentation(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// `line` without up to `indent` of the spaces and tabs it starts with.
fn dedented(line: &str, indent: usize) -> &str {
    &line[indentation(line).min(indent)..]
}

/// `lines` without the indentation all of them share (blank ones aside),
/// and without trailing spaces.
fn dedent(lines: &[&str]) -> Vec<String> {
    let indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| indentation(line))
        .min()
        .unwrap_or(0);
    lines.iter().map(|line| dedented(line, indent).trim_end().to_string()).collect()
}

/// "src/lib.rs:28:9" as the path, the line and the column.
fn line_and_column(text: &str) -> Option<(String, u32, u32)> {
    let mut parts = text.trim().rsplitn(3, ':');
    let column = parts.next()?.parse().ok()?;
    let line = parts.next()?.parse().ok()?;
    let path = parts.next().filter(|path| !path.is_empty())?;
    Some((path.to_string(), line, column))
}

/// The place of a project file's line; None outside the project.
fn source_ref(path: &str, line: u32, locator: &Locator) -> Option<SourceRef> {
    Some(SourceRef {
        file: locator.relative(path)?,
        line,
    })
}

fn frame_ref(frame: Option<FailureFrame>) -> Option<SourceRef> {
    frame.map(|frame| SourceRef {
        file: frame.file,
        line: frame.line,
    })
}

/// What follows "panicked at " in a Rust panic's first line: "thread
/// 'tests::adds' (8950225) panicked at src/lib.rs:28:9:" (the message on
/// the lines after it; older versions print no thread id), or before Rust
/// 1.73 "thread 'tests::adds' panicked at 'message', src/lib.rs:28:9".
fn rust_panic_header(line: &str) -> Option<&str> {
    let text = line.trim_start().strip_prefix("thread '")?;
    let (_, rest) = text.split_once("' ")?;
    let rest = match rest.strip_prefix('(') {
        Some(id) => id.split_once(") ")?.1,
        None => rest,
    };
    rest.strip_prefix("panicked at ")
}

/// A Rust panic: its message's lines and where it was raised.
struct RustPanic {
    message: Vec<String>,
    path: String,
    line: u32,
    /// The line after the message.
    end: usize,
}

/// The panic whose first line is `lines[start]`.
fn rust_panic(lines: &[&str], start: usize) -> Option<RustPanic> {
    let after = rust_panic_header(lines[start])?;
    if let Some(quoted) = after.strip_prefix('\'') {
        // Before 1.73: the message, quoted, then "', src/lib.rs:28:9".
        let mut message = Vec::new();
        for (offset, line) in lines[start..].iter().enumerate() {
            let text = if offset == 0 { quoted } else { *line };
            if let Some((before, place)) = text.rsplit_once("', ") {
                if let Some((path, number, _)) = line_and_column(place) {
                    message.push(before.to_string());
                    return Some(RustPanic {
                        message,
                        path,
                        line: number,
                        end: start + offset + 1,
                    });
                }
            }
            message.push(text.to_string());
        }
        return None;
    }
    let (path, number, _) = line_and_column(after.trim_end().strip_suffix(':')?)?;
    let end = lines[start + 1..]
        .iter()
        .position(|line| rust_message_end(line))
        .map_or(lines.len(), |offset| start + 1 + offset);
    Some(RustPanic {
        message: lines[start + 1..end].iter().map(|line| line.to_string()).collect(),
        path,
        line: number,
        end,
    })
}

/// The line after a panic's message: a blank one, libtest's note, the
/// backtrace, or another panic.
fn rust_message_end(line: &str) -> bool {
    let text = line.trim();
    text.is_empty()
        || text.starts_with("note: ")
        || text == "stack backtrace:"
        || rust_panic_header(line).is_some()
}

/// A Rust test's failure: `assert_eq!`, `assert_ne!` and `assert!` as
/// assertions, a `#[should_panic]` test that did not panic as it should,
/// any other panic (`unwrap` on an `Err` or a `None`, `panic!`) as a panic.
fn rust_failure(lines: &[&str], locator: &Locator) -> Option<TestFailure> {
    if let Some(failure) = rust_not_panicked(lines, locator) {
        return Some(failure);
    }
    let start = lines.iter().position(|line| rust_panic_header(line).is_some())?;
    let panic = rust_panic(lines, start)?;
    let at = source_ref(&panic.path, panic.line, locator);
    if let Some(failure) = rust_wrong_panic(&lines[start..], at.clone()) {
        return Some(failure);
    }
    if let Some(failure) = rust_assertion(&panic.message, at) {
        return Some(failure);
    }
    let frame = rust_frame(&panic, &lines[panic.end..], locator);
    Some(TestFailure::Panic {
        message: panic.message.join("\n").trim().to_string(),
        frame,
        test: None,
    })
}

/// A `#[should_panic]` test that returned: "test did not panic as expected
/// at src/lib.rs:76:8" (a "note: " before it, and no place, in old Rust).
fn rust_not_panicked(lines: &[&str], locator: &Locator) -> Option<TestFailure> {
    let rest = lines.iter().find_map(|line| {
        let text = line.trim();
        text.strip_prefix("note: ")
            .unwrap_or(text)
            .strip_prefix("test did not panic as expected")
    })?;
    let at = rest
        .trim()
        .strip_prefix("at ")
        .and_then(line_and_column)
        .and_then(|(path, line, _)| source_ref(&path, line, locator));
    Some(TestFailure::Assertion {
        message: "test did not panic as expected".to_string(),
        expected: None,
        actual: None,
        at,
        test: None,
    })
}

/// A `#[should_panic(expected = "…")]` test whose panic said something
/// else: "panic did not contain expected string" over "panic message: …"
/// and "expected substring: …".
fn rust_wrong_panic(lines: &[&str], at: Option<SourceRef>) -> Option<TestFailure> {
    const WRONG: &str = "panic did not contain expected string";
    let start = lines.iter().position(|line| {
        let text = line.trim();
        text.strip_prefix("note: ").unwrap_or(text) == WRONG
    })?;
    let value = |key: &str| {
        lines[start + 1..]
            .iter()
            .take(2)
            .find_map(|line| line.trim().strip_prefix(key).map(rust_old_value))
    };
    Some(TestFailure::Assertion {
        message: WRONG.to_string(),
        expected: value("expected substring:"),
        actual: value("panic message:"),
        at,
        test: None,
    })
}

/// Rust's assertions, as it prints them since 1.73 ("assertion `left ==
/// right` failed: message" over "  left: 2" and " right: 1") and before
/// ("assertion failed: `(left == right)`" over "  left: `2`," and " right:
/// `1`: message"): the right is what was expected, the left what came.
fn rust_assertion(message: &[String], at: Option<SourceRef>) -> Option<TestFailure> {
    let first = message.first()?.trim();
    let value = |key: &str| {
        message
            .iter()
            .skip(1)
            .find_map(|line| line.trim_start().strip_prefix(key).map(str::to_string))
    };
    let assertion = |message: String, expected: Option<String>, actual: Option<String>| {
        Some(TestFailure::Assertion {
            message,
            expected,
            actual,
            at: at.clone(),
            test: None,
        })
    };
    for (equal, form) in [
        (true, "assertion `left == right` failed"),
        (false, "assertion `left != right` failed"),
    ] {
        if first.starts_with(form) {
            let right = if equal { value("right: ") } else { None };
            return assertion(first.to_string(), right, value("left: "));
        }
    }
    for (equal, form) in [
        (true, "assertion failed: `(left == right)`"),
        (false, "assertion failed: `(left != right)`"),
    ] {
        if first.starts_with(form) {
            let left = value("left: ").map(|left| rust_old_value(&left));
            let (right, note) = value("right: ").map_or((None, None), |right| rust_old_right(&right));
            let message = match note {
                Some(note) => format!("{first}: {note}"),
                None => first.to_string(),
            };
            return assertion(message, if equal { right } else { None }, left);
        }
    }
    if first.starts_with("assertion failed: ") {
        return assertion(first.to_string(), None, None);
    }
    None
}

/// Old Rust's right value, "`1`" or "`1`: message", as the value and the
/// message.
fn rust_old_right(text: &str) -> (Option<String>, Option<String>) {
    match text.strip_prefix('`').and_then(|inner| inner.split_once("`: ")) {
        Some((value, note)) => (Some(value.to_string()), Some(note.trim().to_string())),
        None => (Some(rust_old_value(text)), None),
    }
}

/// Old Rust's "`2`," as "2"; a value without backticks as it is.
fn rust_old_value(text: &str) -> String {
    let text = text.trim().trim_end_matches(',');
    text.strip_prefix('`')
        .and_then(|inner| inner.strip_suffix('`'))
        .unwrap_or(text)
        .to_string()
}

/// Where a panic happened, in the project: its place when that is a project
/// file (named after the backtrace's frame there, when one was printed;
/// unnamed otherwise), else the backtrace's first frame in the project.
fn rust_frame(panic: &RustPanic, rest: &[&str], locator: &Locator) -> Option<FailureFrame> {
    let backtrace = rust_backtrace(rest);
    let Some(file) = locator.relative(&panic.path) else {
        return project_frame(&backtrace, locator);
    };
    let function = backtrace
        .iter()
        .find(|frame| {
            frame.line == panic.line && locator.relative(&frame.path).as_deref() == Some(file.as_str())
        })
        .map(|frame| frame.function.clone())
        .unwrap_or_default();
    Some(FailureFrame {
        function,
        test: is_test_file(&file),
        file,
        line: panic.line,
    })
}

/// The frames of a "stack backtrace:": "   2: rcases::helper" over
/// "             at ./src/lib.rs:20:5".
fn rust_backtrace(lines: &[&str]) -> Vec<RawFrame> {
    let Some(start) = lines.iter().position(|line| line.trim() == "stack backtrace:") else {
        return Vec::new();
    };
    let mut frames = Vec::new();
    for pair in lines[start + 1..].windows(2) {
        let Some((number, function)) = pair[0].trim().split_once(": ") else {
            continue;
        };
        if number.parse::<u32>().is_err() {
            continue;
        }
        if let Some((path, line, _)) = pair[1].trim().strip_prefix("at ").and_then(line_and_column) {
            frames.push(RawFrame {
                function: rust_function(function),
                path,
                line,
            });
        }
    }
    frames
}

/// "rcases::tests::explicit_panic::{closure#0}" as "tests::explicit_panic":
/// without its crate, its closures and the symbol's hash.
fn rust_function(text: &str) -> String {
    let mut text = text.trim();
    while let Some((rest, last)) = text.rsplit_once("::") {
        let hash = last.len() == 17 && last.starts_with('h') && last[1..].chars().all(|c| c.is_ascii_hexdigit());
        if !hash && !last.starts_with("{closure") {
            break;
        }
        text = rest;
    }
    if text.starts_with('<') {
        return text.to_string();
    }
    text.split_once("::").map_or(text, |(_, rest)| rest).to_string()
}

/// rustc's errors: "error[E0308]: mismatched types" (or "error: …") over
/// " --> src/lib.rs:7:5" and the code it shows, until a blank line. Its
/// warnings are left out, and so are cargo's errors without a place ("could
/// not compile …") when some error has one.
fn rust_build_errors(lines: &[&str], locator: &Locator) -> Vec<BuildError> {
    let mut located = Vec::new();
    let mut unlocated = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if !(lines[index].starts_with("error:") || lines[index].starts_with("error[")) {
            index += 1;
            continue;
        }
        let end = lines[index + 1..]
            .iter()
            .position(|line| line.trim().is_empty() || rust_diagnostic(line))
            .map_or(lines.len(), |offset| index + 1 + offset);
        let error = rust_build_error(&lines[index..end], locator);
        if error.line.is_some() {
            located.push(error);
        } else {
            unlocated.push(error);
        }
        index = end;
    }
    let mut errors = if located.is_empty() { unlocated } else { located };
    errors.truncate(MAX_ITEMS);
    errors
}

/// The first line of one of rustc's errors or warnings.
fn rust_diagnostic(line: &str) -> bool {
    ["error:", "error[", "warning:", "warning["]
        .iter()
        .any(|start| line.starts_with(start))
}

/// One error of rustc, from its first line to the blank line after it.
fn rust_build_error(block: &[&str], locator: &Locator) -> BuildError {
    let message = block[0].trim().to_string();
    let place = block.iter().position(|line| line.trim_start().starts_with("--> "));
    let location = place.and_then(|index| line_and_column(block[index].trim_start().trim_start_matches("--> ")));
    let detail = place
        .map(|index| {
            block[index + 1..]
                .iter()
                .filter(|line| line.trim() != "|")
                .take(SNIPPET_LINES)
                .map(|line| line.trim_end().to_string())
                .collect()
        })
        .unwrap_or_default();
    match location {
        Some((path, line, column)) => BuildError {
            file: locator.relative(&path).or(Some(path)),
            line: Some(line),
            column: Some(column),
            message,
            detail,
        },
        None => BuildError {
            file: None,
            line: None,
            column: None,
            message,
            detail,
        },
    }
}

/// What a Python traceback says.
struct PythonError {
    /// Outermost first, as Python prints them.
    frames: Vec<RawFrame>,
    /// The exception's line ("AssertionError: 4 != 5") and those after it.
    exception: Vec<String>,
    /// The code shown under the last frame: what a SyntaxError points at.
    snippet: Vec<String>,
}

/// pytest's short traceback: "tests/test_calc.py:9: in test_total" lines,
/// each over its code, then the exception on "E   " lines (the last of
/// them, when an exception led to another).
fn pytest_error(lines: &[&str]) -> Option<PythonError> {
    let marked = |line: &&str| *line == "E" || line.starts_with("E ");
    let last = lines.iter().rposition(marked)?;
    let first = lines[..last].iter().rposition(|line| !marked(line)).map_or(0, |index| index + 1);
    let previous = lines[..first].iter().rposition(marked).map_or(0, |index| index + 1);
    let mut frames: Vec<RawFrame> = lines[previous..first]
        .iter()
        .filter_map(|line| pytest_location(line))
        .collect();
    // --tb=long ends with "tests/test_calc.py:9: AssertionError".
    frames.extend(lines.get(last + 1).and_then(|line| pytest_location(line)));
    let body: Vec<String> = lines[first..=last].iter().map(|line| without_mark(line)).collect();
    if body.iter().any(|line| python_file_line(line).is_some()) {
        // A SyntaxError says its place the way a traceback does.
        let body: Vec<&str> = body.iter().map(String::as_str).collect();
        let mut inner = traceback_error(&body)?;
        frames.append(&mut inner.frames);
        inner.frames = frames;
        return Some(inner);
    }
    (!frames.is_empty()).then_some(PythonError {
        frames,
        exception: body,
        snippet: Vec::new(),
    })
}

/// An "E   …" line of pytest without its mark.
fn without_mark(line: &str) -> String {
    let rest = &line[1..];
    let spaces = rest.len() - rest.trim_start_matches(' ').len();
    rest[spaces.min(3)..].trim_end().to_string()
}

/// "tests/test_calc.py:9: in test_total" (or "…:9: AssertionError").
fn pytest_location(line: &str) -> Option<RawFrame> {
    if line.starts_with(char::is_whitespace) {
        return None;
    }
    let at = line.find(".py:")? + 3;
    let (number, rest) = line[at + 1..].split_once(':')?;
    Some(RawFrame {
        function: rest.trim().strip_prefix("in ").unwrap_or("").to_string(),
        path: line[..at].to_string(),
        line: number.parse().ok()?,
    })
}

/// `  File "tests/test_calc.py", line 8, in test_equal`; the place of a
/// SyntaxError names no function.
fn python_file_line(line: &str) -> Option<RawFrame> {
    let rest = line.trim_start().strip_prefix("File \"")?;
    let (path, rest) = rest.split_once("\", line ")?;
    let (number, function) = rest.split_once(", in ").unwrap_or((rest, ""));
    Some(RawFrame {
        function: function.trim().to_string(),
        path: path.to_string(),
        line: number.trim().parse().ok()?,
    })
}

/// A traceback as Python prints it: from its last "Traceback (most recent
/// call last):" (or its first `File "…"` line) the frames, each over its
/// code, then the exception at the start of a line.
fn traceback_error(lines: &[&str]) -> Option<PythonError> {
    let start = match lines.iter().rposition(|line| line.trim() == TRACEBACK) {
        Some(index) => index + 1,
        None => lines.iter().position(|line| python_file_line(line).is_some())?,
    };
    let mut frames = Vec::new();
    let mut snippet = Vec::new();
    let mut index = start;
    while let Some(line) = lines.get(index) {
        if let Some(frame) = python_file_line(line) {
            frames.push(frame);
            snippet.clear();
        } else if line.starts_with(char::is_whitespace) {
            snippet.push(*line);
        } else if !line.trim().is_empty() {
            break;
        }
        index += 1;
    }
    if frames.is_empty() || index >= lines.len() {
        return None;
    }
    let exception = lines[index..]
        .iter()
        .take_while(|line| {
            !line.starts_with("During handling") && !line.starts_with("The above exception") && line.trim() != TRACEBACK
        })
        .take(MAX_ITEMS)
        .map(|line| line.trim_end().to_string())
        .collect();
    Some(PythonError {
        frames,
        exception,
        snippet: dedent(&snippet),
    })
}

/// "KeyError: 'db'" as its name and its message; pytest's "assert 3 == 4"
/// has no name.
fn python_exception(line: &str) -> (Option<&str>, &str) {
    let named = |name: &str| !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.');
    match line.split_once(": ") {
        Some((name, message)) if named(name) => (Some(name), message),
        _ if named(line) => (Some(line), ""),
        _ => (None, line),
    }
}

/// A Python test's failure: an AssertionError (pytest's rewritten "assert
/// 3 == 4": what came on the left, what was expected on the right;
/// unittest's "4 != 5" likewise) or pytest's `Failed` as an assertion at
/// the last frame in the project, any other exception as a panic there.
fn python_failure(lines: &[&str], locator: &Locator) -> Option<TestFailure> {
    let error = pytest_error(lines).or_else(|| traceback_error(lines))?;
    let first = error.exception.first()?.trim();
    let innermost: Vec<RawFrame> = error.frames.iter().rev().cloned().collect();
    let frame = project_frame(&innermost, locator);
    let (name, text) = python_exception(first);
    let asserted = first.starts_with("assert ")
        || name.is_some_and(|name| name.ends_with("AssertionError") || name == "Failed");
    if !asserted {
        return Some(TestFailure::Panic {
            message: first.to_string(),
            frame,
            test: None,
        });
    }
    let mut message = text.to_string();
    let mut expected = None;
    let mut actual = None;
    // pytest's comparison: on the line, or under the test's own message.
    let compared = std::iter::once(text)
        .chain(error.exception.get(1).map(|line| line.trim()))
        .find_map(|line| line.strip_prefix("assert "));
    if let Some(compared) = compared {
        if let Some((left, right)) = compared.split_once(" == ") {
            actual = Some(left.to_string());
            expected = Some(right.to_string());
        }
        // pytest shortens long values with "...": what differs is below.
        if compared.contains("...") {
            if let Some((left, right)) = pytest_differing(&error.exception) {
                actual = Some(left);
                expected = Some(right);
            }
        }
    } else {
        let compared = text.split_once(" : ").map_or(text, |(compared, _)| compared);
        if let Some((left, right)) = compared.split_once(" != ") {
            actual = Some(left.to_string());
            expected = Some(right.to_string());
        }
    }
    // pytest's "+  where 3 = add(1, 2)" says where a value came from.
    let wheres = error
        .exception
        .iter()
        .skip(1)
        .filter_map(|line| line.trim().strip_prefix('+').map(str::trim_start))
        .filter(|line| line.starts_with("where "))
        .take(WHERE_LINES);
    for line in wheres {
        message.push('\n');
        message.push_str(line);
    }
    Some(TestFailure::Assertion {
        message,
        expected,
        actual,
        at: frame_ref(frame),
        test: None,
    })
}

/// pytest's "Differing items:" over "{'age': 36} != {'age': 37}", or "At
/// index 1 diff: 'b' != 'c'", as what came and what was expected.
fn pytest_differing(exception: &[String]) -> Option<(String, String)> {
    let line = exception.iter().enumerate().find_map(|(index, line)| {
        let text = line.trim();
        if text == "Differing items:" {
            return exception.get(index + 1).map(|next| next.trim());
        }
        text.strip_prefix("At index ").and_then(|rest| rest.split_once(" diff: ")).map(|(_, rest)| rest)
    })?;
    let (left, right) = line.split_once(" != ")?;
    Some((left.to_string(), right.to_string()))
}

/// A Python test module that did not import: the exception (a SyntaxError
/// at its place, with the code it shows; another at the last frame in the
/// project, or the module pytest was importing).
fn python_build_errors(lines: &[&str], locator: &Locator) -> Vec<BuildError> {
    let Some(error) = pytest_error(lines).or_else(|| traceback_error(lines)) else {
        return Vec::new();
    };
    let Some(first) = error.exception.first() else {
        return Vec::new();
    };
    let message = first.trim().to_string();
    let (name, _) = python_exception(&message);
    let syntax = matches!(name, Some("SyntaxError" | "IndentationError" | "TabError"));
    let frame = if syntax {
        error.frames.last().cloned()
    } else {
        error.frames.iter().rev().find(|frame| locator.relative(&frame.path).is_some()).cloned()
    };
    let (file, line) = match frame {
        Some(frame) => (locator.relative(&frame.path).or(Some(frame.path)), Some(frame.line)),
        None => (pytest_module(lines).and_then(|path| locator.relative(path)), None),
    };
    let detail: Vec<String> = if syntax {
        error.snippet.iter().take(SNIPPET_LINES).cloned().collect()
    } else {
        error
            .exception
            .iter()
            .skip(1)
            .filter(|line| !line.trim().is_empty())
            .take(SNIPPET_LINES)
            .cloned()
            .collect()
    };
    vec![BuildError {
        file,
        line,
        column: None,
        message,
        detail,
    }]
}

/// The module of pytest's "ImportError while importing test module
/// '/x/tests/test_a.py'.".
fn pytest_module<'a>(lines: &[&'a str]) -> Option<&'a str> {
    lines.iter().find_map(|line| {
        let rest = line.trim().strip_prefix("ImportError while importing test module '")?;
        rest.split_once('\'').map(|(path, _)| path)
    })
}

/// "file:///x/a.test.js:12:5", "src/a.js:12:5" or "a.js:12" as the path
/// and the line.
fn js_location(text: &str) -> Option<(String, u32)> {
    if let Some((path, line, _)) = line_and_column(text) {
        return Some((path, line));
    }
    let (path, line) = text.trim().rsplit_once(':')?;
    let line = line.parse().ok()?;
    (!path.is_empty()).then(|| (path.to_string(), line))
}

/// A JavaScript stack frame: "at fn (path:12:5)", "at path:12:5", "at
/// async fn (…)"; `bare`, node:test's TAP prints them without the "at ".
fn js_frame(line: &str, bare: bool) -> Option<RawFrame> {
    let text = line.trim();
    let text = match text.strip_prefix("at ") {
        Some(rest) => rest,
        None if bare => text,
        None => return None,
    };
    let (function, place) = match text.strip_suffix(')').and_then(|text| text.rsplit_once(" (")) {
        Some((function, place)) => (function.trim_start_matches("async ").trim(), place),
        None => ("", text),
    };
    let (path, line) = js_location(place)?;
    Some(RawFrame {
        function: function.to_string(),
        path,
        line,
    })
}

/// An error's first line, "AssertionError [ERR_ASSERTION]: message",
/// "TypeError: message" or "Error: message", as its head (the name and its
/// code) and its message.
fn js_error_header(line: &str) -> Option<(&str, &str)> {
    let text = line.trim();
    let (head, message) = text.split_once(": ").unwrap_or((text, ""));
    let name = head.split(" [").next().unwrap_or(head);
    let named = (name.ends_with("Error") || name.ends_with("Exception"))
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '.');
    named.then_some((head, message))
}

/// A JavaScript test's failure: node:test's TAP block, else the first error
/// printed with its stack. An assertion (an AssertionError, jest's
/// "expect(…)", or anything that says what was expected or came) is at the
/// first frame in the project; any other error is a panic there.
fn js_failure(lines: &[&str], locator: &Locator) -> Option<TestFailure> {
    if let Some(failure) = tap_failure(lines, locator) {
        return Some(failure);
    }
    let start = lines.iter().position(|line| js_error_header(line).is_some())?;
    let (head, message) = js_error_header(lines[start])?;
    // node:test's spec reporter indents the error.
    let indent = indentation(lines[start]);
    let body: Vec<&str> = lines[start + 1..].iter().map(|line| dedented(line, indent)).collect();
    let frames: Vec<RawFrame> = body.iter().filter_map(|line| js_frame(line, false)).collect();
    let (expected, actual) = js_expected_actual(message, &body);
    let name = head.split(" [").next().unwrap_or(head);
    let asserted = name.contains("AssertionError") || message.starts_with("expect(") || expected.is_some() || actual.is_some();
    if frames.is_empty() && !asserted {
        return None;
    }
    let frame = project_frame(&frames, locator);
    if !asserted {
        return Some(TestFailure::Panic {
            message: lines[start].trim().to_string(),
            frame,
            test: None,
        });
    }
    let message = if message.is_empty() { name } else { message };
    Some(TestFailure::Assertion {
        message: message.trim().trim_end_matches(':').trim().to_string(),
        expected,
        actual,
        at: frame_ref(frame),
        test: None,
    })
}

/// What a JavaScript assertion expected and what came, from the lines after
/// its first: jest's and vitest's "Expected: 4" / "Received: 3", vitest's
/// "- Expected:" / "+ Received:" blocks, their "- Expected" / "+ Received"
/// diffs, node:assert's "+ actual - expected" diff and "3 !== 4", and the
/// "expected: …" / "actual: …" the runner's reporter or node adds.
fn js_expected_actual(message: &str, body: &[&str]) -> (Option<String>, Option<String>) {
    let line_value = |key: &str| {
        body.iter()
            .find_map(|line| line.trim_start().strip_prefix(key).map(|value| value.trim().to_string()))
    };
    let (expected, received) = (line_value("Expected: "), line_value("Received: "));
    if expected.is_some() || received.is_some() {
        return (expected, received);
    }
    if let (Some(expected), Some(received)) = (block_after(body, "- Expected:"), block_after(body, "+ Received:")) {
        return (Some(expected), Some(received));
    }
    if let Some(index) = body.iter().position(|line| line.starts_with("- Expected")) {
        if body.get(index + 1).is_some_and(|line| line.starts_with("+ Received")) {
            return diff_sides(&body[index + 2..]);
        }
    }
    if let Some(index) = body.iter().position(|line| line.trim() == "+ actual - expected") {
        return diff_sides(&body[index + 1..]);
    }
    if message.starts_with("Expected values to be") {
        let compared = body
            .iter()
            .take_while(|line| js_frame(line, false).is_none())
            .find_map(|line| line.split_once(" !== ").or_else(|| line.split_once(" != ")));
        if let Some((left, right)) = compared {
            return (Some(right.trim().to_string()), Some(left.trim().to_string()));
        }
    }
    (keyed_value(body, "expected: "), keyed_value(body, "actual: "))
}

/// The lines after the line `key` up to a blank one, joined.
fn block_after(body: &[&str], key: &str) -> Option<String> {
    let index = body.iter().position(|line| line.trim() == key)?;
    let value: Vec<&str> = body[index + 1..]
        .iter()
        .take_while(|line| !line.trim().is_empty())
        .copied()
        .collect();
    (!value.is_empty()).then(|| dedent(&value).join("\n"))
}

/// Both sides of a diff whose "-" lines are what was expected and whose "+"
/// lines are what came, after its header and up to a blank line or the
/// stack: each side with the lines they share.
fn diff_sides(lines: &[&str]) -> (Option<String>, Option<String>) {
    let diff: Vec<&str> = lines
        .iter()
        .skip_while(|line| line.trim().is_empty())
        .take_while(|line| !line.trim().is_empty() && js_frame(line, false).is_none())
        .copied()
        .collect();
    if diff.is_empty() {
        return (None, None);
    }
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    for line in diff {
        // node:assert points at the first difference with a "^" line.
        if line.trim().chars().all(|c| c == '^') {
            continue;
        }
        let rest = line.get(2..).unwrap_or("");
        match line.chars().next() {
            Some('-') => expected.push(rest),
            Some('+') => actual.push(rest),
            _ => {
                expected.push(rest);
                actual.push(rest);
            }
        }
    }
    (Some(dedent(&expected).join("\n")), Some(dedent(&actual).join("\n")))
}

/// "expected: 3" (`key`) and the lines that go on with its value: more
/// indented, or closing a bracket. Without the comma node's inspected
/// errors put after each value.
fn keyed_value(body: &[&str], key: &str) -> Option<String> {
    let index = body.iter().position(|line| line.trim_start().starts_with(key))?;
    let indent = indentation(body[index]);
    let mut value = vec![body[index].trim_start()[key.len()..].trim_end()];
    for line in &body[index + 1..] {
        let goes_on = indentation(line) > indent || line.trim_start().starts_with(['}', ']', ')']);
        if line.trim().is_empty() || !goes_on {
            break;
        }
        value.push(dedented(line, indent).trim_end());
    }
    let value = value.join("\n");
    Some(value.trim().trim_end_matches(',').to_string())
}

/// A failure as node:test's TAP reporter prints it: "not ok 1 - name" over
/// a YAML block with its "error", "expected", "actual", "location" and
/// "stack".
fn tap_failure(lines: &[&str], locator: &Locator) -> Option<TestFailure> {
    let start = lines.iter().position(|line| line.trim_start().starts_with("not ok "))?;
    if lines.get(start + 1)?.trim() != "---" {
        return None;
    }
    let block_start = start + 2;
    let end = lines[block_start..]
        .iter()
        .position(|line| line.trim() == "...")
        .map_or(lines.len(), |offset| block_start + offset);
    let block = &lines[block_start..end];
    let field = |key: &str| yaml_field(block, key);
    let stack = field("stack").unwrap_or_default();
    let frames: Vec<RawFrame> = stack.lines().filter_map(|line| js_frame(line, true)).collect();
    let frame = project_frame(&frames, locator).or_else(|| {
        let (path, line) = js_location(&field("location")?)?;
        let file = locator.relative(&path)?;
        Some(FailureFrame {
            function: String::new(),
            test: is_test_file(&file),
            file,
            line,
        })
    });
    let (expected, actual) = (field("expected"), field("actual"));
    let asserted = field("name").is_some_and(|name| name.contains("AssertionError"))
        || field("code").as_deref() == Some("ERR_ASSERTION")
        || expected.is_some()
        || actual.is_some();
    let error = field("error").unwrap_or_default();
    let message = error.lines().next().unwrap_or("").trim().trim_end_matches(':').trim().to_string();
    if !asserted {
        return Some(TestFailure::Panic {
            message,
            frame,
            test: None,
        });
    }
    Some(TestFailure::Assertion {
        message,
        expected,
        actual,
        at: frame_ref(frame),
        test: None,
    })
}

/// A field of a YAML block at its own indentation: "key: value" (a quoted
/// value unquoted), or "key:" / "key: |-" over the more indented lines of
/// its value.
fn yaml_field(block: &[&str], key: &str) -> Option<String> {
    let base = block.iter().find(|line| !line.trim().is_empty()).map(|line| indentation(line))?;
    let index = block.iter().position(|line| {
        indentation(line) == base
            && line.trim_start().strip_prefix(key).is_some_and(|rest| rest.starts_with(':'))
    })?;
    let rest = block[index].trim_start()[key.len() + 1..].trim();
    if !rest.is_empty() && !matches!(rest, "|" | "|-" | ">" | ">-") {
        return Some(yaml_scalar(rest));
    }
    let nested: Vec<&str> = block[index + 1..]
        .iter()
        .take_while(|line| line.trim().is_empty() || indentation(line) > base)
        .copied()
        .collect();
    let value = dedent(&nested).join("\n").trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// A YAML scalar without its quotes.
fn yaml_scalar(text: &str) -> String {
    if let Some(inner) = text.strip_prefix('\'').and_then(|text| text.strip_suffix('\'')) {
        return inner.replace("''", "'");
    }
    if let Some(inner) = text.strip_prefix('"').and_then(|text| text.strip_suffix('"')) {
        return inner.replace("\\\"", "\"");
    }
    text.to_string()
}

/// The JavaScript files that did not load: for each, the error and where
/// it is. jest says each under "● Test suite failed to run"; node prints
/// "file:line" over the code before the error; vitest and the others start
/// each error with its name at the start of a line.
fn js_build_errors(lines: &[&str], locator: &Locator) -> Vec<BuildError> {
    let mut starts = Vec::new();
    let mut placed_without_error = false;
    for (index, line) in lines.iter().enumerate() {
        let first_column = !line.starts_with(char::is_whitespace);
        let place = first_column && js_place_line(line);
        let header = first_column && js_error_header(line).is_some();
        if line.trim() == JEST_SUITE_FAILED || place || (header && !placed_without_error) {
            starts.push(index);
            placed_without_error = place;
        } else if header {
            placed_without_error = false;
        }
    }
    let mut errors: Vec<BuildError> = Vec::new();
    for (position, start) in starts.iter().enumerate() {
        let end = starts.get(position + 1).copied().unwrap_or(lines.len());
        if let Some(error) = js_build_error(&lines[*start..end], locator) {
            if !errors.contains(&error) {
                errors.push(error);
            }
        }
    }
    errors
}

/// node's "file:///x/test/a.test.js:4" over the code an error is in.
fn js_place_line(line: &str) -> bool {
    let text = line.trim_end();
    !text.contains(char::is_whitespace)
        && js_location(text).is_some_and(|(path, _)| path.contains('/') || path.contains('.'))
}

/// One file that did not load, from the start of what says it.
fn js_build_error(section: &[&str], locator: &Locator) -> Option<BuildError> {
    let jest = section[0].trim() == JEST_SUITE_FAILED;
    let header = if jest {
        section.iter().skip(1).position(|line| !line.trim().is_empty())? + 1
    } else {
        section.iter().position(|line| js_error_header(line).is_some())?
    };
    let mut message = section[header].trim().to_string();
    // "Transform failed with 1 error:" says which on the next line.
    if message.ends_with(':') {
        if let Some(next) = section[header + 1..].iter().find(|line| !line.trim().is_empty()) {
            if js_frame(next, false).is_none() {
                message = format!("{message} {}", next.trim());
            }
        }
    }
    let mut place: Option<(String, u32, Option<u32>)> = None;
    let mut detail = Vec::new();
    // babel's "SyntaxError: /x/src/a.test.js: Unexpected token (4:23)".
    if let Some((head, rest)) = js_error_header(&message).map(|(head, rest)| (head.to_string(), rest.to_string())) {
        if let Some((path, text)) = rest.split_once(": ").filter(|(path, _)| js_source_path(path)) {
            let position = text
                .trim_end()
                .strip_suffix(')')
                .and_then(|text| text.rsplit_once('('))
                .and_then(|(_, position)| position.split_once(':'))
                .and_then(|(line, column)| Some((line.parse().ok()?, column.parse().ok())));
            if let Some((line, column)) = position {
                place = Some((path.to_string(), line, column));
            }
            message = format!("{head}: {text}");
        }
    }
    // node's "file:///x/a.test.js:4" over the code, when it is the project's.
    if place.is_none() && header > 0 && js_place_line(section[0]) {
        if let Some((path, line)) = js_location(section[0].trim()) {
            if locator.relative(&path).is_some() {
                let code: Vec<&str> = section[1..header].iter().filter(|line| !line.trim().is_empty()).copied().collect();
                detail = dedent(&code);
                place = Some((path, line, None));
            }
        }
    }
    // vite's and oxc's "╭─[ src/a.test.ts:4:19 ]".
    if place.is_none() {
        place = section[header..].iter().find_map(|line| {
            let (_, rest) = line.split_once('[')?;
            let (inside, _) = rest.split_once(']')?;
            let (path, line, column) = line_and_column(inside)?;
            locator.relative(&path)?;
            Some((path, line, Some(column)))
        });
    }
    if place.is_none() {
        let frames: Vec<RawFrame> = section.iter().filter_map(|line| js_frame(line, false)).collect();
        place = project_frame(&frames, locator).map(|frame| (frame.file, frame.line, None));
    }
    if detail.is_empty() {
        let code: Vec<&str> = section[header..].iter().filter(|line| code_frame_line(line)).copied().collect();
        detail = dedent(&code);
    }
    detail.truncate(SNIPPET_LINES);
    let (file, line, column) = match place {
        Some((path, line, column)) => (locator.relative(&path).or(Some(path)), Some(line), column),
        // node's "Cannot find module '…' imported from /x/test/a.test.js".
        None => {
            let importer = message.split_once(" imported from ").map(|(_, path)| path.trim());
            (importer.and_then(|path| locator.relative(path)), None, None)
        }
    };
    Some(BuildError {
        file,
        line,
        column,
        message,
        detail,
    })
}

/// A path of a JavaScript or TypeScript source file.
fn js_source_path(path: &str) -> bool {
    path.contains('/')
        && [".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts", ".vue", ".svelte"]
            .iter()
            .any(|extension| path.ends_with(extension))
}

/// A line of the code a JavaScript tool shows under an error: "> 4 |
/// code", "    |    ^", " 4 │ code".
fn code_frame_line(line: &str) -> bool {
    let text = line.trim_start().trim_start_matches('>').trim_start();
    let text = text.trim_start_matches(|c: char| c.is_ascii_digit()).trim_start();
    let rest = text.strip_prefix('|').or_else(|| text.strip_prefix('│'));
    rest.is_some_and(|rest| !rest.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locator() -> Locator {
        Locator::new(
            [
                "app/services/auth_service/service.go",
                "app/services/auth_service/password_policy.go",
                "app/services/auth_service/auth_service_test.go",
                "app/controllers/edge_login.go",
                "app/controllers/edge_login_test.go",
                "app/containers/auth_container.go",
                "app/hang/hang_test.go",
            ]
            .map(str::to_string),
        )
    }

    /// The shape of a real report of the race detector, inside a test's
    /// output with its subtests.
    const RACE_OUTPUT: &str = "=== RUN   TestSignUpFromEdge_CreatesAndLogsIn
=== RUN   TestSignUpFromEdge_CreatesAndLogsIn/v1,_the_country_of_the_edge
==================
WARNING: DATA RACE
Write at 0x00c0004b2a38 by goroutine 214:
  app/services/auth_service.(*Service).recordLastLogin()
      /private/tmp/dl-x/tree/app/services/auth_service/service.go:661 +0x84
  app/services/auth_service.(*PasswordPolicy).checkAsync.func1()
      /private/tmp/dl-x/tree/app/services/auth_service/password_policy.go:94 +0x1d0

Previous read at 0x00c0004b2a38 by goroutine 7:
  app/controllers.TestSignUpFromEdge_CreatesAndLogsIn.func1()
      /private/tmp/dl-x/tree/app/controllers/edge_login_test.go:437 +0x5a4
  testing.tRunner()
      /usr/local/go/src/testing/testing.go:1792 +0x225

Goroutine 214 (running) created at:
  app/services/auth_service.(*PasswordPolicy).checkAsync()
      /private/tmp/dl-x/tree/app/services/auth_service/password_policy.go:90 +0x2b0
  app/services/auth_service.(*Service).SignUpFromEdge()
      /private/tmp/dl-x/tree/app/services/auth_service/service.go:412 +0x6e4

Goroutine 7 (running) created at:
  testing.(*T).Run()
      /usr/local/go/src/testing/testing.go:1851 +0x8f2
  testing.runTests.func1()
      /usr/local/go/src/testing/testing.go:2279 +0x85
==================
    testing.go:1490: race detected during execution of test
=== RUN   TestSignUpFromEdge_CreatesAndLogsIn/v2
==================
WARNING: DATA RACE
Write at 0x00c0004b2a38 by goroutine 230:
  app/services/auth_service.(*Service).recordLastLogin()
      /private/tmp/dl-x/tree/app/services/auth_service/service.go:661 +0x84
  app/services/auth_service.(*PasswordPolicy).checkAsync.func1()
      /private/tmp/dl-x/tree/app/services/auth_service/password_policy.go:94 +0x1d0

Previous read at 0x00c0004b2a38 by goroutine 9:
  app/controllers.TestSignUpFromEdge_CreatesAndLogsIn.func1()
      /private/tmp/dl-x/tree/app/controllers/edge_login_test.go:437 +0x5a4

Goroutine 230 (running) created at:
  app/services/auth_service.(*PasswordPolicy).checkAsync()
      /private/tmp/dl-x/tree/app/services/auth_service/password_policy.go:90 +0x2b0
==================
    testing.go:1490: race detected during execution of test
--- FAIL: TestSignUpFromEdge_CreatesAndLogsIn (1.84s)
    --- FAIL: TestSignUpFromEdge_CreatesAndLogsIn/v1,_the_country_of_the_edge (0.92s)
    --- FAIL: TestSignUpFromEdge_CreatesAndLogsIn/v2 (0.88s)
FAIL
";

    fn frame(function: &str, file: &str, line: u32) -> FailureFrame {
        FailureFrame {
            function: function.to_string(),
            file: file.to_string(),
            line,
            test: is_test_file(file),
        }
    }

    #[test]
    fn a_race_report_says_who_accessed_what_and_where_their_goroutines_came_from() {
        let failure = test_failure(RACE_OUTPUT, &[], None, &locator());

        let TestFailure::Race { races } = failure else {
            panic!("{failure:?}");
        };
        // The same race in both subtests (only the goroutine numbers differ)
        // is said once.
        assert_eq!(races.len(), 1);
        let accesses = &races[0].accesses;
        assert_eq!(
            accesses[0],
            RaceAccess {
                kind: RaceAccessKind::Write,
                previous: false,
                goroutine: Some(214),
                frame: Some(frame("(*Service).recordLastLogin", "app/services/auth_service/service.go", 661)),
                created_at: Some(frame(
                    "(*PasswordPolicy).checkAsync",
                    "app/services/auth_service/password_policy.go",
                    90
                )),
            }
        );
        assert_eq!(
            accesses[1],
            RaceAccess {
                kind: RaceAccessKind::Read,
                previous: true,
                goroutine: Some(7),
                frame: Some(frame(
                    "TestSignUpFromEdge_CreatesAndLogsIn.func1",
                    "app/controllers/edge_login_test.go",
                    437
                )),
                // Created by the test framework: the test's own goroutine.
                created_at: None,
            }
        );
        assert!(accesses[1].frame.as_ref().unwrap().test);

        // Another race at other places is one more.
        let (first, second) = RACE_OUTPUT.split_at(RACE_OUTPUT.rfind(RACE_START).unwrap());
        let other = format!("{first}{}", second.replace("service.go:661", "service.go:670"));
        let TestFailure::Race { races } = test_failure(&other, &[], None, &locator()) else {
            panic!();
        };
        assert_eq!(races.len(), 2);
        assert_eq!(races[1].accesses[1].goroutine, Some(9));
    }

    #[test]
    fn a_race_the_output_lost_is_read_from_the_runners_reports() {
        let report = RACE_OUTPUT
            .lines()
            .skip_while(|line| !line.starts_with(RACE_START))
            .take_while(|line| !line.starts_with("    testing.go"))
            .collect::<Vec<_>>()
            .join("\n");
        let output = "    testing.go:1490: race detected during execution of test\n--- FAIL: TestX (0.1s)\n";

        let failure = test_failure(output, &[&report, &report], None, &locator());

        let TestFailure::Race { races } = failure else {
            panic!("{failure:?}");
        };
        assert_eq!(races.len(), 1, "repeats are dropped");
        // A main goroutine, an atomic access, and sizes are read too.
        let header = access_header("Previous atomic write of size 8 at 0x00c000 by main goroutine:");
        assert_eq!(header, Some((RaceAccessKind::Write, true, None)));
        assert_eq!(access_header("Read at 0x1 by goroutine x:"), None);
    }

    #[test]
    fn testify_says_what_was_expected_and_what_came_and_where() {
        let output = "=== RUN   TestAuthenticateAD_EveryRefusalIsTheGenericError
=== RUN   TestAuthenticateAD_EveryRefusalIsTheGenericError/AD_accepts
=== RUN   TestAuthenticateAD_EveryRefusalIsTheGenericError/AD_refuses
    auth_service_test.go:218:
        \tError Trace:\t/private/tmp/dl-x/tree/app/services/auth_service/auth_service_test.go:218
        \tError:      \tNot equal:
        \t            \texpected: \"credenciais inválidas\"
        \t            \tactual  : \"ldap: invalid credentials (49)\"
        \t            \t
        \t            \tDiff:
        \tTest:       \tTestAuthenticateAD_EveryRefusalIsTheGenericError/AD_refuses
=== RUN   TestAuthenticateAD_EveryRefusalIsTheGenericError/AD_unreachable
--- FAIL: TestAuthenticateAD_EveryRefusalIsTheGenericError (0.04s)
";
        assert_eq!(
            test_failure(output, &[], None, &locator()),
            TestFailure::Assertion {
                message: "Not equal".to_string(),
                expected: Some("\"credenciais inválidas\"".to_string()),
                actual: Some("\"ldap: invalid credentials (49)\"".to_string()),
                at: Some(SourceRef {
                    file: "app/services/auth_service/auth_service_test.go".to_string(),
                    line: 218
                }),
                test: Some("TestAuthenticateAD_EveryRefusalIsTheGenericError/AD_refuses".to_string()),
            }
        );

        let unexpected = "    a_test.go:9:
        \tError Trace:\ta_test.go:9
        \tError:      \tReceived unexpected error:
        \t            \tconnection refused
        \tTest:       \tTestDial
";
        let TestFailure::Assertion { message, expected, actual, at, .. } =
            test_failure(unexpected, &[], Some("net/a_test.go"), &locator())
        else {
            panic!();
        };
        assert_eq!(message, "Received unexpected error");
        assert_eq!((expected, actual.as_deref()), (None, Some("connection refused")));
        assert_eq!(at.unwrap().file, "net/a_test.go", "placed by the test's own file");
    }

    #[test]
    fn a_plain_error_message_is_an_assertion_with_its_place() {
        let output = "=== RUN   TestService_Request/gateway_declines
    service_test.go:88: expected error wrapping ErrDeclined,
        got nil
--- FAIL: TestService_Request/gateway_declines (0.00s)
";
        assert_eq!(
            test_failure(output, &[], Some("internal/refund/service_test.go"), &locator()),
            TestFailure::Assertion {
                message: "expected error wrapping ErrDeclined,\ngot nil".to_string(),
                expected: None,
                actual: None,
                at: Some(SourceRef {
                    file: "internal/refund/service_test.go".to_string(),
                    line: 88
                }),
                test: Some("TestService_Request/gateway_declines".to_string()),
            }
        );
    }

    #[test]
    fn a_panic_says_its_message_and_the_first_frame_of_the_project() {
        let output = "=== RUN   TestSignUp
--- FAIL: TestSignUp (0.00s)
panic: runtime error: invalid memory address or nil pointer dereference [recovered]
\tpanic: runtime error: invalid memory address or nil pointer dereference
[signal SIGSEGV: segmentation violation code=0x2 addr=0x0 pc=0x1009]

goroutine 7 [running]:
testing.tRunner.func1.2({0x100a, 0x100b})
\t/usr/local/go/src/testing/testing.go:1734 +0x1ac
panic({0x100a?, 0x100b?})
\t/usr/local/go/src/runtime/panic.go:792 +0x124
app/controllers.(*EdgeController).SignUp(0x0, {0x1, 0x2})
\t/private/tmp/dl-x/tree/app/controllers/edge_login.go:58 +0x31c
app/controllers.TestSignUp(0xc000)
\t/private/tmp/dl-x/tree/app/controllers/edge_login_test.go:20 +0x44
";
        assert_eq!(
            test_failure(output, &[], None, &locator()),
            TestFailure::Panic {
                message: "runtime error: invalid memory address or nil pointer dereference".to_string(),
                frame: Some(frame("(*EdgeController).SignUp", "app/controllers/edge_login.go", 58)),
                test: Some("TestSignUp".to_string()),
            }
        );
    }

    #[test]
    fn a_timeout_says_what_was_running_and_where() {
        let output = "=== RUN   TestHang
panic: test timed out after 10m0s
\trunning tests:
\t\tTestHang (10m0s)
\t\tTestHang/forever (10m0s)

goroutine 33 [running]:
testing.(*M).startAlarm.func1()
\t/usr/local/go/src/testing/testing.go:2484 +0x308
created by time.goFunc
\t/usr/local/go/src/time/sleep.go:215 +0x38

goroutine 6 [sleep]:
time.Sleep(0x3b9aca00)
\t/usr/local/go/src/runtime/time.go:338 +0x158
app/hang.TestHang.func1(0xc0000)
\t/private/tmp/dl-x/tree/app/hang/hang_test.go:9 +0x2c
";
        assert_eq!(
            test_failure(output, &[], None, &locator()),
            TestFailure::Timeout {
                message: "test timed out after 10m0s".to_string(),
                frame: Some(frame("TestHang.func1", "app/hang/hang_test.go", 9)),
                running: vec!["TestHang".to_string(), "TestHang/forever".to_string()],
            }
        );
    }

    #[test]
    fn a_build_that_failed_lists_its_errors_with_their_places() {
        let output = "# app/containers [app/containers.test]
app/containers/auth_container.go:47:31: not enough arguments in call to auth_service.NewService
\thave (*sql.DB, *ldap.Client)
\twant (*sql.DB, *ldap.Client, auth_service.PasswordPolicy)
./wire.go:12: undefined: wire
FAIL\tapp/containers [build failed]
";
        assert_eq!(
            package_failure(output, PackageOutcomeKind::BuildFailed, &locator()),
            TestFailure::Build {
                errors: vec![
                    BuildError {
                        file: Some("app/containers/auth_container.go".to_string()),
                        line: Some(47),
                        column: Some(31),
                        message: "not enough arguments in call to auth_service.NewService".to_string(),
                        detail: vec![
                            "have (*sql.DB, *ldap.Client)".to_string(),
                            "want (*sql.DB, *ldap.Client, auth_service.PasswordPolicy)".to_string(),
                        ],
                    },
                    BuildError {
                        file: Some("wire.go".to_string()),
                        line: Some(12),
                        column: None,
                        message: "undefined: wire".to_string(),
                        detail: Vec::new(),
                    },
                ]
            }
        );
        // A package that failed outside its tests reads like a test.
        let setup = package_failure("panic: setup is not ready\n\ngoroutine 1 [running]:\n", PackageOutcomeKind::Fail, &locator());
        assert!(matches!(setup, TestFailure::Panic { ref message, frame: None, .. } if message == "setup is not ready"), "{setup:?}");
    }

    #[test]
    fn what_is_not_recognised_keeps_its_last_meaningful_lines() {
        let output = "=== RUN   TestOdd\nsomething odd\n\nexit status 2\nit broke\n--- FAIL: TestOdd (0.00s)\nFAIL\n";
        assert_eq!(
            test_failure(output, &[], None, &locator()),
            TestFailure::Other {
                lines: vec!["something odd".to_string(), "it broke".to_string()]
            }
        );
        assert_eq!(
            package_failure("FAIL\tapp/x [build failed]\n", PackageOutcomeKind::BuildFailed, &locator()),
            TestFailure::Other { lines: Vec::new() }
        );
        assert_eq!(test_failure("", &[], None, &locator()), TestFailure::Other { lines: Vec::new() });
    }

    #[test]
    fn paths_are_placed_in_the_project() {
        let locator = locator();
        assert_eq!(
            locator.relative("/Users/me/Library/Application Support/x/tree/app/controllers/edge_login.go").as_deref(),
            Some("app/controllers/edge_login.go")
        );
        assert_eq!(locator.relative("/usr/local/go/src/testing/testing.go"), None);
        assert_eq!(locator.relative("/Users/me/go/pkg/mod/github.com/x/y.go"), None);
        assert_eq!(locator.relative("/run/tree/unknown/new.go").as_deref(), Some("unknown/new.go"));
        assert_eq!(locator.relative("/elsewhere/new.go"), None);
        assert_eq!(locator.relative("./wire.go").as_deref(), Some("wire.go"));
        assert_eq!(locator.short("edge_login_test.go", None), "app/controllers/edge_login_test.go");
        assert_eq!(short_function("created by testing.(*T).Run in goroutine 6"), "(*T).Run");
        assert_eq!(short_function("example.com/p.F[...](0x1, {0x2, 0x3})"), "F[...]");
        assert_eq!(location("\t/a b/c.go:12 +0x1f"), Some(("/a b/c.go".to_string(), 12)));
    }

    fn assertion(message: &str, expected: Option<&str>, actual: Option<&str>, at: Option<(&str, u32)>) -> TestFailure {
        TestFailure::Assertion {
            message: message.to_string(),
            expected: expected.map(str::to_string),
            actual: actual.map(str::to_string),
            at: at.map(|(file, line)| SourceRef {
                file: file.to_string(),
                line,
            }),
            test: None,
        }
    }

    fn panic_at(message: &str, frame: Option<FailureFrame>) -> TestFailure {
        TestFailure::Panic {
            message: message.to_string(),
            frame,
            test: None,
        }
    }

    fn build_error(file: &str, line: Option<u32>, column: Option<u32>, message: &str, detail: &[&str]) -> BuildError {
        BuildError {
            file: Some(file.to_string()),
            line,
            column,
            message: message.to_string(),
            detail: detail.iter().map(|line| line.to_string()).collect(),
        }
    }

    fn files(files: &[&str]) -> Locator {
        Locator::new(files.iter().map(|file| file.to_string()))
    }

    /// The crates of the Rust analyzer's fixtures (testdata/failing and
    /// testdata/broken) and of a scratch crate with more cases.
    fn rust_locator() -> Locator {
        files(&["src/lib.rs", "tests/api.rs", "bad/src/lib.rs", "bad/tests/unparsable.rs", "good/src/lib.rs"])
    }

    // The outputs below are real: libtest's captured output of failed tests
    // and rustc's rendered errors, as the Rust analyzer reported them for
    // its testdata/failing and testdata/broken and for a scratch crate;
    // Rust 1.101 nightly.

    #[test]
    fn rust_assertions_say_the_right_as_expected_and_the_left_as_what_came() {
        let output = "about to halve

thread 'tests::halves_wrongly' (8950225) panicked at src/lib.rs:28:9:
assertion `left == right` failed: halve rounds up
  left: 2
 right: 1
";
        assert_eq!(
            test_failure(output, &[], Some("src/lib.rs"), &rust_locator()),
            assertion(
                "assertion `left == right` failed: halve rounds up",
                Some("1"),
                Some("2"),
                Some(("src/lib.rs", 28))
            )
        );

        let structs = "
thread 'tests::structs_differ' (8957903) panicked at src/lib.rs:29:9:
assertion `left == right` failed
  left: Point { x: 1, y: 2 }
 right: Point { x: 1, y: 3 }
";
        assert_eq!(
            test_failure(structs, &[], None, &rust_locator()),
            assertion(
                "assertion `left == right` failed",
                Some("Point { x: 1, y: 3 }"),
                Some("Point { x: 1, y: 2 }"),
                Some(("src/lib.rs", 29))
            )
        );

        let integration = "
thread 'parses_through_the_api' (8957946) panicked at tests/api.rs:5:5:
assertion `left == right` failed
  left: 41
 right: 42
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
";
        assert_eq!(
            test_failure(integration, &[], None, &rust_locator()),
            assertion("assertion `left == right` failed", Some("42"), Some("41"), Some(("tests/api.rs", 5)))
        );

        let not_equal = "
thread 'tests::not_equal' (8957899) panicked at src/lib.rs:39:9:
assertion `left != right` failed: should differ
  left: 4
 right: 4
";
        assert_eq!(
            test_failure(not_equal, &[], None, &rust_locator()),
            assertion(
                "assertion `left != right` failed: should differ",
                None,
                Some("4"),
                Some(("src/lib.rs", 39))
            )
        );

        // Before Rust 1.73 (written after its format, not captured): the
        // message quoted in the first line, the values in backticks.
        let old = "thread 'tests::halves_wrongly' panicked at 'assertion failed: `(left == right)`
  left: `2`,
 right: `1`: halve rounds up', src/lib.rs:28:9
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
";
        assert_eq!(
            test_failure(old, &[], None, &rust_locator()),
            assertion(
                "assertion failed: `(left == right)`: halve rounds up",
                Some("1"),
                Some("2"),
                Some(("src/lib.rs", 28))
            )
        );
        let old_panic = "thread 'main' panicked at 'the config is not loaded', src/lib.rs:20:5\n";
        assert_eq!(
            test_failure(old_panic, &[], None, &rust_locator()),
            panic_at("the config is not loaded", Some(frame("", "src/lib.rs", 20)))
        );
    }

    #[test]
    fn rust_asserts_and_should_panic_tests_are_assertions() {
        let plain = "
thread 'tests::plain_assert' (8957900) panicked at src/lib.rs:45:9:
assertion failed: ready
";
        assert_eq!(
            test_failure(plain, &[], None, &rust_locator()),
            assertion("assertion failed: ready", None, None, Some(("src/lib.rs", 45)))
        );

        let returned = "test did not panic as expected at src/lib.rs:76:8\n";
        assert_eq!(
            test_failure(returned, &[], None, &rust_locator()),
            assertion("test did not panic as expected", None, None, Some(("src/lib.rs", 76)))
        );

        let wrong = "
thread 'tests::wrong_panic' (8957907) panicked at src/lib.rs:83:9:
bang
panic did not contain expected string
      panic message: \"bang\"
 expected substring: \"boom\"
";
        assert_eq!(
            test_failure(wrong, &[], None, &rust_locator()),
            assertion(
                "panic did not contain expected string",
                Some("\"boom\""),
                Some("\"bang\""),
                Some(("src/lib.rs", 83))
            )
        );
    }

    #[test]
    fn other_rust_panics_say_their_message_and_place() {
        let index = "
thread 'tests::panics' (8950226) panicked at src/lib.rs:40:23:
index out of bounds: the len is 0 but the index is 3
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
";
        assert_eq!(
            test_failure(index, &[], None, &rust_locator()),
            panic_at(
                "index out of bounds: the len is 0 but the index is 3",
                Some(frame("", "src/lib.rs", 40))
            )
        );

        let unwrap = "
thread 'tests::unwrap_err' (8957904) panicked at src/lib.rs:56:22:
called `Result::unwrap()` on an `Err` value: ParseIntError { kind: InvalidDigit }
";
        assert_eq!(
            test_failure(unwrap, &[], None, &rust_locator()),
            panic_at(
                "called `Result::unwrap()` on an `Err` value: ParseIntError { kind: InvalidDigit }",
                Some(frame("", "src/lib.rs", 56))
            )
        );

        // `assert!` with its own message prints only the message: a panic.
        let lines = "
thread 'tests::multiline_panic' (8957898) panicked at src/lib.rs:88:9:
first line
second line
";
        assert_eq!(
            test_failure(lines, &[], None, &rust_locator()),
            panic_at("first line\nsecond line", Some(frame("", "src/lib.rs", 88)))
        );

        // With RUST_BACKTRACE=1, the backtrace names the function.
        let backtrace = "thread 'tests::explicit_panic' (8963341) panicked at src/lib.rs:20:5:
the config is not loaded
stack backtrace:
   0: __rustc::rust_begin_unwind
             at /rustc/2d20b42b883dcfc014f3c2c56081eeb6091c2179/library/std/src/panicking.rs:676:5
   1: core::panicking::panic_fmt
             at /rustc/2d20b42b883dcfc014f3c2c56081eeb6091c2179/library/core/src/panicking.rs:80:14
   2: rcases::helper
             at ./src/lib.rs:20:5
   3: rcases::explode
             at ./src/lib.rs:16:5
   4: rcases::tests::explicit_panic
             at ./src/lib.rs:71:9
   5: rcases::tests::explicit_panic::{closure#0}
             at ./src/lib.rs:70:24
   6: <rcases::tests::explicit_panic::{closure#0} as core::ops::function::FnOnce<()>>::call_once
             at /Users/me/.rustup/toolchains/nightly-aarch64-apple-darwin/lib/rustlib/src/rust/library/core/src/ops/function.rs:250:5
note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.
";
        assert_eq!(
            test_failure(backtrace, &[], None, &rust_locator()),
            panic_at("the config is not loaded", Some(frame("helper", "src/lib.rs", 20)))
        );
        // Raised in a dependency: the backtrace's first frame in the project.
        let dependency = backtrace.replacen(
            "panicked at src/lib.rs:20:5:",
            "panicked at /Users/me/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/serde-1.0.0/src/de.rs:9:1:",
            1,
        );
        assert_eq!(
            test_failure(&dependency, &[], None, &rust_locator()),
            panic_at("the config is not loaded", Some(frame("helper", "src/lib.rs", 20)))
        );
        assert_eq!(rust_function("rcases::tests::explicit_panic::{closure#0}"), "tests::explicit_panic");
        assert_eq!(rust_function("rcases::helper::h0123456789abcdef"), "helper");
    }

    #[test]
    fn rustc_errors_are_build_errors_with_their_place_and_code() {
        let broken = "error[E0308]: mismatched types
  --> bad/src/lib.rs:11:26
   |
11 |         let text: &str = super::triple(3);
   |                   ----   ^^^^^^^^^^^^^^^^ expected `&str`, found `i32`
   |                   |
   |                   expected due to this


";
        assert_eq!(
            package_failure(broken, PackageOutcomeKind::BuildFailed, &rust_locator()),
            TestFailure::Build {
                errors: vec![build_error(
                    "bad/src/lib.rs",
                    Some(11),
                    Some(26),
                    "error[E0308]: mismatched types",
                    &[
                        "11 |         let text: &str = super::triple(3);",
                        "   |                   ----   ^^^^^^^^^^^^^^^^ expected `&str`, found `i32`",
                        "   |                   |",
                        "   |                   expected due to this",
                    ]
                )]
            }
        );

        let unparsable = "error: mismatched closing delimiter: `}`
 --> bad/tests/unparsable.rs:5:17
  |
4 | fn never_runs() {
  |                 - closing delimiter possibly meant for this
5 |     let value = (1 + ;
  |                 ^ unclosed delimiter
6 | }
  | ^ mismatched closing delimiter
";
        let TestFailure::Build { errors } = package_failure(unparsable, PackageOutcomeKind::BuildFailed, &rust_locator())
        else {
            panic!();
        };
        assert_eq!(
            (errors[0].file.as_deref(), errors[0].line, errors[0].message.as_str(), errors[0].detail.len()),
            (Some("bad/tests/unparsable.rs"), Some(5), "error: mismatched closing delimiter: `}`", 6)
        );

        // cargo's own output: warnings and cargo's summaries are left out.
        let cargo = "   Compiling rbuild v0.1.0 (/private/tmp/dl-x/tree)
error[E0308]: mismatched types
 --> src/lib.rs:7:5
  |
6 | pub fn label(count: u32) -> String {
  |                             ------ expected `String` because of return type
7 |     count
  |     ^^^^^ expected `String`, found `u32`
  |
help: try using a conversion method
  |
7 |     count.to_string()
  |          ++++++++++++

error[E0425]: cannot find function `missing_function` in this scope
  --> src/lib.rs:11:5
   |
11 |     missing_function(2)
   |     ^^^^^^^^^^^^^^^^ not found in this scope

warning: unused variable: `unused`
 --> src/lib.rs:2:9
  |
2 |     let unused = 3;
  |         ^^^^^^ help: if this is intentional, prefix it with an underscore: `_unused`
  |
  = note: `#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default

error[E0308]: mismatched types
  --> src/lib.rs:20:36
   |
20 |         assert_eq!(total(&[1, 2]), \"3\");
   |                                    ^^^ expected `u32`, found `&str`

Some errors have detailed explanations: E0308, E0425.
For more information about an error, try `rustc --explain E0308`.
warning: `rbuild` (lib) generated 1 warning
error: could not compile `rbuild` (lib) due to 2 previous errors; 1 warning emitted
warning: build failed, waiting for other jobs to finish...
";
        let TestFailure::Build { errors } = package_failure(cargo, PackageOutcomeKind::BuildFailed, &rust_locator()) else {
            panic!();
        };
        assert_eq!(
            errors[0],
            build_error(
                "src/lib.rs",
                Some(7),
                Some(5),
                "error[E0308]: mismatched types",
                &[
                    "6 | pub fn label(count: u32) -> String {",
                    "  |                             ------ expected `String` because of return type",
                    "7 |     count",
                    "  |     ^^^^^ expected `String`, found `u32`",
                    "help: try using a conversion method",
                    "7 |     count.to_string()",
                    "  |          ++++++++++++",
                ]
            )
        );
        let places: Vec<_> = errors.iter().map(|error| (error.line, error.message.as_str())).collect();
        assert_eq!(
            places,
            vec![
                (Some(7), "error[E0308]: mismatched types"),
                (Some(11), "error[E0425]: cannot find function `missing_function` in this scope"),
                (Some(20), "error[E0308]: mismatched types"),
            ]
        );
    }

    /// The Python analyzer's testdata/failing and testdata/importerror, and
    /// a scratch project with more cases.
    fn python_locator() -> Locator {
        files(&[
            "calc.py",
            "test_calc.py",
            "app/__init__.py",
            "app/calc.py",
            "app/broken.py",
            "tests/test_calc.py",
            "tests/test_syntax.py",
            "tests/test_own_syntax.py",
            "tests/test_broken.py",
        ])
    }

    // The outputs below are real: what the Python analyzer reported for
    // failed tests and modules (pytest 9.2 with --tb=short, and unittest
    // through its own runner), Python 3.14.

    #[test]
    fn pytest_assertions_say_what_came_and_what_was_expected() {
        let output = "----- Captured stdout call -----
about to fail
test_calc.py:14: in test_add_wrong
    assert add(1, 2) == 4
E   assert 3 == 4
E    +  where 3 = add(1, 2)
";
        assert_eq!(
            test_failure(output, &[], Some("test_calc.py"), &python_locator()),
            assertion(
                "assert 3 == 4\nwhere 3 = add(1, 2)",
                Some("4"),
                Some("3"),
                Some(("test_calc.py", 14))
            )
        );

        let message = "tests/test_calc.py:14: in test_message
    assert ready, \"the service is not ready\"
E   AssertionError: the service is not ready
E   assert False
";
        assert_eq!(
            test_failure(message, &[], None, &python_locator()),
            assertion("the service is not ready", None, None, Some(("tests/test_calc.py", 14)))
        );

        // Long values are shortened; what differs is said below them.
        let dict = "tests/test_calc.py:18: in test_dict
    assert user() == {\"name\": \"ada\", \"age\": 37, \"city\": \"london\"}
E   AssertionError: assert {'name': 'ada...ty': 'london'} == {'name': 'ada...ty': 'london'}
E
E     Omitting 2 identical items, use -vv to show
E     Differing items:
E     {'age': 36} != {'age': 37}
E     Use -v to get more diff
";
        assert_eq!(
            test_failure(dict, &[], None, &python_locator()),
            assertion(
                "assert {'name': 'ada...ty': 'london'} == {'name': 'ada...ty': 'london'}",
                Some("{'age': 37}"),
                Some("{'age': 36}"),
                Some(("tests/test_calc.py", 18))
            )
        );

        let strings = "tests/test_calc.py:22: in test_strings
    assert \"hello world\" == \"hello there\"
E   AssertionError: assert 'hello world' == 'hello there'
E
E     - hello there
E     + hello world
";
        assert_eq!(
            test_failure(strings, &[], None, &python_locator()),
            assertion(
                "assert 'hello world' == 'hello there'",
                Some("'hello there'"),
                Some("'hello world'"),
                Some(("tests/test_calc.py", 22))
            )
        );

        let raises = "tests/test_calc.py:26: in test_raises
    with pytest.raises(ValueError):
         ^^^^^^^^^^^^^^^^^^^^^^^^^
E   Failed: DID NOT RAISE ValueError
";
        assert_eq!(
            test_failure(raises, &[], None, &python_locator()),
            assertion("DID NOT RAISE ValueError", None, None, Some(("tests/test_calc.py", 26)))
        );

        // A unittest case run by pytest.
        let unittest = "tests/test_calc.py:44: in test_equal
    self.assertEqual(total([1, 2]), 5)
E   AssertionError: 4 != 5
";
        assert_eq!(
            test_failure(unittest, &[], None, &python_locator()),
            assertion("4 != 5", Some("5"), Some("4"), Some(("tests/test_calc.py", 44)))
        );

        let membership = "tests/test_calc.py:39: in test_in
    assert 5 in [1, 2, 3]
E   assert 5 in [1, 2, 3]
";
        assert_eq!(
            test_failure(membership, &[], None, &python_locator()),
            assertion("assert 5 in [1, 2, 3]", None, None, Some(("tests/test_calc.py", 39)))
        );
    }

    #[test]
    fn other_python_exceptions_are_panics_at_the_innermost_project_frame() {
        let output = "test_calc.py:19: in test_divide
    assert divide(a, b) >= 1
           ^^^^^^^^^^^^
calc.py:8: in divide
    return a / b
           ^^^^^
E   TypeError: unsupported operand type(s) for /: 'str' and 'int'
";
        assert_eq!(
            test_failure(output, &[], None, &python_locator()),
            panic_at(
                "TypeError: unsupported operand type(s) for /: 'str' and 'int'",
                Some(frame("divide", "calc.py", 8))
            )
        );

        let key = "tests/test_calc.py:31: in test_key_error
    config(\"db\")
app/calc.py:6: in config
    raise KeyError(name)
E   KeyError: 'db'
";
        assert_eq!(
            test_failure(key, &[], None, &python_locator()),
            panic_at("KeyError: 'db'", Some(frame("config", "app/calc.py", 6)))
        );
        assert!(is_test_file("tests/test_calc.py") && is_test_file("test_calc.py") && !is_test_file("app/calc.py"));
    }

    #[test]
    fn unittest_tracebacks_are_read_too() {
        let equal = "Traceback (most recent call last):
  File \"/private/tmp/dl-x/tree/tests/test_calc.py\", line 8, in test_equal
    self.assertEqual(total([1, 2]), 5)
    ~~~~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^
AssertionError: 4 != 5

";
        assert_eq!(
            test_failure(equal, &[], None, &python_locator()),
            assertion("4 != 5", Some("5"), Some("4"), Some(("tests/test_calc.py", 8)))
        );

        let dict = "Traceback (most recent call last):
  File \"/private/tmp/dl-x/tree/tests/test_calc.py\", line 14, in test_dict
    self.assertEqual(user(), {\"name\": \"ada\", \"age\": 37, \"city\": \"london\"})
    ~~~~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: {'name': 'ada', 'age': 36, 'city': 'london'} != {'name': 'ada', 'age': 37, 'city': 'london'}
- {'age': 36, 'city': 'london', 'name': 'ada'}
?          ^

+ {'age': 37, 'city': 'london', 'name': 'ada'}
?          ^

";
        let TestFailure::Assertion { expected, actual, .. } = test_failure(dict, &[], None, &python_locator()) else {
            panic!();
        };
        assert_eq!(
            (expected.as_deref(), actual.as_deref()),
            (
                Some("{'name': 'ada', 'age': 37, 'city': 'london'}"),
                Some("{'name': 'ada', 'age': 36, 'city': 'london'}")
            )
        );

        let truth = "Traceback (most recent call last):
  File \"/private/tmp/dl-x/tree/tests/test_calc.py\", line 11, in test_true
    self.assertTrue(total([]) == 0, \"empty is zero\")
    ~~~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: False is not true : empty is zero
";
        assert_eq!(
            test_failure(truth, &[], None, &python_locator()),
            assertion("False is not true : empty is zero", None, None, Some(("tests/test_calc.py", 11)))
        );

        let key = "Traceback (most recent call last):
  File \"/private/tmp/dl-x/tree/tests/test_calc.py\", line 17, in test_key_error
    config(\"db\")
    ~~~~~~^^^^^^
  File \"/private/tmp/dl-x/tree/app/calc.py\", line 6, in config
    raise KeyError(name)
KeyError: 'db'
";
        assert_eq!(
            test_failure(key, &[], None, &python_locator()),
            panic_at("KeyError: 'db'", Some(frame("config", "app/calc.py", 6)))
        );
    }

    #[test]
    fn a_python_module_that_did_not_import_is_a_build_error() {
        let missing = "ImportError while importing test module '/private/tmp/dl-x/tree/tests/test_broken.py'.
Hint: make sure your test modules/packages have valid Python names.
Traceback:
/Users/me/miniconda3/lib/python3.14/importlib/__init__.py:88: in import_module
    return _bootstrap._gcd_import(name[level:], package, level)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
tests/test_broken.py:1: in <module>
    from app.missing import nothing
E   ModuleNotFoundError: No module named 'app.missing'
";
        assert_eq!(
            package_failure(missing, PackageOutcomeKind::BuildFailed, &python_locator()),
            TestFailure::Build {
                errors: vec![build_error(
                    "tests/test_broken.py",
                    Some(1),
                    None,
                    "ModuleNotFoundError: No module named 'app.missing'",
                    &[]
                )]
            }
        );

        // A module it imports does not parse: the error is there.
        let syntax = ".venv/lib/python3.14/site-packages/_pytest/python.py:528: in importtestmodule
    mod = import_path(
<frozen importlib._bootstrap>:1406: in _gcd_import
    ???
.venv/lib/python3.14/site-packages/_pytest/assertion/rewrite.py:185: in exec_module
    exec(co, module.__dict__)
tests/test_syntax.py:1: in <module>
    from app.broken import broken
E     File \"/private/tmp/dl-x/tree/app/broken.py\", line 1
E       def broken(:
E                  ^
E   SyntaxError: invalid syntax
";
        assert_eq!(
            package_failure(syntax, PackageOutcomeKind::BuildFailed, &python_locator()),
            TestFailure::Build {
                errors: vec![build_error(
                    "app/broken.py",
                    Some(1),
                    None,
                    "SyntaxError: invalid syntax",
                    &["def broken(:", "           ^"]
                )]
            }
        );

        let own = "/Users/me/miniconda3/lib/python3.14/ast.py:46: in parse
    return compile(source, filename, mode, flags,
E     File \"/private/tmp/dl-x/tree/tests/test_own_syntax.py\", line 1
E       def test_x(:
E                  ^
E   SyntaxError: invalid syntax
";
        let TestFailure::Build { errors } = package_failure(own, PackageOutcomeKind::BuildFailed, &python_locator()) else {
            panic!();
        };
        assert_eq!((errors[0].file.as_deref(), errors[0].line), (Some("tests/test_own_syntax.py"), Some(1)));

        // unittest, through the analyzer's own runner.
        let unittest = "Traceback (most recent call last):
  File \"/opt/difflens/analyzers/python/difflens_analyzer/inproject/_difflens_unittest.py\", line 128, in main
    loaded = loader.loadTestsFromName(module[\"module\"])
  File \"/Users/me/miniconda3/lib/python3.14/unittest/loader.py\", line 141, in loadTestsFromName
    module = __import__(module_name)
  File \"/private/tmp/dl-x/tree/tests/test_syntax.py\", line 1, in <module>
    from app.broken import broken
  File \"/private/tmp/dl-x/tree/app/broken.py\", line 1
    def broken(:
               ^
SyntaxError: invalid syntax
";
        assert_eq!(
            package_failure(unittest, PackageOutcomeKind::BuildFailed, &python_locator()),
            TestFailure::Build {
                errors: vec![build_error(
                    "app/broken.py",
                    Some(1),
                    None,
                    "SyntaxError: invalid syntax",
                    &["def broken(:", "           ^"]
                )]
            }
        );

        let unittest_import = "ImportError: Failed to import test module: test_calc
Traceback (most recent call last):
  File \"/Users/me/miniconda3/lib/python3.14/unittest/loader.py\", line 141, in loadTestsFromName
    module = __import__(module_name)
  File \"/private/tmp/dl-x/tree/tests/test_calc.py\", line 3, in <module>
    import pytest
ModuleNotFoundError: No module named 'pytest'


";
        assert_eq!(
            package_failure(unittest_import, PackageOutcomeKind::BuildFailed, &python_locator()),
            TestFailure::Build {
                errors: vec![build_error(
                    "tests/test_calc.py",
                    Some(3),
                    None,
                    "ModuleNotFoundError: No module named 'pytest'",
                    &[]
                )]
            }
        );
    }

    /// The JavaScript analyzer's jest-proj, vitest-proj, mocha-proj,
    /// nodetest-proj and failing, with more cases added to copies of them.
    fn js_locator() -> Locator {
        files(&[
            "src/sum.js",
            "src/__tests__/more.test.js",
            "src/missing.test.js",
            "src/broken.test.js",
            "src/math.ts",
            "src/more.test.ts",
            "src/missing.test.ts",
            "src/syntax.test.ts",
            "src/strings/upper.test.ts",
            "lib/stack.js",
            "test/more.spec.js",
            "test/stack.spec.js",
            "test/more.test.js",
            "test/missing.test.js",
            "test/broken/syntax.test.js",
        ])
    }

    // The outputs below are real: what the JavaScript analyzer's reporters
    // recorded for failed tests and packages (jest 30, vitest 4, mocha 11,
    // node:test of Node 26), stacks trimmed; node:test's TAP and spec
    // reporters run directly.

    #[test]
    fn jest_says_expected_and_received_and_the_frame_in_the_test() {
        let to_be = "Error: expect(received).toBe(expected) // Object.is equality

Expected: 4
Received: 3
    at Object.toBe (/private/tmp/dl-x/tree/src/__tests__/more.test.js:20:21)
    at Promise.finally.completed (/private/tmp/dl-x/tree/node_modules/jest-circus/build/jestAdapterInit.js:1834:28)
    at new Promise (<anonymous>)
    at processTicksAndRejections (node:internal/process/task_queues:104:5)
";
        assert_eq!(
            test_failure(to_be, &[], None, &js_locator()),
            assertion(
                "expect(received).toBe(expected) // Object.is equality",
                Some("4"),
                Some("3"),
                Some(("src/__tests__/more.test.js", 20))
            )
        );

        let to_equal = r#"Error: expect(received).toEqual(expected) // deep equality

- Expected  - 2
+ Received  + 2

  Object {
    "items": Array [
      "a",
-     "c",
+     "b",
    ],
-   "total": 4,
+   "total": 3,
  }
    at Object.toEqual (/private/tmp/dl-x/tree/src/__tests__/more.test.js:8:51)
    at Promise.finally.completed (/private/tmp/dl-x/tree/node_modules/jest-circus/build/jestAdapterInit.js:1834:28)
"#;
        assert_eq!(
            test_failure(to_equal, &[], None, &js_locator()),
            assertion(
                "expect(received).toEqual(expected) // deep equality",
                Some("Object {\n  \"items\": Array [\n    \"a\",\n    \"c\",\n  ],\n  \"total\": 4,\n}"),
                Some("Object {\n  \"items\": Array [\n    \"a\",\n    \"b\",\n  ],\n  \"total\": 3,\n}"),
                Some(("src/__tests__/more.test.js", 8))
            )
        );

        let truthy = "Error: expect(received).toBeTruthy()

Received: 0
    at Object.toBeTruthy (/private/tmp/dl-x/tree/src/__tests__/more.test.js:16:18)
";
        assert_eq!(
            test_failure(truthy, &[], None, &js_locator()),
            assertion(
                "expect(received).toBeTruthy()",
                None,
                Some("0"),
                Some(("src/__tests__/more.test.js", 16))
            )
        );

        let thrown = "Error: no config named db
    at load (/private/tmp/dl-x/tree/src/__tests__/more.test.js:4:9)
    at Object.load (/private/tmp/dl-x/tree/src/__tests__/more.test.js:12:3)
    at Promise.finally.completed (/private/tmp/dl-x/tree/node_modules/jest-circus/build/jestAdapterInit.js:1834:28)
";
        assert_eq!(
            test_failure(thrown, &[], None, &js_locator()),
            panic_at("Error: no config named db", Some(frame("load", "src/__tests__/more.test.js", 4)))
        );
    }

    #[test]
    fn vitest_says_expected_and_received_after_its_stack() {
        let to_be = "AssertionError: expected 'hello world' to be 'hello there' // Object.is equality
    at /private/tmp/dl-x/tree/src/more.test.ts:22:25
    at file:///private/tmp/dl-x/tree/node_modules/@vitest/runner/dist/chunk-artifact.js:302:11
    at new Promise (<anonymous>)
    at runWithTimeout (file:///private/tmp/dl-x/tree/node_modules/@vitest/runner/dist/chunk-artifact.js:2272:10)
Expected: \"hello there\"
Received: \"hello world\"
";
        assert_eq!(
            test_failure(to_be, &[], None, &js_locator()),
            assertion(
                "expected 'hello world' to be 'hello there' // Object.is equality",
                Some("\"hello there\""),
                Some("\"hello world\""),
                Some(("src/more.test.ts", 22))
            )
        );

        let to_equal = r#"AssertionError: expected { total: 3, items: [ 'a', 'b' ] } to deeply equal { total: 4, items: [ 'a', 'c' ] }
    at /private/tmp/dl-x/tree/src/more.test.ts:10:51
    at file:///private/tmp/dl-x/tree/node_modules/@vitest/runner/dist/chunk-artifact.js:302:11
- Expected
+ Received

  {
    "items": [
      "a",
-     "c",
+     "b",
    ],
-   "total": 4,
+   "total": 3,
  }"#;
        let TestFailure::Assertion { expected, actual, at, .. } = test_failure(to_equal, &[], None, &js_locator()) else {
            panic!();
        };
        assert_eq!(expected.as_deref(), Some("{\n  \"items\": [\n    \"a\",\n    \"c\",\n  ],\n  \"total\": 4,\n}"));
        assert_eq!(actual.as_deref(), Some("{\n  \"items\": [\n    \"a\",\n    \"b\",\n  ],\n  \"total\": 3,\n}"));
        assert_eq!(at.map(|at| (at.file, at.line)), Some(("src/more.test.ts".to_string(), 10)));

        let truthy = "AssertionError: expected +0 to be truthy
    at /private/tmp/dl-x/tree/src/more.test.ts:18:21
    at file:///private/tmp/dl-x/tree/node_modules/@vitest/runner/dist/chunk-artifact.js:302:11
- Expected:
true

+ Received:
0";
        assert_eq!(
            test_failure(truthy, &[], None, &js_locator()),
            assertion("expected +0 to be truthy", Some("true"), Some("0"), Some(("src/more.test.ts", 18)))
        );
    }

    #[test]
    fn node_assert_errors_read_as_mocha_and_node_test_print_them() {
        let simple = "AssertionError [ERR_ASSERTION]: Expected values to be strictly equal:

2 !== 3

    at Context.<anonymous> (test/stack.spec.js:16:14)
    at process.processImmediate (node:internal/timers:504:21)
";
        assert_eq!(
            test_failure(simple, &[], None, &js_locator()),
            assertion(
                "Expected values to be strictly equal",
                Some("3"),
                Some("2"),
                Some(("test/stack.spec.js", 16))
            )
        );

        let deep = "AssertionError [ERR_ASSERTION]: Expected values to be strictly deep-equal:
+ actual - expected

  {
+   size: 2,
-   size: 3,
    tag: 'x'
  }

    at Context.<anonymous> (test/more.spec.js:11:12)
    at process.processImmediate (node:internal/timers:504:21)
";
        assert_eq!(
            test_failure(deep, &[], None, &js_locator()),
            assertion(
                "Expected values to be strictly deep-equal",
                Some("{\n  size: 3,\n  tag: 'x'\n}"),
                Some("{\n  size: 2,\n  tag: 'x'\n}"),
                Some(("test/more.spec.js", 11))
            )
        );

        let strings = "AssertionError [ERR_ASSERTION]: Expected values to be strictly equal:
+ actual - expected

+ 'hello world'
- 'hello there'
         ^

    at Context.<anonymous> (test/more.spec.js:23:12)
";
        assert_eq!(
            test_failure(strings, &[], None, &js_locator()),
            assertion(
                "Expected values to be strictly equal",
                Some("'hello there'"),
                Some("'hello world'"),
                Some(("test/more.spec.js", 23))
            )
        );

        // The analyzer's reporter adds what was expected and what came.
        let ok = "AssertionError [ERR_ASSERTION]: the stack has items
    at Context.<anonymous> (test/more.spec.js:19:12)
    at process.processImmediate (node:internal/timers:504:21)
expected: true
actual: false
";
        assert_eq!(
            test_failure(ok, &[], None, &js_locator()),
            assertion("the stack has items", Some("true"), Some("false"), Some(("test/more.spec.js", 19)))
        );

        // node:test's stacks name files by URL.
        let node = "AssertionError [ERR_ASSERTION]: Expected values to be strictly equal:

110 !== 90

    at TestContext.<anonymous> (file:///private/tmp/dl-x/tree/test/price.test.js:12:12)
    at Test.runInAsyncScope (node:async_hooks:226:14)
    at Test.run (node:internal/test_runner/test:1382:25)
";
        assert_eq!(
            test_failure(node, &[], None, &files(&["test/price.test.js", "lib/price.js"])),
            assertion(
                "Expected values to be strictly equal",
                Some("90"),
                Some("110"),
                Some(("test/price.test.js", 12))
            )
        );

        let thrown = "Error: no config named db
    at load (file:///private/tmp/dl-x/tree/test/more.test.js:7:9)
    at TestContext.<anonymous> (file:///private/tmp/dl-x/tree/test/more.test.js:15:3)
    at Test.runInAsyncScope (node:async_hooks:226:14)
";
        assert_eq!(
            test_failure(thrown, &[], None, &js_locator()),
            panic_at("Error: no config named db", Some(frame("load", "test/more.test.js", 7)))
        );
    }

    #[test]
    fn node_test_tap_and_spec_reports_are_read() {
        let tap = "TAP version 13
# Subtest: deep
not ok 1 - deep
  ---
  duration_ms: 1.218083
  type: 'test'
  location: '/private/tmp/dl-x/tree/test/more.test.js:10:1'
  failureType: 'testCodeFailure'
  error: |-
    Expected values to be strictly deep-equal:
    + actual - expected

      {
        size: 2,
    +   word: 'ba'
    -   word: 'ab'
      }

  code: 'ERR_ASSERTION'
  name: 'AssertionError'
  expected:
    word: 'ab'
    size: 2
  actual:
    word: 'ba'
    size: 2
  operator: 'deepStrictEqual'
  stack: |-
    TestContext.<anonymous> (file:///private/tmp/dl-x/tree/test/more.test.js:11:10)
    Test.runInAsyncScope (node:async_hooks:226:14)
    Test.run (node:internal/test_runner/test:1382:25)
  ...
";
        assert_eq!(
            test_failure(tap, &[], None, &js_locator()),
            assertion(
                "Expected values to be strictly deep-equal",
                Some("word: 'ab'\nsize: 2"),
                Some("word: 'ba'\nsize: 2"),
                Some(("test/more.test.js", 11))
            )
        );

        let thrown = "# Subtest: throws in code
not ok 2 - throws in code
  ---
  duration_ms: 0.058292
  type: 'test'
  location: '/private/tmp/dl-x/tree/test/more.test.js:14:1'
  failureType: 'testCodeFailure'
  error: 'no config named db'
  code: 'ERR_TEST_FAILURE'
  stack: |-
    load (file:///private/tmp/dl-x/tree/test/more.test.js:7:9)
    TestContext.<anonymous> (file:///private/tmp/dl-x/tree/test/more.test.js:15:3)
    Test.runInAsyncScope (node:async_hooks:226:14)
  ...
";
        assert_eq!(
            test_failure(thrown, &[], None, &js_locator()),
            panic_at("no config named db", Some(frame("load", "test/more.test.js", 7)))
        );

        let spec = "✖ strings (0.1305ms)
ℹ tests 1
ℹ fail 1

✖ failing tests:

test at test/more.test.js:12:1
✖ strings (0.1305ms)
  AssertionError [ERR_ASSERTION]: Expected values to be strictly equal:

  'cba' !== 'abc'

      at TestContext.<anonymous> (file:///private/tmp/dl-x/tree/test/more.test.js:13:10)
      at Test.runInAsyncScope (node:async_hooks:226:14) {
    generatedMessage: true,
    code: 'ERR_ASSERTION',
    actual: 'cba',
    expected: 'abc',
    operator: 'strictEqual',
    diff: 'simple'
  }
";
        assert_eq!(
            test_failure(spec, &[], None, &js_locator()),
            assertion(
                "Expected values to be strictly equal",
                Some("'abc'"),
                Some("'cba'"),
                Some(("test/more.test.js", 13))
            )
        );
        let deep_spec = "  AssertionError [ERR_ASSERTION]: Expected values to be strictly deep-equal:
  + actual - expected

    {
      size: 2,
  +   word: 'ba'
  -   word: 'ab'
    }

      at TestContext.<anonymous> (file:///private/tmp/dl-x/tree/test/more.test.js:9:10)
      at Test.runInAsyncScope (node:async_hooks:226:14) {
    generatedMessage: true,
    code: 'ERR_ASSERTION',
    actual: { word: 'ba', size: 2 },
    expected: { word: 'ab', size: 2 },
";
        let TestFailure::Assertion { expected, actual, .. } = test_failure(deep_spec, &[], None, &js_locator()) else {
            panic!();
        };
        assert_eq!(expected.as_deref(), Some("{\n  size: 2,\n  word: 'ab'\n}"));
        assert_eq!(actual.as_deref(), Some("{\n  size: 2,\n  word: 'ba'\n}"));
    }

    #[test]
    fn a_javascript_file_that_did_not_load_is_a_build_error() {
        let node = "file:///private/tmp/dl-x/tree/test/broken/syntax.test.js:4
  return (1;
           ^

SyntaxError: Unexpected token ';'
    at compileSourceTextModule (node:internal/modules/esm/utils:354:16)
    at ModuleLoader.moduleStrategy (node:internal/modules/esm/translators:90:18)

Node.js v26.3.0
";
        assert_eq!(
            package_failure(node, PackageOutcomeKind::BuildFailed, &js_locator()),
            TestFailure::Build {
                errors: vec![build_error(
                    "test/broken/syntax.test.js",
                    Some(4),
                    None,
                    "SyntaxError: Unexpected token ';'",
                    &["return (1;", "         ^"]
                )]
            }
        );

        let module = "node:internal/modules/esm/resolve:271
    throw new ERR_MODULE_NOT_FOUND(
          ^

Error [ERR_MODULE_NOT_FOUND]: Cannot find module '/private/tmp/dl-x/tree/src/not-there.js' imported from /private/tmp/dl-x/tree/test/missing.test.js
    at finalizeResolution (node:internal/modules/esm/resolve:271:11)
    at moduleResolve (node:internal/modules/esm/resolve:865:10) {
  code: 'ERR_MODULE_NOT_FOUND',
  url: 'file:///private/tmp/dl-x/tree/src/not-there.js'
}

Node.js v26.3.0
";
        assert_eq!(
            package_failure(module, PackageOutcomeKind::BuildFailed, &js_locator()),
            TestFailure::Build {
                errors: vec![build_error(
                    "test/missing.test.js",
                    None,
                    None,
                    "Error [ERR_MODULE_NOT_FOUND]: Cannot find module '/private/tmp/dl-x/tree/src/not-there.js' imported from /private/tmp/dl-x/tree/test/missing.test.js",
                    &[]
                )]
            }
        );

        let jest = r#"  ● Test suite failed to run

    Cannot find module './not-there' from 'src/missing.test.js'

    > 1 | const { nothing } = require("./not-there");
        |                     ^
      2 |
      3 | test("never runs", () => {
      4 |   expect(nothing).toBe(1);

      at Resolver._throwModNotFoundError (node_modules/jest-resolve/build/index.js:1031:11)
      at Object.require (src/missing.test.js:1:21)

  ● Test suite failed to run

    SyntaxError: /private/tmp/dl-x/tree/src/broken.test.js: Unexpected token, expected "," (4:23)

      2 |
      3 | test("never runs", () => {
    > 4 |   expect(sum(1)).toBe(1;
        |                        ^
      5 | });
      6 |

      at constructor (node_modules/@babel/parser/src/parse-error.ts:96:45)
          at parser.next (<anonymous>)
"#;
        assert_eq!(
            package_failure(jest, PackageOutcomeKind::BuildFailed, &js_locator()),
            TestFailure::Build {
                errors: vec![
                    build_error(
                        "src/missing.test.js",
                        Some(1),
                        None,
                        "Cannot find module './not-there' from 'src/missing.test.js'",
                        &[
                            r#"> 1 | const { nothing } = require("./not-there");"#,
                            "    |                     ^",
                            r#"  3 | test("never runs", () => {"#,
                            "  4 |   expect(nothing).toBe(1);",
                        ]
                    ),
                    build_error(
                        "src/broken.test.js",
                        Some(4),
                        Some(23),
                        r#"SyntaxError: Unexpected token, expected "," (4:23)"#,
                        &[
                            r#"  3 | test("never runs", () => {"#,
                            "> 4 |   expect(sum(1)).toBe(1;",
                            "    |                        ^",
                            "  5 | });",
                        ]
                    ),
                ]
            }
        );

        // vitest colours its parse errors even when not on a terminal.
        let vitest = "Error: Transform failed with 1 error:

␛[31m[PARSE_ERROR] ␛[0mExpected `,` or `)` but found `;`
   ␛[38;5;246m╭␛[0m␛[38;5;246m─␛[0m␛[38;5;246m[␛[0m src/syntax.test.ts:4:19 ␛[38;5;246m]␛[0m
   ␛[38;5;246m│␛[0m
 ␛[38;5;246m4 │␛[0m ␛[38;5;249m ␛[0m␛[38;5;249m ␛[0m␛[38;5;249mexpect(1).toBe␛[0m(␛[38;5;249m1␛[0m;
 ␛[38;5;240m  │␛[0m                 ┬ ┬
 ␛[38;5;240m  │␛[0m                 ╰──── Opened here
␛[38;5;246m───╯␛[0m

    at transformWithOxc (file:///private/tmp/dl-x/tree/node_modules/vite/dist/node/chunks/node.js:7673:19)
    at TransformPluginContext.transform (file:///private/tmp/dl-x/tree/node_modules/vite/dist/node/chunks/node.js:7747:26)
Error: Cannot find module './not-there' imported from /private/tmp/dl-x/tree/src/missing.test.ts
    at reviveInvokeError (file:///private/tmp/dl-x/tree/node_modules/vite/dist/node/module-runner.js:546:14)
    at process.processTicksAndRejections (node:internal/process/task_queues:104:5)
    at /private/tmp/dl-x/tree/src/missing.test.ts:3:1
    at async VitestModuleEvaluator._runInlinedModule (file:///private/tmp/dl-x/tree/node_modules/vitest/dist/module-evaluator.js:206:4)
"
        .replace('␛', "\u{1b}");
        assert_eq!(
            package_failure(&vitest, PackageOutcomeKind::BuildFailed, &js_locator()),
            TestFailure::Build {
                errors: vec![
                    build_error(
                        "src/syntax.test.ts",
                        Some(4),
                        Some(19),
                        "Error: Transform failed with 1 error: [PARSE_ERROR] Expected `,` or `)` but found `;`",
                        &[
                            "4 │   expect(1).toBe(1;",
                            "  │                 ┬ ┬",
                            "  │                 ╰──── Opened here",
                        ]
                    ),
                    build_error(
                        "src/missing.test.ts",
                        Some(3),
                        None,
                        "Error: Cannot find module './not-there' imported from /private/tmp/dl-x/tree/src/missing.test.ts",
                        &[]
                    ),
                ]
            }
        );
    }

    #[test]
    fn paths_of_other_languages_are_placed_in_the_project() {
        let locator = files(&["crates/core/src/lib.rs", "web/src/app.test.ts", "My App/src/a.js"]);
        // Relative to a project in a folder of the tree.
        assert_eq!(locator.relative("src/lib.rs").as_deref(), Some("crates/core/src/lib.rs"));
        assert_eq!(locator.relative("./src/app.test.ts").as_deref(), Some("web/src/app.test.ts"));
        assert_eq!(locator.relative("src/unknown.rs").as_deref(), Some("src/unknown.rs"));
        assert_eq!(
            locator.relative("file:///private/tmp/dl-x/tree/My%20App/src/a.js").as_deref(),
            Some("My App/src/a.js")
        );
        for outside in [
            "/Users/me/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/serde-1.0.0/src/lib.rs",
            "/Users/me/.cargo/git/checkouts/x-1/abc/src/lib.rs",
            "/rustc/2d20b42b883dcfc014f3c2c56081eeb6091c2179/library/std/src/panicking.rs",
            "/Users/me/.rustup/toolchains/nightly-aarch64-apple-darwin/lib/rustlib/src/rust/library/core/src/ops/function.rs",
            "/private/tmp/dl-x/tree/node_modules/jest-circus/build/index.js",
            "node_modules/jest-resolve/build/index.js",
            "file:///private/tmp/dl-x/tree/node_modules/vite/dist/node/chunks/node.js",
            "node:internal/process/task_queues",
            "<anonymous>",
            ".venv/lib/python3.14/site-packages/_pytest/python.py",
            "/Users/me/miniconda3/lib/python3.14/importlib/__init__.py",
            "<frozen importlib._bootstrap>",
        ] {
            assert_eq!(locator.relative(outside), None, "{outside}");
        }
        // A project file named like a dependency's is not taken for it.
        assert_eq!(files(&["index.js"]).relative("/x/tree/node_modules/a/index.js"), None);
        assert!(is_test_file("src/__tests__/sum.test.js") && is_test_file("tests/api.rs") && !is_test_file("src/lib.rs"));
        assert_eq!(without_colours("\u{1b}[31mred\u{1b}[0m plain"), "red plain");
    }
}
