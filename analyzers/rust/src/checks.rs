//! The `check` command: clippy on the tree (checks.schema.json), its
//! findings (clippy's lints and rustc's warnings, with the lint name as
//! rule and the level the compiler gave it as severity) per file and line.
//!
//! Clippy compiles the project, which runs its build scripts and proc
//! macros, so it never runs on the tree itself nor freely: each workspace
//! is copied into a scratch folder (without `target/` and `.git/`), its
//! dependencies are fetched there (`cargo fetch`, which runs nothing), and
//! clippy runs offline under the macOS sandbox, writing only in that
//! folder and, with --cache-dir, in `<cache-dir>/clippy`, the target folder
//! kept between checks (see `cache`: the copy is set modified now first,
//! so its own crates are always checked again). Where the sandbox is not
//! available, clippy is reported unavailable rather than run unconfined.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use serde_json::Value;

use crate::cache;
use crate::process;
use crate::protocol::{Check, CheckRun, Finding};
use crate::sandbox;
use crate::workspace::{self, Tree, Unit};

pub const TOOL: &str = "clippy";

/// The folder of --cache-dir clippy builds in, apart from the tests' so
/// that a check never waits for a test build's lock.
const CACHE_FOLDER: &str = "clippy";

/// One checker's outcome over the workspaces of the tree.
struct Outcome {
    /// File, line, rule, message and severity.
    findings: BTreeSet<(String, u32, String, String, &'static str)>,
    failures: Vec<String>,
    missing: Option<String>,
    /// Problems that did not stop the check, for the run's diagnostics.
    notes: Vec<String>,
}

impl Outcome {
    fn fail(&mut self, why: String) {
        if !why.is_empty() && !self.failures.contains(&why) {
            self.failures.push(why);
        }
    }

    fn into_check(self) -> Check {
        let mut check = Check {
            tool: TOOL.to_string(),
            status: "clean".to_string(),
            message: None,
            findings: self
                .findings
                .into_iter()
                .map(|(file, line, rule, message, severity)| Finding {
                    file,
                    line,
                    message,
                    rule: (!rule.is_empty()).then_some(rule),
                    severity: severity.to_string(),
                })
                .collect(),
        };
        if let Some(why) = self.missing {
            check.status = "unavailable".to_string();
            check.message = Some(why);
            check.findings.clear();
        } else if let Some(first) = self.failures.first() {
            check.status = "failed".to_string();
            check.message = Some(match self.failures.len() {
                1 => first.clone(),
                count => format!("{first} (and {} more problems)", count - 1),
            });
        } else if !check.findings.is_empty() {
            check.status = "findings".to_string();
        }
        check
    }
}

/// Why clippy cannot run here, if it cannot.
fn unavailable() -> Option<String> {
    if !process::on_path("cargo") {
        return Some("cargo is not installed (no cargo on the PATH)".to_string());
    }
    let mut command = Command::new("cargo");
    command.args(["clippy", "--version"]);
    match process::capture(command, Instant::now() + std::time::Duration::from_secs(60)) {
        Ok((_, finished)) if finished.success() => {}
        _ => return Some("clippy is not installed (rustup component add clippy)".to_string()),
    }
    if !sandbox::available() {
        return Some("clippy runs the project's build scripts and proc macros, which this analyzer only allows inside the macOS sandbox".to_string());
    }
    None
}

/// Clippy on every workspace of `dir`; with `cache_dir`, its build output
/// is kept there for the next check.
pub fn check(dir: &Path, deadline: Instant, cache_dir: Option<&Path>) -> std::io::Result<CheckRun> {
    let tree = Tree::new(dir)?;
    let workspace = workspace::discover(&tree, deadline);
    let mut run = CheckRun::new();
    run.diagnostics.extend(workspace.warnings.iter().cloned());
    let mut outcome = Outcome {
        findings: BTreeSet::new(),
        failures: Vec::new(),
        missing: unavailable(),
        notes: Vec::new(),
    };
    if outcome.missing.is_none() {
        for (position, unit) in workspace.units.iter().enumerate() {
            clippy(&tree, unit, position, deadline, cache_dir, &mut outcome);
        }
    }
    run.diagnostics.append(&mut outcome.notes);
    run.checks.push(outcome.into_check());
    Ok(run)
}

/// A scratch folder of this process, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn create(name: &str) -> std::io::Result<Self> {
        // Unique within the process too: the unit tests check at once.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("difflens-{name}-{}-{next}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)?;
        Ok(Self(path.canonicalize()?))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Copies `from` into `to`, without build output, VCS data and
/// dependencies' folders; links stay links.
pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(name.to_str(), Some("target" | ".git" | "node_modules")) {
            continue;
        }
        let source = entry.path();
        let destination = to.join(&name);
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&source)?, &destination)?;
        } else if kind.is_dir() {
            copy_tree(&source, &destination)?;
        } else {
            std::fs::copy(&source, &destination)?;
        }
    }
    Ok(())
}

