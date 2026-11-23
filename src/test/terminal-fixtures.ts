import type { Folder, TerminalLocation, TerminalSession, Worktree } from "@/lib/api";

export const PROJECT_PATH = "/Users/dev/work/payments-api";

export function makeFolder(overrides: Partial<Folder> = {}): Folder {
  return {
    path: PROJECT_PATH,
    displayPath: "~/work/payments-api",
    kind: "project",
    relativePath: null,
    ...overrides,
  };
}

/** The DiffLens worktree of `branch` in the project of `makeProject`. */
export function difflensFolder(branch: string): Folder {
  const relativePath = `.difflens/worktrees/${branch}`;
  return {
    path: `${PROJECT_PATH}/${relativePath}`,
    displayPath: `~/work/payments-api/${relativePath}`,
    kind: "difflens",
    relativePath,
  };
}

/** The project folder, with `main` checked out. */
export function makeLocation(overrides: Partial<TerminalLocation> = {}): TerminalLocation {
  return { folder: makeFolder(), branch: "main", plan: null, ...overrides };
}

/** A branch checked out nowhere: what creating its worktree would do. */
export function noFolder(branch: string, upstream: string | null = null): TerminalLocation {
  return { folder: null, branch, plan: { relativePath: `.difflens/worktrees/${branch}`, upstream } };
}

export function makeSession(id: string, overrides: Partial<TerminalSession> = {}): TerminalSession {
  return {
    id,
    kind: "shell",
    name: "zsh",
    cwd: PROJECT_PATH,
    projectId: "p-1",
    projectName: "payments-api",
    target: null,
    branch: "main",
    openedAt: "2026-11-20T12:00:00Z",
    foreground: null,
    ended: false,
    exitCode: null,
    ...overrides,
  };
}

export function makeWorktree(branch: string, overrides: Partial<Worktree> = {}): Worktree {
  return {
    folder: difflensFolder(branch),
    branch,
    createdAt: "2026-11-19T12:00:00Z",
    changedFiles: 0,
    ...overrides,
  };
}
