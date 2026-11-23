import { i18n } from "@/i18n";
import type { ChangeSet, ReviewTarget } from "@/lib/api";

/** Breadcrumb label of a review: the branch name, or "Uncommitted changes" in the language shown. */
export const targetLabel = (target: ReviewTarget) =>
  target.kind === "branch" ? target.branch : i18n.t("review.target.workingTree");

/** Identity of a target, to remount the review screen when it changes. */
export const targetKey = (target: ReviewTarget) =>
  target.kind === "branch" ? `branch:${target.branch}:${target.base}` : "working_tree";

export const shortSha = (sha: string) => sha.slice(0, 7);

/**
 * What a branch's change set was compared with, as the review names it: the ref it was read from ("origin/develop",
 * or "develop" when origin has none), or the pull request's develop when its destination commit was used. The
 * target's base name before the change set is read, and for the working tree.
 */
export function baseLabel(target: ReviewTarget, changeSet: ChangeSet | null): string {
  const base = changeSet?.base ?? null;
  if (base === null) return target.kind === "branch" ? target.base : "";
  if (base.kind === "pull_request") return i18n.t("review.bar.pullRequestBase", { base: base.name });
  return base.refName ?? base.name;
}
