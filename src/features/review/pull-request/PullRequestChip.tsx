import { openUrl } from "@tauri-apps/plugin-opener";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import {
  barNote,
  type PullRequestStatus,
  pullRequestState,
} from "@/features/review/pull-request/pull-request-status";
import { errorMessage } from "@/lib/format";

interface PullRequestChipProps {
  status: PullRequestStatus;
  /** Opens Preferences, offered when the host is not connected. */
  onOpenPreferences?: () => void;
}

/** The review's pull request in the top bar (the prototype's `.ext` "PR #318" link), or why there is none. */
export function PullRequestChip({ status, onOpenPreferences }: PullRequestChipProps) {
  const { t } = useTranslation();
  const [openError, setOpenError] = useState<string | null>(null);
  if (status.kind === "loading") {
    return <Skeleton role="status" aria-label={t("review.pullRequest.looking")} className="h-3.5 w-16" />;
  }
  if (status.kind === "linked") {
    const { pullRequest } = status;
    const state = pullRequestState(pullRequest.state);
    const open = () => {
      setOpenError(null);
      openUrl(pullRequest.url).catch((cause: unknown) => setOpenError(errorMessage(cause)));
    };
    return (
      // `contents`: the parts are items of the bar itself, so only the title gives way when the bar is short, rather
      // than the whole chip shrinking under its link and state.
      <span className="contents text-xs">
        <a
          href={pullRequest.url}
          onClick={(event) => {
            event.preventDefault();
            open();
          }}
          // Carries the title too, which the bar drops when it is narrow.
          title={openError ?? t("review.pullRequest.openOn", { title: pullRequest.title, host: status.hostName })}
          className="inline-flex flex-none items-center gap-1.5 rounded-sm border px-[9px] py-[3px] text-xs whitespace-nowrap text-foreground no-underline outline-none hover:border-primary hover:text-primary focus-visible:outline-2 focus-visible:outline-ring"
        >
          PR #{pullRequest.number}
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <path d="M3.5 1.5h5v5M8.5 1.5l-7 7" fill="none" stroke="currentColor" strokeWidth="1.3" />
          </svg>
        </a>
        <span
          className="max-w-[260px] min-w-0 truncate text-xs text-muted-foreground @max-6xl:hidden"
          title={pullRequest.title}
        >
          {pullRequest.title}
        </span>
        <span className="flex-none rounded-[9px] border px-[7px] text-[11px] text-muted-foreground @max-[44rem]:hidden">
          {state}
        </span>
      </span>
    );
  }
  const note = barNote(status);
  if (note === null) return null;
  return (
    // Not an alert: the review works the same without its pull request, so this waits to be read.
    <span
      title={note}
      className="flex min-w-0 items-center gap-2 text-xs whitespace-nowrap text-muted-foreground"
    >
      <span className="min-w-0 truncate">{note}</span>
      {status.kind === "not_connected" && onOpenPreferences && (
        <Button variant="link" size="inline" className="text-xs text-primary" onClick={onOpenPreferences}>
          {t("review.pullRequest.connect")}
        </Button>
      )}
    </span>
  );
}
