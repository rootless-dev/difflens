//! Types returned to the frontend. They mirror `src/lib/api.ts` field by field.

use serde::{Deserialize, Serialize};

use crate::error::{AppError, CoreText};
use crate::language::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisSupport {
    Full,
    DiffOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub display_path: String,
    pub languages: Vec<String>,
    pub analysis_support: AnalysisSupport,
    pub integration_branches: Vec<String>,
    pub available: bool,
    pub has_remote: bool,
    pub branches_ahead: u32,
    pub last_fetched_at: Option<String>,
    pub added_at: String,
    /// Committer date of the newest commit on a local branch or a branch of
    /// origin; `None` when the repository is unavailable or has no commit.
    pub last_modified_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub sha: String,
    pub subject: String,
    pub authored_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaseSource {
    History,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchBase {
    pub name: String,
    pub source: BaseSource,
    /// The ref the base resolves to, as the user would write it:
    /// `origin/develop`, or `develop` when origin has no such branch.
    pub ref_name: String,
    /// Commit the base resolves to, so the frontend can tell when it moved.
    pub sha: String,
}

/// Where the commit a review compares with comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewBaseKind {
    /// The destination commit of the branch's pull request, as the host
    /// reports it.
    PullRequest,
    /// The base branch of origin, such as `origin/develop`.
    RemoteTracking,
    /// The local base branch: origin has no branch of that name.
    Local,
}

/// The base a branch review was compared with: the commit its old side was
/// worked out from (as the merge base with the branch head), and where that
/// commit came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewBase {
    /// The integration branch, such as `develop`.
    pub name: String,
    pub kind: ReviewBaseKind,
    /// The ref it was read from, such as `origin/develop`; `None` for a pull
    /// request's destination commit.
    pub ref_name: Option<String>,
    /// The commit itself.
    pub sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Upstream {
    pub name: String,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchLocation {
    Local,
    RemoteOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub name: String,
    pub location: BranchLocation,
    pub is_current: bool,
    pub head: Commit,
    pub upstream: Option<Upstream>,
    pub card: Option<String>,
    pub base: Option<BranchBase>,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingTree {
    pub dirty: bool,
    pub files_changed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetail {
    pub project: Project,
    pub branches: Vec<Branch>,
    pub bases: Vec<String>,
    pub working_tree: WorkingTree,
    /// Every worktree of the repository, the project folder first.
    pub worktrees: Vec<Worktree>,
}

/// What a folder a terminal can open in is to the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FolderKind {
    /// The project folder itself: commands there change the user's checkout.
    Project,
    /// A worktree DiffLens created under the project's `.difflens/worktrees/`.
    Difflens,
    /// A worktree the user created elsewhere.
    Worktree,
}

/// A folder of the repository's checkouts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    /// Absolute and canonical.
    pub path: String,
    /// The same path with the home folder written as `~`.
    pub display_path: String,
    pub kind: FolderKind,
    /// Inside the project, with `/` separators (`.difflens/worktrees/<branch>`),
    /// for a DiffLens worktree; `None` otherwise.
    pub relative_path: Option<String>,
}

/// A worktree of the repository, as the project screen lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub folder: Folder,
    /// The local branch checked out there; `None` when detached.
    pub branch: Option<String>,
    /// RFC 3339 time its folder was created; DiffLens worktrees only.
    pub created_at: Option<String>,
    /// Files with uncommitted changes (untracked ones included); DiffLens
    /// worktrees only, `None` when git could not tell.
    pub changed_files: Option<u32>,
}

/// What creating a worktree for a branch checked out nowhere would do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreePlan {
    /// `.difflens/worktrees/<branch>`, inside the project.
    pub relative_path: String,
    /// The remote-tracking branch a new local branch would follow, such as
    /// `origin/feature/x`, when the branch only exists on the remote.
    pub upstream: Option<String>,
}

/// Where the terminals of a screen open: a folder, or for a branch checked out
/// nowhere, what creating its worktree would do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalLocation {
    pub folder: Option<Folder>,
    /// The review's branch, or the one checked out in the project folder
    /// (`None` when its HEAD is detached).
    pub branch: Option<String>,
    /// Set exactly when `folder` is `None`.
    pub plan: Option<WorktreePlan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalKind {
    /// The user's login shell.
    Shell,
    /// Claude Code, started through the login shell.
    Claude,
}

/// One terminal session of the core, alive until closed or until the app
/// quits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSession {
    pub id: String,
    pub kind: TerminalKind,
    /// "zsh", "zsh 2", "Claude Code"...: unique per folder and kind.
    pub name: String,
    /// The folder it runs in, absolute and canonical.
    pub cwd: String,
    pub project_id: String,
    pub project_name: String,
    /// The review it was opened from; `None` for the project screen.
    pub target: Option<ReviewTarget>,
    /// The branch of its location when it was opened.
    pub branch: Option<String>,
    /// RFC 3339.
    pub opened_at: String,
    /// The command line of the process in the foreground other than the
    /// shell itself (empty when it could not be read); `None` when idle.
    pub foreground: Option<String>,
    pub ended: bool,
    /// How its process exited, when it did.
    pub exit_code: Option<i32>,
}

