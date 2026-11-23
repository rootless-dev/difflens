// Contract between the React frontend and the Rust core.
// Every function maps to one Tauri command with the same name in snake_case.
// Rust serialises structs with camelCase field names to match these types.

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { currentLanguage } from "@/i18n";

/** "full" when at least one detected language has an analyzer; otherwise only the diff is available. */
export type AnalysisSupport = "full" | "diff_only";

export interface Project {
  /** UUID assigned when the project is added. */
  id: string;
  /** Folder name, used as the display name. */
  name: string;
  /** Absolute path of the repository on this machine. */
  path: string;
  /** Same path with the home directory written as "~", for display only. */
  displayPath: string;
  /** Detected language ids in lowercase, such as "go" or "typescript". Empty when none was recognised. */
  languages: string[];
  analysisSupport: AnalysisSupport;
  /**
   * Names or glob patterns of the branches work is merged into, such as "develop", "main" or "release/*".
   * They are used as bases and are never listed as branches to review.
   * Defaults when the project is added: main, master, develop, release/*.
   */
  integrationBranches: string[];
  /** False when the folder no longer exists or is no longer a git repository. */
  available: boolean;
  /** True when the repository has at least one remote, so fetchProject can do something. False when unavailable. */
  hasRemote: boolean;
  /** Branches to review (not integration branches) that have commits not yet in their base. 0 when it cannot be computed. */
  branchesAhead: number;
  /** RFC 3339 timestamp of the last successful fetch made by DiffLens, or null if it never fetched. */
  lastFetchedAt: string | null;
  /** RFC 3339 timestamp. */
  addedAt: string;
  /**
   * RFC 3339 committer date of the newest commit on a local branch or a branch of origin. Null when the repository
   * is unavailable or has no commit yet.
   */
  lastModifiedAt: string | null;
}

export interface Commit {
  sha: string;
  subject: string;
  /** RFC 3339 timestamp. */
  authoredAt: string;
}

/** Why a branch is compared with the base it has. */
export type BaseSource =
  /** The integration branch closest to it in history (fewest commits ahead, then fewest behind). */
  | "history"
  /** Chosen by the user for this branch; kept until changed or reset. */
  | "user";

export interface BranchBase {
  /** Name of the integration branch, such as "develop" or "release/2.4". */
  name: string;
  source: BaseSource;
  /** The ref the base is read from: "origin/develop", or "develop" when origin has no branch of that name. */
  refName: string;
  /** Commit the base points at (origin's branch, else the local one), so a cached comparison can tell when it moved. */
  sha: string;
}

/**
 * Where the commit a branch review compares with comes from: the destination commit of the branch's pull request
 * (when this repository has it), else the base branch of origin, else the local base branch.
 */
export type ReviewBaseKind = "pull_request" | "remote_tracking" | "local";

/** The base a branch review was compared with; its old side is where the branch left this commit. */
export interface ReviewBase {
  /** The integration branch, such as "develop". */
  name: string;
  kind: ReviewBaseKind;
  /** The ref it was read from, such as "origin/develop"; null for a pull request's destination commit. */
  refName: string | null;
  sha: string;
}

export interface Upstream {
  /** Remote-tracking branch, such as "origin/feature/ABC-123". */
  name: string;
  /** Local commits not on the remote. */
  ahead: number;
  /** Remote commits not in the local branch, for instance pushed by an agent on another machine. */
  behind: number;
}

export interface Branch {
  /** Branch name without the remote prefix, such as "feature/ABC-123". */
  name: string;
  /** "remote_only" when the branch exists on origin but has no local branch. */
  location: "local" | "remote_only";
  /** True for the branch checked out in the working tree. */
  isCurrent: boolean;
  /**
   * Last commit of the branch as it will be compared: the local branch, or the remote-tracking
   * branch for a remote-only one.
   */
  head: Commit;
  /** The local branch's upstream, or null when it has none or the branch is remote only. */
  upstream: Upstream | null;
  /** Card key found in the branch name, such as "ABC-123" (uppercase letters, a dash, digits), or null. */
  card: string | null;
  /** null when no integration branch exists in this repository. */
  base: BranchBase | null;
  /** Commits on this branch that are not in its base. */
  ahead: number;
  /** Commits in its base that are not on this branch. */
  behind: number;
}

export interface WorkingTree {
  /** True when there are uncommitted changes, staged or not. */
  dirty: boolean;
  filesChanged: number;
}

export interface ProjectDetail {
  project: Project;
  /**
   * Branches to review: local branches and remote-only branches, never integration branches.
   * Includes branches with nothing ahead of their base. Most recently committed first.
   */
  branches: Branch[];
  /** Integration branches that exist in this repository, locally or on origin, as names (patterns resolved). */
  bases: string[];
  workingTree: WorkingTree;
  /** Every worktree of the repository, the project folder first. */
  worktrees: Worktree[];
}

export interface CompareSummary {
  branch: string;
  base: string;
  headSha: string;
  /** Commit the comparison starts from: the merge base of branch and base. */
  mergeBaseSha: string;
  commits: number;
  filesChanged: number;
  additions: number;
  deletions: number;
}

export interface WorkingTreeSummary {
  /** Commit the uncommitted changes are compared with: HEAD. */
  headSha: string;
  filesChanged: number;
  /** Untracked files are counted with all their lines as additions. Binary files count with 0 lines. */
  additions: number;
  deletions: number;
}

export interface FetchResult {
  /** RFC 3339 timestamp, also stored as the project's lastFetchedAt. */
  fetchedAt: string;
  /** Remote-tracking branches that moved, appeared or disappeared in this fetch. */
  branchesUpdated: number;
}

export type ErrorCode =
  | "path_not_found"
  | "not_a_git_repository"
  | "already_added"
  | "destination_exists"
  | "clone_failed"
  | "fetch_failed"
  | "no_remote"
  | "git_not_found"
  | "unknown_project"
  | "unknown_branch"
  | "no_common_history"
  | "unknown_file"
  | "unknown_comment"
  /** The finding, or the analysis or call it belongs to, is gone: a newer analysis replaced it. */
  | "unknown_finding"
  /** No credential is saved for the pull request host. */
  | "not_connected"
  /** The host refused the credential: wrong, expired or revoked token. */
  | "auth_failed"
  /** The credential works but its token lacks a scope. */
  | "missing_scope"
  /** The host does not know the repository, pull request or comment, or the account cannot see it. */
  | "remote_not_found"
  /** The host is limiting requests; the message says when to try again when it knows. */
  | "rate_limited"
  /** The host could not be reached. */
  | "network_failed"
  /** The host answered with an error of its own. */
  | "remote_failed"
  /** Publishing is not possible now; the message says why. */
  | "publish_blocked"
  /** The model provider in use lacks an API key or a model; the message says which. */
  | "model_not_configured"
  /** The model declined to answer; the message says why when the provider does. */
  | "model_refused"
  /** The answer was cut off before it was complete. */
  | "model_truncated"
  /** The model answered, but not with JSON matching the schema it was given. */
  | "model_invalid_output"
  /** The claude command of Claude Code was not found on this machine. */
  | "claude_cli_not_found"
  /** The claude command ran but failed; the message carries its reason. */
  | "claude_cli_failed"
  /** The user stopped the operation. */
  | "cancelled"
  /** The review's branch is checked out in no folder yet: a worktree has to be created before a terminal can open. */
  | "no_folder"
  /** The path is not a worktree DiffLens created in the project. */
  | "unknown_worktree"
  /** The worktree has uncommitted changes; removing it needs `force`. */
  | "worktree_dirty"
  /** A terminal session is open in the worktree. */
  | "terminals_open"
  /** No terminal session has that id: it was closed. */
  | "unknown_terminal"
  | "internal";

/**
 * A sentence the core composed, as a key with values: `reason` names it (`errors.reasons.<reason>` in the catalogs),
 * `params` fill it, and `message` is the same sentence in English, shown when there is no reason (text from
 * elsewhere, or a row stored before reasons existed) or the catalog does not know it. Show it with `errorMessage`.
 */
export interface CoreText {
  message: string;
  reason?: string;
  params?: Record<string, CoreParam>;
}

/** A value that fills a `CoreText`: shown as it is (a path, a branch, a host's words), a number, or a sentence. */
export type CoreParam = string | number | CoreText;

/**
 * Text a tool or runner wrote, as a plain string shown as it is, or a sentence of the core (`CoreText`). Show it with
 * `coreText` from `src/lib/core-text.ts`.
 */
export type CoreMessage = string | CoreText;

/**
 * Shape of every rejected command, and of the other reasons the core gives (a model problem, why publishing is
 * blocked, why a comment was not published). `detail` is text from elsewhere (a host's, git's or a provider's own
 * words) that the message shows as it is. Show it with `errorMessage`, never `message` directly.
 */
export interface ApiError extends CoreText {
  code: ErrorCode;
  detail?: string;
}

export function isApiError(value: unknown): value is ApiError {
  return typeof value === "object" && value !== null && "code" in value && "message" in value;
}

/** Projects in the order they were added, newest first. */
export const listProjects = () => invoke<Project[]>("list_projects");

/** Registers an existing local repository. Fails with path_not_found, not_a_git_repository or already_added. */
export const addLocalProject = (path: string) => invoke<Project>("add_local_project", { path });

/**
 * Clones `url` into `destination` (the full path of the folder to create) and registers it.
 * Fails with destination_exists, clone_failed or git_not_found.
 */
export const cloneProject = (url: string, destination: string) =>
  invoke<Project>("clone_project", { url, destination });

/** Forgets the project. The folder on disk is not touched. */
export const removeProject = (id: string) => invoke<void>("remove_project", { id });

export const getProject = (id: string) => invoke<ProjectDetail>("get_project", { id });

/**
 * Runs `git fetch --prune` on every remote. Only remote-tracking branches change: local branches,
 * the checked out branch and the working tree are never touched. Fails with no_remote or fetch_failed.
 */
export const fetchProject = (id: string) => invoke<FetchResult>("fetch_project", { id });

/**
 * Summary of what `branch` adds on top of `base`, counted from their merge base.
 * `branch` may be a remote-only branch; `base` must be one of ProjectDetail.bases.
 * Fails with unknown_branch or no_common_history.
 */
export const compareBranches = (projectId: string, branch: string, base: string) =>
  invoke<CompareSummary>("compare_branches", { projectId, branch, base });

/** Summary of the uncommitted changes, compared with HEAD. */
export const compareWorkingTree = (projectId: string) =>
  invoke<WorkingTreeSummary>("compare_working_tree", { projectId });

/** What a review looks at: a branch compared with a base, or the uncommitted changes compared with HEAD. */
export type ReviewTarget =
  | { kind: "branch"; branch: string; base: string }
  | { kind: "working_tree" };

export type FileStatus = "added" | "modified" | "deleted" | "renamed" | "type_changed";

export interface ChangedFile {
  /** Path relative to the repository root, with "/" separators, on the new side (the old side for deleted files). */
  path: string;
  /** Path before a rename, otherwise null. */
  oldPath: string | null;
  status: FileStatus;
  additions: number;
  deletions: number;
  binary: boolean;
  /** Language id guessed from the file name, such as "go", "typescript", "yaml", or null. Used for highlighting only. */
  language: string | null;
}

export interface ChangeSet {
  target: ReviewTarget;
  /**
   * The two sides, frozen when the change set was read so every file of one review comes from the same commits.
   * oldSha: merge base of branch and `base.sha`, or HEAD for the working tree.
   * newSha: branch head, or null for the working tree (files are read from disk).
   */
  oldSha: string;
  newSha: string | null;
  /** What a branch was compared with; null for the working tree. Every view of the review uses this one base. */
  base: ReviewBase | null;
  /** Sorted by path. Renames are detected; files ignored by .gitignore never appear. */
  files: ChangedFile[];
}

/** One side of a file. `text` is null when the side does not exist or cannot be shown, and `reason` says why. */
export interface FileSide {
  text: string | null;
  reason: null | "absent" | "binary" | "too_large" | "not_utf8";
  /** Size in bytes, 0 when absent. */
  size: number;
}

export interface FileVersions {
  old: FileSide;
  new: FileSide;
}

/**
 * Lists the changed files of a review target and freezes its two sides. A branch with an open pull request into its
 * base is compared with the pull request's destination commit (the host is only read); see `ReviewBase`.
 * Fails with unknown_branch or no_common_history.
 */
export const listChangedFiles = (projectId: string, target: ReviewTarget) =>
  invoke<ChangeSet>("list_changed_files", { projectId, target });

/**
 * Full content of both sides of one file of a ChangeSet: the old side at `oldSha` (under `oldPath` for a rename),
 * the new side at `newSha`, or from disk when `newSha` is null. Text above 2 MB is not returned (too_large).
 * `path` and `oldPath` must be ones listed in that ChangeSet; anything else fails with unknown_file.
 */
export const getFileVersions = (
  projectId: string,
  oldSha: string,
  newSha: string | null,
  path: string,
  oldPath: string | null,
) => invoke<FileVersions>("get_file_versions", { projectId, oldSha, newSha, path, oldPath });

export type EntryPointKind =
  | "http_route"
  | "rpc_method"
  | "message_handler"
  | "scheduled_job"
  | "cli_command"
  | "main"
  | "test"
  | "exported_api";

