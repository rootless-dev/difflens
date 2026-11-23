//! The `prepare` and `test` commands (tests.schema.json).
//!
//! `prepare` runs `cargo fetch` in every workspace of the tree, so that the
//! test step can build offline. It runs no project code; the target folder
//! the test step builds in is inside the run's copy of the tree or in
//! --cache-dir, which the core opens itself, so it reports no folder to
//! open.
//!
//! `test` runs every test target `cargo test --tests` would (libraries,
//! binaries and integration tests; doc tests are not run), or the
//! `--selection`'s:
//!
//! 1. `cargo build --tests --keep-going --message-format=json` builds them
//!    all at once, offline with `--offline`, into `<workspace>/target`, or
//!    `<cache-dir>/target` with --cache-dir (held for the whole run, the
//!    tree set modified now before each build: see `cache`), and says which
//!    test binary each target became, or the compiler errors of the ones
//!    that did not build (`build_failed`). Only what this build reports is
//!    run, never another binary of the target folder.
//! 2. Each test binary then runs as `cargo test` would run it (from its
//!    package's folder, with Cargo's environment), with libtest's JSON
//!    events (`--format json --report-time`, enabled on a stable toolchain
//!    for that process only with RUSTC_BOOTSTRAP=1), which give every test
//!    its outcome, duration and captured output. Targets with
//!    `harness = false` run plainly and are judged by their exit status.
//! 3. With coverage, the build and the binaries run in cargo-llvm-cov's
//!    environment and its LCOV report gives the blocks (see `coverage`);
//!    the profiles earlier runs left in the target folder are removed
//!    before the build.
//!
//! A "package" of the protocol is a test target, named by its crate root
//! (`shapes`, `shapes/tests/api`), and a test's `function` is the id
//! analyze gives the test function (the root, then the name libtest
//! prints). Each test and package carries the `cargo test` command that
//! runs it alone from the root of the tree (`target_command`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::analyze::Parsed;
use crate::cache;
use crate::coverage;
use crate::index::Index;
use crate::process::{self, Tail};
use crate::protocol::{
    self, Coverage, Package as PackageResult, Progress, Selection, Test, TestRun,
};
use crate::workspace::{Package, Target, TargetKind, Unit};

/// Output limit of a test's or a package's output.
const MAX_OUTPUT: usize = 64 << 10;

/// How often progress is written while tests end; the end of a package is
/// always written.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

pub struct Options {
    pub timeout: Duration,
    pub offline: bool,
    pub race: bool,
    pub cover: bool,
    pub selection: Option<Selection>,
    pub progress: Option<PathBuf>,
    /// The core's folder kept between runs, for the build output.
    pub cache_dir: Option<PathBuf>,
}

/// The folder of --cache-dir the tests build in.
const CACHE_FOLDER: &str = "target";

/// Fetches the dependencies of every workspace of the tree.
pub fn prepare(dir: &Path, deadline: Instant) -> Result<Vec<String>, String> {
    let tree = crate::workspace::Tree::new(dir).map_err(|error| error.to_string())?;
    if crate::workspace::manifest_dirs(&tree.root).is_empty() {
        return Err(format!(
            "{} has no Cargo.toml (nor any folder one level down)",
            tree.root.display()
        ));
    }
    if !process::on_path("cargo") {
        return Err("cargo was not found on the PATH".to_string());
    }
    let workspace = crate::workspace::discover(&tree, deadline);
    let mut roots: Vec<PathBuf> = workspace
        .units
        .iter()
        .map(|unit| unit.root.clone())
        .collect();
    if roots.is_empty() {
        roots = crate::workspace::manifest_dirs(&tree.root);
    }
    for root in roots {
        let mut command = Command::new("cargo");
        command
            .arg("fetch")
            .current_dir(&root)
            .env("CARGO_TERM_COLOR", "never");
        let (_, finished) = process::capture(command, deadline)
            .map_err(|error| format!("cargo could not run: {error}"))?;
        if !finished.success() {
            let rel = tree.rel(&root).unwrap_or_else(|| ".".to_string());
            return Err(format!(
                "{rel}: cargo fetch failed ({}):\n{}",
                finished.describe(),
                process::keep_end(&finished.stderr, 4 << 10)
            ));
        }
    }
    Ok(Vec::new())
}

/// Writes the `--progress` file whole, so that the core never reads half.
struct ProgressFile {
    path: Option<PathBuf>,
    written: Option<Instant>,
}

impl ProgressFile {
    fn update(&mut self, progress: &Progress, force: bool) {
        let Some(path) = &self.path else { return };
        if !force
            && self
                .written
                .is_some_and(|written| written.elapsed() < PROGRESS_INTERVAL)
        {
            return;
        }
        self.written = Some(Instant::now());
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let temporary = path.with_file_name(format!(".{name}.tmp"));
        if let Ok(bytes) = serde_json::to_vec(progress) {
            if std::fs::write(&temporary, bytes).is_ok() {
                let _ = std::fs::rename(&temporary, path);
            }
        }
    }
}

/// One test target to run, and the tests to run of it (None: all).
struct Planned<'p> {
    unit: usize,
    package: &'p Package,
    target: &'p Target,
    root: String,
    filter: Option<Vec<String>>,
    /// The command that runs the target alone (`target_command`).
    command: String,
}

/// `word` as one shell word: as it is when nothing in it is special to a
/// shell, else in single quotes (a quote inside becomes `'\''`).
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.,/:@+".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// The `cargo test` command that runs `target` of `package` alone, from the
/// root of the tree; `manifest` is the workspace's Cargo.toml relative to
/// it when the workspace is not at the root.
fn target_command(manifest: Option<&str>, package: &Package, target: &Target) -> String {
    let mut words = vec!["cargo".to_string(), "test".to_string()];
    if let Some(manifest) = manifest {
        words.push("--manifest-path".to_string());
        words.push(shell_word(manifest));
    }
    words.push("-p".to_string());
    words.push(shell_word(&package.name));
    words.push(format!("--{}", target.kind.word()));
    if target.kind != TargetKind::Lib {
        words.push(shell_word(&target.name));
    }
    words.join(" ")
}