/// What a session's channel carries besides its output, which goes as raw
/// bytes. Serialised as `{ "type": "foreground", "command" }` or
/// `{ "type": "exited", "code" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalEvent {
    Foreground { command: Option<String> },
    Exited { code: Option<i32> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareSummary {
    pub branch: String,
    pub base: String,
    pub head_sha: String,
    pub merge_base_sha: String,
    pub commits: u32,
    pub files_changed: u32,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingTreeSummary {
    pub head_sha: String,
    pub files_changed: u32,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchResult {
    pub fetched_at: String,
    pub branches_updated: u32,
}

/// What a review looks at. Serialised as `{ "kind": "branch", "branch", "base" }`
/// or `{ "kind": "working_tree" }`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewTarget {
    Branch { branch: String, base: String },
    WorkingTree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
    pub language: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSet {
    pub target: ReviewTarget,
    pub old_sha: String,
    pub new_sha: Option<String>,
    /// What a branch was compared with; `None` for the working tree.
    pub base: Option<ReviewBase>,
    pub files: Vec<ChangedFile>,
}

/// Why one side of a file has no text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SideReason {
    Absent,
    Binary,
    TooLarge,
    NotUtf8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSide {
    pub text: Option<String>,
    pub reason: Option<SideReason>,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileVersions {
    pub old: FileSide,
    pub new: FileSide,
}

/// Shared with the analyzer protocol, which uses the same spellings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryPointKind {
    HttpRoute,
    RpcMethod,
    MessageHandler,
    ScheduledJob,
    CliCommand,
    Main,
    Test,
    ExportedApi,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryPoint {
    pub kind: EntryPointKind,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallKind {
    Always,
    Conditional,
    Async,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallResolution {
    Static,
    Dynamic,
}

/// What a link between two functions is, independent of whether the review
/// added, kept or removed it. Shared with the analyzer protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    /// The caller invokes the callee.
    Call,
    /// The caller hands the callee to a mechanism that invokes it later
    /// (a route, a middleware…), named by the evidence.
    Registration,
    /// The caller passes the callee as a value with no known meaning.
    Reference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowNodeState {
    New,
    Modified,
    Deleted,
    Untouched,
    External,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowNode {
    pub id: String,
    pub name: String,
    pub qualified_name: String,
    pub container: String,
    pub state: FlowNodeState,
    pub file: Option<String>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub additions: u32,
    pub deletions: u32,
    pub complexity: Option<u32>,
    pub previous_complexity: Option<u32>,
    pub generated: bool,
    pub test: bool,
    pub entry_points: Vec<EntryPoint>,
    /// Direct calls of a changed function, drawn or not; empty for others.
    pub calls: Vec<FlowCall>,
    /// For an external package node: distinct functions of it called from
    /// the graph. 0 otherwise.
    pub external_functions: u32,
}

/// One direct call made by a node, whether or not the callee is drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowCall {
    pub callee_id: String,
    pub qualified_name: String,
    pub line: u32,
    pub kind: CallKind,
    pub external: bool,
    pub shown: bool,
    pub added: bool,
    pub removed: bool,
    pub relation: Relation,
    /// For a registration, what it is, such as "route POST /refunds".
    pub evidence: Option<String>,
}

/// What the graph is built with; all off by default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowOptions {
    /// Test functions count as changed code and as callers.
    pub include_tests: bool,
    /// The unchanged callers on the paths from entry points down to the
    /// changed functions are drawn.
    pub include_callers: bool,
    /// The direct callees of the changed functions are drawn.
    pub include_callees: bool,
}

/// What the options leave out, counted whether or not they are on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowHidden {
    pub changed_tests: u32,
    pub callers: u32,
    pub callees: u32,
    pub external_packages: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowEdge {
    pub from: String,
    pub to: String,
    pub position: u32,
    pub line: u32,
    pub kind: CallKind,
    pub resolution: CallResolution,
    pub added: bool,
    pub removed: bool,
    pub relation: Relation,
    /// For a registration, what it is; otherwise `None`.
    pub evidence: Option<String>,
}

/// Also read from analyzer output, where unknown fields are rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnalyzerIdentity {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowGraph {
    pub language: String,
    pub analyzer: AnalyzerIdentity,
    pub old_sha: String,
    pub new_sha: Option<String>,
    pub options: FlowOptions,
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
    pub hidden: FlowHidden,
    pub entry_points: Vec<String>,
    pub truncated: bool,
    /// The analyzers' own words, as they gave them, and why an analyzer of
    /// another of the project's languages could not be used (worded by the
    /// core).
    pub diagnostics: Vec<CoreText>,
    pub analyzed_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewSide {
    Old,
    New,
}

/// Serialised as `{ "reason": "no_analyzer", "languages" }`,
/// `{ "reason": "analysis_failed", "side", "message" }` or
/// `{ "reason": "analyzer_unavailable", "analyzer", "message" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum FlowUnavailable {
    NoAnalyzer { languages: Vec<String> },
    /// `message` is worded by the core (see `CoreText`).
    AnalysisFailed { side: ReviewSide, message: CoreText },
    /// A provisioned analyzer could not be prepared (a missing toolchain, a
    /// failed build) or is missing from the installation. `analyzer` is its
    /// language as shown, such as "Python".
    AnalyzerUnavailable { analyzer: String, message: CoreText },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvisionStatus {
    Preparing,
    Ready,
    Failed,
}

/// What a build of an analyzer's environment is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvisionStep {
    /// Copying its source, creating a virtual environment.
    CreatingEnvironment,
    /// pip, uv or npm.
    InstallingDependencies,
    /// cargo.
    Compiling,
}

/// A change of the build of an analyzer's environment (`analyzer_envs`),
/// emitted as the `analyzer-provision` event and listed while it goes on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzerProvision {
    /// The analyzer's id, such as "python" or "jsts".
    pub analyzer: String,
    /// Its language as shown, such as "Python".
    pub language: String,
    pub status: ProvisionStatus,
    /// While preparing, once the first step started.
    pub step: Option<ProvisionStep>,
    /// Why it failed, worded by the core.
    pub message: Option<CoreText>,
}

/// Serialised as `{ "kind": "graph", "graph" }` or `{ "kind": "unavailable",
/// "reason", ... }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FlowResult {
    Graph { graph: Box<FlowGraph> },
    Unavailable(FlowUnavailable),
}

/// Where a step that executes project code runs: a plain child process, or
/// the macOS sandbox (see `sandbox`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    Process,
    #[default]
    Sandbox,
}

/// Shared with the test runner protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestOutcome {
    Pass,
    Fail,
    Skip,
}

/// A changed function, as tests and quality refer to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionRef {
    pub id: String,
    pub qualified_name: String,
}

/// State in the review of the test function that defines a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TestFunctionState {
    New,
    Modified,
    Untouched,
}

/// A place in the project's code, relative to the tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRef {
    pub file: String,
    pub line: u32,
}

/// A frame of a stack in the project's own code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureFrame {
    /// Without its package path, such as "(*Service).recordLastLogin".
    pub function: String,
    pub file: String,
    pub line: u32,
    /// The file is test code.
    pub test: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RaceAccessKind {
    Read,
    Write,
}

/// One side of a data race.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RaceAccess {
    pub kind: RaceAccessKind,
    /// The earlier of the two accesses ("Previous write at …").
    pub previous: bool,
    /// None for the main goroutine.
    pub goroutine: Option<u32>,
    /// The first frame of the access in the project's code.
    pub frame: Option<FailureFrame>,
    /// Where its goroutine was created, first frame in the project's code;
    /// None when the test framework created it (the test's own goroutine)
    /// or the report does not say.
    pub created_at: Option<FailureFrame>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RaceSummary {
    pub accesses: Vec<RaceAccess>,
}

/// One error of a build that failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildError {
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub message: String,
    /// The lines that follow it, indented by the compiler.
    pub detail: Vec<String>,
}