export interface EntryPoint {
  kind: EntryPointKind;
  /** Such as "POST /refunds" or "payment.settled". */
  label: string;
}

/**
 * State of a function in this review. deleted: exists only on the old side. external: a package outside the
 * repository (standard library, dependency), shown as one boundary node per package and never changed.
 */
export type FlowNodeState = "new" | "modified" | "deleted" | "untouched" | "external";

/** One direct call made by a node, whether or not the callee is drawn in the graph. */
export interface FlowCall {
  /** Id of the callee: a function id, or the id of an external package node. */
  calleeId: string;
  /** Such as "audit_service.FillApplication", or "database/sql" for an external package. */
  qualifiedName: string;
  /** Line of the call site in the caller's file. */
  line: number;
  kind: "always" | "conditional" | "async";
  external: boolean;
  /** The callee is drawn in the current graph. */
  shown: boolean;
  /** The call exists only on the new side. */
  added: boolean;
  /** The call exists only on the old side. */
  removed: boolean;
  relation: "call" | "registration" | "reference";
  evidence: string | null;
}

export interface FlowNode {
  /**
   * Stable function id from the analyzer, the same on both sides. External package nodes use
   * "external:<container>" and stand for every function of that package called from the graph.
   */
  id: string;
  /** Short name, such as "Service.Request". */
  name: string;
  /** Such as "refund.Service.Request". */
  qualifiedName: string;
  /** Package, module or namespace, such as "internal/refund". */
  container: string;
  state: FlowNodeState;
  /** Path on the new side (old side for deleted), null for external. */
  file: string | null;
  startLine: number | null;
  endLine: number | null;
  /** Lines of this function added and removed in the review, counted from the diff. 0 for untouched and external. */
  additions: number;
  deletions: number;
  /** Cyclomatic complexity on the new side (old side for deleted), null when unknown. */
  complexity: number | null;
  /** Complexity on the old side, null when the function is new or unknown. */
  previousComplexity: number | null;
  generated: boolean;
  /** Defined in test code, as reported by the analyzer. */
  test: boolean;
  entryPoints: EntryPoint[];
  /**
   * Direct calls made by this function, by line, including callees not drawn (so the popover can list them).
   * Filled for changed functions (new, modified, deleted); empty for the others.
   */
  calls: FlowCall[];
  /** For an external package node: how many distinct functions of the package are called from the graph. Else 0. */
  externalFunctions: number;
}

export interface FlowEdge {
  from: string;
  to: string;
  /** 1-based position of this call among the caller's calls shown in the graph, by line. Not the order things run. */
  position: number;
  /** Line of the call site in the caller's file. */
  line: number;
  kind: "always" | "conditional" | "async";
  resolution: "static" | "dynamic";
  /** The call does not exist on the old side: this review added it. */
  added: boolean;
  /** The call existed on the old side and is gone on the new side. Drawn faded. */
  removed: boolean;
  /**
   * call: `from` invokes `to`. registration: `from` registers `to` with a mechanism the analyzer recognises
   * (a route, a middleware…), see `evidence`. reference: `from` passes `to` as a value with no known meaning.
   * Independent of the change state (added / preserved / removed), which `added` and `removed` carry.
   */
  relation: "call" | "registration" | "reference";
  /** For registration, such as "route POST /refunds" or "middleware"; otherwise null. */
  evidence: string | null;
}

export interface FlowGraph {
  /** Language of the analyzer that produced it, such as "go". */
  language: string;
  analyzer: { name: string; version: string };
  oldSha: string;
  newSha: string | null;
  /** The options this graph was built with. */
  options: FlowOptions;
  /**
   * Nodes, built from the options:
   * - always: every new, modified and deleted function, except generated ones and, without includeTests, test
   *   functions. Nothing unchanged. Edges are the direct relations (call, registration, reference) whose two ends
   *   are both drawn; a relation that only passes through unchanged code is not drawn;
   * - with includeCallers: also the unchanged functions on paths from entry points down to the changed ones
   *   (test functions are not followed as callers without includeTests);
   * - with includeCallees: also the direct callees of the changed functions, in-repository ones as function nodes
   *   and external ones grouped as one node per package.
   * Grouping by file or package and expanding groups is done by the screen from these function-level nodes.
   * Bounded: see `truncated`.
   */
  nodes: FlowNode[];
  edges: FlowEdge[];
  /**
   * What each toggle adds: changed test functions, unchanged callers on the paths from entry points, in-repository
   * callees of the changed functions, and external packages they call. Counted whether or not the matching option
   * is on (0 when nothing would be added).
   */
  hidden: { changedTests: number; callers: number; callees: number; externalPackages: number };
  /** Ids of nodes that are entry points reaching at least one changed function. */
  entryPoints: string[];
  /**
   * True when what the context toggles add was cut to stay readable (more than ~300 added nodes). The changed
   * functions are never cut, so a graph can have more than 300 nodes.
   */
  truncated: boolean;
  /**
   * Analyzer diagnostics from either side, prefixed with "old:" or "new:", as plain strings; and, worded by the core
   * (show each with `coreText`), why the analyzer of another of the project's languages could not be prepared.
   */
  diagnostics: CoreMessage[];
  /** RFC 3339. */
  analyzedAt: string;
}

/**
 * Why a review has no flow: no analyzer for the project's languages, the analysis failed, or an analyzer shipped as
 * source could not be prepared (a missing toolchain, a failed build).
 */
export type FlowUnavailable =
  | { reason: "no_analyzer"; languages: string[] }
  /** `message`: why, worded by the core (show it with `coreText`). */
  | { reason: "analysis_failed"; side: "old" | "new"; message: CoreMessage }
  /** `analyzer`: its language as shown, such as "Python"; `message`: why, worded by the core. */
  | { reason: "analyzer_unavailable"; analyzer: string; message: CoreMessage };

export type FlowResult = { kind: "graph"; graph: FlowGraph } | ({ kind: "unavailable" } & FlowUnavailable);

export interface FlowOptions {
  /** Test functions count as changed code and as callers. Default false. */
  includeTests: boolean;
  /** Draw the unchanged functions on the paths from entry points down to the changes. Default false. */
  includeCallers: boolean;
  /** Draw what the changed functions call: untouched functions and external packages. Default false. */
  includeCallees: boolean;
}

/**
 * Builds the call graph of a review's changes: runs the project's language analyzer on both sides (results are
 * cached per commit, so the merge base shared by many branches is analysed once) and compares them.
 * The working tree side (newSha null) is never cached. May take tens of seconds the first time on a large
 * project; changing only `options` reuses the analyses and is fast. Concurrent calls for the same sides share one
 * run. Fails with unknown_project or unknown_branch.
 */
export const getFlow = (projectId: string, oldSha: string, newSha: string | null, options: FlowOptions) =>
  invoke<FlowResult>("get_flow", { projectId, oldSha, newSha, options });

/**
 * Where a step that executes project code runs. process: a plain child process, with the user's permissions and
 * network. sandbox: the macOS sandbox, with no network beyond the machine itself (loopback stays open, so tests that
 * start a local server work) and writes only inside its own copy of the commit and the toolchain's caches.
 * Docker comes with the Run tab.
 */
export type Isolation = "process" | "sandbox";

export type TestOutcome = "pass" | "fail" | "skip";

/** A changed function, as tests and quality refer to it. */
export interface FunctionRef {
  /** Analyzer function id, the same as FlowNode.id. */
  id: string;
  /** Such as "refund.Service.Request". */
  qualifiedName: string;
}

/** A place in the project's code, relative to its root. */
export interface SourceRef {
  file: string;
  line: number;
}

/** A frame of a stack in the project's own code. */
export interface FailureFrame extends SourceRef {
  /** Without its package path, such as "(*Service).recordLastLogin". */
  function: string;
  /** The file is test code. */
  test: boolean;
}

/** One side of a data race. */
export interface RaceAccess {
  kind: "read" | "write";
  /** The earlier of the two accesses. */
  previous: boolean;
  /** Null for the main goroutine. */
  goroutine: number | null;
  /** The first frame of the access in the project's code. */
  frame: FailureFrame | null;
  /** Where its goroutine was created; null when the test framework created it (the test's own goroutine). */
  createdAt: FailureFrame | null;
}

export interface BuildError {
  file: string | null;
  line: number | null;
  column: number | null;
  message: string;
  /** The indented lines the compiler printed after it. */
  detail: string[];
}

/** Why a test or a package failed, read from its output by the core. */
export type TestFailure =
  | { kind: "race"; races: { accesses: RaceAccess[] }[] }
  | {
      kind: "assertion";
      message: string;
      expected: string | null;
      actual: string | null;
      at: SourceRef | null;
      /** Full name of the (sub)test whose output said it. */
      test: string | null;
    }
  | { kind: "panic"; message: string; frame: FailureFrame | null; test: string | null }
  | { kind: "timeout"; message: string; frame: FailureFrame | null; running: string[] }
  | { kind: "build"; errors: BuildError[] }
  /** Nothing recognised: the last meaningful lines. */
  | { kind: "other"; lines: string[] };

export interface TestCase {
  /** Unique within a run: container and full name. */
  id: string;
  /** Package, module or namespace, such as "internal/refund". */
  container: string;
  /** Container of the package it ran in, as `packages` lists it: what a selection names. */
  package: string;
  /** Full name as the framework prints it, such as "TestService_Request/gateway_declines". */
  name: string;
  /**
   * Where `name` nests, from its top-level test down to itself: joined with "/" they give `name`. A top-level test
   * has one level even when its name holds "/" (a Rust doc test "src/lib.rs - add (line 5)", a pytest case
   * "test_url[a/b]"); a Go subtest reads ["TestService_Request", "gateway_declines"]; a JavaScript test, the title of
   * each describe level and its own.
   */
  hierarchy: string[];
  /**
   * What the screen shows: the last level, with "_" read as spaces for Go only, such as "gateway declines".
   */
  title: string;
  /**
   * The shell command, from the project root, the runner says runs this test alone, as the run ran it (with -race
   * for a Go run that had it). Null when the runner did not say.
   */
  command: string | null;
  /** The top-level test function, null when the runner could not tell. */
  functionId: string | null;
  /** State in this review of the test function that defines it; "untouched" when unknown. */
  state: "new" | "modified" | "untouched";
  outcome: TestOutcome;
  durationMs: number;
  /** For a failed test, its output (at most 64 KiB). Null otherwise. */
  output: string | null;
  /** Where its test function is, on the new side, for "Open test in Diff". */
  file: string | null;
  line: number | null;
  /**
   * The changed production function this test is grouped under: the first new or modified function reached from its
   * test function in the new side's call graph, breadth first, calls in line order. Null when it reaches none.
   */
  exercises: FunctionRef | null;
  /** Whether its package has a changed file in this review. */
  inChangedPackage: boolean;
  /** For a failed test, why. */
  failure: TestFailure | null;
}

export interface PackageOutcome {
  container: string;
  outcome: "pass" | "fail" | "build_failed" | "no_tests";
  durationMs: number;
  /** For build_failed, or a failure no single test explains: compiler or framework output. Null otherwise. */
  output: string | null;
  /** Why, read from that output; null without it. */
  failure: TestFailure | null;
  /** The shell command, from the project root, the runner says runs this package's tests again; null when unknown. */
  command: string | null;
}

/** What a run measures besides the outcomes. */
export interface RunOptions {
  race: boolean;
  coverage: boolean;
}

/**
 * What a partial run runs. A package with `tests: null` runs whole; a test with `subtests: null` runs whole, else
 * only those subtests (named relative to it, as the framework prints them).
 */
export interface RunSelection {
  packages: { package: string; tests: { name: string; subtests: string[] | null }[] | null }[];
}

/** The last partial run of a new side: merged into the run shown when it completed. */
export interface PartialRun {
  startedAt: string;
  durationMs: number;
  status: "completed" | "failed" | "timed_out" | "cancelled";
  isolation: Isolation;
  /** Packages the selection named. */
  packages: number;
  /** Top-level tests it reported. */
  tests: number;
}

/** How far the run going on got. */
export interface TestProgress {
  packagesDone: number;
  packagesTotal: number;
  /** Top-level tests finished so far, and of those the failed ones. */
  tests: number;
  failed: number;
  /** A package running now. */
  current: string | null;
}

/** Lines that hold statements (coverable) and, of those, the ones a test ran (covered). */
export interface LineCoverage {
  covered: number;
  coverable: number;
}

