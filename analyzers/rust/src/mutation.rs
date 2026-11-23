//! The `mutate` command (mutation.schema.json): mutation testing of the
//! lines a review adds, driving cargo-mutants (`engine`).
//!
//! 1. Candidate lines (rule 1): the lines `--changed` adds that a
//!    `--coverage` block with a count above 0 spans.
//! 2. Listing: for each workspace of the tree with candidate lines, a
//!    unified diff is written whose added lines are exactly those lines
//!    (one hunk `@@ -0,0 +<line>,<n> @@` per run of lines, with their own
//!    text, which cargo-mutants checks against the source), and `cargo
//!    mutants --list --json --workspace --in-diff <diff>` lists the mutants
//!    of the code they touch. Only those whose first character is on a
//!    candidate line are kept (cargo-mutants also lists a function's
//!    whole-body replacements when any line of the body changed), without
//!    generated files and test functions; each belongs to the innermost
//!    function `analyze` lists around its line.
//! 3. Cap (rule 4): round-robin across functions, deterministic.
//! 4. Results kept (rule 6): `<cache-dir>/mutation/results.json`, by a
//!    fingerprint of the function's source, its package's test code (the
//!    `#[cfg(test)]` code of its crates and its test and bench targets),
//!    cargo-mutants' version and the mutant itself.
//! 5. The run, per workspace, of exactly the mutants left (`--re
//!    '^<name>$'` for each, the names `--list` gave). First the packages'
//!    tests are built (`cargo test --no-run --package=<name>@<version>`):
//!    into `<cache-dir>/mutation/target` with --cache-dir (locked, the tree
//!    set modified now first, as `cache` explains), then copied to the
//!    workspace's `target/`; without it into that `target/` directly. With
//!    `--copy-target=true` every build folder of cargo-mutants starts from
//!    that copy, so dependencies are built once, not once per job.
//!    cargo-mutants then copies the workspace into folders of its own (in
//!    TMPDIR, here a folder of the run, removed at the end), runs the
//!    baseline (the tests of the mutated packages on the unmutated code)
//!    and each mutant with its package's tests only, `--jobs` at a time,
//!    each stopped after `max(--min-mutant-timeout, ceil(3 x baseline))`
//!    (`--timeout-multiplier 3 --minimum-test-timeout <seconds>`: it
//!    measures the baseline itself). `CARGO_TARGET_DIR` is removed from its
//!    environment, so that no two build folders share a target folder.
//!    `mutants.out/outcomes.json`, which it rewrites after each mutant, is
//!    read for the progress file while it runs and for the results after.
//! 6. Outcomes: CaughtMutant is killed (the first `test … FAILED` line of
//!    its log names the test), MissedMutant survived, Timeout timeout,
//!    Unviable (it does not build) and anything else error. A baseline that
//!    fails makes every mutant of its workspace an error, with a
//!    diagnostic. At the time limit cargo-mutants gets SIGTERM (it stops
//!    its tests, which it runs in process groups of their own), the
//!    mutants without a result are left out, and a diagnostic says how
//!    many.
//!
//! Operators: whole-body replacements (`FnValue`) are return_value, with
//! the body as `original` and the replacement expression as `mutated`;
//! `delete !` is negation_removal; binary operators are read from the
//! change (see `binary_operator`); match arms, guards, struct fields and
//! `delete -` are other. cargo-mutants' spans are 1-based lines and
//! columns in characters, the end exclusive: the protocol's own.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::analyze::Parsed;
use crate::cache;
use crate::engine::{self, Engine};
use crate::index::Index;
use crate::process::{self, Finished};
use crate::protocol::{
    self, Changed, CoverageFile, Identity, Mutant, MutationProgress, MutationRun, MutationSummary,
};
use crate::workspace::Unit;

pub struct Options {
    pub timeout: Duration,
    pub offline: bool,
    /// The core's folder kept between runs: build output and results.
    pub cache_dir: Option<PathBuf>,
    /// Where `prepare --tools` installed cargo-mutants.
    pub tools: Option<PathBuf>,
    pub max_mutants: usize,
    pub jobs: usize,
    pub min_mutant_timeout: Duration,
    pub progress: Option<PathBuf>,
}

/// The default of `--jobs`: half the CPUs, at least 1.
pub fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(|cpus| cpus.get() / 2)
        .unwrap_or(1)
        .max(1)
}

/// The per-mutant time limit is this many times the baseline's tests.
const TIMEOUT_MULTIPLIER: &str = "3";
/// How often outcomes.json is read for the progress file.
const POLL: Duration = Duration::from_millis(300);
/// cargo-mutants gets SIGTERM this long before the deadline.
const INTERRUPT_GRACE: Duration = Duration::from_secs(2);
/// The folder of --cache-dir: results.json and the build output.
const CACHE_FOLDER: &str = "mutation";
const RESULTS_FILE: &str = "results.json";
/// Results kept at most, the most recently used first.
const MAX_KEPT: usize = 50_000;
/// Part of every fingerprint: a change of how results are made forgets
/// the kept ones.
const FINGERPRINT_VERSION: &str = "difflens-rust-mutation-1";

/// A function `analyze` lists, by its lines.
#[derive(Debug, Clone)]
struct FnLines {
    id: String,
    file: String,
    start: u32,
    end: u32,
    test: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
struct Position {
    line: u32,
    column: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
struct Span {
    start: Position,
    end: Position,
}

/// One mutant of `cargo mutants --list --json`, the fields read.
#[derive(Debug, Clone, Deserialize)]
struct Listed {
    name: String,
    /// Relative to the workspace root, with "/".
    file: String,
    #[serde(default)]
    package: String,
    genre: String,
    #[serde(default)]
    replacement: String,
    span: Span,
}

/// A mutant on a candidate line.
#[derive(Debug, Clone)]
struct Candidate {
    unit: usize,
    /// cargo-mutants' name (with file, line and column), for `--re`.
    name: String,
    package: String,
    /// Relative to the tree.
    file: String,
    /// Relative to the workspace root, as cargo-mutants names its logs.
    unit_file: String,
    span: Span,
    original: String,
    mutated: String,
    operator: &'static str,
    description: String,
    function: Option<usize>,
}

/// The result of one mutant.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Tested {
    status: String,
    killed_by: Option<String>,
    duration_ms: u64,
    cached: bool,
    /// Worth keeping: it depends on the code only (not a baseline that
    /// failed, nor a crash of the engine).
    keep: bool,
}

/// outcomes.json, the fields read.
#[derive(Debug, Clone, Default, Deserialize)]
struct Outcomes {
    #[serde(default)]
    outcomes: Vec<Outcome>,
}

#[derive(Debug, Clone, Deserialize)]
struct Outcome {
    /// "Baseline", or {"Mutant": {"name": …}}.
    scenario: Value,
    summary: String,
    #[serde(default)]
    log_path: Option<String>,
    #[serde(default)]
    phase_results: Vec<PhaseResult>,
}

#[derive(Debug, Clone, Deserialize)]
struct PhaseResult {
    phase: String,
    /// Seconds.
    duration: f64,
}

impl Outcome {
    fn mutant_name(&self) -> Option<&str> {
        self.scenario.get("Mutant")?.get("name")?.as_str()
    }

    fn is_baseline(&self) -> bool {
        self.scenario.as_str() == Some("Baseline")
    }

    /// How long its tests ran, or the whole of it when they never did.
    fn duration_ms(&self) -> u64 {
        let seconds = self
            .phase_results
            .iter()
            .find(|phase| phase.phase == "Test")
            .map(|phase| phase.duration)
            .unwrap_or_else(|| self.phase_results.iter().map(|phase| phase.duration).sum());
        (seconds.max(0.0) * 1000.0).round() as u64
    }
}

/// The protocol's status of cargo-mutants' summary of a mutant.
fn status(summary: &str) -> &'static str {
    match summary {
        "CaughtMutant" => "killed",
        "MissedMutant" => "survived",
        "Timeout" => "timeout",
        // Unviable (it does not build), Failure and anything new.
        _ => "error",
    }
}

/// The family of a binary operator replacement (rule 2). Arithmetic
/// operators replaced by other arithmetic operators are arithmetic, the
/// compound assignments too, but `+=` and `-=` into each other are
/// increment; bitwise and shift operators are other.
fn binary_operator(from: &str, to: &str) -> &'static str {
    const ARITHMETIC: &[&str] = &["+", "-", "*", "/", "%"];
    const COMPOUND: &[&str] = &["+=", "-=", "*=", "/=", "%="];
    match (from, to) {
        ("<", "<=") | ("<=", "<") | (">", ">=") | (">=", ">") => "relational_boundary",
        ("==", "!=") | ("!=", "==") | ("<", ">=") | (">=", "<") | (">", "<=") | ("<=", ">") => {
            "relational_negation"
        }
        ("&&", "||") | ("||", "&&") => "logical",
        ("+=", "-=") | ("-=", "+=") => "increment",
        _ if ARITHMETIC.contains(&from) && ARITHMETIC.contains(&to) => "arithmetic",
        _ if COMPOUND.contains(&from) && COMPOUND.contains(&to) => "arithmetic",
        _ => "other",
    }
}

