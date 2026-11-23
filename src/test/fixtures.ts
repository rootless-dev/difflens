import type {
  ApiError,
  BaseSource,
  Branch,
  BranchBase,
  ChangedFile,
  ChangeSet,
  CompareSummary,
  ErrorCode,
  FileSide,
  Project,
  ProjectDetail,
} from "@/lib/api";

export function makeProject(overrides: Partial<Project> = {}): Project {
  return {
    id: "p-1",
    name: "payments-api",
    path: "/Users/dev/work/payments-api",
    displayPath: "~/work/payments-api",
    languages: ["go"],
    analysisSupport: "full",
    integrationBranches: ["main", "master", "develop", "release/*"],
    available: true,
    hasRemote: true,
    branchesAhead: 0,
    lastFetchedAt: null,
    addedAt: "2026-11-10T12:00:00Z",
    lastModifiedAt: null,
    ...overrides,
  };
}

export function makeBranch(name: string, overrides: Partial<Branch> = {}): Branch {
  return {
    name,
    location: "local",
    isCurrent: false,
    head: { sha: `sha-${name}`, subject: `Work on ${name}`, authoredAt: "2026-11-09T12:00:00Z" },
    upstream: null,
    card: /[A-Z]+-\d+/.exec(name)?.[0] ?? null,
    base: makeBase("develop"),
    ahead: 1,
    behind: 0,
    ...overrides,
  };
}

export function makeBase(name: string, source: BaseSource = "history", sha = `sha-${name}`): BranchBase {
  return { name, source, refName: `origin/${name}`, sha };
}

export function makeDetail(overrides: Partial<ProjectDetail> = {}): ProjectDetail {
  return {
    project: makeProject(),
    branches: [],
    bases: ["develop", "main"],
    workingTree: { dirty: false, filesChanged: 0 },
    worktrees: [],
    ...overrides,
  };
}

export function makeSummary(
  branch: string,
  base: string,
  overrides: Partial<CompareSummary> = {},
): CompareSummary {
  return {
    branch,
    base,
    headSha: `sha-${branch}`,
    mergeBaseSha: "a1b2c3d",
    commits: 2,
    filesChanged: 12,
    additions: 384,
    deletions: 57,
    ...overrides,
  };
}

export function makeChangedFile(path: string, overrides: Partial<ChangedFile> = {}): ChangedFile {
  return {
    path,
    oldPath: null,
    status: "modified",
    additions: 3,
    deletions: 1,
    binary: false,
    language: "go",
    ...overrides,
  };
}

export function makeChangeSet(files: ChangedFile[], overrides: Partial<ChangeSet> = {}): ChangeSet {
  const target = overrides.target ?? { kind: "branch", branch: "feat/refund-flow", base: "main" };
  return {
    target,
    oldSha: "a1b2c3d4e5f6a7b8",
    newSha: "e7f3b10c9d8e7f6a",
    // A branch is compared with its base on origin unless a test says otherwise.
    base:
      target.kind === "branch"
        ? { name: target.base, kind: "remote_tracking", refName: `origin/${target.base}`, sha: "c0ffee0123456789" }
        : null,
    files,
    ...overrides,
  };
}

export const textSide = (text: string): FileSide => ({ text, reason: null, size: text.length });

export const absentSide: FileSide = { text: null, reason: "absent", size: 0 };

export function apiError(code: ErrorCode, message: string): ApiError {
  return { code, message };
}

export function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}