export interface TestRun {
  language: string;
  runner: { name: string; version: string };
  oldSha: string;
  /** null: the working tree, as it was when the run started. */
  newSha: string | null;
  isolation: Isolation;
  /** RFC 3339. */
  startedAt: string;
  durationMs: number;
  /**
   * completed: the runner reported (tests may still have failed). failed: the runner could not report, see message.
   * timed_out: stopped after the time limit, nothing reported. cancelled: stopped by cancelTests.
   */
  status: "completed" | "failed" | "timed_out" | "cancelled";
  /**
   * For failed: the runner's error output (a string, at most 64 KiB), or why in the core's words (show it with
   * `coreText`). Null otherwise.
   */
  message: CoreMessage | null;
  /** Every test and subtest, failed ones first, then by container and name. */
  tests: TestCase[];
  /** Counted over `tests`, subtests included. added: tests whose test function is new in this review. */
  summary: { passed: number; failed: number; skipped: number; added: number };
  packages: PackageOutcome[];
  /**
   * Null when coverage was not measured. changedLines: over the lines this review adds or changes on the new side,
   * production code only. files: the same, per changed production file with coverable lines, by path. project: every
   * coverable line of the project's own code.
   */
  coverage: {
    changedLines: LineCoverage;
    files: ({ path: string } & LineCoverage)[];
    project: LineCoverage;
  } | null;
  /** The race detector was on. */
  raceDetector: boolean;
  races: { container: string; test: string | null; output: string }[];
  /** The runner's own (strings), then the core's (show each with `coreText`). */
  diagnostics: CoreMessage[];
  /** What the last run asked for, partial or not. */
  options: RunOptions;
  /**
   * The last partial run: when it completed, the tests it ran replaced theirs in this run and the rest kept their
   * results (startedAt and durationMs stay the whole run's); otherwise nothing changed. Null when the last run ran
   * everything. Coverage is the union of both runs on the same code, never brought by a partial run alone.
   */
  partial: PartialRun | null;
}

export type TestsUnavailable = { reason: "no_runner"; languages: string[] };

/**
 * The last run for these two sides, if any, and how the next run of this project will be isolated (the last
 * choice made with runTests, sandbox until then).
 */
export interface TestsState {
  run: TestRun | null;
  /** A run for these sides is going on right now (started from this or another window). */
  running: boolean;
  /** What that run runs; null when it runs everything or none runs. */
  runningSelection: RunSelection | null;
  isolation: Isolation;
  unavailable: TestsUnavailable | null;
}

/**
 * What getTestsState returns without running anything. Never executes project code. Fails with unknown_project, or
 * unknown_branch when a sha is not a commit.
 */
export const getTestsState = (projectId: string, oldSha: string, newSha: string | null) =>
  invoke<TestsState>("get_tests_state", { projectId, oldSha, newSha });

/**
 * Runs the project's unit tests on the new side of a review, on a throwaway copy of that commit (or of the working
 * tree when newSha is null), never in the project's folder: first the runner's prepare step as a plain process
 * (it may download dependencies, executes no project code), then the tests themselves isolated as asked. Records
 * `isolation` as the project's choice. Resolves when the run ends, however it ends, with the run as getTestsState
 * would return it; the run is kept per commit, so it survives a restart (working tree runs are kept in memory only).
 * A second call for the same sides while one runs joins it when it asks the same, and fails with internal
 * (test_run_busy) otherwise. Times out after 10 minutes. The run's TMPDIR is a short folder of its own (under
 * /private/tmp on macOS), so Unix sockets tests create there fit the path limit. `options` defaults to both on;
 * with a `selection`, only those tests run and their results are merged into the last complete run (see
 * TestRun.partial). Fails with unknown_project, unknown_branch when a sha is not a commit, or internal when the
 * project has no test runner.
 */
export const runTests = (
  projectId: string,
  oldSha: string,
  newSha: string | null,
  isolation: Isolation,
  options: RunOptions,
  selection: RunSelection | null,
) => invoke<TestRun>("run_tests", { projectId, oldSha, newSha, isolation, options, selection });

/** How far the run going on for this new side got; null when none runs or it has not said yet. */
export const getTestsProgress = (projectId: string, newSha: string | null) =>
  invoke<TestProgress | null>("get_tests_progress", { projectId, newSha });

/** Stops a run for these sides; its runTests call resolves with status "cancelled". No-op when none runs. */
export const cancelTests = (projectId: string, oldSha: string, newSha: string | null) =>
  invoke<void>("cancel_tests", { projectId, oldSha, newSha });

/** The family of change a mutant makes (analyzers/protocol/mutation.schema.json, rule 2). */
export type MutationOperator =
  | "relational_boundary"
  | "relational_negation"
  | "arithmetic"
  | "logical"
  | "negation_removal"
  | "return_value"
  | "call_removal"
  | "increment"
  | "other";

/**
 * killed: a test failed, so the tests verify this piece. survived: every test passed: they execute it without verifying
 * it. timeout: stopped after its time limit, most likely an infinite loop. error: it could not be tested (it does not
 * compile, its tests fail without it). no_coverage: the engine found that no test executes it.
 */
export type MutantStatus = "killed" | "survived" | "timeout" | "error" | "no_coverage";

/**
 * One mutant of a line the review adds. `original` is the text replaced between (line, column) and (endLine,
 * endColumn), 1-based columns in code points, endColumn exclusive; `mutated` what replaces it, empty for a removal.
 * `before` and `after` are the whole lines it spans on the new side as they are and as mutated; both empty when the
 * core could not read them.
 */
export interface Mutant {
  /** Unique in the run; the key of its signal (`mutant:<id>`). */
  id: string;
  file: string;
  line: number;
  column: number;
  endLine: number;
  endColumn: number;
  original: string;
  mutated: string;
  operator: MutationOperator;
  /** The engine's own words for it, in English. */
  description: string;
  status: MutantStatus;
  /** For a killed one, a test that failed, when the engine tells. */
  killedBy: string | null;
  durationMs: number;
  /** Its result was reused from an earlier run. */
  cached: boolean;
  /** The function of the new side holding it. */
  function: FunctionRef | null;
  before: string[];
  after: string[];
}

export interface MutationSummary {
  /** Mutants found on the lines tried, before the cap. */
  candidates: number;
  /** Only `maxMutants` of the candidates were tested, sampled evenly across the changed functions. */
  sampled: boolean;
  maxMutants: number;
  killed: number;
  survived: number;
  timeout: number;
  error: number;
  noCoverage: number;
  cached: number;
}

/** A mutation testing run of a review: the mutants of the lines it adds that the last test run with coverage ran. */
export interface MutationRun {
  language: string;
  runner: { name: string; version: string };
  /** What made and ran the mutants; null when the run did not report. */
  engine: { name: string; version: string } | null;
  oldSha: string;
  newSha: string | null;
  isolation: Isolation;
  /** RFC 3339. */
  startedAt: string;
  durationMs: number;
  /** As a test run's. Only completed runs are kept; runMutation may resolve with another. */
  status: "completed" | "failed" | "timed_out" | "cancelled";
  /** Why a failed run failed: the analyzer's own output, or the core's words (show it with `coreText`). */
  message: CoreMessage | null;
  /** When the test run whose coverage chose the lines started. */
  coverageRunAt: string | null;
  /** By file, line and column. */
  mutants: Mutant[];
  summary: MutationSummary;
  /** The analyzer's own, then the core's (show each with `coreText`). */
  diagnostics: CoreMessage[];
}

/** How far the mutation run going on got: baseline while the tests run on the unmutated code first. */
export interface MutationProgress {
  phase: "baseline" | "mutating";
  total: number;
  done: number;
  killed: number;
  survived: number;
  timeout: number;
  /** Errors and mutants no test executes. */
  error: number;
  /** "file:line" of a mutant being tested now. */
  current: string | null;
}

/**
 * Why mutation testing cannot run now. no_runner: no analyzer of the project's languages runs tests. needs_coverage:
 * there is no completed test run with coverage of the new side yet, which tells which changed lines the tests run.
 */
export type MutationUnavailable = { reason: "no_runner"; languages: string[] } | { reason: "needs_coverage" };

export interface MutationState {
  /** The last completed run of these sides. */
  run: MutationRun | null;
  /** A run for these sides is going on right now (started from this or another window). */
  running: boolean;
  unavailable: MutationUnavailable | null;
  /** The most mutants one run tests. */
  maxMutants: number;
}

/** The last mutation run of these sides, whether one goes on, and why none can run now. Runs nothing. */
export const getMutationState = (projectId: string, oldSha: string, newSha: string | null) =>
  invoke<MutationState>("get_mutation_state", { projectId, oldSha, newSha });

/**
 * Runs mutation testing on the lines the review adds that the last test run with coverage executed, isolated as the
 * project's tests are. Only on a click: it takes minutes, and resolves when it ends, however it ends. Fails with
 * mutation_needs_coverage, mutation_run_busy, no_test_runner or no_test_runner_no_languages.
 */
export const runMutation = (projectId: string, oldSha: string, newSha: string | null) =>
  invoke<MutationRun>("run_mutation", { projectId, oldSha, newSha });

/** How far the mutation run for this new side got; null when none runs or the analyzer has not said yet. */
export const getMutationProgress = (projectId: string, newSha: string | null) =>
  invoke<MutationProgress | null>("get_mutation_progress", { projectId, newSha });

/** Stops the mutation run for this new side; its runMutation call resolves with status "cancelled". */
export const cancelMutation = (projectId: string, newSha: string | null) =>
  invoke<void>("cancel_mutation", { projectId, newSha });

/** Limits the Quality tab marks as crossed. Fixed defaults until Project settings exist. */
export interface QualityLimits {
  /** Cyclomatic complexity per function. Default 10. */
  complexity: number;
  /** CRAP score per function. Default 30. */
  crap: number;
  /** Share of changed lines covered, 0 to 1. Default 0.8. */
  changedLinesCoverage: number;
  /** Normalized tokens a copy must share to count as duplicated code. Default 50. */
  duplicationTokens: number;
  /** Lines a copy must span to count as duplicated code (both minimums apply). Default 6. */
  duplicationLines: number;
}

export interface QualityFunction extends FunctionRef {
  /** Short name, such as "Service.Request". */
  name: string;
  container: string;
  file: string;
  startLine: number;
  endLine: number;
  state: "new" | "modified";
  /** On the new side; null when the analyzer could not compute it. */
  complexity: number | null;
  /** On the old side; null for a new function. */
  previousComplexity: number | null;
  /** Coverage of this function's changed lines in the run used; null without a completed run. */
  changedLines: LineCoverage | null;
  /**
   * CRAP score from complexity and the coverage of all the function's lines (not only the changed ones):
   * complexity² × (1 − coverage)³ + complexity. Null without complexity or a completed run.
   */
  crap: number | null;
  /** The run used measured this function's lines and none of them ran: no test executes it. False without one. */
  untested: boolean;
  /**
   * The function's lines on the new side, from `startLine`, only for a function that crosses a limit (to hand to a
   * coding agent), cut at 400 lines; empty for the others and when the file could not be read.
   */
  code: string[];
}

/**
 * Where the coverage and CRAP of the functions come from: measured by the run used; no_run without a completed run;
 * off when that run was asked not to measure coverage; not_measured when it was asked and measured none, `reasons`
 * being the analyzer's own words about why (such as a coverage tool missing from the project).
 */
export interface QualityCoverage {
  state: "measured" | "no_run" | "off" | "not_measured";
  reasons: string[];
}

/**
 * A limit crossed: complexity or CRAP above its limit, or the coverage of changed lines below its target. `function`
 * is null for the coverage of the whole review's changed lines; a function's own changed lines are one of its limits.
 */
export interface CrossedLimit {
  kind: "complexity" | "crap" | "changed_lines_coverage";
  function: FunctionRef | null;
  /** The measured value: complexity, CRAP, or coverage from 0 to 1. */
  value: number;
  limit: number;
}

export interface StaticCheck {
  /** Such as "go vet" or "staticcheck". */
  tool: string;
  status: "clean" | "findings" | "unavailable" | "failed";
  message: string | null;
  /** `severity` is "warning" for documents kept from before tools reported it. */
  findings: { file: string; line: number; message: string; rule: string | null; severity: CheckSeverity }[];
}

/** How serious the tool says a finding is, error the most. */
export type CheckSeverity = "error" | "warning" | "info";

/**
 * Go vet and staticcheck's SA checks point at defects; staticcheck's S, ST, QF and U checks at style. Another tool's
 * finding is style when its severity is info, correctness otherwise.
 */
export type CheckClass = "correctness" | "style";

/** A static check finding on a line the review adds, with the changed production function holding it, if any. */
export interface LocatedCheck {
  tool: string;
  rule: string | null;
  file: string;
  line: number;
  /** The tool's own words, in English, never translated. */
  message: string;
  class: CheckClass;
  severity: CheckSeverity;
  function: FunctionRef | null;
}

/**
 * copies_existing: identical, or identical but for identifiers and literals, to code the review did not add.
 * repeated_in_change: the same block two or more times in the code the review adds.
 */
export type DuplicateKind = "copies_existing" | "repeated_in_change";

/** One occurrence of duplicated code on the new side, with its lines (at most 80; empty when unreadable). */
export interface DuplicateBlock {
  file: string;
  startLine: number;
  endLine: number;
  code: string[];
}

/** Duplicated code the review introduces: `block` on lines it adds; `others` the code it copies or its repetitions. */
export interface Duplicate {
  kind: DuplicateKind;
  block: DuplicateBlock;
  others: DuplicateBlock[];
  /** Normalized tokens the copies share. */
  tokens: number;
  /** Lines of `block`. */
  lines: number;
  /** The changed production function holding the block, if any. */
  function: FunctionRef | null;
}