/// The family of a mutant from cargo-mutants' genre and its change.
fn operator(genre: &str, original: &str, mutated: &str) -> &'static str {
    match genre {
        "FnValue" => "return_value",
        "UnaryOperator" if original.trim() == "!" && mutated.trim().is_empty() => {
            "negation_removal"
        }
        "BinaryOperator" => binary_operator(original.trim(), mutated.trim()),
        _ => "other",
    }
}

/// The byte offset of a 1-based (line, column) position of `text`, the
/// column in characters; the column just past a line's last character is
/// its end.
fn byte_offset(text: &str, position: Position) -> Option<usize> {
    let column = usize::try_from(position.column.checked_sub(1)?).ok()?;
    let mut line_start = 0;
    for (number, line) in text.split_inclusive('\n').enumerate() {
        if number + 1 == position.line as usize {
            let content = line.strip_suffix('\n').unwrap_or(line);
            if column == content.chars().count() {
                return Some(line_start + content.len());
            }
            return content
                .char_indices()
                .nth(column)
                .map(|(byte, _)| line_start + byte);
        }
        line_start += line.len();
    }
    None
}

/// The text of `text` between two positions, the end exclusive.
fn span_text(text: &str, span: Span) -> Option<String> {
    let start = byte_offset(text, span.start)?;
    let end = byte_offset(text, span.end)?;
    (start <= end).then(|| text[start..end].to_string())
}

/// The lines `start..=end` of `text`, 1-based.
fn lines_of(text: &str, start: u32, end: u32) -> String {
    text.lines()
        .skip(start.saturating_sub(1) as usize)
        .take(end.saturating_sub(start) as usize + 1)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The candidate lines of each file (rule 1): added, and spanned by a block
/// that ran.
fn candidate_lines(changed: &Changed, coverage: &CoverageFile) -> BTreeMap<String, BTreeSet<u32>> {
    let mut covered: HashMap<&str, Vec<(u32, u32)>> = HashMap::new();
    for block in coverage.blocks.iter().filter(|block| block.count > 0) {
        covered
            .entry(block.file.as_str())
            .or_default()
            .push((block.start_line, block.end_line));
    }
    let mut out: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for file in &changed.files {
        let Some(blocks) = covered.get(file.path.as_str()) else {
            continue;
        };
        for [start, end] in &file.added {
            for (block_start, block_end) in blocks {
                let (from, to) = ((*start).max(*block_start), (*end).min(*block_end));
                if from <= to {
                    out.entry(file.path.clone()).or_default().extend(from..=to);
                }
            }
        }
    }
    out
}

/// A unified diff whose added lines are exactly `lines` of each file (its
/// path relative to the workspace root; each line's number and text, in
/// order): what `--in-diff` reads.
fn synthesize_diff(files: &[(String, Vec<(u32, String)>)]) -> String {
    let mut out = String::new();
    for (path, lines) in files.iter().filter(|(_, lines)| !lines.is_empty()) {
        out.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));
        let mut first = 0;
        while first < lines.len() {
            let mut last = first;
            while last + 1 < lines.len() && lines[last + 1].0 == lines[last].0 + 1 {
                last += 1;
            }
            out.push_str(&format!(
                "@@ -0,0 +{},{} @@\n",
                lines[first].0,
                last - first + 1
            ));
            for (_, text) in &lines[first..=last] {
                out.push('+');
                out.push_str(text);
                out.push('\n');
            }
            first = last + 1;
        }
    }
    out
}

/// `text` as a regular expression matching it exactly (what `regex::escape`
/// escapes).
fn regex_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The name of the first test a cargo test log says failed.
fn first_failed_test(log: &str) -> Option<String> {
    log.lines().find_map(|line| {
        line.trim_end()
            .strip_prefix("test ")?
            .strip_suffix(" ... FAILED")
            .map(str::to_string)
    })
}

/// The file name cargo-mutants gives a mutant's log, without `.log`.
fn log_base(unit_file: &str, position: Position) -> String {
    let file: String = unit_file
        .replace('/', "__")
        .chars()
        .map(|c| match c {
            '\\' | ' ' | ':' | '<' | '>' | '?' | '*' | '|' | '"' => '_',
            c => c,
        })
        .collect();
    format!("{file}_line_{}_col_{}", position.line, position.column)
}

/// A stable 128-bit FNV-1a hash of `parts`, each prefixed by its length,
/// in hex.
fn fingerprint(parts: &[&str]) -> String {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let mut hash = OFFSET;
    for part in parts {
        let length = (part.len() as u64).to_le_bytes();
        for byte in length.iter().chain(part.as_bytes()) {
            hash ^= u128::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
    }
    format!("{hash:032x}")
}

/// The innermost function holding `line` of `file`: the one with the fewest
/// lines; on a tie, the one starting last (a closure after its parent).
fn innermost(functions: &[FnLines], file: &str, line: u32) -> Option<usize> {
    functions
        .iter()
        .enumerate()
        .filter(|(_, function)| {
            function.file == file && function.start <= line && line <= function.end
        })
        .min_by_key(|(position, function)| {
            (
                function.end - function.start,
                std::cmp::Reverse(function.start),
                std::cmp::Reverse(*position),
            )
        })
        .map(|(position, _)| position)
}

/// The workspace a file of the tree belongs to: the one whose root is the
/// longest prefix of it (roots relative to the tree, "." for the tree).
fn unit_of(roots: &[Option<String>], file: &str) -> Option<usize> {
    roots
        .iter()
        .enumerate()
        .filter_map(|(position, root)| {
            let root = root.as_deref()?;
            let inside = root == "." || file.starts_with(&format!("{root}/"));
            inside.then_some((position, if root == "." { 0 } else { root.len() }))
        })
        .max_by_key(|(_, length)| *length)
        .map(|(position, _)| position)
}

/// The mutants rule 4 takes from `groups` (each in source order, the
/// groups in order): round-robin, until `max`. Returned sorted.
fn round_robin(groups: &[Vec<usize>], max: usize) -> Vec<usize> {
    let mut taken = Vec::new();
    let mut depth = 0;
    while taken.len() < max {
        let before = taken.len();
        for group in groups {
            if taken.len() == max {
                break;
            }
            if let Some(index) = group.get(depth) {
                taken.push(*index);
            }
        }
        if taken.len() == before {
            break;
        }
        depth += 1;
    }
    taken.sort_unstable();
    taken
}

/// The groups of rule 4 of `candidates` (sorted in rule 7's order):
/// functions by file then start line, the mutants outside any function as
/// one group per file.
fn groups(candidates: &[Candidate], functions: &[FnLines]) -> Vec<Vec<usize>> {
    let mut by_key: BTreeMap<(String, u32, String), Vec<usize>> = BTreeMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let key = match candidate.function {
            Some(function) => (
                candidate.file.clone(),
                functions[function].start,
                functions[function].id.clone(),
            ),
            None => (candidate.file.clone(), 0, String::new()),
        };
        by_key.entry(key).or_default().push(index);
    }
    by_key.into_values().collect()
}

/// The files of the tree, read once.
struct Texts {
    root: PathBuf,
    files: HashMap<String, Option<String>>,
}

impl Texts {
    fn get(&mut self, rel: &str) -> Option<&str> {
        let root = &self.root;
        self.files
            .entry(rel.to_string())
            .or_insert_with(|| std::fs::read_to_string(root.join(rel)).ok())
            .as_deref()
    }
}

/// The functions `analyze` lists, closures included.
fn functions(index: &Index) -> Vec<FnLines> {
    let graph = crate::walk::walk_all(index);
    index
        .fns
        .iter()
        .chain(graph.closures.iter())
        .filter_map(|definition| {
            Some(FnLines {
                id: definition.id.clone(),
                file: index.files.list.get(definition.file)?.rel.clone(),
                start: definition.start,
                end: definition.end.max(definition.start),
                test: definition.test,
            })
        })
        .collect()
}