/// Why a test or a package failed, read from its output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TestFailure {
    Race {
        races: Vec<RaceSummary>,
    },
    #[serde(rename_all = "camelCase")]
    Assertion {
        message: String,
        expected: Option<String>,
        actual: Option<String>,
        at: Option<SourceRef>,
        /// Full name of the (sub)test whose output said it.
        test: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Panic {
        message: String,
        frame: Option<FailureFrame>,
        test: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Timeout {
        message: String,
        frame: Option<FailureFrame>,
        /// The tests still running when the time ran out.
        running: Vec<String>,
    },
    Build {
        errors: Vec<BuildError>,
    },
    /// Nothing recognised: the last meaningful lines.
    Other {
        lines: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestCase {
    pub id: String,
    pub container: String,
    /// Container of the package it ran in, as `packages` lists it: what a
    /// selection names.
    pub package: String,
    pub name: String,
    /// Where `name` nests, from the top-level test down to this one; joined
    /// with "/" they give `name`.
    pub hierarchy: Vec<String>,
    pub title: String,
    /// What the runner says runs this test again on its own, from the root
    /// of the project.
    pub command: Option<String>,
    pub function_id: Option<String>,
    pub state: TestFunctionState,
    pub outcome: TestOutcome,
    pub duration_ms: u64,
    pub output: Option<String>,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub exercises: Option<FunctionRef>,
    pub in_changed_package: bool,
    /// For a failed test, why, read from its output.
    pub failure: Option<TestFailure>,
}

/// Shared with the test runner protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageOutcomeKind {
    Pass,
    Fail,
    BuildFailed,
    NoTests,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageOutcome {
    pub container: String,
    pub outcome: PackageOutcomeKind,
    pub duration_ms: u64,
    pub output: Option<String>,
    /// For a package with output (it did not build, or failed outside any
    /// test), why, read from that output.
    pub failure: Option<TestFailure>,
    /// What the runner says runs this package's tests again, from the root
    /// of the project.
    pub command: Option<String>,
}

/// Lines that hold statements (coverable) and, of those, the ones a test
/// ran (covered).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineCoverage {
    pub covered: u32,
    pub coverable: u32,
}

impl LineCoverage {
    /// Covered share from 0 to 1, `None` without coverable lines.
    pub fn ratio(self) -> Option<f64> {
        (self.coverable > 0).then(|| f64::from(self.covered) / f64::from(self.coverable))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCoverage {
    pub path: String,
    pub covered: u32,
    pub coverable: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCoverage {
    pub changed_lines: LineCoverage,
    pub files: Vec<FileCoverage>,
    pub project: LineCoverage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestRunStatus {
    Completed,
    Failed,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestSummary {
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub added: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RaceReport {
    pub container: String,
    pub test: Option<String>,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRun {
    pub language: String,
    pub runner: AnalyzerIdentity,
    pub old_sha: String,
    pub new_sha: Option<String>,
    pub isolation: Isolation,
    pub started_at: String,
    pub duration_ms: u64,
    pub status: TestRunStatus,
    /// Why a failed run failed: the runner's own output, or the core's words.
    pub message: Option<CoreText>,
    pub tests: Vec<TestCase>,
    pub summary: TestSummary,
    pub packages: Vec<PackageOutcome>,
    pub coverage: Option<RunCoverage>,
    pub race_detector: bool,
    pub races: Vec<RaceReport>,
    /// The runner's own (text without a reason), then the core's.
    pub diagnostics: Vec<CoreText>,
    /// What the last run asked for, partial or not.
    pub options: RunOptions,
    /// The last partial run of this new side, merged into this one when it
    /// completed; None when the last run ran everything.
    pub partial: Option<PartialRun>,
}

/// What a run measures besides the tests' outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOptions {
    pub race: bool,
    pub coverage: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            race: true,
            coverage: true,
        }
    }
}

/// What a partial run runs: packages by the container `packages` names them
/// with (tests.schema.json, `$defs/selection`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunSelection {
    pub packages: Vec<SelectedPackage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectedPackage {
    pub package: String,
    /// None for the whole package.
    pub tests: Option<Vec<SelectedTest>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectedTest {
    /// Top-level name.
    pub name: String,
    /// Relative to the test; None for the whole test.
    pub subtests: Option<Vec<String>>,
}

/// A partial run: when, how it ended and what it covered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartialRun {
    /// RFC 3339.
    pub started_at: String,
    pub duration_ms: u64,
    pub status: TestRunStatus,
    pub isolation: Isolation,
    /// Packages the selection named.
    pub packages: u32,
    /// Top-level tests it reported.
    pub tests: u32,
}

/// How far the run going on got, as the runner last wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestProgress {
    pub packages_done: u32,
    pub packages_total: u32,
    pub tests: u32,
    pub failed: u32,
    pub current: Option<String>,
}

/// Serialised as `{ "reason": "no_runner", "languages" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum TestsUnavailable {
    NoRunner { languages: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestsState {
    pub run: Option<TestRun>,
    pub running: bool,
    /// What the run going on runs; None when it runs everything or none runs.
    pub running_selection: Option<RunSelection>,
    pub isolation: Isolation,
    pub unavailable: Option<TestsUnavailable>,
}

/// The family of change a mutant makes (`mutation.schema.json`, rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationOperator {
    RelationalBoundary,
    RelationalNegation,
    Arithmetic,
    Logical,
    NegationRemoval,
    ReturnValue,
    CallRemoval,
    Increment,
    Other,
}

/// What became of a mutant when its tests ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutantStatus {
    /// A test failed: the tests verify this piece.
    Killed,
    /// Every test passed: they execute it without verifying it.
    Survived,
    /// Stopped after its time limit, most likely an infinite loop.
    Timeout,
    /// It could not be tested: it does not compile, its tests fail without it.
    Error,
    /// The engine found that no test executes it.
    NoCoverage,
}

/// One mutant of a line the review adds, with the lines it spans on the new
/// side as they are (`before`) and as mutated (`after`); both empty when
/// the file could not be read or does not hold `original` where the
/// analyzer said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mutant {
    /// Unique in the run; the key of its signal (`mutant:<id>`).
    pub id: String,
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub original: String,
    pub mutated: String,
    pub operator: MutationOperator,
    /// The engine's own words for it, in English.
    pub description: String,
    pub status: MutantStatus,
    /// For a killed one, a test that failed, when the engine tells.
    pub killed_by: Option<String>,
    pub duration_ms: u64,
    /// Its result was reused from an earlier run.
    pub cached: bool,
    /// The function of the new side holding it.
    pub function: Option<FunctionRef>,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationSummary {
    /// Mutants found on the lines tried, before the cap.
    pub candidates: u32,
    /// Only `max_mutants` of the candidates were tested, sampled evenly
    /// across the changed functions.
    pub sampled: bool,
    pub max_mutants: u32,
    pub killed: u32,
    pub survived: u32,
    pub timeout: u32,
    pub error: u32,
    pub no_coverage: u32,
    pub cached: u32,
}

/// A mutation testing run of a review: the mutants of the lines it adds that
/// the last test run with coverage executed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationRun {
    pub language: String,
    pub runner: AnalyzerIdentity,
    /// What made and ran the mutants; None when the run did not report.
    pub engine: Option<AnalyzerIdentity>,
    pub old_sha: String,
    pub new_sha: Option<String>,
    pub isolation: Isolation,
    pub started_at: String,
    pub duration_ms: u64,
    pub status: TestRunStatus,
    /// Why a failed run failed: the analyzer's own output, or the core's words.
    pub message: Option<CoreText>,
    /// When the test run whose coverage chose the lines started.
    pub coverage_run_at: Option<String>,
    /// By file, line and column.
    pub mutants: Vec<Mutant>,
    pub summary: MutationSummary,
    /// The analyzer's own (text without a reason), then the core's.
    pub diagnostics: Vec<CoreText>,
}

/// How far the mutation run going on got, as the analyzer last wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationPhase {
    /// The tests run on the unmutated code first.
    Baseline,
    Mutating,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationProgress {
    pub phase: MutationPhase,
    pub total: u32,
    pub done: u32,
    pub killed: u32,
    pub survived: u32,
    pub timeout: u32,
    /// Errors and mutants no test executes.
    pub error: u32,
    /// "file:line" of a mutant being tested now.
    pub current: Option<String>,
}

/// Serialised as `{ "reason": "no_runner", "languages" }` or
/// `{ "reason": "needs_coverage" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum MutationUnavailable {
    /// No analyzer of the project's languages runs tests.
    NoRunner { languages: Vec<String> },
    /// There is no completed test run with coverage of the new side yet: it
    /// tells which changed lines the tests execute.
    NeedsCoverage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationState {
    /// The last completed run of these sides.
    pub run: Option<MutationRun>,
    pub running: bool,
    /// Why mutation testing cannot run now.
    pub unavailable: Option<MutationUnavailable>,
    /// The most mutants one run tests.
    pub max_mutants: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityLimits {
    pub complexity: u32,
    pub crap: f64,
    pub changed_lines_coverage: f64,
    /// The size a copy must reach to count as duplicated code: this many
    /// normalized tokens and this many lines, both.
    pub duplication_tokens: u32,
    pub duplication_lines: u32,
}

impl Default for QualityLimits {
    fn default() -> Self {
        Self {
            complexity: 10,
            crap: 30.0,
            changed_lines_coverage: 0.8,
            duplication_tokens: 50,
            duplication_lines: 6,
        }
    }
}

/// State of a changed production function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangedState {
    New,
    Modified,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityFunction {
    pub id: String,
    pub qualified_name: String,
    pub name: String,
    pub container: String,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub state: ChangedState,
    pub complexity: Option<u32>,
    pub previous_complexity: Option<u32>,
    pub changed_lines: Option<LineCoverage>,
    pub crap: Option<f64>,
    /// The run used measured this function's lines and none of them ran:
    /// no test executes it. False without a run that measured coverage.
    pub untested: bool,
    /// The function's lines on the new side, for a function that crosses a
    /// limit (to hand to a coding agent), at most
    /// `quality::MAX_FUNCTION_LINES`; empty for the others and when the
    /// file could not be read.
    pub code: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitKind {
    Complexity,
    Crap,
    ChangedLinesCoverage,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrossedLimit {
    pub kind: LimitKind,
    pub function: Option<FunctionRef>,
    pub value: f64,
    pub limit: f64,
}

/// Shared with the checks protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Clean,
    Findings,
    Unavailable,
    Failed,
}

/// How serious a checker says a finding is, mapped from its own levels.
/// Shared with the checks protocol.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckSeverity {
    Error,
    /// Also what a document kept from before severities were reported reads
    /// as.
    #[default]
    Warning,
    Info,
}

/// Shared with the checks protocol, which closes the object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckFinding {
    pub file: String,
    pub line: u32,
    pub message: String,
    pub rule: Option<String>,
    #[serde(default)]
    pub severity: CheckSeverity,
}

/// Shared with the checks protocol, which closes the object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StaticCheck {
    pub tool: String,
    pub status: CheckStatus,
    pub message: Option<String>,
    pub findings: Vec<CheckFinding>,
}

/// What a static check finding most likely is: a defect (go vet, and
/// staticcheck's SA checks, among others) or a matter of style (staticcheck's
/// S, ST, QF and U checks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckClass {
    Correctness,
    Style,
}

/// A static check finding on a line the review adds, with the changed
/// production function holding that line, when one does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocatedCheck {
    pub tool: String,
    pub rule: Option<String>,
    pub file: String,
    pub line: u32,
    /// The tool's own words, never translated.
    pub message: String,
    pub severity: CheckSeverity,
    pub class: CheckClass,
    pub function: Option<FunctionRef>,
}