export interface QualityReport {
  language: string;
  oldSha: string;
  newSha: string | null;
  limits: QualityLimits;
  /**
   * Every new and modified production function (not tests, not generated), by complexity descending, then by
   * qualified name.
   */
  functions: QualityFunction[];
  /** Function limits in the order of `functions`, the review-wide coverage limit last. */
  crossed: CrossedLimit[];
  /** The static checkers on the new side, in the analyzer's order. */
  checks: StaticCheck[];
  /** The findings of `checks` on lines the review adds, by file and line. */
  checkFindings: LocatedCheck[];
  /**
   * Duplicated production code the review introduces, by the block's file and line: tests and translation catalogs
   * are never in it, as copies or as originals.
   */
  duplicates: Duplicate[];
  /** Duplicated test code the review introduces, copies between tests only, by the block's file and line. */
  testDuplicates: Duplicate[];
  /**
   * From the last completed test run for these sides: not_run when there is none or its race detector was off,
   * otherwise clean or races.
   */
  raceDetector: "not_run" | "clean" | "races";
  /** When the run that coverage, CRAP and races come from started, or null when there is none. */
  testRunAt: string | null;
  coverage: QualityCoverage;
  /** The checkers' own (strings), and why a checker run failed in the core's words (show each with `coreText`). */
  diagnostics: CoreMessage[];
}

export type QualityResult =
  | { kind: "report"; report: QualityReport }
  | ({ kind: "unavailable" } & FlowUnavailable);

/**
 * Quality of a review's changed functions: complexity from the same analyses as getFlow (cached), coverage, CRAP and
 * races from the last completed test run for these sides if any, and the static checkers run on the new side
 * (cached per commit, never on the working tree's own folder). Never executes project code. Fails with
 * unknown_project, or unknown_branch when a sha is not a commit.
 */
export const getQuality = (projectId: string, oldSha: string, newSha: string | null) =>
  invoke<QualityResult>("get_quality", { projectId, oldSha, newSha });

/**
 * A review as the screen has it open: its target and the two sides its ChangeSet froze. Marks and comments are taken
 * against these sides, so what is recorded is what the user was looking at.
 */
export interface ReviewRef {
  target: ReviewTarget;
  oldSha: string;
  newSha: string | null;
}

/** Where a changed function is, from FlowNode: on the new side, or on the old side when deleted. */
export interface FunctionSpan {
  id: string;
  file: string;
  startLine: number;
  endLine: number;
  deleted: boolean;
}

/** reviewed: its content is still what was reviewed. reopened: it changed since it was marked. */
export type MarkStatus = "reviewed" | "reopened";

export interface MarkedFile {
  path: string;
  status: MarkStatus;
  /** RFC 3339. */
  reviewedAt: string;
}

export interface MarkedFunction {
  /** FlowNode.id. */
  id: string;
  status: MarkStatus;
  reviewedAt: string;
}

/**
 * A line comment. Written here it is a draft kept only in DiffLens until "Publish comments"; published, or brought
 * in from the pull request, it carries the remote id and author. A comment with remoteAuthorId null was written here
 * by the user ("You") and not published yet.
 */
/**
 * Where a comment is: on a line of a file of the diff, or on the pull request as a whole (a general comment, on no file
 * or line: its path and lineText are empty, its line 0, its currentLine null, and it is never outdated).
 */
export type CommentKind = "line" | "general";

export interface ReviewComment {
  id: string;
  /** A reply has the kind of its thread. */
  kind: CommentKind;
  /** The thread's first comment, for a reply (threads are one level deep). Null for the first comment. */
  parentId: string | null;
  path: string;
  side: "old" | "new";
  /** Line it was written on, on that side. */
  line: number;
  /** Text of that line then. */
  lineText: string;
  /** Head when it was written; null for the working tree. */
  commitSha: string | null;
  body: string;
  state: "draft" | "published";
  remoteId: string | null;
  remoteAuthorId: string | null;
  remoteAuthorName: string | null;
  /** Set on a thread's first comment; it stands for the whole thread. */
  resolved: boolean;
  /** RFC 3339. */
  createdAt: string;
  updatedAt: string;
  /**
   * Line to show it under in this review: its own line while it reads the same, else the nearest line (within 50)
   * that reads exactly as it did, else its own line kept inside the file. Null when the file or that side is no longer
   * part of the review.
   */
  currentLine: number | null;
  /** Its own line no longer reads as it did ("Code changed since this comment"). */
  outdated: boolean;
  /** Written by the user, here or on the pull request by the connected account. Only these can be edited and deleted. */
  mine: boolean;
  /** A change to a published comment that waits for "Publish comments". */
  pendingChange: "edit" | "delete" | null;
  /** Why the last attempt to publish it failed (shown with `commentPublishError`). */
  publishError: ApiError | null;
}

export type SinceUnavailableReason =
  /** The working tree has no commits to compare. */
  | "working_tree"
  /** Nothing was marked as reviewed yet. */
  | "no_checkpoint"
  /** The head is still the commit of the last review. */
  | "up_to_date"
  /** The commit of the last review is gone from the repository (history rewritten). */
  | "checkpoint_missing";

/**
 * What changed from the checkpoint (the head at the most recent review action) to the review's head. `files` is a
 * change set from checkpointSha to the head: each file's diff is read with getFileVersions(checkpointSha, newSha).
 */
export type SinceLastReview =
  | { kind: "available"; checkpointSha: string; commits: number; files: ChangedFile[] }
  | { kind: "unavailable"; reason: SinceUnavailableReason };

export interface ReviewState {
  /** Marked files that are in the change set; any other file is unreviewed. */
  files: MarkedFile[];
  /** Marked functions among those asked about; any other is unreviewed. */
  functions: MarkedFunction[];
  /** Oldest first, replies included. */
  comments: ReviewComment[];
  /** The most recent review action (marking a file or a function), null before the first. */
  checkpoint: { sha: string | null; at: string } | null;
  since: SinceLastReview;
  summary: {
    totalFiles: number;
    reviewedFiles: number;
    reopenedFiles: number;
    /** Among the functions asked about. */
    reopenedFunctions: number;
    filesChangedSince: number;
    /** Unresolved threads whose line no longer reads as it did. */
    outdatedComments: number;
    /** Drafts, replies, edits and deletes "Publish comments" would send. */
    unpublished: number;
    /** Draft threads held back because their line changed since they were written. */
    staleDrafts: number;
  };
  /** Drafts written from a finding or a static check, while they are there (not deleted). */
  signalDrafts: SignalDraft[];
}

/** A review comment drafted from a model finding or a static check finding, linked to it until it is deleted. */
export interface SignalDraft {
  /** `finding:<id>`, `check:<tool>:<rule>:<path>:<line>:<message>`, `duplicate:…`, or `mutant:<id>`. */
  signal: string;
  commentId: string;
  kind: CommentKind;
}

/** What a draft is written from: the signal, where it is on the new side, and the comment's text. */
export interface SignalDraftRequest {
  signal: string;
  path: string;
  line: number;
  endLine: number;
  body: string;
}

/**
 * Everything recorded in a review (by project and target, in DiffLens's own database, never in the project), read
 * against the review's sides. Function statuses are given for `functions`, the changed functions of the flow graph.
 * Fails with unknown_project, or unknown_branch when a side is not a commit.
 */
export const getReviewState = (projectId: string, review: ReviewRef, functions: FunctionSpan[]) =>
  invoke<ReviewState>("get_review_state", { projectId, review, functions });

/**
 * Marks a changed file as reviewed with a fingerprint of its new side (its blob, or its bytes on disk for the
 * working tree; deleted files get a fixed one), or with reviewed false forgets the mark. Marking moves the checkpoint
 * to the review's head. Fails with unknown_file for a file the review does not change.
 */
export const setFileReviewed = (projectId: string, review: ReviewRef, path: string, reviewed: boolean) =>
  invoke<void>("set_file_reviewed", { projectId, review, path, reviewed });

/** Like setFileReviewed, for a changed function, fingerprinted by its source lines on the new side. */
export const setFunctionReviewed = (projectId: string, review: ReviewRef, fn: FunctionSpan, reviewed: boolean) =>
  invoke<void>("set_function_reviewed", { projectId, review, function: fn, reviewed });

/**
 * Comments one line of one side of a changed file, as a draft. Resolves to the comment id. Fails with unknown_file
 * when the file is not changed, that side has no text or no such line.
 */
export const addComment = (
  projectId: string,
  review: ReviewRef,
  path: string,
  side: "old" | "new",
  line: number,
  body: string,
) => invoke<string>("add_comment", { projectId, review, path, side, line, body });

/**
 * Comments the pull request as a whole (a general comment, on no file or line): a draft until "Publish comments", like
 * a line comment. Resolves to the new comment's id.
 */
export const addGeneralComment = (projectId: string, review: ReviewRef, body: string) =>
  invoke<string>("add_general_comment", { projectId, review, body });

/**
 * Writes a draft review comment from a model finding or a static check finding, with the text given (no model is
 * asked): on the last of its lines the review adds or changes, on the pull request as a whole otherwise, starting
 * with the place it is about in the language shown now. While the draft written before is there, resolves to it.
 */
export const draftSignalComment = (projectId: string, review: ReviewRef, request: SignalDraftRequest) =>
  invoke<SignalDraft>("draft_signal_comment", { projectId, review, request, language: currentLanguage() });

/**
 * Asks the model of the Insights task, only after a click on Write comment, to write a short review comment from the
 * model finding `findingId` (the problem, why it matters, a suggestion and code only when it helps), in the language
 * shown now, and keeps it as a draft linked to the finding (its signal `finding:<id>`), placed like draftSignalComment.
 * While that draft is there, resolves to it without a call. `runId` lets cancelModelRun stop it (it then fails with
 * cancelled). Fails with model_not_configured when the task's model is not set up, unknown_finding, or a model error;
 * nothing is kept then.
 */
export const writeFindingComment = (projectId: string, review: ReviewRef, findingId: string, runId: string) =>
  invoke<SignalDraft>("write_finding_comment", {
    projectId,
    review,
    findingId,
    language: currentLanguage(),
    runId,
  });

/** Replies in the thread of `parentId` (a reply to a reply joins the same thread). Resolves to the new id. */
export const replyToComment = (projectId: string, parentId: string, body: string) =>
  invoke<string>("reply_to_comment", { projectId, parentId, body });

/**
 * Only for the user's own comments (mine). A published one keeps its old text on the pull request until the next
 * publish (pendingChange "edit"). Fails with unknown_comment when it is gone.
 */
export const editComment = (projectId: string, commentId: string, body: string) =>
  invoke<void>("edit_comment", { projectId, commentId, body });

/**
 * Deletes one of the user's comments; deleting a thread's first comment deletes its replies. A published one is only
 * marked (pendingChange "delete") and leaves the pull request on the next publish; restoreComment takes that back.
 */
export const deleteComment = (projectId: string, commentId: string) =>
  invoke<void>("delete_comment", { projectId, commentId });

/** Takes back the delete of a published comment that was not published yet. */
export const restoreComment = (projectId: string, commentId: string) =>
  invoke<void>("restore_comment", { projectId, commentId });

/**
 * Resolves or reopens the thread the comment belongs to. A thread already on the pull request is resolved there too,
 * at once (it is not part of the publish batch); that can fail like any host call (auth_failed, network_failed…).
 */
export const setCommentResolved = (projectId: string, commentId: string, resolved: boolean) =>
  invoke<void>("set_comment_resolved", { projectId, commentId, resolved });

/** An account on a pull request host. `id` is the host's stable id, the one comments are attributed with. */
export interface Account {
  id: string;
  name: string;
}

/** A pull request host DiffLens can connect to, and whether it is. The token never leaves the core. */
export interface Connection {
  /** Such as "bitbucket" or "github". */
  host: string;
  /** Such as "Bitbucket Cloud" or "GitHub". */
  hostName: string;
  /** Permissions the API token needs, as the host names them, such as "read:pullrequest:bitbucket" or "repo". */
  scopes: string[];
  /**
   * Whether the user enters a user name besides the token (the Atlassian account email for Bitbucket). False for
   * GitHub, whose token alone says who it is: testing and saving take an empty user name.
   */
  needsUsername: boolean;
  /** The saved user name (the Atlassian account email for Bitbucket, the login for GitHub), null when not connected. */
  username: string | null;
  /** The account the saved token belongs to, found when it was saved. */
  account: Account | null;
  /** RFC 3339. */
  connectedAt: string | null;
}

/** Every supported host, connected or not. */
export const getConnections = () => invoke<Connection[]>("get_connections");

/**
 * Asks the host who `username` and `token` belong to, without saving anything. With token null, the saved token is
 * used (only for the saved user name; otherwise not_connected). Fails with auth_failed, missing_scope, network_failed…
 */
export const testConnection = (host: string, username: string, token: string | null) =>
  invoke<Account>("test_connection", { host, username, token });

/** Checks the credential with the host, then keeps the token in the macOS Keychain and the account in DiffLens. */
export const saveConnection = (host: string, username: string, token: string) =>
  invoke<Connection>("save_connection", { host, username, token });

/** Forgets the connection and deletes its token from the Keychain. */
export const disconnect = (host: string) => invoke<void>("disconnect", { host });

/** A language DiffLens is written in, as a BCP 47 tag. Also the language the model writes findings and summaries in. */
export type Language = "pt-BR" | "en";

/** The language chosen in Preferences: a language, or "system" to follow the operating system's. */
export type LanguagePreference = "system" | Language;

/** The language chosen in Preferences; "system" until one is chosen. */
export const getLanguagePreference = () => invoke<LanguagePreference>("get_language_preference");