/// The cache folder clippy builds `copy` in, held, or None (with a note)
/// when it cannot be used and the scratch folder is built in instead.
fn cache_target(
    cache_dir: Option<&Path>,
    copy: &Path,
    deadline: Instant,
    outcome: &mut Outcome,
) -> Option<cache::Held> {
    let held = cache::acquire(cache_dir?, CACHE_FOLDER, deadline)
        .and_then(|held| {
            // Its own crates must be checked again, whatever an earlier
            // copy left there (see `cache`).
            cache::touch_tree(copy)
                .map(|()| held)
                .map_err(|error| format!("the copy could not be set modified now: {error}"))
        })
        .map_err(|why| {
            let note = format!("clippy did not use the build output of earlier checks: {why}");
            if !outcome.notes.contains(&note) {
                outcome.notes.push(note);
            }
        });
    held.ok()
}

fn clippy(
    tree: &Tree,
    unit: &Unit,
    position: usize,
    deadline: Instant,
    cache_dir: Option<&Path>,
    outcome: &mut Outcome,
) {
    let unit_rel = tree.rel(&unit.root).unwrap_or_else(|| ".".to_string());
    let scratch = match Scratch::create(&format!("clippy-{position}")) {
        Ok(scratch) => scratch,
        Err(error) => {
            return outcome.fail(format!("clippy could not get a scratch folder: {error}"))
        }
    };
    let copy = scratch.0.join("tree");
    let temporary = scratch.0.join("tmp");
    if let Err(error) =
        copy_tree(&unit.root, &copy).and_then(|()| std::fs::create_dir_all(&temporary))
    {
        return outcome.fail(format!("clippy could not copy {unit_rel}: {error}"));
    }
    let env = |command: &mut Command, target: &Path| {
        command
            .current_dir(&copy)
            .env("CARGO_TARGET_DIR", target)
            .env("CARGO_TERM_COLOR", "never")
            .env("CARGO_INCREMENTAL", "0")
            .env("TMPDIR", &temporary);
    };
    // Fetching runs no project code; it may need the network.
    let mut fetch = Command::new("cargo");
    fetch.arg("fetch");
    env(&mut fetch, &scratch.0.join("target"));
    if let Ok((_, finished)) = process::capture(fetch, deadline) {
        if !finished.success() && finished.timed_out {
            return outcome.fail("clippy did not finish before the time limit".to_string());
        }
    }
    // Held until clippy is done.
    let held = cache_target(cache_dir, &copy, deadline, outcome);
    let target = match &held {
        Some(held) => held.dir.clone(),
        None => scratch.0.join("target"),
    };
    let writable: Vec<&Path> = held.iter().map(|held| held.dir.as_path()).collect();
    let mut command = sandbox::command(
        &scratch.0,
        &writable,
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--keep-going",
            "--offline",
            "--message-format=json",
        ],
    );
    env(&mut command, &target);
    let mut errors = Vec::new();
    let mut finished_ok = None;
    let result = process::stream(command, deadline, |line| {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return;
        };
        match message["reason"].as_str() {
            Some("compiler-message") => {
                diagnostic(&copy, &unit_rel, &message["message"], outcome, &mut errors)
            }
            Some("build-finished") => finished_ok = message["success"].as_bool(),
            _ => {}
        }
    });
    let finished = match result {
        Ok(finished) => finished,
        Err(error) => return outcome.fail(format!("cargo clippy could not run: {error}")),
    };
    if finished.timed_out {
        return outcome.fail("clippy did not finish before the time limit".to_string());
    }
    if let Some(first) = errors.first() {
        let more = if errors.len() > 1 {
            format!(" (and {} more errors)", errors.len() - 1)
        } else {
            String::new()
        };
        return outcome.fail(format!("the code does not compile: {first}{more}"));
    }
    if finished_ok == Some(false) || !finished.success() {
        let why = process::first_line(
            &finished
                .stderr
                .lines()
                .filter(|line| line.trim_start().starts_with("error"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let why = if why.is_empty() {
            finished.describe()
        } else {
            why
        };
        outcome.fail(format!("cargo clippy failed in {unit_rel}: {why}"));
    }
}

/// One compiler message: a finding when it is a lint, a compile error to
/// report as the check's failure otherwise.
fn diagnostic(
    workdir: &Path,
    unit_rel: &str,
    message: &Value,
    outcome: &mut Outcome,
    errors: &mut Vec<String>,
) {
    let Some(severity) = severity(message["level"].as_str().unwrap_or("")) else {
        return;
    };
    let text = message["message"].as_str().unwrap_or("").to_string();
    let code = message["code"]["code"].as_str().unwrap_or("").to_string();
    let Some(span) = message["spans"].as_array().and_then(|spans| {
        spans
            .iter()
            .find(|span| span["is_primary"].as_bool() == Some(true))
    }) else {
        return;
    };
    let file = span["file_name"].as_str().unwrap_or("");
    let line = span["line_start"]
        .as_u64()
        .and_then(|line| u32::try_from(line).ok())
        .unwrap_or(1)
        .max(1);
    let path = workspace::normalize(&workdir.join(file));
    let Ok(rest) = path.strip_prefix(workdir) else {
        return;
    };
    let rest = rest.to_string_lossy().replace('\\', "/");
    let rel = if unit_rel == "." {
        rest
    } else {
        format!("{unit_rel}/{rest}")
    };
    let is_lint = !code.is_empty() && !is_error_code(&code);
    if severity == "error" && !is_lint {
        errors.push(format!("{rel}:{line}: {text}"));
        return;
    }
    if !is_lint {
        return;
    }
    outcome.findings.insert((rel, line, code, text, severity));
}

/// The protocol's severity of a compiler message level: "error" (a lint
/// set to deny included) and "error: internal compiler error" are errors,
/// "warning" a warning, "note", "help" and "failure-note" info.
fn severity(level: &str) -> Option<&'static str> {
    Some(match level {
        "error" | "error: internal compiler error" => "error",
        "warning" => "warning",
        "note" | "help" | "failure-note" => "info",
        _ => return None,
    })
}

/// rustc's error codes: "E0308".
fn is_error_code(code: &str) -> bool {
    code.len() == 5 && code.starts_with('E') && code[1..].chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lints_are_findings_and_errors_are_failures() {
        let workdir = Path::new("/scratch/tree");
        let mut outcome = Outcome {
            findings: BTreeSet::new(),
            failures: Vec::new(),
            missing: None,
            notes: Vec::new(),
        };
        let mut errors = Vec::new();
        let lint: Value = serde_json::json!({
            "level": "warning", "message": "unneeded `return` statement",
            "code": { "code": "clippy::needless_return" },
            "spans": [{ "is_primary": true, "file_name": "bad/src/lib.rs", "line_start": 8 }]
        });
        diagnostic(workdir, "crates", &lint, &mut outcome, &mut errors);
        let denied: Value = serde_json::json!({
            "level": "error", "message": "used `unwrap()` on an `Option` value",
            "code": { "code": "clippy::unwrap_used" },
            "spans": [{ "is_primary": true, "file_name": "bad/src/lib.rs", "line_start": 9 }]
        });
        diagnostic(workdir, "crates", &denied, &mut outcome, &mut errors);
        let error: Value = serde_json::json!({
            "level": "error", "message": "mismatched types", "code": { "code": "E0308" },
            "spans": [{ "is_primary": true, "file_name": "bad/src/lib.rs", "line_start": 11 }]
        });
        diagnostic(workdir, "crates", &error, &mut outcome, &mut errors);
        let summary: Value = serde_json::json!({ "level": "error", "message": "aborting", "code": null, "spans": [] });
        diagnostic(workdir, "crates", &summary, &mut outcome, &mut errors);
        assert_eq!(errors, ["crates/bad/src/lib.rs:11: mismatched types"]);
        let check = outcome.into_check();
        assert_eq!(check.status, "findings");
        let found: Vec<(&str, u32, Option<&str>, &str)> = check
            .findings
            .iter()
            .map(|finding| {
                (
                    finding.file.as_str(),
                    finding.line,
                    finding.rule.as_deref(),
                    finding.severity.as_str(),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                (
                    "crates/bad/src/lib.rs",
                    8,
                    Some("clippy::needless_return"),
                    "warning"
                ),
                (
                    "crates/bad/src/lib.rs",
                    9,
                    Some("clippy::unwrap_used"),
                    "error"
                ),
            ]
        );
    }

    #[test]
    fn compiler_levels_become_severities() {
        for (level, expected) in [
            ("error", Some("error")),
            ("error: internal compiler error", Some("error")),
            ("warning", Some("warning")),
            ("note", Some("info")),
            ("help", Some("info")),
            ("failure-note", Some("info")),
            ("", None),
        ] {
            assert_eq!(severity(level), expected, "{level}");
        }
    }

    fn check_in(tree: &Path, cache_dir: Option<&Path>) -> Check {
        let deadline = Instant::now() + std::time::Duration::from_secs(300);
        let mut run = check(tree, deadline, cache_dir).unwrap();
        assert!(
            !tree.join("target").exists() && !tree.join("Cargo.lock").exists(),
            "the tree is not written"
        );
        run.checks.remove(0)
    }

    fn check_of(fixture: &str) -> Check {
        let (_folder, tree) = crate::testing::TempDir::with_fixture(fixture);
        check_in(&tree, None)
    }

    #[test]
    fn clippy_reports_lints_and_code_that_does_not_compile() {
        let lints = check_of("failing");
        if lints.status == "unavailable" {
            return; // no clippy or no sandbox on this machine
        }
        assert_eq!(lints.status, "findings", "{:?}", lints.message);
        assert_eq!(lints.findings.len(), 1);
        assert_eq!(
            (
                lints.findings[0].file.as_str(),
                lints.findings[0].line,
                lints.findings[0].rule.as_deref()
            ),
            ("src/lib.rs", 5, Some("clippy::needless_return"))
        );
        assert_eq!(lints.findings[0].severity, "warning");
        let broken = check_of("broken");
        assert_eq!(broken.status, "failed");
        assert!(
            broken
                .message
                .as_deref()
                .unwrap()
                .starts_with("the code does not compile: bad/"),
            "{:?}",
            broken.message
        );
    }

    #[test]
    fn a_kept_target_folder_never_gives_the_findings_of_other_code() {
        let folder = crate::testing::TempDir::new();
        let cache_dir = folder.0.join("cache");
        let (first, second) = (folder.0.join("first"), folder.0.join("second"));
        crate::testing::write_crate(
            &first,
            "kept",
            &[("src/lib.rs", "pub fn answer() -> u32 {\n    return 1;\n}\n")],
        );
        // The same crate at another path, other code, older mtimes.
        crate::testing::write_crate(
            &second,
            "kept",
            &[(
                "src/lib.rs",
                "pub fn answer() -> u32 {\n    let value = 2;\n    value\n}\n",
            )],
        );
        crate::testing::age_tree(&second);
        let check = check_in(&first, Some(&cache_dir));
        if check.status == "unavailable" {
            return; // no clippy or no sandbox on this machine
        }
        let rules = |check: &Check| -> Vec<String> {
            check
                .findings
                .iter()
                .filter_map(|finding| finding.rule.clone())
                .collect()
        };
        assert_eq!(rules(&check), ["clippy::needless_return"], "{check:?}");
        assert!(
            cache_dir.join(CACHE_FOLDER).join("debug").is_dir(),
            "clippy built in the cache folder"
        );
        let check = check_in(&second, Some(&cache_dir));
        assert_eq!(rules(&check), ["clippy::let_and_return"], "{check:?}");
    }

    #[test]
    fn a_missing_checker_is_unavailable_with_no_findings() {
        let mut findings = BTreeSet::new();
        findings.insert((
            "a.rs".to_string(),
            1,
            String::new(),
            "x".to_string(),
            "warning",
        ));
        let check = Outcome {
            findings,
            failures: vec!["x".to_string()],
            missing: Some("not installed".to_string()),
            notes: Vec::new(),
        }
        .into_check();
        assert_eq!(
            (check.status.as_str(), check.message.as_deref()),
            ("unavailable", Some("not installed"))
        );
        assert!(check.findings.is_empty());
    }
}