/// How code the review adds is duplicated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateKind {
    /// Identical, or identical but for identifiers and literals, to code the
    /// review did not add.
    CopiesExisting,
    /// The same block two or more times in the code the review adds.
    RepeatedInChange,
}

/// Which code a duplicate was found in (dupes.schema.json, step 2): the
/// analyzers search production code and test code apart, so a copy never
/// pairs a test with production code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateScope {
    Production,
    Test,
}

/// One occurrence of duplicated code on the new side, with its lines as they
/// are there (at most `duplication::MAX_CODE_LINES` of them; empty when the
/// file could not be read).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateBlock {
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub code: Vec<String>,
}

/// Duplicated code the review introduces: `block`, on lines it adds, and
/// `others`, the existing code it copies or the other times it is repeated
/// in the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Duplicate {
    pub kind: DuplicateKind,
    pub block: DuplicateBlock,
    pub others: Vec<DuplicateBlock>,
    /// Normalized tokens the copies share.
    pub tokens: u32,
    /// Lines of `block`.
    pub lines: u32,
    /// The changed production function holding the block, when one does.
    #[serde(default)]
    pub function: Option<FunctionRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RaceDetectorState {
    NotRun,
    Clean,
    Races,
}

/// Where the coverage and CRAP of the Quality tab's functions come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageState {
    /// The run used measured coverage.
    Measured,
    /// No completed run of the new side.
    NoRun,
    /// The run used was asked not to measure coverage.
    Off,
    /// The run used was asked to, and measured none; `reasons` says why.
    NotMeasured,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityCoverage {
    pub state: CoverageState,
    /// For `not_measured`: the analyzer's own diagnostics about coverage
    /// ("coverage was not measured: coverage.py is not installed ..."),
    /// shown as they are.
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityReport {
    pub language: String,
    pub old_sha: String,
    pub new_sha: Option<String>,
    pub limits: QualityLimits,
    pub functions: Vec<QualityFunction>,
    pub crossed: Vec<CrossedLimit>,
    pub checks: Vec<StaticCheck>,
    /// The findings of `checks` on lines the review adds, by file and line.
    pub check_findings: Vec<LocatedCheck>,
    /// Duplicated production code the review introduces, by the block's
    /// file and line: tests and translation catalogs are never in it, as
    /// copies or as originals.
    pub duplicates: Vec<Duplicate>,
    /// Duplicated test code the review introduces, between test code only,
    /// by the block's file and line.
    pub test_duplicates: Vec<Duplicate>,
    pub race_detector: RaceDetectorState,
    pub test_run_at: Option<String>,
    /// Whether the run used measured coverage, and why not.
    pub coverage: QualityCoverage,
    /// The checkers' own (text without a reason), and why a checker run
    /// failed, in the core's words.
    pub diagnostics: Vec<CoreText>,
}

/// Serialised as `{ "kind": "report", "report" }` or `{ "kind":
/// "unavailable", "reason", ... }`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QualityResult {
    Report { report: Box<QualityReport> },
    Unavailable(FlowUnavailable),
}

/// A review as the screen has it open: what it compares, and the two sides
/// its change set froze. Marks and comments are taken against those sides,
/// not against whatever the branch points at by then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRef {
    pub target: ReviewTarget,
    pub old_sha: String,
    /// `None` for the working tree.
    pub new_sha: Option<String>,
}

/// Where a changed function is, as the flow graph has it: on the new side,
/// or on the old side for a deleted function.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionSpan {
    pub id: String,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub deleted: bool,
}

/// State of something marked as reviewed: its content is still what was
/// reviewed, or it changed since (reopened).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkStatus {
    Reviewed,
    Reopened,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkedFile {
    pub path: String,
    pub status: MarkStatus,
    pub reviewed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkedFunction {
    pub id: String,
    pub status: MarkStatus,
    pub reviewed_at: String,
}

/// Kept only in DiffLens (draft), or also on the pull request (published).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentState {
    Draft,
    Published,
}

/// Where a comment is: on a line of a file of the diff, or on the pull
/// request as a whole (a general comment, with no file or line: its `path`
/// and `line_text` are empty, its `line` 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentKind {
    Line,
    General,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewComment {
    pub id: String,
    pub kind: CommentKind,
    /// The thread's first comment, for a reply.
    pub parent_id: Option<String>,
    pub path: String,
    pub side: ReviewSide,
    /// Line it was written on.
    pub line: u32,
    /// Text of that line then.
    pub line_text: String,
    pub commit_sha: Option<String>,
    pub body: String,
    pub state: CommentState,
    pub remote_id: Option<String>,
    pub remote_author_id: Option<String>,
    pub remote_author_name: Option<String>,
    pub resolved: bool,
    pub created_at: String,
    pub updated_at: String,
    /// Line to show it under now; `None` when its file or side is no longer
    /// part of the review, and for a general comment.
    pub current_line: Option<u32>,
    /// The line it was written on no longer reads as it did.
    pub outdated: bool,
    /// Written by the user: here, or on the pull request by the connected
    /// account. Only these can be edited and deleted.
    pub mine: bool,
    /// A change to a published comment that waits for "Publish comments".
    pub pending_change: Option<PendingChange>,
    /// Why the last attempt to publish it failed.
    pub publish_error: Option<AppError>,
}

/// What publishing will do to a published comment changed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingChange {
    Edit,
    Delete,
}