/** Keeps the language chosen in Preferences. */
export const setLanguagePreference = (preference: LanguagePreference) =>
  invoke<void>("set_language_preference", { preference });

/** Who answers when DiffLens asks a model. claude_cli is the claude command of Claude Code, with its own sign-in. */
export type ModelProviderKey = "claude_cli" | "anthropic" | "openai" | "gemini" | "openrouter";

/** How hard the model thinks before answering; only the Anthropic API takes it. */
export type Effort = "low" | "medium" | "high" | "xhigh" | "max";

export const EFFORTS: Effort[] = ["low", "medium", "high", "xhigh", "max"];

/** A model a provider offers. */
export interface ModelInfo {
  /** What the provider is asked for, such as "claude-opus-5-5". */
  id: string;
  /** Such as "Claude Opus 5.5"; the id when the provider has no name. */
  name: string;
}

export interface ClaudeCliStatus {
  /** Where claude was found; null when it was not. */
  path: string | null;
  /** Such as "2.1.328 (Claude Code)"; null when it could not be read. */
  version: string | null;
}

export interface ModelProviderState {
  provider: ModelProviderKey;
  /** Such as "Anthropic API". */
  name: string;
  /** False for Claude Code, which uses its own sign-in. */
  needsKey: boolean;
  /** An API key is in the Keychain. The key itself never comes back from the core. */
  keySaved: boolean;
  /** Where the user creates an API key; empty for Claude Code. */
  keyUrl: string;
  model: string | null;
  /** Only for the providers that take one (the Anthropic API). */
  effort: Effort | null;
  /** Offered without asking the provider: the aliases Claude Code accepts. Empty for the APIs. */
  suggestedModels: ModelInfo[];
  /** Only for Claude Code. */
  cli: ClaudeCliStatus | null;
  /** It has everything it needs to be asked: a key when it needs one, a model, the claude command. */
  ready: boolean;
  /** It can be asked once given a model: a key is saved, or the claude command was found. Only these can be chosen for a task. */
  configured: boolean;
}

/**
 * What DiffLens asks a model for, each with a model of its own or the default one. summary is the Overview summary
 * with the card's criteria (one call); brief_explanation is Explain (functions, test functions included, groups and
 * a full analysis); detailed_explanation is Explain in detail; test_diagnosis is Explain the failure on the Tests tab;
 * review_comment is Write review comment, from the explanation of a failure; translation is every Translate.
 */
export type ModelTask =
  | "insights"
  | "summary"
  | "brief_explanation"
  | "detailed_explanation"
  | "test_diagnosis"
  | "review_comment"
  | "translation";

/** Every task, in the order Preferences lists them. A new task is one more entry here and its name in the catalogs. */
export const MODEL_TASKS: readonly ModelTask[] = [
  "insights",
  "summary",
  "brief_explanation",
  "detailed_explanation",
  "test_diagnosis",
  "review_comment",
  "translation",
];

/** A model chosen for one task instead of the default. */
export interface ModelTaskChoice {
  provider: ModelProviderKey;
  model: string;
  /** Only for the providers that take one (the Anthropic API). */
  effort: Effort | null;
}

export interface ModelTaskState {
  task: ModelTask;
  /** null: the task uses the default model. */
  choice: ModelTaskChoice | null;
  /**
   * The provider chosen for it is not configured any more: its calls fail, naming the task, until it is set up again
   * or another model is chosen. It never falls back to the default on its own.
   */
  needsAttention: boolean;
}

export interface ModelSettings {
  /** The provider of the default model, which every task without a model of its own uses; Claude Code until the user picks another. */
  activeProvider: ModelProviderKey;
  /** Every provider, configured or not. */
  providers: ModelProviderState[];
  /** Every task, in MODEL_TASKS order. */
  tasks: ModelTaskState[];
}

export interface ModelKeySaved {
  settings: ModelSettings;
  /** The models the key can use, listed when it was checked. */
  models: ModelInfo[];
}

/** The answer to the small question Test asks. */
export interface ModelTest {
  provider: ModelProviderKey;
  providerName: string;
  /** The model that answered, as the provider reports it. */
  model: string;
  /** What the model wrote. */
  reply: string;
  elapsedMs: number;
  inputTokens: number | null;
  outputTokens: number | null;
}

/** Model settings and the state of every provider. Asks no model; for Claude Code it runs `claude --version`. */
export const getModelSettings = () => invoke<ModelSettings>("get_model_settings");

/** Makes `provider` the one reviews are read by. */
export const setModelProvider = (provider: ModelProviderKey) =>
  invoke<ModelSettings>("set_model_provider", { provider });

/** Saves the model (blank for none) and, for the Anthropic API, the effort chosen for `provider`. */
export const saveModelChoice = (provider: ModelProviderKey, model: string, effort: Effort | null) =>
  invoke<ModelSettings>("save_model_choice", { provider, model, effort });

/**
 * Gives `task` a model of its own, or (null) puts it back on the default. Only a configured provider can be chosen;
 * fails with model_not_configured otherwise, or without a model.
 */
export const setModelTask = (task: ModelTask, choice: ModelTaskChoice | null) =>
  invoke<ModelSettings>("set_model_task", { task, choice });

/**
 * Checks the key with the provider by listing its models (no model is asked), then keeps it in the macOS Keychain.
 * The key is sent once, here. Fails with auth_failed, network_failed… and then nothing is saved.
 */
export const setModelKey = (provider: ModelProviderKey, key: string) =>
  invoke<ModelKeySaved>("set_model_key", { provider, key });

/** Deletes the provider's key from the Keychain. */
export const removeModelKey = (provider: ModelProviderKey) =>
  invoke<ModelSettings>("remove_model_key", { provider });

/**
 * The models a provider offers for the saved key; Claude Code's aliases for claude_cli. Fails with
 * model_not_configured without a key.
 */
export const listModels = (provider: ModelProviderKey) => invoke<ModelInfo[]>("list_models", { provider });

/**
 * Asks the default model a tiny question with a tiny schema, only when the user clicks Test. `runId` names the call
 * so cancelModelRun can stop it (it then fails with cancelled).
 */
export const testModel = (runId: string) => invoke<ModelTest>("test_model", { runId });

/** Stops the model call `runId`. Does nothing when it already ended. */
export const cancelModelRun = (runId: string) => invoke<void>("cancel_model_run", { runId });

/** Who would be asked now for a task (its own model or the default one), from the settings, and whether they can be. */
export interface ModelInUse {
  provider: ModelProviderKey;
  providerName: string;
  model: string | null;
  ready: boolean;
  /**
   * What to do in Preferences › Model when it is not ready, such as "Save an API key for the OpenAI API…", as the
   * error asking would fail with (shown with `modelProblem`).
   */
  problem: ApiError | null;
  /** Calls of one analysis asked at once: 1 for Claude Code, 3 for the APIs. */
  parallelCalls: number;
}

/** How much a finding would hurt. high: wrong result, data loss, security hole or crash on a realistic path. */
export type Severity = "high" | "medium" | "low";

export type FindingCategory =
  | "correctness"
  | "concurrency"
  | "security"
  | "data_loss"
  | "error_handling"
  | "tests"
  | "observability";

/** What the user concluded about a finding. */
export type FindingVerdict = "pending" | "confirmed" | "dismissed";

/**
 * How a finding came to be marked fixed. model_checked: a later analysis showed the model the finding with the code as
 * it is now and it answered that the problem is gone. file_left_change: a later analysis found its file no longer part
 * of the change, with no model asked. user_marked: the user marked it fixed.
 */
export type ResolutionKind = "model_checked" | "file_left_change" | "user_marked";

/** A finding marked fixed: how, by which model (model_checked only), when and why. Its verdict stays as it was. */
export interface FindingResolution {
  kind: ResolutionKind;
  /** The model that answered, for model_checked only. */
  model: string | null;
  /** RFC 3339. */
  at: string;
  /** The model's one sentence on why it is fixed, for model_checked only. */
  reason: string | null;
}

export type LocationProblem =
  /** The path is not a changed file of the review (any more). */
  | "not_changed"
  /** The file is changed, but the model was not shown it in the call that wrote the finding. */
  | "not_sent"
  /** The line is not in the new side of the file. */
  | "outside_file"
  /** The cited lines no longer read as they did and were not found again in the file. */
  | "code_changed";

/** Where a finding is in the review now: its cited lines, followed if they moved within the file. */
export type FindingLocation =
  | { kind: "found"; line: number; endLine: number; moved: boolean }
  | { kind: "not_found"; problem: LocationProblem };

/** A line of an excerpt, read from the file at the review's new side, never from the model. */
export interface ExcerptLine {
  line: number;
  text: string;
  /** One of the lines the finding cites. */
  cited: boolean;
}

/** The analysis that wrote a finding. */
export interface FindingSource {
  analysisId: string;
  provider: ModelProviderKey;
  providerName: string;
  /** The model that answered. */
  model: string;
  /** RFC 3339. */
  analyzedAt: string;
  /** Head the analysis read; null for the working tree. */
  headSha: string | null;
  /** From an analysis before the latest one: later analyses re-check it rather than replace it. */
  earlier: boolean;
}

export interface Finding {
  id: string;
  source: FindingSource;
  title: string;
  severity: Severity;
  category: FindingCategory;
  /** As the model cited it, at the analysis's head. */
  path: string;
  line: number;
  endLine: number;
  location: FindingLocation;
  /** The cited lines and two around them; empty when the location is not found. */
  excerpt: ExcerptLine[];
  explanation: string;
  suggestedDirection: string;
  /** What in the code shows it, citing lines; read from the code, not reproduced. */
  evidence: string[];
  /** Related, but not proof. */
  related: string[];
  verdict: FindingVerdict;
  /** RFC 3339; null while pending. */
  verdictAt: string | null;
  /** Confirmed or dismissed, not fixed, and the code around it changed since (or can no longer be found). */
  recheck: boolean;
  /** Set once it is marked fixed, by a later analysis or by the user; null while it is open. */
  resolution: FindingResolution | null;
  /** The language the model wrote the text above in: its analysis's. Analyses before languages are "en". */
  language: Language;
  /** The same text in other languages, each translated on a click on Translate. Never the original's language. */
  translations: FindingTranslation[];
}

/** The text of a finding the model wrote, as it reads in one language. */
export interface FindingProse {
  title: string;
  explanation: string;
  suggestedDirection: string;
  evidence: string[];
  related: string[];
}

export interface FindingTranslation extends FindingProse {
  language: Language;
}

/** One call of an analysis: the files it sends. pending: not asked yet, or stopped before it was. */
export interface InsightsBatch {
  index: number;
  paths: string[];
  tokens: number;
  status: "pending" | "done" | "failed";
  /** Why it failed (rate_limited, model_truncated, cancelled…). */
  error: ApiError | null;
}

export interface InsightsAnalysis {
  id: string;
  headSha: string | null;
  provider: ModelProviderKey;
  providerName: string;
  /** The model asked for, or the one that answered once one did. */
  model: string;
  /** RFC 3339. */
  createdAt: string;
  /** Calls to run at once. */
  parallelCalls: number;
  batches: InsightsBatch[];
  /** The language its findings are written in: the one shown when Analyze was clicked. */
  language: Language;
}

export interface Insights {
  /** The latest analysis, null before the first. */
  analysis: InsightsAnalysis | null;
  /**
   * By severity. Findings of earlier analyses stay, with source.earlier: a new analysis re-checks the open ones and
   * marks fixed those it finds fixed (resolution), and deletes none.
   */
  findings: Finding[];
}

export interface InsightsPlan {
  /** Files sent. */
  files: number;
  /** Listed to the model but not sent. */
  skipped: { path: string; reason: "deleted" | "binary" | "too_large" | "not_text" }[];
  /** Each call's files, estimated input tokens (characters / 4, prompt included) and files sent as changed lines only. */
  calls: { paths: string[]; tokens: number; truncated: string[] }[];
  tokens: number;
  model: ModelInUse;
}

/** The findings of a review, read against its code (relocated, excerpts, rechecks), and its latest analysis. */
export const getInsights = (projectId: string, review: ReviewRef) =>
  invoke<Insights>("get_insights", { projectId, review });

/** What Analyze would send, in how many calls, and to whom. Asks no model. */
export const planInsights = (projectId: string, review: ReviewRef) =>
  invoke<InsightsPlan>("plan_insights", { projectId, review });

/**
 * Records a new analysis and its calls, all pending. Asks no model: run each call with runInsightsBatch. Fails with
 * model_not_configured when the model in use is not ready. Its findings are written in the language shown now.
 */
export const startInsights = (projectId: string, review: ReviewRef) =>
  invoke<Insights>("start_insights", { projectId, review, language: currentLanguage() });

/** What Translate would send: the findings not yet in `language`, in calls, and to whom. Asks no model. */
export interface TranslationPlan {
  /** Pieces of text to translate: findings, or 1 for a summary. 0 when everything already reads in the language. */
  items: number;
  /** Each call's finding ids (empty for a summary) and estimated input tokens (characters / 4, prompt included). */
  calls: { ids: string[]; tokens: number }[];
  tokens: number;
  model: ModelInUse;
}

