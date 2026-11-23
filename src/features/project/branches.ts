import { i18n } from "@/i18n";
import type { Branch, ProjectDetail } from "@/lib/api";

/** A branch whose head commit is at most this old is active. */
export const ACTIVE_DAYS = 14;

const DAY = 24 * 60 * 60 * 1000;

export interface BranchView {
  /** Text typed in the filter; matches the branch name or its card, ignoring case. */
  query: string;
  /** False shows only branches on this machine; true adds the remote-only ones. */
  includeRemote: boolean;
  now: Date;
}

export interface BranchGroups {
  /** The checked out branch, when it matches the filter. */
  current: Branch[];
  active: Branch[];
  older: Branch[];
  /** Older branches before the filter is applied: the count behind "Show N older branches". */
  olderTotal: number;
  /** Whether a branch to review is checked out at all, even if the filter hides it. */
  hasCurrent: boolean;
}

/** Matches a branch name against integration branch names or glob patterns such as "release/*". */
export function isIntegrationBranch(name: string, patterns: string[]): boolean {
  return patterns.some((pattern) => globToRegExp(pattern).test(name));
}

function globToRegExp(pattern: string): RegExp {
  const source = pattern
    .split("*")
    .map((part) => part.replace(/[.+?^${}()|[\]\\]/g, "\\$&"))
    .join(".*");
  return new RegExp(`^${source}$`);
}

export function matchesFilter(branch: Branch, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (needle === "") {
    return true;
  }
  return (
    branch.name.toLowerCase().includes(needle) ||
    (branch.card?.toLowerCase().includes(needle) ?? false)
  );
}

/** Head commit in the last ACTIVE_DAYS days. A missing or invalid date counts as older. */
export function isActive(branch: Branch, now: Date): boolean {
  const authoredAt = new Date(branch.head.authoredAt).getTime();
  return !Number.isNaN(authoredAt) && now.getTime() - authoredAt <= ACTIVE_DAYS * DAY;
}

/** Branches to review. Integration branches are left out even if the core lists one. */
export function reviewableBranches({ project, branches, bases }: ProjectDetail): Branch[] {
  return branches.filter(
    (branch) =>
      !isIntegrationBranch(branch.name, project.integrationBranches) && !bases.includes(branch.name),
  );
}

/**
 * Splits the branches to review into checked out, active and older, keeping the order received
 * (most recently committed first).
 */
export function groupBranches(detail: ProjectDetail, view: BranchView): BranchGroups {
  const reviewable = reviewableBranches(detail);
  const current = reviewable.filter((branch) => branch.isCurrent);
  const others = reviewable.filter(
    (branch) => !branch.isCurrent && (view.includeRemote || branch.location === "local"),
  );
  const active = others.filter((branch) => isActive(branch, view.now));
  const older = others.filter((branch) => !isActive(branch, view.now));
  const matches = (branch: Branch) => matchesFilter(branch, view.query);

  return {
    current: current.filter(matches),
    active: active.filter(matches),
    older: older.filter(matches),
    olderTotal: older.length,
    hasCurrent: current.length > 0,
  };
}

export interface BranchCounts {
  local: number;
  remoteOnly: number;
}

export function countBranches(branches: Branch[]): BranchCounts {
  const remoteOnly = branches.filter((branch) => branch.location === "remote_only").length;
  return { local: branches.length - remoteOnly, remoteOnly };
}

/** Remote name of an upstream such as "origin/feature/x", or "the remote" when it has no prefix. */
export function remoteName(upstream: string): string {
  const slash = upstream.indexOf("/");
  return slash > 0 ? upstream.slice(0, slash) : i18n.t("project.row.theRemote");
}
