// What the review state says about one file or function, and the words the screen uses for it.

import { isChanged } from "@/features/review/flow/flow-model";
import { shortSha } from "@/features/review/review-target";
import { i18n } from "@/i18n";
import type {
  ChangeSet,
  FlowNode,
  FunctionSpan,
  MarkStatus,
  ReviewRef,
  ReviewState,
  SinceLastReview,
} from "@/lib/api";
import { formatRelativeTime } from "@/lib/format";

/** The review a change set belongs to, with the sides it froze. */
export const reviewRefOf = ({ target, oldSha, newSha }: ChangeSet): ReviewRef => ({ target, oldSha, newSha });

/** Marked files by path. A file that is not in it is unreviewed. */
export function fileMarks(state: ReviewState | null): ReadonlyMap<string, MarkStatus> {
  return new Map((state?.files ?? []).map((file) => [file.path, file.status]));
}

/** Marked functions by id. A function that is not in it is unreviewed. */
export function functionMarks(state: ReviewState | null): ReadonlyMap<string, MarkStatus> {
  return new Map((state?.functions ?? []).map((fn) => [fn.id, fn.status]));
}

/** Files of the change set marked as reviewed whose content is still what was reviewed. */
export function reviewedCount(changeSet: ChangeSet, marks: ReadonlyMap<string, MarkStatus>): number {
  return changeSet.files.filter((file) => marks.get(file.path) === "reviewed").length;
}

export function fileButtonLabel(status: MarkStatus | null): string {
  if (status === "reviewed") return i18n.t("review.marks.reviewed");
  if (status === "reopened") return i18n.t("review.marks.fileReopened");
  return i18n.t("review.marks.markAsReviewed");
}

export function functionButtonLabel(status: MarkStatus | null): string {
  if (status === "reviewed") return i18n.t("review.marks.reviewed");
  if (status === "reopened") return i18n.t("review.marks.markAgain");
  return i18n.t("review.marks.markAsReviewed");
}

/** A function can be marked when the review changed it and the graph says where it is. Untouched ones cannot. */
export function spanOf(node: FlowNode): FunctionSpan | null {
  if (!isChanged(node) || node.file === null || node.startLine === null || node.endLine === null) return null;
  return {
    id: node.id,
    file: node.file,
    startLine: node.startLine,
    endLine: node.endLine,
    deleted: node.state === "deleted",
  };
}

export function functionSpans(nodes: readonly FlowNode[]): FunctionSpan[] {
  return nodes.flatMap((node) => spanOf(node) ?? []);
}

/** Why "Since last review" cannot be chosen, or what it shows when it can. */
export function sinceTitle(since: SinceLastReview | null, checkpointAt: string | null, now = new Date()): string {
  if (since === null) return i18n.t("review.since.loading");
  if (since.kind === "available") {
    const parts = {
      sha: shortSha(since.checkpointSha),
      commits: i18n.t("review.since.commits", { count: since.commits }),
      files: i18n.t("review.since.files", { count: since.files.length }),
    };
    return checkpointAt
      ? i18n.t("review.since.availableAt", { ...parts, when: formatRelativeTime(checkpointAt, now) })
      : i18n.t("review.since.available", parts);
  }
  return i18n.t(`review.since.${since.reason}`);
}
