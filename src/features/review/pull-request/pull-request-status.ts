// What the review says about its pull request: the note in the review bar, and whether "Publish comments" can send
// and why not.

import { i18n } from "@/i18n";
import type { Participant, PublishReport, PullRequestLink, ReviewState, Verdict } from "@/lib/api";
import { publishFailureMessage, readinessReason } from "@/lib/core-text";

/** The pull request of the review as the screen knows it: being looked for, found (or why not), or unreadable. */
export type PullRequestStatus = { kind: "loading" } | { kind: "error"; message: string } | PullRequestLink;

/** Why the review has no pull request to show, in the bar; null when there is one, or nothing to say. */
export function barNote(status: PullRequestStatus): string | null {
  switch (status.kind) {
    case "loading":
    case "working_tree":
    case "linked":
      return null;
    case "no_remote":
      return i18n.t("review.pullRequest.note.noRemote");
    case "unsupported_host":
      return i18n.t("review.pullRequest.note.unsupportedHost", { host: status.host });
    case "not_connected":
      return i18n.t("review.pullRequest.note.notConnected", { host: status.hostName });
    case "no_pull_request":
      return i18n.t("review.pullRequest.note.noPullRequest");
    case "error":
      return status.message;
  }
}

/** The state of a pull request as its chip shows it, in the language shown. */
export const pullRequestState = (state: "open" | "merged" | "declined" | "superseded"): string =>
  i18n.t(`review.pullRequest.state.${state}`);

/** What the Publish button shows: whether it is there at all, whether it can be pressed, and why not. */
export interface PublishControl {
  visible: boolean;
  enabled: boolean;
  label: string;
  /** Why it cannot be pressed, or what it leaves out; null when there is nothing to say. */
  note: string | null;
}

/**
 * The Publish button for the review's pull request, in the language shown. When there is nowhere to publish, the note
 * says the comments stay in DiffLens.
 */
export function publishControl(
  status: PullRequestStatus,
  summary: ReviewState["summary"] | null,
  publishing: boolean,
): PublishControl {
  const unpublished = summary?.unpublished ?? 0;
  const stale = summary?.staleDrafts ?? 0;
  const label = publishing ? i18n.t("review.publish.publishing") : i18n.t("review.publish.label", { count: unpublished });
  const off = (note: string | null): PublishControl => ({ visible: true, enabled: false, label, note });
  switch (status.kind) {
    case "working_tree":
      return { visible: false, enabled: false, label, note: null };
    case "loading":
      return off(i18n.t("review.publish.looking"));
    case "error":
      return off(i18n.t("review.publish.unreadable", { message: status.message }));
    case "no_remote":
      return off(i18n.t("review.publish.noRemote"));
    case "unsupported_host":
      return off(i18n.t("review.publish.unsupportedHost", { host: status.host }));
    case "not_connected":
      return off(i18n.t("review.publish.notConnected", { host: status.hostName }));
    case "no_pull_request":
      return off(i18n.t("review.publish.noPullRequest", { branch: status.branch }));
    case "linked":
      break;
  }
  const held = stale > 0 ? i18n.t("review.publish.heldBack", { count: stale }) : null;
  if (!status.publish.allowed) return off(readinessReason(status.publish));
  if (publishing || summary === null) return off(held);
  if (unpublished === 0) return off(held ?? i18n.t("review.publish.nothing"));
  return { visible: true, enabled: true, label, note: held };
}

type Linked = Extract<PullRequestStatus, { kind: "linked" }>;

/** The connected account's verdict on the pull request, from its participants. */
export function myVerdict(status: Linked): Verdict | null {
  return status.pullRequest.participants.find((participant) => participant.account.id === status.account.id)?.verdict ?? null;
}

/** The other people whose verdict counts: reviewers, and anyone who gave one. Those who concluded come first. */
export function otherReviewers(status: Linked): Participant[] {
  const rank = (participant: Participant) => (participant.verdict === null ? 1 : 0);
  return status.pullRequest.participants
    .filter((participant) => participant.account.id !== status.account.id)
    .filter((participant) => participant.reviewer || participant.verdict !== null)
    .sort((a, b) => rank(a) - rank(b));
}

/** What a reviewer did, read after their name: "approved", "has not reviewed yet". In the language shown. */
export const verdictWords = (verdict: Verdict | null): string => i18n.t(`review.verdict.words.${verdict ?? "none"}`);

/** What the last publish did, for the line next to the button. */
export type PublishOutcome = { kind: "report"; report: PublishReport } | { kind: "error"; message: string };

export function outcomeText(outcome: PublishOutcome): string {
  if (outcome.kind === "error") return outcome.message;
  const { published, failed } = outcome.report;
  const sent = i18n.t("review.publish.published", { count: published });
  if (failed.length === 0) return sent;
  // A general comment has no file or line: it is named as on the pull request.
  const place = (failure: (typeof failed)[number]) =>
    failure.path === "" ? i18n.t("review.comments.general.title") : `${failure.path}:${failure.line}`;
  const listed = failed.map((failure) => `${place(failure)}: ${publishFailureMessage(failure)}`).join(" ");
  return `${sent} ${i18n.t("review.publish.failed", { count: failed.length })} ${listed}`;
}