/// The command that runs one test of a planned target alone, as libtest
/// names it; a `harness = false` target has no name filter, so it runs
/// whole.
fn test_command(planned: &Planned, name: &str) -> String {
    if !planned.target.harness {
        return planned.command.clone();
    }
    format!("{} -- {} --exact", planned.command, shell_word(name))
}

/// What the build said.
#[derive(Default)]
struct Built {
    /// Test binary of each target, by source path.
    executables: HashMap<PathBuf, PathBuf>,
    /// Compiler errors of each target, by source path.
    errors: HashMap<PathBuf, Vec<String>>,
    /// Every error, in order, with the package folder it came from.
    all_errors: Vec<(Option<PathBuf>, String)>,
    /// OUT_DIR of each package with a build script, by package folder.
    out_dirs: HashMap<PathBuf, PathBuf>,
    /// Native library folders the build scripts asked to link.
    linked_paths: Vec<PathBuf>,
    stderr: String,
    timed_out: bool,
}

/// The folder of a package from its id: "path+file:///w/a#0.1.0" (or the
/// older "a 0.1.0 (path+file:///w/a)").
fn package_dir(id: &str) -> Option<PathBuf> {
    let start = id.find("file://")? + "file://".len();
    let rest = &id[start..];
    let end = rest.find(['#', ')', '?']).unwrap_or(rest.len());
    Some(PathBuf::from(percent_decode(&rest[..end])))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(text.get(index + 1..index + 3).unwrap_or(""), 16)
            {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Built {
    fn message(&mut self, message: &Value) {
        let source = |message: &Value| {
            message["target"]["src_path"]
                .as_str()
                .map(|path| crate::workspace::normalize(Path::new(path)))
        };
        match message["reason"].as_str() {
            Some("compiler-artifact") => {
                if message["profile"]["test"].as_bool() == Some(true) {
                    if let (Some(source), Some(executable)) =
                        (source(message), message["executable"].as_str())
                    {
                        self.executables.insert(source, PathBuf::from(executable));
                    }
                }
            }
            Some("compiler-message") => {
                if message["message"]["level"].as_str() == Some("error") {
                    let rendered = message["message"]["rendered"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let dir = message["package_id"].as_str().and_then(package_dir);
                    if let Some(source) = source(message) {
                        self.errors
                            .entry(source)
                            .or_default()
                            .push(rendered.clone());
                    }
                    self.all_errors.push((dir, rendered));
                }
            }
            Some("build-script-executed") => {
                if let (Some(dir), Some(out_dir)) = (
                    message["package_id"].as_str().and_then(package_dir),
                    message["out_dir"].as_str(),
                ) {
                    self.out_dirs.insert(dir, PathBuf::from(out_dir));
                }
                for path in message["linked_paths"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    let path = path.split_once('=').map(|(_, path)| path).unwrap_or(path);
                    let path = PathBuf::from(path);
                    if !self.linked_paths.contains(&path) {
                        self.linked_paths.push(path);
                    }
                }
            }
            _ => {}
        }
    }

    /// Why `target` of `package` has no test binary.
    fn failure(&self, package: &Package, target: &Target) -> String {
        let source = crate::workspace::normalize(&target.src_path);
        if let Some(errors) = self.errors.get(&source) {
            return errors.concat();
        }
        let own: Vec<&str> = self
            .all_errors
            .iter()
            .filter(|(dir, _)| dir.as_deref() == Some(package.dir.as_path()))
            .map(|(_, error)| error.as_str())
            .collect();
        if !own.is_empty() {
            return own.concat();
        }
        if !self.all_errors.is_empty() {
            let errors: Vec<&str> = self
                .all_errors
                .iter()
                .map(|(_, error)| error.as_str())
                .collect();
            return format!("a crate it depends on did not build:\n{}", errors.concat());
        }
        if self.timed_out {
            return "the build did not finish before the run was stopped".to_string();
        }
        if self.stderr.trim().is_empty() {
            return "cargo built no test binary for it".to_string();
        }
        self.stderr.clone()
    }
}

/// A test that ended, or never did.
struct Ended {
    name: String,
    outcome: &'static str,
    duration: Duration,
    output: Option<String>,
}

/// What one test binary reported.
#[derive(Default)]
struct BinaryRun {
    tests: Vec<Ended>,
    running: Vec<(String, Instant)>,
    suite_ended: bool,
    suite_failed: bool,
    other: Option<Tail>,
}

impl BinaryRun {
    /// One line of libtest's JSON events; returns the test that ended.
    fn line(&mut self, line: &str) -> bool {
        let event = if line.starts_with('{') {
            serde_json::from_str::<Value>(line).ok()
        } else {
            None
        };
        let Some(event) = event else {
            let other = self.other.get_or_insert_with(|| Tail::new(MAX_OUTPUT));
            other.push_str(line);
            other.push_str("\n");
            return false;
        };
        let name = event["name"].as_str().unwrap_or_default().to_string();
        match (event["type"].as_str(), event["event"].as_str()) {
            (Some("suite"), Some("ok")) => self.suite_ended = true,
            (Some("suite"), Some("failed")) => {
                self.suite_ended = true;
                self.suite_failed = true;
            }
            (Some("test"), Some("started")) => self.running.push((name, Instant::now())),
            (Some("test"), Some(result @ ("ok" | "failed" | "ignored"))) => {
                let started = self
                    .running
                    .iter()
                    .position(|(running, _)| *running == name)
                    .map(|position| self.running.remove(position).1);
                let duration = event["exec_time"]
                    .as_f64()
                    .map(Duration::from_secs_f64)
                    .or_else(|| started.map(|started| started.elapsed()))
                    .unwrap_or_default();
                let outcome = match result {
                    "ok" => "pass",
                    "failed" => "fail",
                    _ => "skip",
                };
                let output = (outcome == "fail").then(|| {
                    let mut output = event["stdout"].as_str().unwrap_or_default().to_string();
                    if let Some(message) = event["message"].as_str() {
                        output.push_str(message);
                        output.push('\n');
                    }
                    process::keep_end(&output, MAX_OUTPUT)
                });
                self.tests.push(Ended {
                    name,
                    outcome,
                    duration,
                    output,
                });
                return true;
            }
            _ => {}
        }
        false
    }
}

struct Session {
    deadline: Instant,
    run: TestRun,
    progress_file: ProgressFile,
    progress: Progress,
    stopped: bool,
}

impl Session {
    fn diag(&mut self, message: String) {
        self.run.diagnostics.push(message);
    }

    fn report(&mut self, force: bool) {
        self.progress.packages_total = self
            .progress
            .packages_total
            .max(self.progress.packages_done);
        let progress = self.progress.clone();
        self.progress_file.update(&progress, force);
    }
}

/// The time left to the deadline, minus a margin to report in.
pub fn stop_at(started: Instant, timeout: Duration) -> Instant {
    let margin = (timeout / 10).min(Duration::from_secs(5));
    started + timeout.saturating_sub(margin)
}

pub fn run(dir: &Path, options: &Options) -> Result<TestRun, String> {
    let started = Instant::now();
    let deadline = stop_at(started, options.timeout);
    let parsed = Parsed::read(dir, deadline).map_err(|error| error.to_string())?;
    if crate::workspace::manifest_dirs(&parsed.tree.root).is_empty() {
        return Err(format!(
            "{} has no Cargo.toml (nor any folder one level down)",
            parsed.tree.root.display()
        ));
    }
    if !process::on_path("cargo") {
        return Err("cargo was not found on the PATH".to_string());
    }
    let index = parsed.index();
    let mut session = Session {
        deadline,
        run: TestRun::new(),
        progress_file: ProgressFile {
            path: options.progress.clone(),
            written: None,
        },
        progress: Progress {
            protocol: protocol::PROTOCOL,
            ..Progress::default()
        },
        stopped: false,
    };
    session.diag(if options.race {
        "the race detector was off: cargo test has none for Rust".to_string()
    } else {
        "the race detector was off: --no-race was given".to_string()
    });
    if !options.cover {
        session.diag("coverage was not measured: --no-cover was given".to_string());
    }
    let plans = plan(&parsed, options.selection.as_ref(), &mut session);
    session.progress.packages_total = u32::try_from(plans.len()).unwrap_or(u32::MAX);
    session.report(true);

    let mut cover_tools = None;
    if options.cover && !plans.is_empty() {
        match coverage::tools(&parsed.tree.root, deadline) {
            Ok(tools) => cover_tools = Some(tools),
            Err(why) => session.diag(format!("coverage was not measured: {why}")),
        }
    }
    // The target folder kept in --cache-dir, held until the run ends.
    let mut kept = None;
    if let (Some(cache_dir), false) = (&options.cache_dir, plans.is_empty()) {
        match cache::acquire(cache_dir, CACHE_FOLDER, deadline) {
            Ok(held) => kept = Some(held),
            Err(why) => session.diag(format!(
                "the build output of earlier runs was not used: {why}"
            )),
        }
    }
    let mut blocks = Vec::new();
    let mut reports = 0;
    let mut nothing_built = Vec::new();
    for (unit_index, unit) in parsed.workspace.units.iter().enumerate() {
        let planned: Vec<&Planned> = plans
            .iter()
            .filter(|planned| planned.unit == unit_index)
            .collect();
        if planned.is_empty() {
            continue;
        }
        let rel = parsed
            .tree
            .rel(&unit.root)
            .unwrap_or_else(|| ".".to_string());
        if session.stopped || Instant::now() >= deadline {
            for planned in &planned {
                session.diag(format!(
                    "the tests of {} were not run: the time ran out",
                    planned.root
                ));
            }
            session.stopped = true;
            continue;
        }
        // Right before each build, so that no build of another copy comes
        // between (see `cache`).
        if kept.is_some() {
            if let Err(error) = cache::touch_tree(&parsed.tree.root) {
                session.diag(format!(
                    "the build output of earlier runs was not used: the tree could not be set modified now: {error}"
                ));
                kept = None;
            }
        }
        let target_dir = match &kept {
            Some(held) => held.dir.clone(),
            None => unit.root.join("target"),
        };
        let cover_env = match &cover_tools {
            Some(tools) => match coverage::show_env(&unit.root, &target_dir, tools, deadline)
                .and_then(|env| {
                    coverage::clean_profiles(&target_dir)
                        .map(|()| env)
                        .map_err(|error| {
                            format!("the profiles of an earlier run could not be removed: {error}")
                        })
                }) {
                Ok(env) => Some(env),
                Err(why) => {
                    session.diag(format!("coverage of {rel} was not measured: {why}"));
                    None
                }
            },
            None => None,
        };
        let built = build(
            unit,
            &planned,
            &target_dir,
            cover_env.as_deref(),
            options.offline,
            deadline,
        );
        if built.executables.is_empty() && built.all_errors.is_empty() && !built.timed_out {
            nothing_built.push(format!(
                "cargo could not build the tests of {rel}:\n{}",
                process::keep_end(built.stderr.trim(), 4 << 10)
            ));
        }
        if built.timed_out {
            session.stopped = true;
        }
        let mut ran = false;
        for planned in planned {
            ran |= run_target(
                &mut session,
                &index,
                planned,
                &built,
                &target_dir,
                cover_env.as_deref(),
            );
        }
        if let (Some(tools), Some(env)) = (&cover_tools, &cover_env) {
            if ran && !session.stopped {
                let output = lcov_output();
                match coverage::report(&unit.root, &target_dir, tools, env, &output, deadline) {
                    Ok(text) => {
                        reports += 1;
                        blocks.extend(coverage_blocks(&parsed, &index, &text));
                    }
                    Err(why) => session.diag(format!("coverage of {rel} was not measured: {why}")),
                }
                let _ = std::fs::remove_file(&output);
            }
        }
    }
    if session.run.tests.is_empty()
        && session
            .run
            .packages
            .iter()
            .all(|package| package.outcome == "build_failed")
        && !nothing_built.is_empty()
    {
        return Err(nothing_built.join("\n"));
    }
    if session.stopped {
        let timeout = options.timeout;
        session.diag(format!("the run was stopped after {timeout:?}: results are partial and coverage was not measured"));
    } else if cover_tools.is_some() {
        if reports > 0 {
            session.run.coverage = Some(Coverage { blocks });
        } else {
            session.diag("coverage was not measured: no test binary ran".to_string());
        }
    }
    session.progress.current = None;
    session.report(true);
    Ok(session.run)
}

/// A file of its own for one LCOV report, unique within the process too:
/// runs at the same time (the unit tests) must not read or remove each
/// other's.
fn lcov_output() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("difflens-lcov-{}-{next}.info", std::process::id()))
}

/// The targets to run: every test target, or the selection's.
fn plan<'p>(
    parsed: &'p Parsed,
    selection: Option<&Selection>,
    session: &mut Session,
) -> Vec<Planned<'p>> {
    let selected: Option<HashMap<&str, &protocol::SelectedPackage>> = selection.map(|selection| {
        selection
            .packages
            .iter()
            .map(|package| (package.package.as_str(), package))
            .collect()
    });
    let mut found = HashSet::new();
    let mut plans = Vec::new();
    for spec in &parsed.specs {
        let package = &parsed.workspace.units[spec.unit].packages[spec.package];
        let target = &package.targets[spec.target];
        if !target.test || target.kind == TargetKind::Build || target.required_features {
            continue;
        }
        let filter = match &selected {
            None => None,
            Some(selected) => {
                let Some(choice) = selected.get(spec.root.as_str()) else {
                    continue;
                };
                found.insert(spec.root.clone());
                match &choice.tests {
                    None => None,
                    Some(tests) => {
                        let mut names: Vec<String> = Vec::new();
                        for test in tests {
                            // Rust tests have no subtests: one selected with
                            // some runs whole, with an empty list not at all.
                            if test.subtests.as_ref().is_some_and(Vec::is_empty)
                                || names.contains(&test.name)
                            {
                                continue;
                            }
                            names.push(test.name.clone());
                        }
                        if names.is_empty() {
                            continue;
                        }
                        Some(names)
                    }
                }
            }
        };
        let manifest = parsed
            .tree
            .rel(&parsed.workspace.units[spec.unit].root)
            .filter(|rel| rel != ".")
            .map(|rel| format!("{rel}/Cargo.toml"));
        plans.push(Planned {
            unit: spec.unit,
            package,
            target,
            root: spec.root.clone(),
            filter,
            command: target_command(manifest.as_deref(), package, target),
        });
    }
    if let Some(selection) = selection {
        for package in &selection.packages {
            if !found.contains(&package.package) {
                session.diag(format!(
                    "the package {} was not found, so its tests were not run",
                    package.package
                ));
            }
        }
    }
    plans
}