/// The most recent review action: the head it was taken at (`None` for the
/// working tree) and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub sha: Option<String>,
    pub at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SinceUnavailableReason {
    /// The working tree has no commits to compare.
    WorkingTree,
    /// Nothing was marked as reviewed yet.
    NoCheckpoint,
    /// The head is still the checkpoint.
    UpToDate,
    /// The checkpoint commit is gone from the repository (history rewritten
    /// and collected).
    CheckpointMissing,
}

/// What changed from the checkpoint to the head. Serialised as `{ "kind":
/// "available", "checkpointSha", "commits", "files" }` or `{ "kind":
/// "unavailable", "reason" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SinceLastReview {
    Available {
        checkpoint_sha: String,
        commits: u32,
        files: Vec<ChangedFile>,
    },
    Unavailable {
        reason: SinceUnavailableReason,
    },
}

/// Counts for an overview of the review.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSummary {
    pub total_files: u32,
    pub reviewed_files: u32,
    pub reopened_files: u32,
    pub reopened_functions: u32,
    pub files_changed_since: u32,
    /// Unresolved threads whose line no longer reads as it did.
    pub outdated_comments: u32,
    /// Drafts, replies, edits and deletes "Publish comments" would send.
    pub unpublished: u32,
    /// Draft threads not sent because their line changed since they were
    /// written: their line number may point elsewhere on the pull request.
    pub stale_drafts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewState {
    /// Marked files that are in the change set, by path.
    pub files: Vec<MarkedFile>,
    /// Marked functions among the spans asked about.
    pub functions: Vec<MarkedFunction>,
    /// Oldest first, replies included.
    pub comments: Vec<ReviewComment>,
    pub checkpoint: Option<Checkpoint>,
    pub since: SinceLastReview,
    pub summary: ReviewSummary,
    /// The drafts written from a finding or a static check, while they are
    /// there (not deleted, nor deleted here waiting for a publish).
    pub signal_drafts: Vec<SignalDraft>,
}

/// A review comment written from a model finding or a static check finding
/// (`signal`, such as `finding:<id>`), linked to it until it is deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalDraft {
    pub signal: String,
    pub comment_id: String,
    pub kind: CommentKind,
}

/// An account on a pull request host. `id` is the host's stable id (a UUID
/// on Bitbucket, a number on GitHub), the one comments are attributed with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub name: String,
}

/// A pull request host DiffLens can connect to, and whether it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    /// Key of the host, such as "bitbucket".
    pub host: String,
    /// Such as "Bitbucket Cloud".
    pub host_name: String,
    /// Permissions the token needs, as the host names them.
    pub scopes: Vec<String>,
    /// Whether the user enters a user name besides the token (Bitbucket's
    /// account email); GitHub's token alone says who it is.
    pub needs_username: bool,
    /// The saved user name (the Atlassian account email for Bitbucket, the
    /// login for GitHub); `None` when not connected. The token never leaves
    /// the core.
    pub username: Option<String>,
    /// The account the saved credential belongs to.
    pub account: Option<Account>,
    pub connected_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestState {
    Open,
    Merged,
    Declined,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    /// The number people know it by, such as 318.
    pub number: u64,
    pub title: String,
    /// Kept for the model, which reads it with the diff.
    pub description: String,
    pub state: PullRequestState,
    /// Page of the pull request on the host.
    pub url: String,
    pub author: Option<String>,
    pub source_branch: String,
    /// As the host gives it: Bitbucket abbreviates it to 12 characters.
    pub source_commit: Option<String>,
    pub destination_branch: String,
    pub destination_commit: Option<String>,
    /// Reviewers and everyone who took part, with their verdict.
    pub participants: Vec<Participant>,
}

/// What a reviewer concluded about a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Approved,
    ChangesRequested,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Participant {
    pub account: Account,
    /// Asked to review, rather than only taking part (commenting).
    pub reviewer: bool,
    /// `None` while they have not concluded.
    pub verdict: Option<Verdict>,
}

/// Whether "Publish comments" may send now, and why not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishReadiness {
    pub allowed: bool,
    /// Why not, as the `publish_blocked` error sending would fail with.
    pub reason: Option<Box<AppError>>,
}

/// The pull request of a review, or why there is none. Serialised as
/// `{ "kind": "linked", "pullRequest", "account", "publish" }` or `{ "kind":
/// <reason>, ... }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PullRequestLink {
    /// Uncommitted changes have no pull request.
    WorkingTree,
    /// The project has no origin, or origin is not on a server.
    NoRemote,
    /// Origin is on a host DiffLens has no integration for.
    UnsupportedHost { host: String },
    /// No credential is saved for the host.
    NotConnected { host_name: String },
    /// The host has no open pull request from the reviewed branch.
    NoPullRequest { host_name: String, branch: String },
    Linked {
        host_name: String,
        pull_request: Box<PullRequest>,
        /// The connected account: comments by it are the user's own.
        account: Account,
        publish: PublishReadiness,
        /// Whether Approve and Request changes may be used now.
        verdict: PublishReadiness,
    },
}

/// A comment "Publish comments" could not send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishFailure {
    pub comment_id: String,
    pub path: String,
    pub line: u32,
    /// Why, with the error's fields alongside the others (`code`,
    /// `message`, `reason`, ...).
    #[serde(flatten)]
    pub error: AppError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishReport {
    /// Changes the host accepted.
    pub published: u32,
    /// Kept as they were here, each with its reason.
    pub failed: Vec<PublishFailure>,
    /// Draft threads left alone because their line changed.
    pub stale: u32,
}

/// Who answers when DiffLens asks a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelProviderKind {
    /// The `claude` command of Claude Code, with the sign-in it already has.
    #[serde(rename = "claude_cli")]
    ClaudeCli,
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "gemini")]
    Gemini,
    #[serde(rename = "openrouter")]
    OpenRouter,
}

/// How hard the model thinks before answering; only the Anthropic API takes
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

/// A model a provider offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    /// What the provider is asked for, such as "claude-opus-5-5".
    pub id: String,
    /// Such as "Claude Opus 5.5"; the id when the provider has no name.
    pub name: String,
}

/// Whether the `claude` command was found, where, and its version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCliStatus {
    /// `None` when it was not found.
    pub path: Option<String>,
    /// Such as "2.1.328 (Claude Code)"; `None` when it could not be read.
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProviderState {
    pub provider: ModelProviderKind,
    /// Such as "Anthropic API".
    pub name: String,
    /// False for Claude Code, which uses its own sign-in.
    pub needs_key: bool,
    /// An API key is in the Keychain. The key itself never leaves the core.
    pub key_saved: bool,
    /// Where the user gets an API key; empty for Claude Code.
    pub key_url: String,
    pub model: Option<String>,
    /// Only for the providers that take one.
    pub effort: Option<Effort>,
    /// Models offered without asking the provider: the aliases Claude Code
    /// accepts. Empty for the APIs, whose models are listed on request.
    pub suggested_models: Vec<ModelInfo>,
    /// Only for Claude Code.
    pub cli: Option<ClaudeCliStatus>,
    /// It has everything it needs to be asked.
    pub ready: bool,
    /// It can be asked once given a model: its key is saved, or the claude
    /// command was found. Only these can be chosen for a task.
    pub configured: bool,
}

/// What DiffLens asks a model for, each with a model of its own or the
/// default one. A new task is one more variant here (and its name in the
/// frontend's catalogs); every other place lists `ModelTask::ALL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTask {
    /// Analyze on the Insights tab, and the Insights of a full analysis.
    Insights,
    /// The Overview summary, with the Jira card's criteria (one call).
    Summary,
    /// The short explanation of functions, test functions included: a
    /// node's or a group's Explain, and a full analysis.
    BriefExplanation,
    /// Explain in detail.
    DetailedExplanation,
    /// Explain the failure, on a failing test of the Tests tab.
    TestDiagnosis,
    /// Write review comment, from the explanation of a failure.
    ReviewComment,
    /// Translate, whatever it translates.
    Translation,
}

