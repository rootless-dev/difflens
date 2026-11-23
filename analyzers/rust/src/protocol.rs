//! The documents of the analyzer protocol, version 1. The schemas are in
//! `analyzers/protocol/` of the DiffLens repository: `analysis.schema.json`
//! (analyze), `checks.schema.json` (check), `tests.schema.json` (prepare,
//! test, the `--selection` file and the `--progress` file),
//! `dupes.schema.json` (dupes and its `--changed` file) and
//! `mutation.schema.json` (mutate, its `--coverage` and `--progress`
//! files; its `--changed` file is dupes'). The core rejects
//! a document with a field the schema does not list, so every type here
//! serialises exactly the schema's fields, in camelCase.

// Every type mirrors its schema whole, so some parts are never built here
// (race reports: Rust's cargo test has no race detector).
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

pub const PROTOCOL: u32 = 1;
/// The language id the core detects Rust projects as.
pub const LANGUAGE: &str = "rust";
/// Identity reported by `--version` and in every document.
pub const NAME: &str = "difflens-analyzer-rust";
pub const VERSION: &str = "0.0.0";

#[derive(Debug, Clone, Serialize)]
pub struct Identity {
    pub name: String,
    pub version: String,
}

impl Identity {
    pub fn own() -> Self {
        Self {
            name: NAME.to_string(),
            version: VERSION.to_string(),
        }
    }
}