fn cargo_env(command: &mut Command, target_dir: &Path, cover_env: Option<&[(String, String)]>) {
    command
        .env("CARGO_TARGET_DIR", target_dir)
        .env("CARGO_TERM_COLOR", "never")
        .env("CARGO_INCREMENTAL", "0");
    if let Some(env) = cover_env {
        command.envs(env.iter().map(|(key, value)| (key, value)));
    }
}

/// Builds the test binaries of the planned targets of `unit`.
fn build(
    unit: &Unit,
    planned: &[&Planned],
    target_dir: &Path,
    cover_env: Option<&[(String, String)]>,
    offline: bool,
    deadline: Instant,
) -> Built {
    let mut command = Command::new("cargo");
    command.current_dir(&unit.root).args([
        "build",
        "--tests",
        "--keep-going",
        "--message-format=json",
    ]);
    let packages: Vec<&str> = {
        let mut names: Vec<&str> = planned
            .iter()
            .map(|planned| planned.package.name.as_str())
            .collect();
        names.dedup();
        names
    };
    if packages.len() == unit.packages.len() {
        command.arg("--workspace");
    } else {
        for name in packages {
            command.args(["-p", name]);
        }
    }
    if offline {
        command.arg("--offline");
    }
    cargo_env(&mut command, target_dir, cover_env);
    let mut built = Built::default();
    match process::stream(command, deadline, |line| {
        if let Ok(message) = serde_json::from_str::<Value>(line) {
            built.message(&message);
        }
    }) {
        Ok(finished) => {
            built.timed_out = finished.timed_out;
            built.stderr = finished.stderr;
        }
        Err(error) => built.stderr = format!("cargo could not run: {error}"),
    }
    built
}

