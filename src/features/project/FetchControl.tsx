import { Trans, useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import type { FetchState } from "@/features/project/use-project-fetch";
import { formatRelativeTime } from "@/lib/format";

interface FetchControlProps {
  lastFetchedAt: string | null;
  now: Date;
  state: FetchState;
  /** False for a project whose folder is gone: there is nothing to fetch into. */
  available: boolean;
  /** False for a repository without a remote: there is nothing to fetch from. */
  hasRemote: boolean;
  onFetch: () => void;
}

/** "Fetched 3 min ago [Fetch]" in the page header. */
export function FetchControl({
  lastFetchedAt,
  now,
  state,
  available,
  hasRemote,
  onFetch,
}: FetchControlProps) {
  const { t } = useTranslation();
  const running = state.status === "running";
  const noRemote = available && !hasRemote;
  return (
    <div className="flex items-center gap-2.5">
      <span className="text-xs whitespace-nowrap text-faint">
        {lastFetchedAt === null ? (
          t("project.fetch.never")
        ) : (
          <Trans
            i18nKey="project.fetch.fetchedAt"
            values={{ when: formatRelativeTime(lastFetchedAt, now) }}
            components={{ time: <time dateTime={lastFetchedAt} /> }}
          />
        )}
      </span>
      <Button
        variant="secondary"
        onClick={onFetch}
        disabled={running || !available || !hasRemote}
        aria-busy={running || undefined}
        title={noRemote ? t("project.fetch.noRemote") : t("project.fetch.title")}
      >
        {running ? t("project.fetch.running") : t("project.fetch.button")}
      </Button>
    </div>
  );
}

/** Outcome of the last fetch, shown under the header. */
export function FetchOutcome({ state }: { state: FetchState }) {
  const { t } = useTranslation();
  return (
    <div aria-live="polite" className="empty:hidden">
      {state.status === "done" && (
        <p className="mt-2 text-xs text-muted-foreground">
          {state.branchesUpdated === 0
            ? t("project.fetch.nothingNew")
            : t("project.fetch.updated", { count: state.branchesUpdated })}
        </p>
      )}
      {state.status === "error" && (
        <p role="alert" className="mt-2 text-xs text-destructive">
          {state.message}
        </p>
      )}
    </div>
  );
}