/// A hash of the test code of each package, by (unit, package): the test
/// spans (`#[cfg(test)]` modules and test functions) of the files of its
/// crates, and the whole files of its test and bench targets.
fn test_sources(
    parsed: &Parsed,
    index: &Index,
    texts: &mut Texts,
) -> HashMap<(usize, usize), String> {
    let mut files: HashMap<(usize, usize), BTreeMap<String, bool>> = HashMap::new();
    for module in &index.modules {
        let Some(krate) = index.crates.get(module.krate) else {
            continue;
        };
        let Some(file) = index.files.list.get(module.file) else {
            continue;
        };
        let whole = files
            .entry((krate.spec.unit, krate.spec.package))
            .or_default()
            .entry(file.rel.clone())
            .or_insert(false);
        *whole |= krate.test_code;
    }
    let by_rel: HashMap<&str, usize> = parsed
        .files
        .list
        .iter()
        .enumerate()
        .map(|(position, file)| (file.rel.as_str(), position))
        .collect();
    let mut out = HashMap::new();
    for (key, list) in files {
        let mut parts: Vec<String> = Vec::new();
        for (rel, whole) in list {
            let Some(text) = texts.get(&rel) else {
                continue;
            };
            let code = if whole {
                text.to_string()
            } else {
                let spans = by_rel
                    .get(rel.as_str())
                    .and_then(|file| index.test_spans.get(file))
                    .cloned()
                    .unwrap_or_default();
                let mut spans = spans;
                spans.sort_unstable();
                spans
                    .iter()
                    .map(|(start, end)| lines_of(text, *start, *end))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            if !code.is_empty() {
                parts.push(rel);
                parts.push(code);
            }
        }
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        out.insert(key, fingerprint(&parts));
    }
    out
}

/// results.json of `<cache-dir>/mutation`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Kept {
    entries: BTreeMap<String, KeptResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeptResult {
    status: String,
    killed_by: Option<String>,
    duration_ms: u64,
    /// When it was last tested or reused, in seconds since 1970.
    used: u64,
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

fn load_kept(folder: &Path) -> Kept {
    std::fs::read(folder.join(RESULTS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Adds `results` to results.json and marks `reused` used now, keeping the
/// most recently used MAX_KEPT, under the folder's lock.
fn save_kept(
    cache_dir: &Path,
    results: Vec<(String, KeptResult)>,
    reused: &[String],
    deadline: Instant,
) -> Result<(), String> {
    let held = cache::acquire(cache_dir, CACHE_FOLDER, deadline)?;
    let mut kept = load_kept(&held.dir);
    let now = now_seconds();
    for fingerprint in reused {
        if let Some(entry) = kept.entries.get_mut(fingerprint) {
            entry.used = now;
        }
    }
    kept.entries.extend(results);
    if kept.entries.len() > MAX_KEPT {
        let mut used: Vec<u64> = kept.entries.values().map(|entry| entry.used).collect();
        used.sort_unstable_by(|a, b| b.cmp(a));
        let oldest_kept = used[MAX_KEPT - 1];
        kept.entries.retain(|_, entry| entry.used >= oldest_kept);
    }
    let bytes = serde_json::to_vec(&kept).map_err(|error| error.to_string())?;
    write_whole(&held.dir.join(RESULTS_FILE), &bytes).map_err(|error| error.to_string())
}

/// Writes `path` whole: a temporary file next to it, renamed.
fn write_whole(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)
}

/// The counts of the progress file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Tally {
    done: usize,
    killed: usize,
    survived: usize,
    timeout: usize,
    error: usize,
}

impl Tally {
    fn add(&mut self, status: &str) {
        self.done += 1;
        match status {
            "killed" => self.killed += 1,
            "survived" => self.survived += 1,
            "timeout" => self.timeout += 1,
            _ => self.error += 1,
        }
    }
}

/// The `--progress` file, replaced whole each time.
struct ProgressFile {
    path: Option<PathBuf>,
    total: usize,
}

impl ProgressFile {
    fn write(&self, phase: &str, tally: Tally, current: Option<String>) {
        let Some(path) = &self.path else { return };
        let progress = MutationProgress {
            protocol: protocol::PROTOCOL,
            phase: phase.to_string(),
            total: self.total,
            done: tally.done,
            killed: tally.killed,
            survived: tally.survived,
            timeout: tally.timeout,
            error: tally.error,
            current,
        };
        if let Ok(bytes) = serde_json::to_vec(&progress) {
            let _ = write_whole(path, &bytes);
        }
    }
}

/// A folder of the run in TMPDIR (cargo-mutants' build folders, the diffs,
/// mutants.out), removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn create() -> std::io::Result<Self> {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let next = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("difflens-mutate-{}-{next}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("tmp"))?;
        Ok(Self(path.canonicalize()?))
    }

    fn tmp(&self) -> PathBuf {
        self.0.join("tmp")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Copies the folder `from` into `to`; links stay links.
fn copy_all(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let source = entry.path();
        let destination = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&source)?, &destination)?;
        } else if kind.is_dir() {
            copy_all(&source, &destination)?;
        } else {
            std::fs::copy(&source, &destination)?;
        }
    }
    Ok(())
}

/// Everything one run shares.
struct Run<'r> {
    options: &'r Options,
    engine: &'r Engine,
    tree_root: &'r Path,
    scratch: &'r Scratch,
    deadline: Instant,
    diagnostics: Vec<String>,
}

impl Run<'_> {
    /// The environment of cargo and cargo-mutants.
    fn env(&self, command: &mut Command) {
        command
            .env("CARGO_TERM_COLOR", "never")
            .env("TMPDIR", self.scratch.tmp())
            .env_remove("CARGO_TARGET_DIR");
        if self.options.offline {
            command.env("CARGO_NET_OFFLINE", "true");
        }
    }

    /// cargo-mutants' mutants of `unit` in the code `diff` touches.
    fn list(&self, unit: &Unit, diff: &Path) -> Result<Vec<Listed>, String> {
        let mut command = self.engine.command();
        command
            .args(["--list", "--json", "--workspace", "--colors", "never"])
            .arg("--dir")
            .arg(&unit.root)
            .arg("--in-diff")
            .arg(diff)
            .current_dir(&unit.root);
        self.env(&mut command);
        let (stdout, finished) = process::capture(command, self.deadline)
            .map_err(|error| format!("cargo-mutants could not run: {error}"))?;
        if !finished.success() {
            return Err(format!(
                "cargo-mutants --list failed ({}):\n{}",
                finished.describe(),
                process::keep_end(finished.stderr.trim(), 4 << 10)
            ));
        }
        // Nothing on stdout: no mutant in the code the diff touches.
        if stdout.trim().is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_str(&stdout)
            .map_err(|error| format!("cargo-mutants listed what could not be read: {error}"))
    }

    /// Builds the tests of `packages` of `unit` into `target_dir`.
    fn build(&self, unit: &Unit, packages: &[String], target_dir: &Path) {
        let mut command = Command::new("cargo");
        command.args(["test", "--no-run"]).current_dir(&unit.root);
        for package in packages {
            command.arg(format!("--package={package}"));
        }
        if self.options.offline {
            command.arg("--offline");
        }
        self.env(&mut command);
        command.env("CARGO_TARGET_DIR", target_dir);
        // A build that fails fails cargo-mutants' baseline too, which says
        // why.
        let _ = process::capture(command, self.deadline);
    }

    /// The target folder cargo-mutants copies into its build folders,
    /// built: `<unit>/target`.
    fn prebuild(&mut self, unit: &Unit, packages: &[String]) {
        let target = unit.root.join("target");
        let Some(cache_dir) = &self.options.cache_dir else {
            self.build(unit, packages, &target);
            return;
        };
        let held = cache::acquire(&cache_dir.join(CACHE_FOLDER), "target", self.deadline).and_then(
            |held| {
                // Its own crates must be built again, whatever an earlier
                // copy left there (see `cache`).
                cache::touch_tree(self.tree_root)
                    .map(|()| held)
                    .map_err(|error| format!("the tree could not be set modified now: {error}"))
            },
        );
        match held {
            Ok(held) => {
                self.build(unit, packages, &held.dir);
                let _ = std::fs::remove_dir_all(&target);
                if let Err(error) = copy_all(&held.dir, &target) {
                    let _ = std::fs::remove_dir_all(&target);
                    self.diagnostics.push(format!(
                        "the build output of earlier runs was not used: it could not be copied: {error}"
                    ));
                }
            }
            Err(why) => {
                self.diagnostics.push(format!(
                    "the build output of earlier runs was not used: {why}"
                ));
                self.build(unit, packages, &target);
            }
        }
    }
}