/** What translating the review's findings into `language` would send and to whom. Asks no model. */
export const planInsightsTranslation = (projectId: string, review: ReviewRef, language: Language) =>
  invoke<TranslationPlan>("plan_insights_translation", { projectId, review, language });

/**
 * Asks the model in use to translate the text of the findings `findingIds` into `language`, only after the user
 * clicked Translate: the same findings, verdicts and locations, only their text in another language, kept as one more
 * version of each. A finding the answer leaves out keeps its original text. `runId` lets cancelModelRun stop it (it then
 * fails with cancelled). Fails with model_not_configured or a model error; nothing is kept then.
 */
export const translateInsights = (
  projectId: string,
  review: ReviewRef,
  language: Language,
  findingIds: string[],
  runId: string,
) => invoke<Insights>("translate_insights", { projectId, review, language, findingIds, runId });

/**
 * Asks the model call `batch` of the latest analysis, only after the user clicked Analyze or Retry. A failed or
 * stopped call is recorded on the call (its error) and still resolves; it fails with unknown_finding when a newer
 * analysis replaced this one. `runId` lets cancelModelRun stop it.
 */
export const runInsightsBatch = (
  projectId: string,
  review: ReviewRef,
  analysisId: string,
  batch: number,
  runId: string,
) => invoke<Insights>("run_insights_batch", { projectId, review, analysisId, batch, runId });

/** Confirms, dismisses or sets back to pending a finding; a verdict keeps a fingerprint of the code around it. */
export const setFindingVerdict = (projectId: string, review: ReviewRef, findingId: string, verdict: FindingVerdict) =>
  invoke<Insights>("set_finding_verdict", { projectId, review, findingId, verdict });

/** Marks a finding fixed by the user (`fixed`), or opens it again whoever marked it. Asks no model. */
export const setFindingFixed = (projectId: string, review: ReviewRef, findingId: string, fixed: boolean) =>
  invoke<Insights>("set_finding_fixed", { projectId, review, findingId, fixed });

export interface PullRequest {
  number: number;
  title: string;
  /** Kept for the model, which reads it with the diff. */
  description: string;
  state: "open" | "merged" | "declined" | "superseded";
  /** Page of the pull request on the host. */
  url: string;
  author: string | null;
  sourceBranch: string;
  /** Abbreviated as the host gives it (12 characters on Bitbucket). */
  sourceCommit: string | null;
  destinationBranch: string;
  destinationCommit: string | null;
  /** Reviewers and everyone who took part (commented), with their verdict. The connected account may be one. */
  participants: Participant[];
}

/** What a reviewer concluded about a pull request. */
export type Verdict = "approved" | "changes_requested";

export interface Participant {
  account: Account;
  /** Asked to review, rather than only taking part. */
  reviewer: boolean;
  /** null while they have not concluded. */
  verdict: Verdict | null;
}

/** Whether "Publish comments" may send now, and if not, why ("push first, then Reload"…). */
export interface PublishReadiness {
  allowed: boolean;
  /** Why not, as the publish_blocked error sending would fail with (shown with `readinessReason`). */
  reason: ApiError | null;
}

/** The pull request of a review, or why there is none. */
export type PullRequestLink =
  /** Uncommitted changes have no pull request. */
  | { kind: "working_tree" }
  /** The project has no origin, or origin is not on a server. */
  | { kind: "no_remote" }
  /** Origin is on a host DiffLens has no integration for, such as "github.com". */
  | { kind: "unsupported_host"; host: string }
  | { kind: "not_connected"; hostName: string }
  /** The host has no open pull request from the reviewed branch. */
  | { kind: "no_pull_request"; hostName: string; branch: string }
  | {
      kind: "linked";
      hostName: string;
      pullRequest: PullRequest;
      /** The connected account: comments by it are the user's own. */
      account: Account;
      publish: PublishReadiness;
      /** Whether Approve and Request changes may be used: the pull request is open at the commit the review shows. */
      verdict: PublishReadiness;
    };

/**
 * Finds the open pull request whose source branch is the review's branch (preferring one into the review's base) and
 * brings its inline comments into the review state, by remote id; the review state must be read again afterwards.
 * Comments on the whole pull request or a whole file are not brought in. Only reads from the host. Fails with
 * auth_failed, missing_scope, remote_not_found, rate_limited, network_failed or remote_failed.
 */
export const linkPullRequest = (projectId: string, review: ReviewRef) =>
  invoke<PullRequestLink>("link_pull_request", { projectId, review });

/** A change the host refused, with the error's fields alongside (shown with `publishFailureMessage`). */
export interface PublishFailure extends CoreText {
  commentId: string;
  path: string;
  line: number;
  /** Always sent by the core. */
  code?: ErrorCode;
  detail?: string;
}

export interface PublishReport {
  /** Changes the host accepted. */
  published: number;
  /** Kept as they were, each with its reason (also in the comment's publishError). */
  failed: PublishFailure[];
  /** Draft threads left alone because their line changed. */
  stale: number;
}

/**
 * Sends the review's drafts, replies, edits and deletes to its pull request, in one go. Refused as a whole with
 * publish_blocked (or not_connected) when the review does not show the pull request's diff; otherwise each change the
 * host refuses stays as it was here and is listed in `failed`. An expired token, a missing scope, a rate limit or a
 * lost connection stops the rest, which are listed too. On GitHub, the new threads on lines go as one review, accepted
 * or refused together; replies and comments on the whole pull request go one by one.
 */
export const publishComments = (projectId: string, review: ReviewRef) =>
  invoke<PublishReport>("publish_comments", { projectId, review });

/**
 * Approves or requests changes as the connected account (given true), or takes that back (given false). Giving one
 * verdict takes back the other first (on GitHub, the new review replaces it). Resolves to the pull request as it is
 * afterwards (comments are pulled again too, so read the review state again). Refused with publish_blocked unless the
 * pull request is open at the review's head. Needs the write:pullrequest:bitbucket scope on Bitbucket; without it,
 * fails with missing_scope. On GitHub, taking a verdict back dismisses the review, which needs write access to the
 * repository; without it, fails with missing_scope.
 */
export const setPullRequestVerdict = (projectId: string, review: ReviewRef, verdict: Verdict, given: boolean) =>
  invoke<PullRequestLink>("set_pull_request_verdict", { projectId, review, verdict, given });

/**
 * Pins the base of one branch (source becomes "user"), or with base null goes back to the one found
 * from history. `base` must be one of ProjectDetail.bases. Returns the branch as getProject would.
 */
export const setBranchBase = (projectId: string, branch: string, base: string | null) =>
  invoke<Branch>("set_branch_base", { projectId, branch, base });

/**
 * How a Jira API token reaches the site. classic: on the site itself (https://<site>/rest/api/3). scoped: a token
 * with scopes, through the Atlassian API gateway (https://api.atlassian.com/ex/jira/<cloudId>/rest/api/3).
 */
export type JiraTokenKind = "classic" | "scoped";

/** The Jira Cloud connection, or the lack of one (every field null but scopes). The token never leaves the core. */
export interface JiraConnection {
  /** Such as "acme.atlassian.net". */
  site: string | null;
  /** The Atlassian account email the token belongs to. */
  email: string | null;
  account: Account | null;
  tokenKind: JiraTokenKind | null;
  /** RFC 3339. */
  connectedAt: string | null;
  /** Scopes a token with scopes needs: read:jira-work and read:jira-user. */
  scopes: string[];
}

export const getJiraConnection = () => invoke<JiraConnection>("get_jira_connection");

/**
 * Asks Jira who `email` and `token` belong to on `site` (typed as "acme.atlassian.net", "acme" or a URL), without
 * saving anything: on the site first, then through the gateway for a token with scopes. With token null, the saved
 * token is used (only for the saved site and email; otherwise not_connected). Fails with auth_failed, missing_scope,
 * rate_limited, network_failed or remote_failed.
 */
export const testJiraConnection = (site: string, email: string, token: string | null) =>
  invoke<Account>("test_jira_connection", { site, email, token });

/** Checks the token with Jira, then keeps it in the macOS Keychain and the site and account in DiffLens. */
export const saveJiraConnection = (site: string, email: string, token: string) =>
  invoke<JiraConnection>("save_jira_connection", { site, email, token });

/** Forgets the Jira connection and deletes its token from the Keychain. */
export const disconnectJira = () => invoke<void>("disconnect_jira");

export interface CardComment {
  author: string;
  /** As Jira gives it, such as "2026-11-12T09:30:00.000+0000". */
  createdAt: string;
  /** Plain text. */
  text: string;
}

/** A Jira card. Its description and comments are plain text read from Jira's rich text: never render them as HTML. */
export interface Card {
  key: string;
  /** Page of the card on the site. */
  url: string;
  summary: string;
  status: string;
  issueType: string;
  description: string;
  /** Oldest first. */
  comments: CardComment[];
}

/** The card of a review, or why there is none. */
export type CardLookup =
  /** Uncommitted changes have no branch to take a key from. */
  | { kind: "working_tree" }
  /** Neither the branch name nor the pull request title holds a key such as ABC-123. */
  | { kind: "no_key"; branch: string }
  | { kind: "not_connected"; key: string }
  /** Jira has no such card, or the connected account cannot see it. */
  | { kind: "not_found"; key: string; url: string }
  | { kind: "found"; card: Card };

/**
 * The card of a review target: the first key ([A-Z][A-Z0-9]+-\d+) in the branch name, else in the pull request's
 * title, read from Jira (only read). Fails with auth_failed, missing_scope, rate_limited, network_failed or
 * remote_failed; no key, no connection and no such card are answers, not failures.
 */
export const findCard = (target: ReviewTarget, pullRequestTitle: string | null) =>
  invoke<CardLookup>("find_card", { target, pullRequestTitle });

/** The pull request of a review, as the Overview sends it with the change: untrusted text written with the change. */
export interface PullRequestText {
  title: string;
  description: string;
}

/** How the model judged an acceptance criterion. no_evidence: absent from the diff, not necessarily missing. */
export type CriterionVerdict = "met" | "partly_met" | "not_met" | "no_evidence";

export interface Criterion {
  criterion: string;
  verdict: CriterionVerdict;
  /** One line on why. */
  reasoning: string;
  /** Where the verdict rests, as the model cited it; null without one (always for no_evidence). */
  path: string | null;
  line: number | null;
  /** The place is a line of the new side of a changed file, so it can be opened in the Diff tab. */
  located: boolean;
}

/** What the model wrote about a review on Summarize. */
export interface OverviewSummary {
  /** Head it read; null for the working tree. */
  headSha: string | null;
  provider: ModelProviderKey;
  providerName: string;
  /** The model that answered. */
  model: string;
  /** RFC 3339. */
  createdAt: string;
  /** The change did not fit one call whole and was read condensed. */
  condensed: boolean;
  /** The head moved since: written for an older commit. */
  outdated: boolean;
  cardKey: string | null;
  /** Comments of the card it read. */
  cardComments: number;
  title: string;
  /**
   * One sentence on what the change does, in product terms. In a summary written before `points` existed, the whole
   * paragraph on what the change does.
   */
  lead: string;
  /** How the change works, three to five short points; empty in a summary written before they existed. */
  points: string[];
  /** What the reviewer should know before reading, such as code from other work the base does not have yet; usually empty. */
  attention: string[];
  /** What the card asks; empty without a card. */
  cardSummary: string;
  /** Empty without a card. */
  criteria: Criterion[];
  /** The language the model wrote the text above in: the one shown when Summarize was clicked. Older ones are "en". */
  language: Language;
  /** The same text in other languages, each translated on a click on Translate. Never the original's language. */
  translations: SummaryTranslation[];
}

/** The text of a summary the model wrote, as it reads in one language; criteria in the summary's order. */
export interface SummaryProse {
  title: string;
  lead: string;
  /** One for each of the summary's points, in its order. */
  points: string[];
  attention: string[];
  cardSummary: string;
  criteria: { criterion: string; reasoning: string }[];
}

export interface SummaryTranslation extends SummaryProse {
  language: Language;
}

export interface TouchedFile {
  path: string;
  status: FileStatus;
}

/** What the files of a change say about its shape. */
export interface ChangeShape {
  filesAdded: number;
  filesDeleted: number;
  /** Directories holding a changed file that the old side does not have ("." for the root). */
  directoriesNew: string[];
  directoriesModified: string[];
  /** Dependency manifests and lock files, by name, of any language. */
  manifests: TouchedFile[];
  /** Database migrations, by where they live or how they are named. */
  migrations: TouchedFile[];
}

/** What a model reading this change would not see, from the planning of an Insights analysis. */
export interface LeftOut {
  skipped: InsightsPlan["skipped"];
  /** Too large to send whole: only their changed lines are sent. */
  truncated: string[];
}

export interface Overview {
  /** The latest summary of the review; null before the first Summarize. */
  summary: OverviewSummary | null;
  shape: ChangeShape;
  leftOut: LeftOut;
}

/** What Summarize would send, before it is clicked. */
export interface OverviewPlan {
  /** Estimated input tokens (characters / 4, prompt included). */
  tokens: number;
  /** Files whose code is sent, whole or in part. */
  files: number;
  /** The change does not fit one call whole: a condensed form is sent. */
  condensed: boolean;
  cardKey: string | null;
  cardComments: number;
  model: ModelInUse;
}

