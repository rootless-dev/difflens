import { Button } from "@/components/ui/button";
import {
  outcomeText,
  type PublishOutcome,
  publishControl,
  type PullRequestStatus,
} from "@/features/review/pull-request/pull-request-status";
import type { ReviewState } from "@/lib/api";
import { cn } from "@/lib/utils";

interface PublishButtonProps {
  status: PullRequestStatus;
  summary: ReviewState["summary"] | null;
  publishing: boolean;
  /** A comment is being saved: publishing waits until it is. */
  busy?: boolean;
  outcome: PublishOutcome | null;
  onPublish: () => void;
}

/**
 * "Publish comments (N)" for the Diff toolbar, with why it cannot send when it cannot, and what the last publish did.
 * The only way comments reach the pull request.
 */
export function PublishButton({ status, summary, publishing, busy = false, outcome, onPublish }: PublishButtonProps) {
  const control = publishControl(status, summary, publishing);
  if (!control.visible) return null;
  const failed = outcome !== null && (outcome.kind === "error" || outcome.report.failed.length > 0);
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-2.5 gap-y-1">
      <Button size="xs" disabled={!control.enabled || busy} onClick={onPublish}>
        {control.label}
      </Button>
      {control.note && (
        <span data-testid="publish-note" className="max-w-[420px] min-w-0 text-xs text-muted-foreground">
          {control.note}
        </span>
      )}
      {outcome && (
        <span
          role={failed ? "alert" : "status"}
          className={cn("max-w-[520px] min-w-0 text-xs", failed ? "text-destructive" : "text-add")}
        >
          {outcomeText(outcome)}
        </span>
      )}
    </div>
  );
}