/// Reads outcomes.json; None while it is missing or half written.
fn read_outcomes(out: &Path) -> Option<Outcomes> {
    let bytes = std::fs::read(out.join("mutants.out").join("outcomes.json")).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The progress of a cargo-mutants run from what it wrote so far.
fn poll_progress(
    out: &Path,
    names: &HashSet<&str>,
    logs: &HashMap<String, String>,
    base: Tally,
) -> (&'static str, Tally, Option<String>) {
    let outcomes = read_outcomes(out).unwrap_or_default();
    let mut tally = base;
    let mut finished_logs = HashSet::new();
    let mut phase = "baseline";
    for outcome in &outcomes.outcomes {
        if outcome.is_baseline() {
            phase = "mutating";
        }
        if let Some(log) = &outcome.log_path {
            finished_logs.insert(log.clone());
        }
        if outcome
            .mutant_name()
            .is_some_and(|name| names.contains(name))
        {
            tally.add(status(&outcome.summary));
        }
    }
    let mut current = None;
    if phase == "mutating" {
        let log_dir = out.join("mutants.out").join("log");
        let mut running: Vec<String> = std::fs::read_dir(&log_dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .filter(|name| !finished_logs.contains(&format!("log/{name}")))
            .collect();
        running.sort();
        current = running.iter().find_map(|name| {
            let stem = name.strip_suffix(".log")?;
            // A second log of one name gets "_001".
            let stem = match stem.rsplit_once('_') {
                Some((head, tail))
                    if tail.len() == 3 && tail.chars().all(|c| c.is_ascii_digit()) =>
                {
                    head
                }
                _ => stem,
            };
            logs.get(stem).cloned()
        });
    }
    (phase, tally, current)
}

/// The results of a cargo-mutants run, from its outcomes: by name, in
/// order, onto the `pending` mutants.
fn map_outcomes(
    outcomes: &Outcomes,
    pending: &[usize],
    candidates: &[Candidate],
    out: &Path,
) -> (Option<String>, Vec<(usize, Tested)>) {
    let mut by_name: HashMap<&str, VecDeque<usize>> = HashMap::new();
    for index in pending {
        by_name
            .entry(candidates[*index].name.as_str())
            .or_default()
            .push_back(*index);
    }
    let mut baseline = None;
    let mut results = Vec::new();
    for outcome in &outcomes.outcomes {
        if outcome.is_baseline() {
            baseline = Some(outcome.summary.clone());
            continue;
        }
        let Some(index) = outcome
            .mutant_name()
            .and_then(|name| by_name.get_mut(name))
            .and_then(VecDeque::pop_front)
        else {
            continue;
        };
        let status = status(&outcome.summary);
        let killed_by = if status == "killed" {
            outcome
                .log_path
                .as_ref()
                .and_then(|log| std::fs::read_to_string(out.join("mutants.out").join(log)).ok())
                .and_then(|log| first_failed_test(&log))
        } else {
            None
        };
        results.push((
            index,
            Tested {
                status: status.to_string(),
                killed_by,
                duration_ms: outcome.duration_ms(),
                cached: false,
                keep: true,
            },
        ));
    }
    (baseline, results)
}

fn error_result() -> Tested {
    Tested {
        status: "error".to_string(),
        killed_by: None,
        duration_ms: 0,
        cached: false,
        keep: false,
    }
}

/// Tests the `pending` mutants of `unit` with cargo-mutants; `base` is
/// the progress of the mutants with a result already.
#[allow(clippy::too_many_arguments)]
fn run_unit(
    run: &mut Run,
    unit: &Unit,
    unit_rel: &str,
    diff: &Path,
    pending: &[usize],
    candidates: &[Candidate],
    progress: &ProgressFile,
    base: Tally,
) -> Vec<(usize, Tested)> {
    progress.write("baseline", base, None);
    let mut packages: Vec<String> = pending
        .iter()
        .filter_map(|index| {
            let name = &candidates[*index].package;
            unit.packages
                .iter()
                .find(|package| package.name == *name)
                .map(|package| format!("{}@{}", package.name, package.version))
        })
        .collect();
    packages.sort();
    packages.dedup();
    run.prebuild(unit, &packages);

    let out = run.scratch.0.join(format!(
        "out-{}",
        fingerprint(&[unit_rel]).get(..8).unwrap_or("unit")
    ));
    let mut command = run.engine.command();
    command
        .args(["--workspace", "--colors", "never", "--no-shuffle"])
        .args(["--copy-target=true", "--caught", "--unviable"])
        .arg("--dir")
        .arg(&unit.root)
        .arg("--in-diff")
        .arg(diff)
        .arg("--output")
        .arg(&out)
        .arg(format!("--jobs={}", run.options.jobs.max(1)))
        .arg(format!("--timeout-multiplier={TIMEOUT_MULTIPLIER}"))
        .arg(format!(
            "--minimum-test-timeout={}",
            run.options.min_mutant_timeout.as_secs_f64()
        ))
        .current_dir(&unit.root);
    if run.options.offline {
        command.arg("--cargo-arg=--offline");
    }
    let mut names: Vec<&str> = pending
        .iter()
        .map(|index| candidates[*index].name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    for name in &names {
        command.arg(format!("--re=^{}$", regex_escape(name)));
    }
    run.env(&mut command);
    let name_set: HashSet<&str> = names.iter().copied().collect();
    let logs: HashMap<String, String> = pending
        .iter()
        .map(|index| {
            let candidate = &candidates[*index];
            (
                log_base(&candidate.unit_file, candidate.span.start),
                format!("{}:{}", candidate.file, candidate.span.start.line),
            )
        })
        .collect();
    let deadline = run.deadline;
    let interrupt = deadline.checked_sub(INTERRUPT_GRACE);
    let stop = AtomicBool::new(false);
    let finished: std::io::Result<Finished> = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                let (phase, tally, current) = poll_progress(&out, &name_set, &logs, base);
                progress.write(phase, tally, current);
                std::thread::sleep(POLL);
            }
        });
        let finished = process::stream_until(command, interrupt, deadline, |_| {});
        stop.store(true, Ordering::Relaxed);
        finished
    });
    // Tests cargo-mutants could not stop, in process groups of their own.
    process::kill_matching(&run.scratch.tmp().display().to_string());
    let finished = match finished {
        Ok(finished) => finished,
        Err(error) => {
            run.diagnostics.push(format!(
                "cargo-mutants could not run, so {} mutants of {unit_rel} could not be tested: {error}",
                pending.len()
            ));
            return pending
                .iter()
                .map(|index| (*index, error_result()))
                .collect();
        }
    };
    let outcomes = read_outcomes(&out).unwrap_or_default();
    let (baseline, mut results) = map_outcomes(&outcomes, pending, candidates, &out);
    if baseline
        .as_deref()
        .is_some_and(|summary| summary != "Success")
    {
        let log =
            std::fs::read_to_string(out.join("mutants.out/log/baseline.log")).unwrap_or_default();
        run.diagnostics.push(format!(
            "the tests of {} do not pass on the unmutated code, so its {} mutants could not be tested:\n{}",
            packages.join(", "),
            pending.len(),
            process::keep_end(log.trim(), 4 << 10)
        ));
        return pending
            .iter()
            .map(|index| (*index, error_result()))
            .collect();
    }
    let reported: HashSet<usize> = results.iter().map(|(index, _)| *index).collect();
    let missing: Vec<usize> = pending
        .iter()
        .copied()
        .filter(|index| !reported.contains(index))
        .collect();
    // Left out when the time ran out; else cargo-mutants failed.
    if !missing.is_empty() && !finished.timed_out {
        run.diagnostics.push(format!(
            "cargo-mutants stopped ({}) before it tested {} mutants of {unit_rel}:\n{}",
            finished.describe(),
            missing.len(),
            process::keep_end(finished.stderr.trim(), 4 << 10)
        ));
        results.extend(missing.iter().map(|index| (*index, error_result())));
    }
    results
}