/// Runs one test target; reports whether its binary ran.
fn run_target(
    session: &mut Session,
    index: &Index,
    planned: &Planned,
    built: &Built,
    target_dir: &Path,
    cover_env: Option<&[(String, String)]>,
) -> bool {
    let root = planned.root.clone();
    if session.stopped || Instant::now() >= session.deadline {
        session.diag(format!(
            "the tests of {root} were not run: the time ran out"
        ));
        session.stopped = true;
        return false;
    }
    let source = crate::workspace::normalize(&planned.target.src_path);
    let Some(executable) = built.executables.get(&source) else {
        let output = process::keep_end(&built.failure(planned.package, planned.target), MAX_OUTPUT);
        session.run.packages.push(PackageResult {
            container: root,
            outcome: "build_failed".to_string(),
            duration_ms: 0,
            output: Some(output),
            command: Some(planned.command.clone()),
        });
        session.progress.packages_done += 1;
        session.report(true);
        return false;
    };
    session.progress.current = Some(root.clone());
    session.report(true);
    let mut command = Command::new(executable);
    command.current_dir(&planned.package.dir);
    test_env(&mut command, planned, built, target_dir, cover_env);
    if planned.target.harness {
        command.args([
            "-Z",
            "unstable-options",
            "--format",
            "json",
            "--report-time",
        ]);
        if let Some(names) = &planned.filter {
            command.arg("--exact").args(names);
        }
    }
    let started = Instant::now();
    let mut binary = BinaryRun::default();
    let deadline = session.deadline;
    let mut progress = std::mem::take(&mut session.progress);
    let progress_file = &mut session.progress_file;
    let result = process::stream(command, deadline, |line| {
        if binary.line(line) {
            if let Some(ended) = binary.tests.last() {
                progress.tests += 1;
                if ended.outcome == "fail" {
                    progress.failed += 1;
                }
            }
            progress_file.update(&progress, false);
        }
    });
    session.progress = progress;
    let elapsed = started.elapsed();
    let finished = match result {
        Ok(finished) => finished,
        Err(error) => {
            session.run.packages.push(PackageResult {
                container: root.clone(),
                outcome: "fail".to_string(),
                duration_ms: 0,
                output: Some(format!("the test binary could not be started: {error}")),
                command: Some(planned.command.clone()),
            });
            session.progress.packages_done += 1;
            session.report(true);
            return false;
        }
    };
    let mut other = binary
        .other
        .take()
        .map(|tail| tail.into_string())
        .unwrap_or_default();
    if !finished.stderr.is_empty() {
        other.push_str(&finished.stderr);
    }
    // Tests still running when the binary stopped never finished.
    let why = if finished.timed_out {
        "the test did not finish before the run was stopped".to_string()
    } else {
        format!(
            "the test binary stopped while it ran ({})",
            finished.describe()
        )
    };
    for (name, started) in std::mem::take(&mut binary.running) {
        binary.tests.push(Ended {
            name,
            outcome: "fail",
            duration: started.elapsed(),
            output: Some(process::keep_end(&format!("{why}\n{other}"), MAX_OUTPUT)),
        });
        session.progress.tests += 1;
        session.progress.failed += 1;
    }
    let failed_tests = binary.tests.iter().any(|test| test.outcome == "fail");
    let (outcome, output) = if finished.timed_out {
        session.stopped = true;
        (
            "fail",
            Some(format!(
                "the package did not finish before the run was stopped\n{other}"
            )),
        )
    } else if !planned.target.harness {
        if finished.success() {
            ("pass", None)
        } else {
            (
                "fail",
                Some(format!(
                    "the test binary failed ({})\n{other}",
                    finished.describe()
                )),
            )
        }
    } else if !binary.suite_ended {
        (
            "fail",
            Some(format!(
                "the test binary stopped before it reported its tests ({})\n{other}",
                finished.describe()
            )),
        )
    } else if binary.suite_failed || failed_tests {
        ("fail", (!failed_tests).then_some(other))
    } else if !finished.success() {
        (
            "fail",
            Some(format!(
                "the test binary failed after its tests ({})\n{other}",
                finished.describe()
            )),
        )
    } else if binary.tests.is_empty() {
        ("no_tests", None)
    } else {
        ("pass", None)
    };
    for ended in binary.tests {
        let command = test_command(planned, &ended.name);
        session
            .run
            .tests
            .push(test_result(index, &root, ended, command));
    }
    session.run.packages.push(PackageResult {
        container: root,
        outcome: outcome.to_string(),
        duration_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        output: output.map(|output| process::keep_end(&output, MAX_OUTPUT)),
        command: Some(planned.command.clone()),
    });
    session.progress.packages_done += 1;
    session.progress.current = None;
    session.report(true);
    true
}