/// What `analyze` prints.
#[derive(Debug, Clone, Serialize)]
pub struct Analysis {
    pub protocol: u32,
    pub language: String,
    pub analyzer: Identity,
    pub functions: Vec<Function>,
    pub calls: Vec<Call>,
    pub externals: Vec<External>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Analysis {
    pub fn new() -> Self {
        Self {
            protocol: PROTOCOL,
            language: LANGUAGE.to_string(),
            analyzer: Identity::own(),
            functions: Vec::new(),
            calls: Vec::new(),
            externals: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Function {
    pub id: String,
    pub name: String,
    pub qualified_name: String,
    pub container: String,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub generated: bool,
    pub test: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complexity: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub entry_points: Vec<EntryPoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntryPoint {
    /// http_route, rpc_method, message_handler, scheduled_job, cli_command,
    /// main, test or exported_api.
    pub kind: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Call {
    pub caller: String,
    pub callee: String,
    pub line: u32,
    /// always, conditional or async.
    pub kind: String,
    /// static or dynamic.
    pub resolution: String,
    /// call, registration or reference.
    pub relation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct External {
    pub id: String,
    pub name: String,
    pub container: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    /// warning or error.
    pub level: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

impl Diagnostic {
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            level: "warning".to_string(),
            message: message.into(),
            file: None,
            line: None,
        }
    }
}

/// What `prepare` prints.
#[derive(Debug, Clone, Serialize)]
pub struct Prepare {
    pub protocol: u32,
    /// Absolute folders the test step writes to outside --dir.
    pub writable: Vec<String>,
}

/// What `test` prints.
#[derive(Debug, Clone, Serialize)]
pub struct TestRun {
    pub protocol: u32,
    pub language: String,
    pub runner: Identity,
    pub race: bool,
    pub tests: Vec<Test>,
    pub packages: Vec<Package>,
    pub coverage: Option<Coverage>,
    pub races: Vec<Race>,
    pub diagnostics: Vec<String>,
}

impl TestRun {
    pub fn new() -> Self {
        Self {
            protocol: PROTOCOL,
            language: LANGUAGE.to_string(),
            runner: Identity::own(),
            race: false,
            tests: Vec::new(),
            packages: Vec::new(),
            coverage: None,
            races: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Test {
    pub container: String,
    pub package: String,
    pub name: String,
    /// The levels `name` nests in, joined by "/" giving `name`: one, as
    /// Rust tests have no subtests.
    pub hierarchy: Vec<String>,
    pub function: Option<String>,
    pub file: Option<String>,
    pub line: Option<u32>,
    /// pass, fail or skip.
    pub outcome: String,
    pub duration_ms: u64,
    pub output: Option<String>,
    /// The shell command that runs this test alone, from the root of --dir.
    pub command: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Package {
    pub container: String,
    /// pass, fail, build_failed or no_tests.
    pub outcome: String,
    pub duration_ms: u64,
    pub output: Option<String>,
    /// The shell command that runs this package's tests alone, from the
    /// root of --dir.
    pub command: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Coverage {
    pub blocks: Vec<CoverageBlock>,
}

/// Also a block of the `--coverage` file of `mutate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoverageBlock {
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub statements: u32,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Race {
    pub container: String,
    pub test: Option<String>,
    pub output: String,
}

/// The `--progress` file.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub protocol: u32,
    pub packages_done: u32,
    pub packages_total: u32,
    pub tests: u32,
    pub failed: u32,
    pub current: Option<String>,
}

/// The `--selection` file: what a partial run runs.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub packages: Vec<SelectedPackage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedPackage {
    pub package: String,
    /// None: the whole package.
    pub tests: Option<Vec<SelectedTest>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedTest {
    pub name: String,
    /// None: the whole test.
    pub subtests: Option<Vec<String>>,
}

/// What `check` prints.
#[derive(Debug, Clone, Serialize)]
pub struct CheckRun {
    pub protocol: u32,
    pub language: String,
    pub runner: Identity,
    pub checks: Vec<Check>,
    pub diagnostics: Vec<String>,
}

impl CheckRun {
    pub fn new() -> Self {
        Self {
            protocol: PROTOCOL,
            language: LANGUAGE.to_string(),
            runner: Identity::own(),
            checks: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub tool: String,
    /// clean, findings, unavailable or failed.
    pub status: String,
    pub message: Option<String>,
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub file: String,
    pub line: u32,
    pub message: String,
    pub rule: Option<String>,
    /// error, warning or info.
    pub severity: String,
}

/// What `dupes` prints.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupesRun {
    pub protocol: u32,
    pub language: String,
    pub runner: Identity,
    pub min_tokens: usize,
    pub min_lines: u32,
    pub duplicates: Vec<Duplicate>,
    pub diagnostics: Vec<String>,
}

impl DupesRun {
    pub fn new(min_tokens: usize, min_lines: u32) -> Self {
        Self {
            protocol: PROTOCOL,
            language: LANGUAGE.to_string(),
            runner: Identity::own(),
            min_tokens,
            min_lines,
            duplicates: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Duplicate {
    /// copies_existing or repeated_in_change.
    pub kind: String,
    /// production or test: the scope the group was found in.
    pub scope: String,
    pub block: LineRange,
    pub others: Vec<LineRange>,
    pub tokens: usize,
    pub lines: u32,
}

/// Lines of a file, inclusive.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineRange {
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// The `--changed` file of `dupes` and `mutate`: the lines the review adds,
/// per file, and the files `dupes` leaves out (`mutate` ignores them).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Changed {
    pub protocol: u32,
    #[serde(default)]
    pub exclude: Option<Exclude>,
    pub files: Vec<ChangedFile>,
}

/// Files `dupes` leaves out: those with a folder named one of `dirs`, or a
/// name ending with one of `extensions`, both ignoring ASCII case.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exclude {
    pub dirs: Vec<String>,
    pub extensions: Vec<String>,
}

impl Exclude {
    /// Whether the file at `rel` (relative, "/" separators) is left out.
    pub fn excludes(&self, rel: &str) -> bool {
        let mut segments: Vec<&str> = rel.split('/').collect();
        let name = segments.pop().unwrap_or("");
        let folder_excluded = segments.iter().any(|segment| {
            self.dirs
                .iter()
                .any(|dir| segment.eq_ignore_ascii_case(dir))
        });
        let name = name.to_ascii_lowercase();
        folder_excluded
            || self
                .extensions
                .iter()
                .any(|extension| name.ends_with(&extension.to_ascii_lowercase()))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangedFile {
    pub path: String,
    /// Inclusive 1-based line ranges.
    pub added: Vec<[u32; 2]>,
}

/// What `mutate` prints (mutation.schema.json).
#[derive(Debug, Clone, Serialize)]
pub struct MutationRun {
    pub protocol: u32,
    pub language: String,
    pub runner: Identity,
    pub engine: Identity,
    pub mutants: Vec<Mutant>,
    pub summary: MutationSummary,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mutant {
    pub id: String,
    pub file: String,
    pub line: u32,
    /// 1-based, in Unicode code points.
    pub column: u32,
    pub end_line: u32,
    /// Just past the last character replaced.
    pub end_column: u32,
    pub original: String,
    pub mutated: String,
    /// relational_boundary, relational_negation, arithmetic, logical,
    /// negation_removal, return_value, call_removal, increment or other.
    pub operator: String,
    pub description: String,
    pub function: Option<String>,
    /// killed, survived, timeout, error or no_coverage.
    pub status: String,
    pub killed_by: Option<String>,
    pub duration_ms: u64,
    pub cached: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationSummary {
    pub candidates: usize,
    pub sampled: bool,
    pub max_mutants: usize,
    pub killed: usize,
    pub survived: usize,
    pub timeout: usize,
    pub error: usize,
    pub no_coverage: usize,
    pub cached: usize,
    pub duration_ms: u64,
}

/// The `--progress` file of `mutate`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MutationProgress {
    pub protocol: u32,
    /// baseline or mutating.
    pub phase: String,
    pub total: usize,
    pub done: usize,
    pub killed: usize,
    pub survived: usize,
    pub timeout: usize,
    /// Errors and no_coverage together.
    pub error: usize,
    pub current: Option<String>,
}

/// The `--coverage` file of `mutate`: the blocks of the last test run.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageFile {
    pub protocol: u32,
    pub blocks: Vec<CoverageBlock>,
}
