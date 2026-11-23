import { useState } from "react";
import { useTranslation } from "react-i18next";

import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import {
  myVerdict,
  otherReviewers,
  type PullRequestStatus,
  verdictWords,
} from "@/features/review/pull-request/pull-request-status";
import type { Participant, Verdict } from "@/lib/api";
import { readinessReason } from "@/lib/core-text";
import { cn } from "@/lib/utils";

/** Reviewers shown by name; the rest are counted. */
const SHOWN_REVIEWERS = 3;

interface VerdictControlsProps {
  status: PullRequestStatus;
  /** Drafts, replies, edits and deletes not published yet. */
  unpublished: number;
  /** Why they cannot be published now, or null when they can. */
  publishBlocked: string | null;
  deciding: Verdict | null;
  /** A publish or a comment save is under way. */
  busy: boolean;
  error: string | null;
  onVerdict: (verdict: Verdict, given: boolean) => void;
  /** Publishes the unpublished changes, then gives the verdict if they all went through. */
  onPublishThenVerdict: (verdict: Verdict) => void;
}


/**
 * Approve and Request changes, next to the pull request in the review bar, with the other reviewers' verdicts. Each
 * button shows the user's own verdict and takes it back when pressed again. Nothing happens without a click; with
 * unpublished comments, the user is asked whether to publish them first.
 */
export function VerdictControls({
  status,
  unpublished,
  publishBlocked,
  deciding,
  busy,
  error,
  onVerdict,
  onPublishThenVerdict,
}: VerdictControlsProps) {
  const { t } = useTranslation();
  const [asking, setAsking] = useState<Verdict | null>(null);
  if (status.kind !== "linked") return null;
  const mine = myVerdict(status);
  const allowed = status.verdict.allowed;
  const reason = readinessReason(status.verdict);
  const press = (verdict: Verdict) => {
    const given = mine !== verdict;
    if (given && unpublished > 0) setAsking(verdict);
    else onVerdict(verdict, given);
  };
  const button = (verdict: Verdict) => {
    const on = mine === verdict;
    return (
      <Button
        variant="outline"
        size="xs"
        aria-pressed={on}
        disabled={!allowed || deciding !== null || busy}
        // The reason also stands next to the buttons, but the bar drops it when it is narrow.
        title={!allowed ? (reason ?? undefined) : on ? t(`review.verdict.${verdict}.takeBack`) : undefined}
        onClick={() => press(verdict)}
        className={cn(
          on && verdict === "approved" && "border-add text-add not-disabled:hover:border-add not-disabled:hover:text-add",
          on &&
            verdict === "changes_requested" &&
            "border-del text-del not-disabled:hover:border-del not-disabled:hover:text-del",
        )}
      >
        {on ? t(`review.verdict.${verdict}.given`) : t(`review.verdict.${verdict}.give`)}
      </Button>
    );
  };
  return (
    // `contents`: the reviewers, buttons and notes are items of the bar itself, so the buttons never shrink under the
    // notes, and only the notes give way.
    <span className="contents text-xs">
      <Reviewers reviewers={otherReviewers(status)} />
      {button("approved")}
      {button("changes_requested")}
      {!allowed && reason && (
        <span
          data-testid="verdict-note"
          title={reason}
          className="max-w-[240px] min-w-0 truncate text-muted-foreground @max-5xl:hidden"
        >
          {reason}
        </span>
      )}
      {error && (
        <span role="alert" title={error} className="max-w-[260px] min-w-0 truncate text-destructive">
          {error}
        </span>
      )}
      <AlertDialog open={asking !== null} onOpenChange={(open) => !open && setAsking(null)}>
        <AlertDialogContent>
          {asking && (
            <>
              <AlertDialogHeader>
                <AlertDialogTitle>{t("review.verdict.unpublished", { count: unpublished })}</AlertDialogTitle>
                <AlertDialogDescription>
                  {publishBlocked
                    ? t("review.verdict.blocked", { reason: publishBlocked })
                    : t(`review.verdict.${asking}.ask`)}
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>{t("common.actions.cancel")}</AlertDialogCancel>
                <Button
                  variant="secondary"
                  onClick={() => {
                    setAsking(null);
                    onVerdict(asking, true);
                  }}
                >
                  {t(`review.verdict.${asking}.without`)}
                </Button>
                <Button
                  disabled={publishBlocked !== null}
                  onClick={() => {
                    setAsking(null);
                    onPublishThenVerdict(asking);
                  }}
                >
                  {t(`review.verdict.${asking}.publishAnd`, { count: unpublished })}
                </Button>
              </AlertDialogFooter>
            </>
          )}
        </AlertDialogContent>
      </AlertDialog>
    </span>
  );
}

const MARK: Record<Verdict | "none", { icon: string; className: string }> = {
  approved: { icon: "✓", className: "text-add" },
  changes_requested: { icon: "✕", className: "text-del" },
  none: { icon: "○", className: "text-faint" },
};

const describe = (participant: Participant) => `${participant.account.name} ${verdictWords(participant.verdict)}`;

/** The other reviewers, each with a mark for their verdict; past a few, the rest are counted. */
function Reviewers({ reviewers }: { reviewers: Participant[] }) {
  const { t } = useTranslation();
  if (reviewers.length === 0) return null;
  const shown = reviewers.slice(0, SHOWN_REVIEWERS);
  const hidden = reviewers.slice(SHOWN_REVIEWERS);
  return (
    <ul aria-label={t("review.verdict.reviewers")} className="flex flex-none items-center gap-2 whitespace-nowrap @max-[44rem]:hidden">
      {shown.map((reviewer) => {
        const mark = MARK[reviewer.verdict ?? "none"];
        return (
          <li key={reviewer.account.id || reviewer.account.name} title={describe(reviewer)} className="flex items-center gap-1">
            <span aria-hidden="true" className={mark.className}>
              {mark.icon}
            </span>
            {/* Past a narrow bar only the mark stays; the name and verdict are in the title. */}
            <span className="max-w-[90px] truncate text-muted-foreground @max-[88rem]:hidden">{reviewer.account.name}</span>
            <span className="sr-only">{verdictWords(reviewer.verdict)}</span>
          </li>
        );
      })}
      {hidden.length > 0 && (
        <li title={hidden.map(describe).join(", ")} className="text-muted-foreground">
          +{hidden.length}
          <span className="sr-only"> {t("review.verdict.more", { list: hidden.map(describe).join(", ") })}</span>
        </li>
      )}
    </ul>
  );
}