/** The stored summary, the shape of the change and what a model would not see. Asks no model and not Jira. */
export const getOverview = (projectId: string, review: ReviewRef) =>
  invoke<Overview>("get_overview", { projectId, review });

/** What Summarize would send and to whom. Reads the card from Jira; asks no model. */
export const planOverview = (projectId: string, review: ReviewRef, pullRequest: PullRequestText | null) =>
  invoke<OverviewPlan>("plan_overview", { projectId, review, pullRequest });

/**
 * Reads the card, then asks the model in use one call for the title, lead, points, attention, card summary and criteria, written in the
 * language shown now, and keeps it as the review's summary, replacing an earlier one and its translations; `runId`
 * lets cancelModelRun stop it. The only command of the
 * Overview that asks a model. Fails with model_not_configured, a model error or a Jira error; a failed call keeps the
 * earlier summary.
 */
export const summarizeOverview = (
  projectId: string,
  review: ReviewRef,
  pullRequest: PullRequestText | null,
  runId: string,
) => invoke<Overview>("summarize_overview", { projectId, review, pullRequest, runId, language: currentLanguage() });

/** What translating the review's summary into `language` would send and to whom. Asks no model and not Jira. */
export const planOverviewTranslation = (projectId: string, review: ReviewRef, language: Language) =>
  invoke<TranslationPlan>("plan_overview_translation", { projectId, review, language });

/**
 * Asks the model in use to translate the text of the review's summary into `language`, only after the user clicked
 * Translate: the same verdicts and places, only the text, kept as one more version of it until a new summary replaces
 * it. `runId` lets cancelModelRun stop it. Fails with model_not_configured or a model error; nothing is kept then.
 */
export const translateOverview = (projectId: string, review: ReviewRef, language: Language, runId: string) =>
  invoke<Overview>("translate_overview", { projectId, review, language, runId });

/** Who would be asked now for `task`, for the tooltip of a button that asks a model. Asks no model. */
export const getModelInUse = (task: ModelTask) => invoke<ModelInUse>("get_model_in_use", { task });

/**
 * What an explanation covers: a changed function gets what changed, why and what it is for; a function of the context
 * (an unchanged caller or callee drawn in the graph) only what it is for.
 */
export type ExplanationKind = "changed" | "context";

/** The text of an explanation, as it reads in one language. */
export interface ExplanationProse {
  /** Empty for a function of the context. */
  whatChanged: string;
  /** Empty for a function of the context. */
  why: string;
  purpose: string;
}

export interface ExplanationTranslation extends ExplanationProse {
  language: Language;
}

/**
 * The detailed explanation a model wrote of one function of the review (on Explain in detail, or any explanation
 * written before short ones existed), read against its code now.
 */
export interface FunctionExplanation extends ExplanationProse {
  functionId: string;
  kind: ExplanationKind;
  /** Nothing the model was shown says why: `why` is its inference, and says so. */
  whyInferred: boolean;
  /** The language the model wrote it in: the one shown when the explanation was asked for. */
  language: Language;
  /** The same text in other languages, each translated on a click on Translate. */
  translations: ExplanationTranslation[];
  provider: ModelProviderKey;
  providerName: string;
  model: string;
  /** RFC 3339. */
  createdAt: string;
  headSha: string | null;
  /** The function's lines changed since it was explained. */
  outdated: boolean;
}

/** How much an explanation says: short (the Explain buttons, a full analysis) or in detail, in three parts. */
export type ExplanationLevel = "brief" | "detail";

export interface BriefTranslation {
  language: Language;
  text: string;
}

/**
 * The short explanation a model wrote of one function: one or two sentences on what changed and what it is for (for
 * a function of the context, what it is for), read against its code now.
 */
export interface FunctionBrief {
  functionId: string;
  kind: ExplanationKind;
  text: string;
  /** The language the model wrote it in: the one shown when it was asked for. */
  language: Language;
  /** The same text in other languages, each translated on a click on Translate. */
  translations: BriefTranslation[];
  provider: ModelProviderKey;
  providerName: string;
  model: string;
  /** RFC 3339. */
  createdAt: string;
  headSha: string | null;
  /** The function's lines changed since it was explained. */
  outdated: boolean;
}

/** Both levels a function can have: either, both or neither. */
export interface Explanations {
  /** The detailed ones. */
  functions: FunctionExplanation[];
  /** The short ones. */
  briefs: FunctionBrief[];
}

/** What explaining the changed functions in short would send, before it is clicked. */
export interface ExplanationPlan {
  /** Changed production functions, each explained in short; a full analysis explains no function of the context. */
  changed: number;
  calls: { ids: string[]; tokens: number }[];
  tokens: number;
  model: ModelInUse;
}

/** The review's explanations, read against `functions` (the graph's functions now) to tell the outdated. Asks no model. */
export const getExplanations = (projectId: string, review: ReviewRef, functions: FunctionSpan[]) =>
  invoke<Explanations>("get_explanations", { projectId, review, functions });

/**
 * Asks the model in use to explain the functions `functionIds` at `level`, only after a click on a node's or a group's
 * Explain button (brief) or on a function's Explain in detail (detail), in the language shown now, with the card, the
 * pull request and the branch's commit messages as context. `runId` lets cancelExplanation stop it (it then fails with
 * cancelled). What a call wrote is kept even when another call failed; the other level of each function stays.
 */
export const explainFunctions = (
  projectId: string,
  review: ReviewRef,
  functionIds: string[],
  level: ExplanationLevel,
  pullRequest: PullRequestText | null,
  runId: string,
) =>
  invoke<Explanations>("explain_functions", {
    projectId,
    review,
    functionIds,
    level,
    pullRequest,
    language: currentLanguage(),
    runId,
  });

/** Stops the explanations started under `runId`. Does nothing when they already ended. */
export const cancelExplanation = (runId: string) => invoke<void>("cancel_explanation", { runId });

/** What translating the review's explanations into `language` would send. Asks no model. */
export const planExplanationsTranslation = (projectId: string, review: ReviewRef, language: Language) =>
  invoke<TranslationPlan>("plan_explanations_translation", { projectId, review, language });

/**
 * Asks the model in use to translate the explanations of `functionIds` into `language`, only after a click on
 * Translate. `runId` lets cancelModelRun stop it.
 */
export const translateExplanations = (
  projectId: string,
  review: ReviewRef,
  language: Language,
  functionIds: string[],
  functions: FunctionSpan[],
  runId: string,
) =>
  invoke<Explanations>("translate_explanations", { projectId, review, language, functionIds, functions, runId });

/**
 * Where the cause of a failed test is, by the model's reading: the code under test, the test itself, a test that only
 * fails sometimes, or where it ran (such as the sandbox without network).
 */
export type DiagnosisFault = "code" | "test" | "flaky" | "environment";

/** How sure the model says it is. */
export type DiagnosisConfidence = "high" | "medium" | "low";

export interface DiagnosisTranslation {
  language: Language;
  cause: string;
  fix: string;
}

/**
 * Why a failing test (a top-level test, its subtests included) or a package that did not build failed, as a model
 * explained it on a click on Explain the failure, read against the last run now.
 */
export interface TestDiagnosis {
  /** "<package>::<top-level test>", or "<package>::" for a package that failed outside its tests. */
  key: string;
  package: string;
  /** null for a package. */
  test: string | null;
  /** One to three short sentences. */
  cause: string;
  fault: DiagnosisFault;
  confidence: DiagnosisConfidence;
  /** The line to change, one of the code the model was shown; null when it named none. */
  location: SourceRef | null;
  /** One or two sentences; may be empty. */
  fix: string;
  /** A short snippet of the fixed code; empty when it gave none. */
  fixCode: string;
  /** The language the model wrote it in: the one shown when it was asked for. */
  language: Language;
  /** The prose (cause and fix) in other languages, each translated on a click on Translate. */
  translations: DiagnosisTranslation[];
  provider: ModelProviderKey;
  providerName: string;
  model: string;
  /** RFC 3339. */
  createdAt: string;
  /** When the run it explained started. */
  runStartedAt: string;
  /**
   * The last run no longer has this failure, or reads it otherwise (other failed subtests, another message, value or
   * place; goroutine numbers, addresses and durations aside), or the code the model was shown changed since.
   */
  outdated: boolean;
  /** The review comment written from it (Write review comment) until that comment is deleted; kept when explained again. */
  commentId: string | null;
}

/** The review comment written from the explanation of a failure: a draft of the review, on a line or general. */
export interface WrittenReviewComment {
  /** The failure's key, as TestDiagnosis.key. */
  key: string;
  commentId: string;
  kind: CommentKind;
}

/**
 * Asks the model of the review_comment task, only after a click on Write review comment, to write from the explanation
 * of the failure `key` a review comment for the author of the change, in the language shown now, and keeps it as a
 * draft: on the line the explanation names when the review adds or changes that line, on the pull request as a whole
 * otherwise (naming the file and line). While the comment written before is still there, resolves to it without a
 * call. `runId` lets cancelModelRun stop it.
 */
export const writeReviewComment = (projectId: string, review: ReviewRef, key: string, runId: string) =>
  invoke<WrittenReviewComment>("write_review_comment", {
    projectId,
    review,
    key,
    language: currentLanguage(),
    runId,
  });

/** The review's explanations of failed tests, read against the last run. Asks no model. */
export const getTestDiagnoses = (projectId: string, review: ReviewRef) =>
  invoke<TestDiagnosis[]>("get_test_diagnoses", { projectId, review });

/**
 * Asks the model of the test_diagnosis task why the failing test `test` of `package` (or the package itself, `test`
 * null) failed in the last run, only after a click on Explain the failure, in the language shown now. It is sent the
 * failure as the run read it, how the tests ran, the code it points at (with the review's diff) and the end of the
 * output. The core keeps the call: it waits its turn while the model takes no more calls at once, goes on when the
 * review is left, and asking again for the same failure while it goes on joins it instead of asking twice.
 * cancelTestDiagnosis stops it (it then fails with cancelled). The answer replaces an earlier one of the same failure.
 * Fails when the last run no longer has that failure.
 */
export const diagnoseTestFailure = (
  projectId: string,
  review: ReviewRef,
  pkg: string,
  test: string | null,
  runId: string,
) =>
  invoke<TestDiagnosis>("diagnose_test_failure", {
    projectId,
    review,
    package: pkg,
    test,
    language: currentLanguage(),
    runId,
  });

/** An explanation of a failure being written: asked for, from this screen or one left since, and not ended yet. */
export interface RunningDiagnosis {
  /** As TestDiagnosis.key. */
  key: string;
  package: string;
  test: string | null;
  /** Waiting for another explanation to end: the model takes no more calls at once. */
  queued: boolean;
}

/** The explanations of the review's failures being written. Asks no model. */
export const getRunningTestDiagnoses = (projectId: string, review: ReviewRef) =>
  invoke<RunningDiagnosis[]>("get_running_test_diagnoses", { projectId, review });

/**
 * Waits for the explanation of the failure `key` being written and returns it, as diagnoseTestFailure would; null when
 * none is being written (it ended before: read the kept ones again). Never asks the model.
 */
export const joinTestDiagnosis = (projectId: string, review: ReviewRef, key: string) =>
  invoke<TestDiagnosis | null>("join_test_diagnosis", { projectId, review, key });

/** Stops the explanation of the failure `key` being written, waiting its turn or asking; it then fails with cancelled. */
export const cancelTestDiagnosis = (projectId: string, review: ReviewRef, key: string) =>
  invoke<void>("cancel_test_diagnosis", { projectId, review, key });

/** What translating the explanations of failures `keys` into `language` would send. Asks no model. */
export const planTestDiagnosesTranslation =(projectId: string, review: ReviewRef, language: Language, keys: string[]) =>
  invoke<TranslationPlan>("plan_test_diagnoses_translation", { projectId, review, language, keys });

/**
 * Asks the translation model to translate the explanations of failures `keys` into `language`, only after a click on
 * Translate; `runId` lets cancelModelRun stop it. Returns every explanation, read against the last run.
 */
export const translateTestDiagnoses = (
  projectId: string,
  review: ReviewRef,
  language: Language,
  keys: string[],
  runId: string,
) => invoke<TestDiagnosis[]>("translate_test_diagnoses", { projectId, review, language, keys, runId });

/** The steps of a full analysis, all started at once. */
export type AnalysisStep = "tests" | "metrics" | "insights" | "summary" | "explanations";

export const ANALYSIS_STEPS: AnalysisStep[] = ["tests", "metrics", "insights", "summary", "explanations"];

/** skipped: nothing to do (no test runner, nothing to send). cancelled: stopped by Cancel, or never started. */
export type StepStatus = "pending" | "running" | "done" | "failed" | "skipped" | "cancelled";

export interface StepState {
  step: AnalysisStep;
  status: StepStatus;
  /** Calls answered of `total` (Insights, explanations) or parts done; 0 of 0 while unknown. */
  done: number;
  total: number;
  error: ApiError | null;
}

/** queued: waiting for a slot, as many full analyses as Preferences allows are running. */
export type JobStatus = "queued" | "running" | "finished";

export type JobOutcome = "succeeded" | "failed" | "cancelled";