/// The environment `cargo test` runs a test binary with.
fn test_env(
    command: &mut Command,
    planned: &Planned,
    built: &Built,
    target_dir: &Path,
    cover_env: Option<&[(String, String)]>,
) {
    let package = planned.package;
    let mut parts = package.version.splitn(3, '.');
    let (major, minor) = (parts.next().unwrap_or("0"), parts.next().unwrap_or("0"));
    let rest = parts.next().unwrap_or("0");
    let (patch, pre) = rest.split_once('-').unwrap_or((rest, ""));
    command
        .env("CARGO_MANIFEST_DIR", &package.dir)
        .env("CARGO_MANIFEST_PATH", &package.manifest_path)
        .env("CARGO_PKG_NAME", &package.name)
        .env("CARGO_PKG_VERSION", &package.version)
        .env("CARGO_PKG_VERSION_MAJOR", major)
        .env("CARGO_PKG_VERSION_MINOR", minor)
        .env(
            "CARGO_PKG_VERSION_PATCH",
            patch.split('+').next().unwrap_or(patch),
        )
        .env("CARGO_PKG_VERSION_PRE", pre)
        .env("CARGO_CRATE_NAME", &planned.target.crate_name);
    if let Some(cargo) = find_program("cargo") {
        command.env("CARGO", cargo);
    }
    if let Some(out_dir) = built.out_dirs.get(&package.dir) {
        command.env("OUT_DIR", out_dir);
    }
    if planned.target.harness {
        // libtest's JSON events are unstable options; this lets the stable
        // toolchain's libtest accept them, for this process only.
        command.env("RUSTC_BOOTSTRAP", "1");
    }
    let variable = if cfg!(target_os = "macos") {
        "DYLD_FALLBACK_LIBRARY_PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    let mut paths: Vec<PathBuf> = vec![
        target_dir.join("debug").join("deps"),
        target_dir.join("debug"),
    ];
    paths.extend(built.linked_paths.iter().cloned());
    if let Some(existing) = std::env::var_os(variable) {
        paths.extend(std::env::split_paths(&existing));
    } else if cfg!(target_os = "macos") {
        if let Some(home) = std::env::var_os("HOME") {
            paths.push(PathBuf::from(home).join("lib"));
        }
        paths.push(PathBuf::from("/usr/local/lib"));
        paths.push(PathBuf::from("/usr/lib"));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        command.env(variable, joined);
    }
    if let Some(env) = cover_env {
        command.envs(env.iter().map(|(key, value)| (key, value)));
    }
}