/// A model chosen for one task instead of the default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTaskChoice {
    pub provider: ModelProviderKind,
    pub model: String,
    /// Only for the providers that take one.
    pub effort: Option<Effort>,
}

/// The model of one task, as Preferences shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTaskState {
    pub task: ModelTask,
    /// `None`: the task uses the default model.
    pub choice: Option<ModelTaskChoice>,
    /// The provider chosen for it is not configured any more: its calls
    /// fail, naming the task, until it is set up again or another is chosen.
    /// It never falls back to the default on its own.
    pub needs_attention: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSettings {
    /// The provider of the default model, used by every task without a
    /// model of its own.
    pub active_provider: ModelProviderKind,
    /// Every provider, configured or not.
    pub providers: Vec<ModelProviderState>,
    /// Every task, in `ModelTask::ALL` order.
    pub tasks: Vec<ModelTaskState>,
}

/// A key checked with its provider and saved, and the models it can use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelKeySaved {
    pub settings: ModelSettings,
    pub models: Vec<ModelInfo>,
}

/// The answer to the small question Preferences › Model asks with Test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTest {
    pub provider: ModelProviderKind,
    pub provider_name: String,
    /// The model that answered, as the provider reports it.
    pub model: String,
    /// What the model wrote.
    pub reply: String,
    pub elapsed_ms: u64,
    /// When the provider reports them.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

/// Who would be asked now, read from the settings, and whether they can be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInUse {
    pub provider: ModelProviderKind,
    pub provider_name: String,
    pub model: Option<String>,
    pub ready: bool,
    /// What to do in Preferences › Model when it is not ready, as the error
    /// asking would fail with.
    pub problem: Option<AppError>,
    /// Questions of one piece of work asked at once: 1 for Claude Code.
    pub parallel_calls: u32,
}

/// How much a finding would hurt, by the rubric the model is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingCategory {
    Correctness,
    Concurrency,
    Security,
    DataLoss,
    ErrorHandling,
    Tests,
    Observability,
}

/// What the user concluded about a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingVerdict {
    Pending,
    Confirmed,
    Dismissed,
}

/// How a finding came to be marked fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionKind {
    /// A later analysis showed the model the finding again with the code as
    /// it is now, and it answered that the problem is gone.
    ModelChecked,
    /// A later analysis found its file no longer part of the change; no
    /// model was asked.
    FileLeftChange,
    /// The user marked it fixed.
    UserMarked,
}

/// A finding marked fixed: who decided (the model, named, or the user),
/// when, and why. Its verdict is left as it was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingResolution {
    pub kind: ResolutionKind,
    /// The model that answered, for `ModelChecked` only.
    pub model: Option<String>,
    /// RFC 3339.
    pub at: String,
    /// The model's one sentence on why it is fixed, for `ModelChecked` only.
    pub reason: Option<String>,
}

/// Why a finding cannot be pointed at in the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationProblem {
    /// The path is not a changed file of the review (any more).
    NotChanged,
    /// The file is changed, but the model was not shown it in that call.
    NotSent,
    /// The line is not in the new side of the file.
    OutsideFile,
    /// The cited lines no longer read as they did and were not found again.
    CodeChanged,
}

/// Where a finding is in the review now. Serialised as `{ "kind": "found",
/// "line", "endLine", "moved" }` or `{ "kind": "not_found", "problem" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum FindingLocation {
    Found {
        line: u32,
        end_line: u32,
        /// The cited lines were found elsewhere in the file than cited.
        moved: bool,
    },
    NotFound {
        problem: LocationProblem,
    },
}

/// A line of a finding's excerpt, read from the file at the review's new
/// side, never from the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcerptLine {
    pub line: u32,
    pub text: String,
    /// One of the lines the finding cites.
    pub cited: bool,
}

/// The analysis that wrote a finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingSource {
    pub analysis_id: String,
    pub provider: ModelProviderKind,
    pub provider_name: String,
    pub model: String,
    pub analyzed_at: String,
    /// Head the analysis read; `None` for the working tree.
    pub head_sha: Option<String>,
    /// From an analysis before the latest one: later analyses re-check it
    /// rather than replace it.
    pub earlier: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub id: String,
    pub source: FindingSource,
    pub title: String,
    pub severity: Severity,
    pub category: FindingCategory,
    /// As the model cited it, at the analysis's head.
    pub path: String,
    pub line: u32,
    pub end_line: u32,
    pub location: FindingLocation,
    /// The cited lines and two around them; empty when not found.
    pub excerpt: Vec<ExcerptLine>,
    pub explanation: String,
    pub suggested_direction: String,
    pub evidence: Vec<String>,
    pub related: Vec<String>,
    pub verdict: FindingVerdict,
    pub verdict_at: Option<String>,
    /// Classified, not fixed, and the code it points at changed since, or
    /// can no longer be found.
    pub recheck: bool,
    /// Set once it is marked fixed, by a later analysis or by the user;
    /// `None` while it is open.
    pub resolution: Option<FindingResolution>,
    /// The language the text above is in: its analysis's.
    pub language: Language,
    /// The same text in other languages, translated on a click on
    /// Translate; never in `language`.
    pub translations: Vec<FindingTranslation>,
}

/// A finding's text as it reads in another language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingTranslation {
    pub language: Language,
    pub title: String,
    pub explanation: String,
    pub suggested_direction: String,
    pub evidence: Vec<String>,
    pub related: Vec<String>,
}

/// One call of a translation: the findings it sends (none for a summary).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationCall {
    pub ids: Vec<String>,
    /// Estimated input tokens, the prompt included.
    pub tokens: u64,
}

/// What Translate would send, before it is clicked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationPlan {
    /// Findings to translate, or 1 for a summary; 0 when everything already
    /// reads in the language.
    pub items: u32,
    pub calls: Vec<TranslationCall>,
    pub tokens: u64,
    pub model: ModelInUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsightsBatchStatus {
    /// Not asked yet, or stopped before it was.
    Pending,
    Done,
    Failed,
}

/// One call of an analysis: the files it sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsBatch {
    pub index: u32,
    pub paths: Vec<String>,
    pub tokens: u64,
    pub status: InsightsBatchStatus,
    pub error: Option<crate::error::AppError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsAnalysis {
    pub id: String,
    pub head_sha: Option<String>,
    pub provider: ModelProviderKind,
    pub provider_name: String,
    /// The model asked for, or the one that answered once one did.
    pub model: String,
    pub created_at: String,
    pub parallel_calls: u32,
    pub batches: Vec<InsightsBatch>,
    /// The language its findings are written in.
    pub language: Language,
}

/// The findings of a review and its latest analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Insights {
    pub analysis: Option<InsightsAnalysis>,
    /// By severity, then by place.
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    Deleted,
    Binary,
    TooLarge,
    NotText,
}

/// A changed file listed to the model but not sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedFile {
    pub path: String,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedCall {
    pub paths: Vec<String>,
    /// Estimated input tokens, the prompt included.
    pub tokens: u64,
    /// Files too large for one call, sent as their changed lines only.
    pub truncated: Vec<String>,
}