pub fn mutate(
    dir: &Path,
    changed: &Changed,
    coverage: &CoverageFile,
    options: &Options,
) -> Result<MutationRun, String> {
    let started = Instant::now();
    let deadline = crate::testrun::stop_at(started, options.timeout);
    let engine = engine::locate(options.tools.as_deref())?;
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
    let functions = functions(&index);
    let generated: HashSet<&str> = parsed
        .files
        .list
        .iter()
        .filter(|file| file.generated)
        .map(|file| file.rel.as_str())
        .collect();
    let mut texts = Texts {
        root: parsed.tree.root.clone(),
        files: HashMap::new(),
    };
    let scratch = Scratch::create().map_err(|error| format!("no scratch folder: {error}"))?;
    let mut run = Run {
        options,
        engine: &engine,
        tree_root: &parsed.tree.root,
        scratch: &scratch,
        deadline,
        diagnostics: parsed.workspace.warnings.clone(),
    };
    let lines = candidate_lines(changed, coverage);
    let roots: Vec<Option<String>> = parsed
        .workspace
        .units
        .iter()
        .map(|unit| parsed.tree.rel(&unit.root))
        .collect();

    // List, per workspace with candidate lines.
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut diffs: HashMap<usize, PathBuf> = HashMap::new();
    for (position, unit) in parsed.workspace.units.iter().enumerate() {
        let Some(unit_rel) = roots[position].clone() else {
            continue;
        };
        let mut files = Vec::new();
        for (file, numbers) in &lines {
            if unit_of(&roots, file) != Some(position) || !file.ends_with(".rs") {
                continue;
            }
            let Some(text) = texts.get(file) else {
                continue;
            };
            let all: Vec<&str> = text.lines().collect();
            let wanted: Vec<(u32, String)> = numbers
                .iter()
                .filter_map(|line| Some((*line, all.get(*line as usize - 1)?.to_string())))
                .collect();
            let inside = if unit_rel == "." {
                file.clone()
            } else {
                file[unit_rel.len() + 1..].to_string()
            };
            files.push((inside, wanted));
        }
        let diff_text = synthesize_diff(&files);
        if diff_text.is_empty() {
            continue;
        }
        let diff = scratch.0.join(format!("unit-{position}.diff"));
        std::fs::write(&diff, diff_text).map_err(|error| error.to_string())?;
        let listed = match run.list(unit, &diff) {
            Ok(listed) => listed,
            Err(why) => {
                run.diagnostics.push(format!(
                    "the mutants of {unit_rel} could not be listed: {why}"
                ));
                continue;
            }
        };
        diffs.insert(position, diff);
        for mutant in listed {
            let file = if unit_rel == "." {
                mutant.file.clone()
            } else {
                format!("{unit_rel}/{}", mutant.file)
            };
            let on_candidate = lines
                .get(&file)
                .is_some_and(|numbers| numbers.contains(&mutant.span.start.line));
            if !on_candidate || generated.contains(file.as_str()) {
                continue;
            }
            let function = innermost(&functions, &file, mutant.span.start.line);
            if function.is_some_and(|function| functions[function].test) {
                continue;
            }
            let Some(original) = texts
                .get(&file)
                .and_then(|text| span_text(text, mutant.span))
            else {
                run.diagnostics.push(format!(
                    "{}: its place in {file} could not be read, so it was left out",
                    mutant.name
                ));
                continue;
            };
            let prefix = format!(
                "{}:{}:{}: ",
                mutant.file, mutant.span.start.line, mutant.span.start.column
            );
            let description = mutant
                .name
                .strip_prefix(&prefix)
                .unwrap_or(&mutant.name)
                .to_string();
            candidates.push(Candidate {
                unit: position,
                operator: operator(&mutant.genre, &original, &mutant.replacement),
                name: mutant.name,
                package: mutant.package,
                file,
                unit_file: mutant.file,
                span: mutant.span,
                original,
                mutated: mutant.replacement,
                description,
                function,
            });
        }
    }
    // Rule 7's order; the sample is taken in it.
    candidates.sort_by(|a, b| {
        (&a.file, a.span.start, &a.description).cmp(&(&b.file, b.span.start, &b.description))
    });
    let max = options.max_mutants.max(1);
    let selected = round_robin(&groups(&candidates, &functions), max);

    // Fingerprints and results kept.
    let tests = test_sources(&parsed, &index, &mut texts);
    let mut fingerprints: HashMap<usize, String> = HashMap::new();
    let mut ids: HashMap<usize, String> = HashMap::new();
    for index in &selected {
        let candidate = &candidates[*index];
        let (function_id, function_start, function_text) = match candidate.function {
            Some(function) => {
                let function = &functions[function];
                let text = texts
                    .get(&function.file)
                    .map(|text| lines_of(text, function.start, function.end))
                    .unwrap_or_default();
                (function.id.clone(), function.start, text)
            }
            None => (
                String::new(),
                0,
                texts.get(&candidate.file).unwrap_or_default().to_string(),
            ),
        };
        let package = parsed.workspace.units[candidate.unit]
            .packages
            .iter()
            .position(|package| package.name == candidate.package);
        let test_hash = package
            .and_then(|package| tests.get(&(candidate.unit, package)))
            .cloned()
            .unwrap_or_default();
        let line =
            (candidate.span.start.line - function_start.min(candidate.span.start.line)).to_string();
        let column = candidate.span.start.column.to_string();
        let identity = [
            candidate.file.as_str(),
            function_id.as_str(),
            candidate.operator,
            candidate.original.as_str(),
            candidate.mutated.as_str(),
            line.as_str(),
            column.as_str(),
            candidate.description.as_str(),
        ];
        ids.insert(*index, fingerprint(&identity));
        let mut parts = vec![
            FINGERPRINT_VERSION,
            engine::NAME,
            engine.version.as_str(),
            function_text.as_str(),
            test_hash.as_str(),
        ];
        parts.extend(identity);
        fingerprints.insert(*index, fingerprint(&parts));
    }
    let kept = options
        .cache_dir
        .as_ref()
        .map(|cache_dir| load_kept(&cache_dir.join(CACHE_FOLDER)))
        .unwrap_or_default();
    let mut results: BTreeMap<usize, Tested> = BTreeMap::new();
    for index in &selected {
        if let Some(entry) = kept.entries.get(&fingerprints[index]) {
            results.insert(
                *index,
                Tested {
                    status: entry.status.clone(),
                    killed_by: entry.killed_by.clone(),
                    duration_ms: entry.duration_ms,
                    cached: true,
                    keep: false,
                },
            );
        }
    }
    let progress = ProgressFile {
        path: options.progress.clone(),
        total: selected.len(),
    };
    let tally = |results: &BTreeMap<usize, Tested>| {
        let mut tally = Tally::default();
        for tested in results.values() {
            tally.add(&tested.status);
        }
        tally
    };
    progress.write("baseline", tally(&results), None);

    // Test the rest, per workspace.
    for (position, unit) in parsed.workspace.units.iter().enumerate() {
        let pending: Vec<usize> = selected
            .iter()
            .copied()
            .filter(|index| candidates[*index].unit == position && !results.contains_key(index))
            .collect();
        let (Some(diff), false) = (diffs.get(&position), pending.is_empty()) else {
            continue;
        };
        if Instant::now() >= deadline {
            continue;
        }
        let unit_rel = roots[position].clone().unwrap_or_else(|| ".".to_string());
        let base = tally(&results);
        results.extend(run_unit(
            &mut run,
            unit,
            &unit_rel,
            diff,
            &pending,
            &candidates,
            &progress,
            base,
        ));
    }
    let left_out = selected.len() - results.len();
    if left_out > 0 {
        let (count, verb) = if left_out == 1 {
            ("1 mutant".to_string(), "was")
        } else {
            (format!("{left_out} mutants"), "were")
        };
        run.diagnostics.push(format!(
            "{count} {verb} not tested: the time ran out after {:?}",
            options.timeout
        ));
    }
    if let Some(cache_dir) = &options.cache_dir {
        let now = now_seconds();
        let new: Vec<(String, KeptResult)> = results
            .iter()
            .filter(|(_, tested)| tested.keep)
            .map(|(index, tested)| {
                (
                    fingerprints[index].clone(),
                    KeptResult {
                        status: tested.status.clone(),
                        killed_by: tested.killed_by.clone(),
                        duration_ms: tested.duration_ms,
                        used: now,
                    },
                )
            })
            .collect();
        let reused: Vec<String> = results
            .iter()
            .filter(|(_, tested)| tested.cached)
            .map(|(index, _)| fingerprints[index].clone())
            .collect();
        if let Err(why) = save_kept(cache_dir, new, &reused, deadline + Duration::from_secs(2)) {
            run.diagnostics
                .push(format!("the results of this run were not kept: {why}"));
        }
    }

    // The document, in rule 7's order (`results` is in it: by index).
    let mut summary = MutationSummary {
        candidates: candidates.len(),
        sampled: candidates.len() > max,
        max_mutants: max,
        ..MutationSummary::default()
    };
    let mut mutants = Vec::new();
    let mut used_ids = HashSet::new();
    for (index, tested) in results {
        let candidate = &candidates[index];
        match tested.status.as_str() {
            "killed" => summary.killed += 1,
            "survived" => summary.survived += 1,
            "timeout" => summary.timeout += 1,
            "no_coverage" => summary.no_coverage += 1,
            _ => summary.error += 1,
        }
        if tested.cached {
            summary.cached += 1;
        }
        let mut id = ids[&index].clone();
        let mut n = 2;
        while !used_ids.insert(id.clone()) {
            id = format!("{}-{n}", ids[&index]);
            n += 1;
        }
        mutants.push(Mutant {
            id,
            file: candidate.file.clone(),
            line: candidate.span.start.line,
            column: candidate.span.start.column,
            end_line: candidate.span.end.line,
            end_column: candidate.span.end.column,
            original: candidate.original.clone(),
            mutated: candidate.mutated.clone(),
            operator: candidate.operator.to_string(),
            description: candidate.description.clone(),
            function: candidate
                .function
                .map(|function| functions[function].id.clone()),
            status: tested.status,
            killed_by: tested.killed_by,
            duration_ms: tested.duration_ms,
            cached: tested.cached,
        });
    }
    let final_tally = {
        let mut tally = Tally::default();
        for mutant in &mutants {
            tally.add(&mutant.status);
        }
        tally
    };
    progress.write("mutating", final_tally, None);
    summary.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let diagnostics = std::mem::take(&mut run.diagnostics);
    drop(run);
    drop(scratch);
    Ok(MutationRun {
        protocol: protocol::PROTOCOL,
        language: protocol::LANGUAGE.to_string(),
        runner: Identity::own(),
        engine: Identity {
            name: engine::NAME.to_string(),
            version: engine.version.clone(),
        },
        mutants,
        summary,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ChangedFile, CoverageBlock};
    use crate::testing::{self, TempDir};
    use std::sync::OnceLock;

    fn position(line: u32, column: u32) -> Position {
        Position { line, column }
    }

    fn span(start: (u32, u32), end: (u32, u32)) -> Span {
        Span {
            start: position(start.0, start.1),
            end: position(end.0, end.1),
        }
    }

    fn block(file: &str, start_line: u32, end_line: u32, count: u64) -> CoverageBlock {
        CoverageBlock {
            file: file.to_string(),
            start_line,
            end_line,
            statements: 1,
            count,
        }
    }

    #[test]
    fn candidate_lines_are_added_and_ran() {
        let changed = Changed {
            protocol: 1,
            exclude: None,
            files: vec![
                ChangedFile {
                    path: "src/lib.rs".to_string(),
                    added: vec![[1, 6], [20, 21]],
                },
                ChangedFile {
                    path: "src/other.rs".to_string(),
                    added: vec![[1, 100]],
                },
                ChangedFile {
                    path: "src/huge.rs".to_string(),
                    added: vec![[1, u32::MAX]],
                },
            ],
        };
        let coverage = CoverageFile {
            protocol: 1,
            blocks: vec![
                block("src/lib.rs", 2, 3, 4),
                block("src/lib.rs", 5, 9, 0),
                block("src/lib.rs", 6, 20, 1),
                block("src/huge.rs", 7, 7, 1),
                block("src/untouched.rs", 1, 5, 1),
            ],
        };
        let lines = candidate_lines(&changed, &coverage);
        let of = |file: &str| -> Vec<u32> {
            lines
                .get(file)
                .map(|lines| lines.iter().copied().collect())
                .unwrap_or_default()
        };
        // 5 ran no time; 21 is not covered; other.rs has no coverage.
        assert_eq!(of("src/lib.rs"), [2, 3, 6, 20]);
        assert_eq!(of("src/other.rs"), Vec::<u32>::new());
        assert_eq!(of("src/huge.rs"), [7]);
        assert!(!lines.contains_key("src/untouched.rs"));
    }

    #[test]
    fn the_diff_adds_exactly_the_candidate_lines() {
        let diff = synthesize_diff(&[
            (
                "src/lib.rs".to_string(),
                vec![
                    (3, "    a + b".to_string()),
                    (4, "}".to_string()),
                    (9, "".to_string()),
                ],
            ),
            ("src/empty.rs".to_string(), Vec::new()),
            ("src/b.rs".to_string(), vec![(1, "fn b() {}".to_string())]),
        ]);
        assert_eq!(
            diff,
            "--- a/src/lib.rs\n+++ b/src/lib.rs\n\
             @@ -0,0 +3,2 @@\n+    a + b\n+}\n\
             @@ -0,0 +9,1 @@\n+\n\
             --- a/src/b.rs\n+++ b/src/b.rs\n\
             @@ -0,0 +1,1 @@\n+fn b() {}\n"
        );
        assert_eq!(synthesize_diff(&[]), "");
    }

    #[test]
    fn spans_are_read_in_characters_with_the_end_exclusive() {
        let text = "fn f(é: i32) -> i32 {\n    let z = é + 1;\n    z\n}\n";
        // `+` is the 15th character of its line (its 16th byte).
        assert_eq!(
            span_text(text, span((2, 15), (2, 16))).as_deref(),
            Some("+")
        );
        assert_eq!(
            span_text(text, span((2, 5), (3, 6))).as_deref(),
            Some("let z = é + 1;\n    z")
        );
        // Just past the last character of a line.
        assert_eq!(span_text(text, span((3, 5), (3, 6))).as_deref(), Some("z"));
        assert_eq!(span_text(text, span((4, 2), (4, 2))).as_deref(), Some(""));
        assert_eq!(span_text(text, span((2, 40), (2, 41))), None);
        assert_eq!(span_text(text, span((9, 1), (9, 2))), None);
        assert_eq!(span_text(text, span((2, 0), (2, 1))), None);
        assert_eq!(span_text(text, span((3, 6), (3, 5))), None);
        assert_eq!(lines_of(text, 2, 3), "    let z = é + 1;\n    z");
    }

    #[test]
    fn operators_are_read_from_the_change() {
        for (genre, original, mutated, family) in [
            ("BinaryOperator", "<", "<=", "relational_boundary"),
            ("BinaryOperator", ">=", ">", "relational_boundary"),
            ("BinaryOperator", "==", "!=", "relational_negation"),
            ("BinaryOperator", ">=", "<", "relational_negation"),
            ("BinaryOperator", ">", "<=", "relational_negation"),
            ("BinaryOperator", "<", "==", "other"),
            ("BinaryOperator", "<", ">", "other"),
            ("BinaryOperator", "&&", "||", "logical"),
            ("BinaryOperator", "+", "-", "arithmetic"),
            ("BinaryOperator", "+", "*", "arithmetic"),
            ("BinaryOperator", "%", "/", "arithmetic"),
            ("BinaryOperator", "+=", "-=", "increment"),
            ("BinaryOperator", "-=", "+=", "increment"),
            ("BinaryOperator", "+=", "*=", "arithmetic"),
            ("BinaryOperator", "^=", "|=", "other"),
            ("BinaryOperator", "<<", ">>", "other"),
            ("UnaryOperator", "!", "", "negation_removal"),
            ("UnaryOperator", "-", "", "other"),
            ("FnValue", "a + b", "Default::default()", "return_value"),
            ("FnValue", "v.push(1);", "()", "return_value"),
            ("MatchArm", "3 => 4", "", "other"),
            ("MatchArmGuard", "ok", "true", "other"),
            ("StructField", "a: 1", "", "other"),
            ("SomethingNew", "x", "y", "other"),
        ] {
            assert_eq!(
                operator(genre, original, mutated),
                family,
                "{genre} {original} {mutated}"
            );
        }
    }

    #[test]
    fn outcomes_map_to_statuses() {
        for (summary, expected) in [
            ("CaughtMutant", "killed"),
            ("MissedMutant", "survived"),
            ("Timeout", "timeout"),
            ("Unviable", "error"),
            ("Failure", "error"),
            ("Success", "error"),
        ] {
            assert_eq!(status(summary), expected, "{summary}");
        }
    }

    #[test]
    fn the_sample_takes_functions_in_turn() {
        // Three groups: 3, 1 and 2 mutants.
        let groups = vec![vec![0, 1, 2], vec![3], vec![4, 5]];
        assert_eq!(round_robin(&groups, 4), [0, 1, 3, 4]);
        assert_eq!(round_robin(&groups, 5), [0, 1, 3, 4, 5]);
        assert_eq!(round_robin(&groups, 2), [0, 3]);
        assert_eq!(round_robin(&groups, 100), [0, 1, 2, 3, 4, 5]);
        assert_eq!(round_robin(&[], 3), Vec::<usize>::new());
    }

    fn candidate(file: &str, line: u32, column: u32, function: Option<usize>) -> Candidate {
        Candidate {
            unit: 0,
            name: format!("{file}:{line}:{column}: replace + with - in f"),
            package: "p".to_string(),
            file: file.to_string(),
            unit_file: file.to_string(),
            span: span((line, column), (line, column + 1)),
            original: "+".to_string(),
            mutated: "-".to_string(),
            operator: "arithmetic",
            description: "replace + with - in f".to_string(),
            function,
        }
    }

    fn function(id: &str, file: &str, start: u32, end: u32) -> FnLines {
        FnLines {
            id: id.to_string(),
            file: file.to_string(),
            start,
            end,
            test: false,
        }
    }

    #[test]
    fn groups_are_functions_by_file_and_start_line() {
        let functions = vec![
            function("b::late", "b.rs", 30, 40),
            function("b::early", "b.rs", 1, 10),
            function("a::only", "a.rs", 5, 9),
        ];
        let candidates = vec![
            candidate("a.rs", 2, 1, None),
            candidate("a.rs", 6, 1, Some(2)),
            candidate("a.rs", 7, 1, Some(2)),
            candidate("b.rs", 3, 1, Some(1)),
            candidate("b.rs", 31, 1, Some(0)),
            candidate("b.rs", 32, 1, Some(0)),
        ];
        assert_eq!(
            groups(&candidates, &functions),
            [vec![0], vec![1, 2], vec![3], vec![4, 5]]
        );
        assert_eq!(
            round_robin(&groups(&candidates, &functions), 5),
            [0, 1, 2, 3, 4]
        );
    }

    #[test]
    fn the_innermost_function_holds_a_line() {
        let functions = vec![
            function("m::outer", "a.rs", 1, 20),
            function("m::outer::inner", "a.rs", 5, 8),
            function("m::outer$1", "a.rs", 5, 8),
            function("m::other", "b.rs", 1, 20),
        ];
        assert_eq!(innermost(&functions, "a.rs", 3), Some(0));
        // The closure, listed after a function of the same lines.
        assert_eq!(innermost(&functions, "a.rs", 6), Some(2));
        assert_eq!(innermost(&functions, "a.rs", 21), None);
        assert_eq!(innermost(&functions, "c.rs", 3), None);
    }

    #[test]
    fn files_belong_to_the_deepest_workspace() {
        let roots = vec![
            Some(".".to_string()),
            Some("crates/a".to_string()),
            None,
            Some("crates/ab".to_string()),
        ];
        assert_eq!(unit_of(&roots, "src/lib.rs"), Some(0));
        assert_eq!(unit_of(&roots, "crates/a/src/lib.rs"), Some(1));
        assert_eq!(unit_of(&roots, "crates/ab/src/lib.rs"), Some(3));
        assert_eq!(unit_of(&roots[1..], "src/lib.rs"), None);
    }

    #[test]
    fn small_helpers() {
        assert_eq!(
            regex_escape("src/lib.rs:21:7: replace + with * in add -> i64"),
            "src/lib\\.rs:21:7: replace \\+ with \\* in add \\-> i64"
        );
        assert_eq!(regex_escape("<T as Trait>::f()"), "<T as Trait>::f\\(\\)");
        assert_eq!(
            first_failed_test(
                "running 3 tests\ntest tests::a ... ok\ntest tests::adds ... FAILED\ntest tests::b ... FAILED\n"
            )
            .as_deref(),
            Some("tests::adds")
        );
        assert_eq!(first_failed_test("test result: FAILED. 1 passed"), None);
        assert_eq!(
            log_base("crates/a b/src/lib.rs", position(11, 19)),
            "crates__a_b__src__lib.rs_line_11_col_19"
        );
        let one = fingerprint(&["a", "bc"]);
        assert_eq!(one.len(), 32);
        assert_eq!(one, fingerprint(&["a", "bc"]));
        assert_ne!(one, fingerprint(&["ab", "c"]));
        assert_ne!(one, fingerprint(&["a", "bd"]));
    }

    /// An output folder with outcomes.json and the logs given.
    fn outcomes_folder(outcomes: &Value, logs: &[(&str, &str)]) -> TempDir {
        let folder = TempDir::new();
        let out = folder.0.join("mutants.out");
        std::fs::create_dir_all(out.join("log")).unwrap();
        std::fs::write(out.join("outcomes.json"), outcomes.to_string()).unwrap();
        for (name, text) in logs {
            std::fs::write(out.join("log").join(name), text).unwrap();
        }
        folder
    }

    fn mutant_outcome(name: &str, summary: &str, log: &str, test_seconds: Option<f64>) -> Value {
        let mut phases = vec![
            serde_json::json!({ "phase": "Build", "duration": 0.5, "process_status": "Success", "argv": [] }),
        ];
        if let Some(seconds) = test_seconds {
            phases.push(serde_json::json!({ "phase": "Test", "duration": seconds, "process_status": "Success", "argv": [] }));
        }
        serde_json::json!({
            "scenario": { "Mutant": { "name": name, "file": "src/lib.rs" } },
            "summary": summary,
            "log_path": format!("log/{log}"),
            "diff_path": null,
            "phase_results": phases,
        })
    }

    #[test]
    fn outcomes_become_results_of_the_mutants_by_name() {
        let candidates = vec![
            candidate("src/lib.rs", 3, 5, None),
            candidate("src/lib.rs", 4, 5, None),
            candidate("src/lib.rs", 5, 5, None),
            candidate("src/lib.rs", 6, 5, None),
            candidate("src/lib.rs", 7, 5, None),
        ];
        let outcomes = serde_json::json!({
            "outcomes": [
                { "scenario": "Baseline", "summary": "Success", "log_path": "log/baseline.log", "phase_results": [] },
                mutant_outcome(&candidates[0].name, "CaughtMutant", "a.log", Some(0.0615)),
                mutant_outcome(&candidates[1].name, "MissedMutant", "b.log", Some(0.1)),
                mutant_outcome(&candidates[2].name, "Timeout", "c.log", Some(3.0)),
                mutant_outcome(&candidates[3].name, "Unviable", "d.log", None),
                mutant_outcome("src/lib.rs:99:1: not asked for", "MissedMutant", "e.log", Some(0.1)),
            ],
            "total_mutants": 5,
        });
        let folder = outcomes_folder(
            &outcomes,
            &[("a.log", "running 1 test\ntest tests::adds ... FAILED\n")],
        );
        let pending = [0, 1, 2, 3, 4];
        let (baseline, results) = map_outcomes(
            &serde_json::from_value(outcomes).unwrap(),
            &pending,
            &candidates,
            &folder.0,
        );
        assert_eq!(baseline.as_deref(), Some("Success"));
        let found: Vec<(usize, &str, Option<&str>, u64)> = results
            .iter()
            .map(|(index, tested)| {
                (
                    *index,
                    tested.status.as_str(),
                    tested.killed_by.as_deref(),
                    tested.duration_ms,
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                (0, "killed", Some("tests::adds"), 62),
                (1, "survived", None, 100),
                (2, "timeout", None, 3000),
                // Built for 0.5 s, never tested.
                (3, "error", None, 500),
            ]
        );
        assert!(results
            .iter()
            .all(|(_, tested)| tested.keep && !tested.cached));
    }

    #[test]
    fn progress_is_read_from_what_cargo_mutants_wrote() {
        let candidates = [
            candidate("src/lib.rs", 3, 5, None),
            candidate("src/lib.rs", 4, 5, None),
        ];
        let names: HashSet<&str> = candidates.iter().map(|c| c.name.as_str()).collect();
        let logs: HashMap<String, String> = candidates
            .iter()
            .map(|c| {
                (
                    log_base(&c.unit_file, c.span.start),
                    format!("{}:{}", c.file, c.span.start.line),
                )
            })
            .collect();
        let base = Tally {
            done: 2,
            killed: 2,
            ..Tally::default()
        };
        let empty = TempDir::new();
        assert_eq!(
            poll_progress(&empty.0, &names, &logs, base),
            ("baseline", base, None)
        );
        let outcomes = serde_json::json!({ "outcomes": [
            { "scenario": "Baseline", "summary": "Success", "log_path": "log/baseline.log", "phase_results": [] },
            mutant_outcome(&candidates[0].name, "MissedMutant", "src__lib.rs_line_3_col_5.log", Some(0.1)),
        ]});
        let folder = outcomes_folder(
            &outcomes,
            &[
                ("baseline.log", ""),
                ("src__lib.rs_line_3_col_5.log", ""),
                ("src__lib.rs_line_4_col_5_001.log", ""),
            ],
        );
        let (phase, tally, current) = poll_progress(&folder.0, &names, &logs, base);
        assert_eq!(phase, "mutating");
        assert_eq!(
            tally,
            Tally {
                done: 3,
                killed: 2,
                survived: 1,
                ..Tally::default()
            }
        );
        assert_eq!(current.as_deref(), Some("src/lib.rs:4"));
        // Half written: read as nothing yet.
        std::fs::write(
            folder.0.join("mutants.out/outcomes.json"),
            "{\"outcomes\": [",
        )
        .unwrap();
        assert_eq!(poll_progress(&folder.0, &names, &logs, base).1, base);
    }

    #[test]
    fn kept_results_are_merged_and_marked_used() {
        let folder = TempDir::new();
        let result = |status: &str, used: u64| KeptResult {
            status: status.to_string(),
            killed_by: None,
            duration_ms: 5,
            used,
        };
        let soon = Instant::now() + Duration::from_secs(10);
        save_kept(
            &folder.0,
            vec![("a".to_string(), result("killed", 1))],
            &[],
            soon,
        )
        .unwrap();
        save_kept(
            &folder.0,
            vec![("b".to_string(), result("survived", 2))],
            &["a".to_string()],
            soon,
        )
        .unwrap();
        let kept = load_kept(&folder.0.join(CACHE_FOLDER));
        assert_eq!(kept.entries.len(), 2);
        assert_eq!(kept.entries["b"].status, "survived");
        assert!(kept.entries["a"].used > 1, "reused now");
        // Unreadable: nothing kept, no failure.
        std::fs::write(folder.0.join(CACHE_FOLDER).join(RESULTS_FILE), "not json").unwrap();
        assert!(load_kept(&folder.0.join(CACHE_FOLDER)).entries.is_empty());
    }

    // The tests below run cargo-mutants itself.

    /// cargo-mutants, installed by `prepare --tools` once per test process
    /// into the analyzer's own target folder (kept between test runs:
    /// prepare installs only when it is missing).
    fn tools() -> PathBuf {
        static TOOLS: OnceLock<PathBuf> = OnceLock::new();
        TOOLS
            .get_or_init(|| {
                let tools = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/mutation-tools");
                // A copy: cargo fetch writes a Cargo.lock.
                let (_folder, tree) = TempDir::with_fixture("mutation");
                let (code, stdout, stderr) = cli(&[
                    "prepare",
                    "--dir",
                    tree.to_str().unwrap(),
                    "--tools",
                    tools.to_str().unwrap(),
                    "--timeout",
                    "15m",
                ]);
                assert_eq!(code, 0, "{stderr}");
                assert_eq!(stdout.trim(), r#"{"protocol":1,"writable":[]}"#);
                tools
            })
            .clone()
    }

    fn cli(args: &[&str]) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let code = crate::cli::run(&args, &mut stdout, &mut stderr);
        (
            code,
            String::from_utf8(stdout).unwrap(),
            String::from_utf8(stderr).unwrap(),
        )
    }

    /// Runs `mutate` on `tree` with the fixture's changed and coverage
    /// files (or those given) and `extra` flags; the document, checked
    /// against the schema.
    fn mutate_document(tree: &Path, files: Option<(&Path, &Path)>, extra: &[&str]) -> Value {
        let fixture = testing::testdata("mutation");
        let (changed, coverage) = match files {
            Some((changed, coverage)) => (changed.to_path_buf(), coverage.to_path_buf()),
            None => (fixture.join("changed.json"), fixture.join("coverage.json")),
        };
        let tools = tools();
        let mut args = vec![
            "mutate",
            "--dir",
            tree.to_str().unwrap(),
            "--changed",
            changed.to_str().unwrap(),
            "--coverage",
            coverage.to_str().unwrap(),
            "--tools",
            tools.to_str().unwrap(),
            "--timeout",
            "5m",
            "--jobs",
            "2",
            "--min-mutant-timeout",
            "3s",
            "--offline",
        ];
        args.extend(extra);
        let (code, stdout, stderr) = cli(&args);
        assert_eq!(code, 0, "{stderr}");
        assert!(stdout.ends_with('\n') && stdout.trim().lines().count() == 1);
        let document: Value = serde_json::from_str(stdout.trim()).unwrap();
        let schema = testing::schema("mutation.schema.json");
        assert_eq!(
            testing::validate(&document, &schema, &schema),
            Vec::<String>::new()
        );
        document
    }

    fn find<'d>(document: &'d Value, line: u64, original: &str, mutated: &str) -> &'d Value {
        document["mutants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|mutant| {
                mutant["line"] == line
                    && mutant["original"] == original
                    && mutant["mutated"] == mutated
            })
            .unwrap_or_else(|| panic!("no mutant {original} -> {mutated} on line {line}"))
    }

    #[test]
    #[ignore = "installs cargo-mutants (network, about 40 s the first time) and runs it"]
    fn mutants_are_killed_survive_time_out_and_are_kept() {
        let (folder, tree) = TempDir::with_fixture("mutation");
        let cache_dir = folder.0.join("cache");
        let progress = folder.0.join("progress.json");
        let document = mutate_document(
            &tree,
            None,
            &[
                "--cache-dir",
                cache_dir.to_str().unwrap(),
                "--progress",
                progress.to_str().unwrap(),
            ],
        );
        assert_eq!(document["engine"]["name"], "cargo-mutants");
        assert_eq!(document["engine"]["version"], engine::VERSION);
        assert_eq!(document["diagnostics"], serde_json::json!([]));
        // The boundary no test tries.
        let survivor = find(&document, 11, ">", ">=");
        assert_eq!(
            (
                &survivor["status"],
                &survivor["operator"],
                &survivor["column"],
                &survivor["endColumn"],
                &survivor["function"],
                &survivor["killedBy"],
                &survivor["description"],
            ),
            (
                &serde_json::json!("survived"),
                &serde_json::json!("relational_boundary"),
                &serde_json::json!(19),
                &serde_json::json!(20),
                &serde_json::json!("mutation::Account::withdraw"),
                &Value::Null,
                &serde_json::json!("replace > with >= in Account::withdraw"),
            )
        );
        // Every mutant of `add` is caught, by its test.
        let adds: Vec<&Value> = document["mutants"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|mutant| mutant["function"] == "mutation::add")
            .collect();
        assert_eq!(adds.len(), 5);
        for mutant in adds {
            assert_eq!(
                (&mutant["status"], &mutant["killedBy"]),
                (
                    &serde_json::json!("killed"),
                    &serde_json::json!("tests::adds")
                ),
                "{mutant}"
            );
        }
        let body = find(&document, 21, "a + b", "0");
        assert_eq!(
            (&body["operator"], &body["endLine"], &body["endColumn"]),
            (
                &serde_json::json!("return_value"),
                &serde_json::json!(21),
                &serde_json::json!(10)
            )
        );
        // `i += 1` into `i *= 1`: the loop never ends.
        let endless = find(&document, 28, "+=", "*=");
        assert_eq!(endless["status"], "timeout");
        // `untested` (lines 34 to 36) never ran: no mutant.
        assert!(document["mutants"]
            .as_array()
            .unwrap()
            .iter()
            .all(|mutant| mutant["line"].as_u64().unwrap() < 34));
        let summary = &document["summary"];
        assert_eq!(
            (
                &summary["candidates"],
                &summary["sampled"],
                &summary["killed"],
                &summary["survived"],
                &summary["timeout"],
                &summary["error"],
                &summary["cached"],
            ),
            (
                &serde_json::json!(21),
                &serde_json::json!(false),
                &serde_json::json!(19),
                &serde_json::json!(1),
                &serde_json::json!(1),
                &serde_json::json!(0),
                &serde_json::json!(0),
            )
        );
        let lines: Vec<(u64, u64)> = document["mutants"]
            .as_array()
            .unwrap()
            .iter()
            .map(|mutant| {
                (
                    mutant["line"].as_u64().unwrap(),
                    mutant["column"].as_u64().unwrap(),
                )
            })
            .collect();
        assert!(
            lines.windows(2).all(|pair| pair[0] <= pair[1]),
            "rule 7: {lines:?}"
        );
        let written: Value = serde_json::from_slice(&std::fs::read(&progress).unwrap()).unwrap();
        let schema = testing::schema("mutation.schema.json");
        assert_eq!(
            testing::validate(&written, &schema, &schema["$defs"]["progress"]),
            Vec::<String>::new()
        );
        assert_eq!(
            (
                &written["phase"],
                &written["total"],
                &written["done"],
                &written["survived"]
            ),
            (
                &serde_json::json!("mutating"),
                &serde_json::json!(21),
                &serde_json::json!(21),
                &serde_json::json!(1)
            )
        );

        // Another copy, `steps` changed: its mutants are tested again, the
        // others reused.
        let (_second_folder, second) = TempDir::with_fixture("mutation");
        let source = second.join("src/lib.rs");
        let text = std::fs::read_to_string(&source).unwrap();
        std::fs::write(&source, text.replace("    i * 2\n", "    2 * i\n")).unwrap();
        let again = mutate_document(&second, None, &["--cache-dir", cache_dir.to_str().unwrap()]);
        for mutant in again["mutants"].as_array().unwrap() {
            let reused = mutant["function"] != "mutation::steps";
            assert_eq!(mutant["cached"], reused, "{mutant}");
            if reused {
                let first = find(
                    &document,
                    mutant["line"].as_u64().unwrap(),
                    mutant["original"].as_str().unwrap(),
                    mutant["mutated"].as_str().unwrap(),
                );
                assert_eq!(
                    (&mutant["id"], &mutant["status"], &mutant["durationMs"]),
                    (&first["id"], &first["status"], &first["durationMs"])
                );
            }
        }
        assert_eq!(again["summary"]["cached"], 12);
        assert_eq!(find(&again, 28, "+=", "*=")["status"], "timeout");
        assert_eq!(find(&again, 30, "*", "+")["status"], "killed");
    }

    #[test]
    #[ignore = "installs cargo-mutants (network, about 40 s the first time) and runs it"]
    fn over_the_cap_functions_are_sampled_in_turn() {
        let (_folder, tree) = TempDir::with_fixture("mutation");
        let document = mutate_document(&tree, None, &["--max-mutants", "4"]);
        let summary = &document["summary"];
        assert_eq!(
            (
                &summary["candidates"],
                &summary["sampled"],
                &summary["maxMutants"]
            ),
            (
                &serde_json::json!(21),
                &serde_json::json!(true),
                &serde_json::json!(4)
            )
        );
        let taken: Vec<(&str, &str)> = document["mutants"]
            .as_array()
            .unwrap()
            .iter()
            .map(|mutant| {
                (
                    mutant["function"].as_str().unwrap(),
                    mutant["description"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            taken,
            [
                (
                    "mutation::Account::withdraw",
                    "replace Account::withdraw -> bool with false"
                ),
                (
                    "mutation::Account::withdraw",
                    "replace Account::withdraw -> bool with true"
                ),
                ("mutation::add", "replace add -> i64 with -1"),
                ("mutation::steps", "replace steps -> u32 with 0"),
            ]
        );
        assert!(document["mutants"]
            .as_array()
            .unwrap()
            .iter()
            .all(|mutant| mutant["status"] == "killed"));
    }

    #[test]
    #[ignore = "installs cargo-mutants (network, about 40 s the first time) and runs it"]
    fn tests_failing_without_mutants_make_every_mutant_an_error() {
        let folder = TempDir::new();
        let tree = folder.0.join("tree");
        testing::write_crate(
            &tree,
            "failing-baseline",
            &[(
                "src/lib.rs",
                "pub fn double(x: i32) -> i32 {\n    x * 2\n}\n\n#[test]\nfn doubles() {\n    assert_eq!(double(2), 5);\n}\n",
            )],
        );
        let changed = folder.0.join("changed.json");
        std::fs::write(
            &changed,
            r#"{"protocol":1,"files":[{"path":"src/lib.rs","added":[[1,8]]}]}"#,
        )
        .unwrap();
        let coverage = folder.0.join("coverage.json");
        std::fs::write(
            &coverage,
            r#"{"protocol":1,"blocks":[{"file":"src/lib.rs","startLine":1,"endLine":3,"statements":2,"count":1}]}"#,
        )
        .unwrap();
        let document = mutate_document(&tree, Some((&changed, &coverage)), &[]);
        let mutants = document["mutants"].as_array().unwrap();
        assert_eq!(mutants.len(), 5, "{document}");
        assert!(mutants.iter().all(|mutant| mutant["status"] == "error"));
        assert_eq!(document["summary"]["error"], 5);
        let diagnostics = document["diagnostics"].as_array().unwrap();
        assert!(
            diagnostics.iter().any(|message| message
                .as_str()
                .unwrap()
                .contains("do not pass on the unmutated code")),
            "{diagnostics:?}"
        );
    }
}