fn find_program(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// A test of the protocol: placed on the function analyze lists for it.
/// Names a macro generated under the test (rstest's `case_1`) are placed on
/// the closest function above them. Rust tests have no subtests: the
/// hierarchy is the name alone, whatever it holds.
fn test_result(index: &Index, root: &str, ended: Ended, command: String) -> Test {
    let mut candidate = format!("{root}::{}", ended.name);
    let function = loop {
        if let Some(function) = index.function(&candidate).filter(|function| function.test) {
            break Some(function);
        }
        match candidate.rfind("::") {
            Some(cut) if cut > root.len() => candidate.truncate(cut),
            _ => break None,
        }
    };
    let file = function
        .and_then(|function| index.files.list.get(function.file))
        .map(|file| file.rel.clone());
    Test {
        container: function
            .map(|function| function.container.clone())
            .unwrap_or_else(|| root.to_string()),
        package: root.to_string(),
        hierarchy: vec![ended.name.clone()],
        name: ended.name,
        function: function.map(|function| function.id.clone()),
        file,
        line: function.map(|function| function.start),
        outcome: ended.outcome.to_string(),
        duration_ms: u64::try_from(ended.duration.as_millis()).unwrap_or(u64::MAX),
        output: ended.output,
        command: Some(command),
    }
}

/// The blocks of an LCOV report that cover the project's own code: files of
/// the tree, not generated, without test code.
fn coverage_blocks(parsed: &Parsed, index: &Index, report: &str) -> Vec<protocol::CoverageBlock> {
    let by_rel: HashMap<&str, usize> = parsed
        .files
        .list
        .iter()
        .enumerate()
        .map(|(position, file)| (file.rel.as_str(), position))
        .collect();
    let mut out = Vec::new();
    let mut merged: BTreeMap<String, BTreeMap<u32, u64>> = BTreeMap::new();
    for (path, lines) in coverage::parse_lcov(report) {
        let Some(rel) = parsed.tree.rel(&path) else {
            continue;
        };
        let file = by_rel.get(rel.as_str()).copied();
        if file.is_some_and(|file| parsed.files.list[file].generated) {
            continue;
        }
        let spans: &[(u32, u32)] = file
            .and_then(|file| index.test_spans.get(&file))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let entry = merged.entry(rel).or_default();
        for (line, count) in lines {
            if spans
                .iter()
                .any(|(start, end)| *start <= line && line <= *end)
            {
                continue;
            }
            *entry.entry(line).or_insert(0) += count;
        }
    }
    for (rel, lines) in merged {
        out.extend(coverage::blocks(&rel, &lines));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn options(folder: &TempDir) -> Options {
        Options {
            timeout: Duration::from_secs(300),
            offline: true,
            race: true,
            cover: true,
            selection: None,
            progress: Some(folder.0.join("progress.json")),
            cache_dir: None,
        }
    }

    fn commands(run: &TestRun) -> Vec<(&str, &str)> {
        run.packages
            .iter()
            .map(|package| {
                (
                    package.container.as_str(),
                    package.command.as_deref().unwrap(),
                )
            })
            .collect()
    }

    fn outcomes(run: &TestRun) -> Vec<(String, String)> {
        run.packages
            .iter()
            .map(|package| (package.container.clone(), package.outcome.clone()))
            .collect()
    }

    #[test]
    fn a_run_reports_every_test_with_its_function_and_coverage() {
        let (folder, tree) = TempDir::with_fixture("failing");
        let run = run(&tree, &options(&folder)).unwrap();
        assert_eq!(
            outcomes(&run),
            [
                ("failing".to_string(), "fail".to_string()),
                ("failing/tests/api".to_string(), "pass".to_string()),
                ("failing/tests/custom".to_string(), "pass".to_string()),
            ]
        );
        let test = |name: &str| run.tests.iter().find(|test| test.name == name).unwrap();
        let failed = test("tests::halves_wrongly");
        assert_eq!(failed.outcome, "fail");
        assert_eq!(
            failed.function.as_deref(),
            Some("failing::tests::halves_wrongly")
        );
        assert_eq!(
            (failed.container.as_str(), failed.package.as_str()),
            ("failing::tests", "failing")
        );
        assert_eq!(failed.file.as_deref(), Some("src/lib.rs"));
        assert!(failed
            .output
            .as_deref()
            .unwrap()
            .contains("halve rounds up"));
        assert_eq!(test("tests::ignored").outcome, "skip");
        assert_eq!(test("tests::adds").output, None);
        let integration = test("adds_through_the_api");
        assert_eq!(
            integration.function.as_deref(),
            Some("failing/tests/api::adds_through_the_api")
        );
        for test in &run.tests {
            assert_eq!(
                (test.hierarchy.len(), test.hierarchy.join("/")),
                (1, test.name.clone())
            );
        }
        assert_eq!(
            failed.command.as_deref(),
            Some("cargo test -p failing --lib -- tests::halves_wrongly --exact")
        );
        assert_eq!(
            integration.command.as_deref(),
            Some("cargo test -p failing --test api -- adds_through_the_api --exact")
        );
        assert_eq!(
            commands(&run),
            [
                ("failing", "cargo test -p failing --lib"),
                ("failing/tests/api", "cargo test -p failing --test api"),
                // harness = false: no name filter, the target runs whole.
                (
                    "failing/tests/custom",
                    "cargo test -p failing --test custom"
                ),
            ]
        );
        let progress: Value =
            serde_json::from_slice(&std::fs::read(folder.0.join("progress.json")).unwrap())
                .unwrap();
        assert_eq!(progress["packagesDone"], 3);
        assert_eq!(progress["tests"], 5);
        assert_eq!(progress["failed"], 2);
        // Coverage of the library, not of its tests.
        if let Some(coverage) = &run.coverage {
            assert!(coverage
                .blocks
                .iter()
                .all(|block| block.file == "src/lib.rs" && block.end_line < 16));
            assert!(coverage.blocks.iter().any(|block| block.count == 0));
        } else {
            assert!(
                run.diagnostics
                    .iter()
                    .any(|message| message.starts_with("coverage was not measured")),
                "{:?}",
                run.diagnostics
            );
        }
    }

    #[test]
    fn targets_that_do_not_build_are_build_failures_and_the_others_run() {
        let (folder, tree) = TempDir::with_fixture("broken");
        let mut options = options(&folder);
        options.cover = false;
        let run = run(&tree, &options).unwrap();
        assert_eq!(
            outcomes(&run),
            [
                ("good".to_string(), "pass".to_string()),
                ("bad".to_string(), "build_failed".to_string()),
                (
                    "bad/tests/unparsable".to_string(),
                    "build_failed".to_string()
                ),
            ]
        );
        assert!(run.packages[1]
            .output
            .as_deref()
            .unwrap()
            .contains("mismatched types"));
        assert!(run
            .diagnostics
            .contains(&"coverage was not measured: --no-cover was given".to_string()));
        assert!(run.coverage.is_none());
    }

    #[test]
    fn a_selection_runs_only_what_it_names() {
        let (folder, tree) = TempDir::with_fixture("workspace");
        let mut options = options(&folder);
        options.cover = false;
        options.race = false;
        options.selection = Some(
            serde_json::from_value(serde_json::json!({ "packages": [
                { "package": "ledger_core", "tests": [
                    { "name": "tests::balances", "subtests": null },
                    { "name": "tests::ignores_zero", "subtests": [] }
                ]},
                { "package": "ledger_core/tests/flow", "tests": null },
                { "package": "missing", "tests": null }
            ]}))
            .unwrap(),
        );
        let run = run(&tree, &options).unwrap();
        assert_eq!(
            outcomes(&run),
            [
                ("ledger_core".to_string(), "pass".to_string()),
                ("ledger_core/tests/flow".to_string(), "pass".to_string())
            ]
        );
        let names: Vec<&str> = run.tests.iter().map(|test| test.name.as_str()).collect();
        assert_eq!(names, ["tests::balances", "a_day_of_entries"]);
        assert_eq!(
            run.tests[0].command.as_deref(),
            Some("cargo test -p ledger-core --lib -- tests::balances --exact")
        );
        assert_eq!(
            commands(&run),
            [
                ("ledger_core", "cargo test -p ledger-core --lib"),
                (
                    "ledger_core/tests/flow",
                    "cargo test -p ledger-core --test flow"
                )
            ]
        );
        assert!(run
            .diagnostics
            .contains(&"the package missing was not found, so its tests were not run".to_string()));
        assert!(run
            .diagnostics
            .contains(&"the race detector was off: --no-race was given".to_string()));
    }

    #[test]
    fn prepare_fetches_and_opens_no_folder() {
        let (_folder, tree) = TempDir::with_fixture("workspace");
        assert_eq!(
            prepare(&tree, Instant::now() + Duration::from_secs(120)).unwrap(),
            Vec::<String>::new()
        );
        let empty = TempDir::new();
        assert!(prepare(&empty.0, Instant::now() + Duration::from_secs(10))
            .unwrap_err()
            .contains("no Cargo.toml"));
    }

    #[test]
    fn libtest_events_become_tests() {
        let mut run = BinaryRun::default();
        for line in [
            r#"{ "type": "suite", "event": "started", "test_count": 3 }"#,
            r#"{ "type": "test", "event": "started", "name": "tests::adds" }"#,
            r#"{ "type": "test", "event": "started", "name": "tests::fails" }"#,
            r#"{ "type": "test", "event": "started", "name": "tests::hangs" }"#,
            r#"{ "type": "test", "name": "tests::adds", "event": "ok", "exec_time": 0.002 }"#,
            r#"{ "type": "test", "name": "tests::fails", "event": "failed", "exec_time": 0.01, "stdout": "boom\n" }"#,
            "not an event",
        ] {
            run.line(line);
        }
        assert_eq!(run.tests.len(), 2);
        assert_eq!(
            (run.tests[0].outcome, run.tests[0].duration.as_millis()),
            ("pass", 2)
        );
        assert_eq!(run.tests[1].output.as_deref(), Some("boom\n"));
        assert_eq!(run.running.len(), 1);
        assert!(!run.suite_ended);
        assert_eq!(run.other.unwrap().into_string(), "not an event\n");
    }

    #[test]
    fn package_ids_give_their_folder() {
        assert_eq!(
            package_dir("path+file:///w/my%20crate#0.1.0"),
            Some(PathBuf::from("/w/my crate"))
        );
        assert_eq!(
            package_dir("good 0.1.0 (path+file:///w/good)"),
            Some(PathBuf::from("/w/good"))
        );
        assert_eq!(
            package_dir("registry+https://github.com/rust-lang/crates.io-index#itoa@1.0.0"),
            None
        );
    }

    #[test]
    fn a_kept_target_folder_runs_this_tree_and_counts_only_this_run() {
        let folder = TempDir::new();
        let cache_dir = folder.0.join("cache");
        let (first, second) = (folder.0.join("first"), folder.0.join("second"));
        crate::testing::write_crate(
            &first,
            "kept",
            &[(
                "src/lib.rs",
                "pub fn answer() -> u32 {\n    1\n}\n\n#[test]\nfn first() {\n    assert_eq!(answer(), 1);\n}\n",
            )],
        );
        // The same crate at another path, other code, older mtimes: Cargo
        // would take the first copy's build as fresh for it.
        crate::testing::write_crate(
            &second,
            "kept",
            &[(
                "src/lib.rs",
                "pub fn answer() -> u32 {\n    2\n}\n\npub fn unused() -> u32 {\n    3\n}\n\n#[test]\nfn second() {\n    assert_eq!(answer(), 2);\n}\n",
            )],
        );
        crate::testing::age_tree(&second);
        let mut options = options(&folder);
        options.cache_dir = Some(cache_dir.clone());
        let ran = |run: &TestRun| -> Vec<(String, String)> {
            run.tests
                .iter()
                .map(|test| (test.name.clone(), test.outcome.clone()))
                .collect()
        };
        let run_first = run(&first, &options).unwrap();
        assert_eq!(ran(&run_first), [("first".to_string(), "pass".to_string())]);
        let target = cache_dir.join(CACHE_FOLDER);
        assert!(target.join("debug").is_dir() && !first.join("target").exists());
        // Profiles of an earlier run: unreadable, so a report that merged
        // them would fail.
        std::fs::write(target.join("kept-1-1.profraw"), "not a profile").unwrap();
        std::fs::write(target.join("kept.profdata"), "not a profile").unwrap();
        let run_second = run(&second, &options).unwrap();
        assert_eq!(
            ran(&run_second),
            [("second".to_string(), "pass".to_string())],
            "the second copy's own code ran"
        );
        assert_eq!(
            outcomes(&run_second),
            [("kept".to_string(), "pass".to_string())]
        );
        assert!(!second.join("target").exists());
        let Some(coverage) = &run_second.coverage else {
            assert!(
                coverage::tools(&second, Instant::now() + Duration::from_secs(60)).is_err(),
                "coverage could be measured: {:?}",
                run_second.diagnostics
            );
            return; // no cargo-llvm-cov or llvm-tools on this machine
        };
        assert!(!target.join("kept-1-1.profraw").exists());
        let count = |line: u32| -> Option<u64> {
            coverage
                .blocks
                .iter()
                .find(|block| block.start_line <= line && line <= block.end_line)
                .map(|block| block.count)
        };
        assert!(count(2).is_some_and(|count| count > 0), "{coverage:?}");
        assert_eq!(count(6), Some(0), "{coverage:?}");
    }

    #[test]
    fn commands_run_one_target_or_one_test_from_the_root() {
        let folder = TempDir::new();
        let tree = folder.0.join("tree");
        crate::testing::write_crate(
            &tree.join("inner"),
            "nested-pkg",
            &[
                ("src/lib.rs", "#[test]\nfn adds() {}\n"),
                ("src/main.rs", "fn main() {}\n"),
                ("src/bin/tool.rs", "fn main() {}\n"),
                ("tests/api.rs", "#[test]\nfn works() {}\n"),
            ],
        );
        let parsed = Parsed::read(&tree, Instant::now() + Duration::from_secs(60)).unwrap();
        let mut session = Session {
            deadline: Instant::now(),
            run: TestRun::new(),
            progress_file: ProgressFile {
                path: None,
                written: None,
            },
            progress: Progress::default(),
            stopped: false,
        };
        let plans = plan(&parsed, None, &mut session);
        let found: Vec<(&str, &str)> = plans
            .iter()
            .map(|planned| (planned.root.as_str(), planned.command.as_str()))
            .collect();
        let base = "cargo test --manifest-path inner/Cargo.toml -p nested-pkg";
        assert_eq!(
            found,
            [
                ("nested_pkg", format!("{base} --lib").as_str()),
                (
                    "nested_pkg/bin/nested-pkg",
                    format!("{base} --bin nested-pkg").as_str()
                ),
                ("nested_pkg/bin/tool", format!("{base} --bin tool").as_str()),
                (
                    "nested_pkg/tests/api",
                    format!("{base} --test api").as_str()
                ),
            ]
        );
        assert_eq!(
            test_command(&plans[0], "tests::adds"),
            format!("{base} --lib -- tests::adds --exact")
        );
        // Names a shell would read otherwise are quoted.
        assert_eq!(
            test_command(&plans[3], "src/lib.rs - add (line 5)"),
            format!("{base} --test api -- 'src/lib.rs - add (line 5)' --exact")
        );
        assert_eq!(
            test_command(&plans[3], "it's"),
            format!("{base} --test api -- 'it'\\''s' --exact")
        );
        let mut custom = plans[3].target.clone();
        custom.harness = false;
        let package = plans[3].package;
        let unharnessed = Planned {
            unit: 0,
            package,
            target: &custom,
            root: plans[3].root.clone(),
            filter: None,
            command: target_command(None, package, &custom),
        };
        assert_eq!(
            test_command(&unharnessed, "works"),
            "cargo test -p nested-pkg --test api"
        );
        for (kind, word) in [
            (TargetKind::Example, "example"),
            (TargetKind::Bench, "bench"),
        ] {
            let mut target = plans[3].target.clone();
            target.kind = kind;
            target.name = "my demo".to_string();
            assert_eq!(
                target_command(Some("a b/Cargo.toml"), package, &target),
                format!(
                    "cargo test --manifest-path 'a b/Cargo.toml' -p nested-pkg --{word} 'my demo'"
                )
            );
        }
    }

    #[test]
    fn shell_words_are_quoted_only_when_a_shell_would_read_them() {
        for (word, quoted) in [
            ("tests::adds", "tests::adds"),
            ("crates/a-b/Cargo.toml", "crates/a-b/Cargo.toml"),
            ("a b", "'a b'"),
            ("it's", "'it'\\''s'"),
            ("$HOME", "'$HOME'"),
            ("~x", "'~x'"),
            ("=x", "'=x'"),
            ("", "''"),
        ] {
            assert_eq!(shell_word(word), quoted, "{word}");
        }
    }

    #[test]
    fn the_margin_leaves_time_to_report() {
        let now = Instant::now();
        assert_eq!(
            stop_at(now, Duration::from_secs(600)),
            now + Duration::from_secs(595)
        );
        assert_eq!(
            stop_at(now, Duration::from_secs(10)),
            now + Duration::from_secs(9)
        );
    }
}