/// What Analyze would send, before it is clicked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsPlan {
    /// Files sent.
    pub files: u32,
    pub skipped: Vec<SkippedFile>,
    pub calls: Vec<PlannedCall>,
    pub tokens: u64,
    pub model: ModelInUse,
}

/// How a Jira API token reaches the site: a classic token on the site's own
/// address, or a token with scopes through the Atlassian API gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JiraTokenKind {
    Classic,
    Scoped,
}

/// The Jira Cloud connection, or the lack of one. The token never leaves
/// the core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JiraConnection {
    /// Such as "acme.atlassian.net"; `None` when not connected.
    pub site: Option<String>,
    pub email: Option<String>,
    pub account: Option<Account>,
    pub token_kind: Option<JiraTokenKind>,
    pub connected_at: Option<String>,
    /// Scopes a token with scopes needs, as Atlassian names them.
    pub scopes: Vec<String>,
}

/// A comment of a Jira card, as plain text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardComment {
    pub author: String,
    /// As Jira gives it, such as "2026-11-12T09:30:00.000+0000".
    pub created_at: String,
    pub text: String,
}

/// A Jira card, its rich text read as plain text (lists and headings kept).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub key: String,
    /// Page of the card on the site.
    pub url: String,
    pub summary: String,
    pub status: String,
    pub issue_type: String,
    pub description: String,
    /// Oldest first.
    pub comments: Vec<CardComment>,
}

/// The card of a review, or why there is none. Serialised like
/// `PullRequestLink`, with a `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum CardLookup {
    /// Uncommitted changes have no branch to take a key from.
    WorkingTree,
    /// Neither the branch name nor the pull request title holds a key.
    NoKey {
        branch: String,
    },
    /// A key was found, but Jira is not connected.
    NotConnected {
        key: String,
    },
    /// Jira has no such card, or the account cannot see it.
    NotFound {
        key: String,
        url: String,
    },
    Found {
        card: Box<Card>,
    },
}

/// The pull request the frontend found for a review, for the overview: its
/// title is where a card key is looked for when the branch has none.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestText {
    pub title: String,
    pub description: String,
}

/// How the model judged an acceptance criterion against the change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CriterionVerdict {
    Met,
    PartlyMet,
    NotMet,
    /// Nothing in the change shows it either way: absent from the diff, not
    /// necessarily missing everywhere.
    NoEvidence,
}

/// An acceptance criterion the model read in the card, with its verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Criterion {
    pub criterion: String,
    pub verdict: CriterionVerdict,
    /// One line on why.
    pub reasoning: String,
    /// Where the verdict rests, as the model cited it; `None` without one.
    pub path: Option<String>,
    pub line: Option<u32>,
    /// The cited place is a line of a file the change has: it can be opened.
    pub located: bool,
}

/// What the model wrote about a review: what the change does, what its card
/// asks, and the card's criteria judged against the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverviewSummary {
    /// Head it read; `None` for the working tree.
    pub head_sha: Option<String>,
    pub provider: ModelProviderKind,
    pub provider_name: String,
    pub model: String,
    pub created_at: String,
    /// The change did not fit one call whole and was read condensed.
    pub condensed: bool,
    /// The head moved since: it was written for an older commit.
    pub outdated: bool,
    pub card_key: Option<String>,
    /// Comments of the card it read.
    pub card_comments: u32,
    pub title: String,
    /// One sentence on what the change does, in product terms. In a summary
    /// written before `points` existed, the whole paragraph.
    pub lead: String,
    /// How the change works, three to five short points; empty in a summary
    /// written before they existed.
    pub points: Vec<String>,
    /// What the reviewer should know before reading, such as code from other
    /// work the base does not have yet; usually empty.
    pub attention: Vec<String>,
    /// Empty without a card.
    pub card_summary: String,
    pub criteria: Vec<Criterion>,
    /// The language the text above is in: the one asked for on Summarize.
    pub language: Language,
    /// The same text in other languages, translated on a click on
    /// Translate; never in `language`.
    pub translations: Vec<SummaryTranslation>,
}

/// The text of a criterion in another language, in the summary's order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CriterionText {
    pub criterion: String,
    pub reasoning: String,
}

/// A summary's text as it reads in another language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryTranslation {
    pub language: Language,
    pub title: String,
    pub lead: String,
    /// One for each point of the summary, in its order.
    pub points: Vec<String>,
    pub attention: Vec<String>,
    pub card_summary: String,
    pub criteria: Vec<CriterionText>,
}

/// A changed file of a kind the shape of a change names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TouchedFile {
    pub path: String,
    pub status: FileStatus,
}

/// What can be told about the shape of a change from its files alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeShape {
    pub files_added: u32,
    pub files_deleted: u32,
    /// Directories holding a changed file that the old side does not have.
    pub directories_new: Vec<String>,
    /// Directories holding a changed file that the old side has.
    pub directories_modified: Vec<String>,
    /// Dependency manifests and lock files, by name, of any language.
    pub manifests: Vec<TouchedFile>,
    /// Database migrations, by where they live or how they are named.
    pub migrations: Vec<TouchedFile>,
}

/// What a model reading this change would not see, from the planning of an
/// Insights analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeftOut {
    pub skipped: Vec<SkippedFile>,
    /// Too large to send whole: only their changed lines are sent.
    pub truncated: Vec<String>,
}

/// The Overview tab, without a model call: the stored summary, if any, and
/// what is derived from the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub summary: Option<OverviewSummary>,
    pub shape: ChangeShape,
    pub left_out: LeftOut,
}

/// What Summarize would send, before it is clicked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverviewPlan {
    /// Estimated input tokens, the prompt included.
    pub tokens: u64,
    /// Files whose code is sent, whole or in part.
    pub files: u32,
    /// The change does not fit one call whole: a condensed form is sent.
    pub condensed: bool,
    /// The card sent with it, if any, and how many of its comments.
    pub card_key: Option<String>,
    pub card_comments: u32,
    pub model: ModelInUse,
}

/// What an explanation of a function covers: a changed function gets what
/// changed, why and what it is for; a function of the context (an unchanged
/// caller or callee) only what it is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplanationKind {
    Changed,
    Context,
}

/// An explanation's text as it reads in another language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplanationTranslation {
    pub language: Language,
    pub what_changed: String,
    pub why: String,
    pub purpose: String,
}

/// The detailed explanation a model wrote of one function of the review (on
/// Explain in detail, or any explanation written before short ones existed),
/// read against the function's code as the review shows it now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionExplanation {
    pub function_id: String,
    pub kind: ExplanationKind,
    /// Empty for a function of the context.
    pub what_changed: String,
    /// Empty for a function of the context.
    pub why: String,
    /// Nothing the model was shown says why: `why` is its inference.
    pub why_inferred: bool,
    pub purpose: String,
    pub language: Language,
    pub translations: Vec<ExplanationTranslation>,
    pub provider: ModelProviderKind,
    pub provider_name: String,
    pub model: String,
    pub created_at: String,
    pub head_sha: Option<String>,
    /// The function's lines changed since it was explained.
    pub outdated: bool,
}

/// How much an explanation says: a short one (what the full analysis and
/// the Explain buttons write), or the detailed one in three parts that a
/// click on Explain in detail asks for, one function at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplanationLevel {
    Brief,
    Detail,
}

/// A short explanation's text as it reads in another language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefTranslation {
    pub language: Language,
    pub text: String,
}

/// The short explanation a model wrote of one function: one or two
/// sentences on what changed and what it is for (for a function of the
/// context, what it is for), read against the function's code as the review
/// shows it now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionBrief {
    pub function_id: String,
    pub kind: ExplanationKind,
    pub text: String,
    pub language: Language,
    pub translations: Vec<BriefTranslation>,
    pub provider: ModelProviderKind,
    pub provider_name: String,
    pub model: String,
    pub created_at: String,
    pub head_sha: Option<String>,
    /// The function's lines changed since it was explained.
    pub outdated: bool,
}