/** One line of a full analysis's log: a code and its values, worded by the screen in the language shown. */
export type LogEvent =
  | { code: "queued" }
  | { code: "started" }
  | { code: "tests_started" }
  | { code: "tests_finished"; passed: number; failed: number; skipped: number; durationMs: number }
  | { code: "tests_unavailable" }
  | { code: "flow_measured"; changed: number }
  | { code: "flow_unavailable" }
  | { code: "quality_measured"; crossed: number }
  | { code: "coverage_waiting" }
  | { code: "coverage_measured"; crossed: number }
  | { code: "coverage_skipped" }
  | { code: "insights_planned"; files: number; calls: number }
  | { code: "insights_call_started"; number: number; total: number }
  | { code: "insights_call_answered"; number: number; total: number; findings: number }
  | { code: "insights_call_failed"; number: number; total: number; error: ApiError }
  | { code: "summary_started" }
  | { code: "summary_written" }
  | { code: "explanations_planned"; changed: number; calls: number }
  | { code: "nothing_to_explain" }
  | { code: "explanations_call_started"; number: number; total: number }
  | { code: "explanations_call_answered"; number: number; total: number; functions: number }
  | { code: "explanations_call_failed"; number: number; total: number; error: ApiError }
  | { code: "step_failed"; step: AnalysisStep; error: ApiError }
  | { code: "step_cancelled"; step: AnalysisStep }
  | { code: "cancelling" }
  | { code: "finished" }
  | { code: "finished_with_failures"; steps: AnalysisStep[] }
  | { code: "cancelled" };

export type LogLine = LogEvent & {
  /** RFC 3339. */
  at: string;
};

/** A full analysis as the screens follow it, without its log. */
export interface JobSummary {
  id: string;
  projectId: string;
  /** The project's display name, its folder's. */
  projectName: string;
  review: ReviewRef;
  status: JobStatus;
  outcome: JobOutcome | null;
  /** Cancel was clicked; the steps in flight are stopping. */
  cancelling: boolean;
  steps: StepState[];
  /** 0 to 100. */
  progress: number;
  queuedAt: string;
  startedAt: string | null;
  finishedAt: string | null;
}

/** A full analysis with its whole log, kept in the core while the app runs. */
export interface FullAnalysisJob extends JobSummary {
  log: LogLine[];
}

/** What the core emits (`FULL_ANALYSIS_EVENT`) as a full analysis moves: its state and the line it logged, if any. */
export interface FullAnalysisEvent {
  job: JobSummary;
  line: LogLine | null;
}

export const FULL_ANALYSIS_EVENT = "full-analysis";

/** What a full analysis would do and send, before it is confirmed. */
export interface FullAnalysisPlan {
  /** The project has a test runner: the tests run in the sandbox. */
  tests: boolean;
  /** Each part names the model of its own task. */
  insights: InsightsPlan;
  summary: OverviewPlan;
  explanations: ExplanationPlan;
  /** Full analyses allowed at once, and how many run now. */
  limit: number;
  running: number;
}

/** What a full analysis would do and send. Reads the card from Jira; asks no model and runs no test. */
export const planFullAnalysis = (projectId: string, review: ReviewRef, pullRequest: PullRequestText | null) =>
  invoke<FullAnalysisPlan>("plan_full_analysis", { projectId, review, pullRequest });

/**
 * Starts a full analysis of the review in the core, after the user confirmed it, written in the language shown now;
 * queued when as many as Preferences allows already run. One already going for the review is returned as it is.
 */
export const startFullAnalysis = (projectId: string, review: ReviewRef, pullRequest: PullRequestText | null) =>
  invoke<FullAnalysisJob>("start_full_analysis", { projectId, review, pullRequest, language: currentLanguage() });

/** Stops a full analysis: a queued one ends at once; a running one stops its steps and keeps what they saved. */
export const cancelFullAnalysis = (jobId: string) => invoke<void>("cancel_full_analysis", { jobId });

/** Forgets a finished full analysis, so its review shows its tabs again. */
export const dismissFullAnalysis = (jobId: string) => invoke<void>("dismiss_full_analysis", { jobId });

/** Every full analysis of this run of the app, with its whole log. */
export const getFullAnalyses = () => invoke<FullAnalysisJob[]>("get_full_analyses");

/** How many full analyses may run at once (default 4). */
export const getFullAnalysisLimit = () => invoke<number>("get_full_analysis_limit");

/** Keeps how many full analyses may run at once (1 to 16) and returns what was kept. */
export const setFullAnalysisLimit = (limit: number) => invoke<number>("set_full_analysis_limit", { limit });

/** Calls `handler` with every change of a full analysis; resolves to the function that stops listening. */
export const listenToFullAnalyses = (handler: (event: FullAnalysisEvent) => void): Promise<() => void> =>
  listen<FullAnalysisEvent>(FULL_ANALYSIS_EVENT, (event) => handler(event.payload));

/**
 * What a folder a terminal can open in is to the project. project: the project folder itself, where commands change
 * the user's own checkout. difflens: a worktree DiffLens created under `.difflens/worktrees/`. worktree: one the user
 * created elsewhere.
 */
export type FolderKind = "project" | "difflens" | "worktree";

export interface Folder {
  /** Absolute and canonical. */
  path: string;
  /** The same path with the home folder written as "~", for display only. */
  displayPath: string;
  kind: FolderKind;
  /** `.difflens/worktrees/<branch>`, with "/" separators, for a DiffLens worktree; null otherwise. */
  relativePath: string | null;
}

/** A worktree of the repository. */
export interface Worktree {
  folder: Folder;
  /** The local branch checked out there; null when its HEAD is detached. */
  branch: string | null;
  /** RFC 3339 time its folder was created; DiffLens worktrees only. */
  createdAt: string | null;
  /** Files with uncommitted changes, untracked ones included; DiffLens worktrees only, null when git could not tell. */
  changedFiles: number | null;
}

/** What creating the worktree of a branch checked out nowhere would do. */
export interface WorktreePlan {
  /** `.difflens/worktrees/<branch>`, inside the project. */
  relativePath: string;
  /** The remote-tracking branch a new local branch would follow (such as "origin/feature/x"), for a remote-only branch. */
  upstream: string | null;
}

/** Where a screen's terminals open: a folder, or for a branch checked out nowhere, the worktree it could have. */
export interface TerminalLocation {
  folder: Folder | null;
  /** The review's branch, or the one checked out in the project folder (null when detached). */
  branch: string | null;
  /** Set exactly when folder is null. */
  plan: WorktreePlan | null;
}

/** shell: the user's login shell. claude: Claude Code, started through it, with no prompt or argument. */
export type TerminalKind = "shell" | "claude";

/** A terminal session of the core: alive until closed or until the app quits, whatever screen is shown. */
export interface TerminalSession {
  id: string;
  kind: TerminalKind;
  /** "zsh", "zsh 2", "Claude Code": unique per folder and kind. */
  name: string;
  /** The folder it runs in, absolute and canonical: Folder.path of its location. */
  cwd: string;
  projectId: string;
  projectName: string;
  /** The review it was opened from; null for the project screen. */
  target: ReviewTarget | null;
  /** The branch of its location when it was opened. */
  branch: string | null;
  /** RFC 3339. */
  openedAt: string;
  /** Command line of the process in the foreground other than the shell (empty when unreadable); null when idle. */
  foreground: string | null;
  /** Its process exited on its own; the session stays until closed. */
  ended: boolean;
  exitCode: number | null;
}

/** What a session's channel carries besides its output. */
export type TerminalEvent =
  | { type: "foreground"; command: string | null }
  | { type: "exited"; code: number | null };

/** One message of a session's channel: output bytes as the program wrote them, or an event. */
export type TerminalMessage = { kind: "output"; data: Uint8Array } | { kind: "event"; event: TerminalEvent };

/** A channel whose raw messages (output) arrive as ArrayBuffers and the others as JSON events. */
function terminalChannel(onMessage: (message: TerminalMessage) => void): Channel<unknown> {
  const channel = new Channel<unknown>();
  channel.onmessage = (message) => {
    if (message instanceof ArrayBuffer) {
      onMessage({ kind: "output", data: new Uint8Array(message) });
    } else if (ArrayBuffer.isView(message)) {
      onMessage({ kind: "output", data: new Uint8Array(message.buffer, message.byteOffset, message.byteLength) });
    } else if (Array.isArray(message)) {
      onMessage({ kind: "output", data: Uint8Array.from(message as number[]) });
    } else {
      onMessage({ kind: "event", event: message as TerminalEvent });
    }
  };
  return channel;
}

/** Where terminals open for `target`, or for the project screen when null. Fails with unknown_branch. */
export const getTerminalLocation = (projectId: string, target: ReviewTarget | null) =>
  invoke<TerminalLocation>("get_terminal_location", { projectId, target });

/** Adds `.difflens/` to the repository's info/exclude (once): the first step of creating a worktree. */
export const excludeDifflensFolder = (projectId: string) => invoke<void>("exclude_difflens_folder", { projectId });

/**
 * Creates the DiffLens worktree of `branch` (a local branch tracking origin for a remote-only one) and returns where
 * its terminals now open. Never touches the project folder. Fails with destination_exists or unknown_branch.
 */
export const createWorktree = (projectId: string, branch: string) =>
  invoke<TerminalLocation>("create_worktree", { projectId, branch });

/**
 * Removes a DiffLens worktree; never its branch. Fails with terminals_open while a session is open in it, with
 * worktree_dirty when it has uncommitted changes and `force` is false, and with unknown_worktree for any other folder.
 */
export const removeWorktree = (projectId: string, path: string, force: boolean) =>
  invoke<void>("remove_worktree", { projectId, path, force });

/** Opens a session where `target`'s terminals open (the project folder when null). Fails with no_folder. */
export const openTerminal = (
  projectId: string,
  target: ReviewTarget | null,
  kind: TerminalKind,
  onMessage: (message: TerminalMessage) => void,
) => invoke<TerminalSession>("open_terminal", { projectId, target, kind, output: terminalChannel(onMessage) });

/**
 * Sends the session's output to `onMessage` from now on, and with `replay` first its scrollback (the last 1 MB it
 * printed) and its exit when it ended. The channel attached before gets nothing more.
 */
export const attachTerminal = (id: string, replay: boolean, onMessage: (message: TerminalMessage) => void) =>
  invoke<TerminalSession>("attach_terminal", { id, replay, output: terminalChannel(onMessage) });

/** Typed input; written in the order the calls arrive, so callers send one at a time. */
export const writeTerminal = (id: string, data: string) => invoke<void>("write_terminal", { id, data });

export const resizeTerminal = (id: string, cols: number, rows: number) =>
  invoke<void>("resize_terminal", { id, cols, rows });

/** Ends the session: its processes are hung up, then killed after a short grace. Closing twice does nothing. */
export const closeTerminal = (id: string) => invoke<void>("close_terminal", { id });

/** Every session of the core, oldest first: they outlive a reload of the window. */
export const listTerminals = () => invoke<TerminalSession[]>("list_terminals");

/** What building an analyzer's environment is doing: creating it, installing its dependencies, compiling it. */
export type ProvisionStep = "creating_environment" | "installing_dependencies" | "compiling";

/**
 * A change of the build of an analyzer shipped as source (Python, JS/TS, Rust), which the core prepares the first
 * time a project of its language needs it. The request that needs it (getFlow, getQuality, the tests) waits for it and
 * then goes on by itself; this only says what is happening meanwhile.
 */
export interface AnalyzerProvision {
  /** The analyzer's id, such as "python" or "jsts". */
  analyzer: string;
  /** Its language as shown, such as "Python" or "JavaScript/TypeScript". */
  language: string;
  status: "preparing" | "ready" | "failed";
  /** While preparing, once the first step started. */
  step: ProvisionStep | null;
  /** Why it failed, worded by the core (show it with `coreText`). */
  message: CoreMessage | null;
}

export const ANALYZER_PROVISION_EVENT = "analyzer-provision";

/** The analyzer environments being built now, for a view opened during a build. */
export const getAnalyzerProvisions = () => invoke<AnalyzerProvision[]>("get_analyzer_provisions");

/** Calls `handler` with every change of an analyzer's build; resolves to the function that stops listening. */
export const listenToAnalyzerProvisions = (handler: (event: AnalyzerProvision) => void): Promise<() => void> =>
  listen<AnalyzerProvision>(ANALYZER_PROVISION_EVENT, (event) => handler(event.payload));

/** What a Claude Code session runs: `claude`, or the custom command once one is saved. */
export type ClaudeCommandMode = "default" | "custom";

/** Preferences › Terminal: the choice, and the custom command saved (kept while the default is chosen). */
export interface ClaudeCommandSetting {
  mode: ClaudeCommandMode;
  custom: string | null;
}

/** The most characters a custom command may have, as the core checks it. */
export const MAX_CLAUDE_COMMAND_CHARS = 1000;

export const getClaudeCommand = () => invoke<ClaudeCommandSetting>("get_claude_command");

/** Chooses what sessions opened from now on run; sessions already open keep theirs. */
export const setClaudeCommandMode = (mode: ClaudeCommandMode) =>
  invoke<ClaudeCommandSetting>("set_claude_command_mode", { mode });

/** Saves the custom command (trimmed) and chooses it. Fails when it is empty, too long or not a single line. */
export const saveClaudeCommand = (command: string) => invoke<ClaudeCommandSetting>("save_claude_command", { command });