/// What the model wrote about the review's functions: the detailed
/// explanations (`functions`, those written before short ones existed
/// included) and the short ones (`briefs`). A function can have either, both
/// or neither.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Explanations {
    pub functions: Vec<FunctionExplanation>,
    pub briefs: Vec<FunctionBrief>,
}

impl Explanations {
    pub fn none() -> Self {
        Self {
            functions: Vec::new(),
            briefs: Vec::new(),
        }
    }
}

/// One call explaining functions: their ids and its estimated input tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplanationCall {
    pub ids: Vec<String>,
    pub tokens: u64,
}

/// What explaining the changed functions in short would send, before it is
/// clicked (a full analysis explains no function of the context).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplanationPlan {
    /// Changed production functions, each explained in short.
    pub changed: u32,
    pub calls: Vec<ExplanationCall>,
    pub tokens: u64,
    pub model: ModelInUse,
}

/// Where the cause of a failed test is, by the model's reading: the code
/// under test, the test itself, a test that fails only sometimes, or the
/// place it ran (such as the sandbox without network).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosisFault {
    Code,
    Test,
    Flaky,
    Environment,
}

/// How sure the model says it is of a diagnosis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosisConfidence {
    High,
    Medium,
    Low,
}

/// The prose of a diagnosis as it reads in another language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosisTranslation {
    pub language: Language,
    pub cause: String,
    pub fix: String,
}

/// Why a failing test (or a package that did not build) failed, as a model
/// explained it on a click on Explain the failure, read against the last
/// run of the review now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestDiagnosis {
    /// "<package>::<top-level test>", or "<package>::" for a package that
    /// failed outside its tests.
    pub key: String,
    pub package: String,
    /// None for a package that failed outside its tests.
    pub test: Option<String>,
    pub cause: String,
    pub fault: DiagnosisFault,
    pub confidence: DiagnosisConfidence,
    /// A line of the code the model was shown; None when it named none.
    pub location: Option<SourceRef>,
    pub fix: String,
    /// A short snippet of the proposed fix; empty when it gave none.
    pub fix_code: String,
    pub language: Language,
    pub translations: Vec<DiagnosisTranslation>,
    pub provider: ModelProviderKind,
    pub provider_name: String,
    pub model: String,
    /// RFC 3339.
    pub created_at: String,
    /// When the run it explained started.
    pub run_started_at: String,
    /// The last run reads the failure otherwise (or no longer has it), or
    /// the code the model was shown changed since.
    pub outdated: bool,
    /// The review comment written from it (Write review comment), until
    /// that comment is deleted; kept when the failure is explained again.
    pub comment_id: Option<String>,
}

/// The review comment written from the explanation of a failure: a draft
/// of the review, on a line or on the pull request as a whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WrittenReviewComment {
    /// The failure's key, as `TestDiagnosis::key`.
    pub key: String,
    pub comment_id: String,
    pub kind: CommentKind,
}

/// An explanation of a failure being written: asked for (from this screen
/// or one left since) and not ended yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningDiagnosis {
    /// As `TestDiagnosis::key`.
    pub key: String,
    pub package: String,
    pub test: Option<String>,
    /// Waiting for another explanation to end: the model takes this many
    /// calls at once.
    pub queued: bool,
}

/// The steps of a full analysis, all started at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisStep {
    Tests,
    Metrics,
    Insights,
    Summary,
    Explanations,
}

impl AnalysisStep {
    pub const ALL: [AnalysisStep; 5] = [
        AnalysisStep::Tests,
        AnalysisStep::Metrics,
        AnalysisStep::Insights,
        AnalysisStep::Summary,
        AnalysisStep::Explanations,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Running,
    Done,
    /// It ended with an error; the others went on.
    Failed,
    /// It had nothing to do, such as tests in a project without a runner.
    Skipped,
    /// Cancel stopped it, or it never started.
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepState {
    pub step: AnalysisStep,
    pub status: StepStatus,
    /// Calls answered of `total` (Insights, explanations), or parts of the
    /// step done; 0 of 0 while not known.
    pub done: u32,
    pub total: u32,
    pub error: Option<AppError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    /// Waiting for a slot: as many full analyses as Preferences allows are
    /// running.
    Queued,
    Running,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobOutcome {
    Succeeded,
    /// At least one step failed; the others' results are kept.
    Failed,
    Cancelled,
}

/// One line of a full analysis's log: a code and its values, worded by the
/// screen in the language shown. Serialised as `{ "code": "...", ... }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum LogEvent {
    Queued,
    Started,
    TestsStarted,
    TestsFinished {
        passed: u32,
        failed: u32,
        skipped: u32,
        duration_ms: u64,
    },
    TestsUnavailable,
    FlowMeasured {
        changed: u32,
    },
    FlowUnavailable,
    QualityMeasured {
        crossed: u32,
    },
    CoverageWaiting,
    CoverageMeasured {
        crossed: u32,
    },
    CoverageSkipped,
    InsightsPlanned {
        files: u32,
        calls: u32,
    },
    InsightsCallStarted {
        number: u32,
        total: u32,
    },
    InsightsCallAnswered {
        number: u32,
        total: u32,
        findings: u32,
    },
    InsightsCallFailed {
        number: u32,
        total: u32,
        error: AppError,
    },
    SummaryStarted,
    SummaryWritten,
    ExplanationsPlanned {
        changed: u32,
        calls: u32,
    },
    NothingToExplain,
    ExplanationsCallStarted {
        number: u32,
        total: u32,
    },
    ExplanationsCallAnswered {
        number: u32,
        total: u32,
        functions: u32,
    },
    ExplanationsCallFailed {
        number: u32,
        total: u32,
        error: AppError,
    },
    StepFailed {
        step: AnalysisStep,
        error: AppError,
    },
    StepCancelled {
        step: AnalysisStep,
    },
    Cancelling,
    Finished,
    FinishedWithFailures {
        steps: Vec<AnalysisStep>,
    },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    /// RFC 3339.
    pub at: String,
    #[serde(flatten)]
    pub event: LogEvent,
}

/// A full analysis as the screens follow it, without its log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSummary {
    pub id: String,
    pub project_id: String,
    /// The project's display name, its folder's.
    pub project_name: String,
    pub review: ReviewRef,
    pub status: JobStatus,
    pub outcome: Option<JobOutcome>,
    /// Cancel was clicked; the steps in flight are stopping.
    pub cancelling: bool,
    pub steps: Vec<StepState>,
    /// 0 to 100: each step counts the same, Insights and the explanations
    /// by their calls.
    pub progress: u32,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

/// A full analysis with its whole log, kept while the app runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FullAnalysisJob {
    #[serde(flatten)]
    pub summary: JobSummary,
    pub log: Vec<LogLine>,
}

/// What the core emits as a full analysis moves: its state and the line it
/// logged, if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FullAnalysisEvent {
    pub job: JobSummary,
    pub line: Option<LogLine>,
}

/// What a full analysis would do and send, before it is confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FullAnalysisPlan {
    /// The project has a test runner: the tests run in the sandbox.
    pub tests: bool,
    /// Each part names the model of its own task.
    pub insights: InsightsPlan,
    pub summary: OverviewPlan,
    pub explanations: ExplanationPlan,
    /// Full analyses allowed at once, and how many are going now.
    pub limit: u32,
    pub running: u32,
}
