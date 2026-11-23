import { useCallback, useEffect, useState } from "react";

import type { PullRequestStatus } from "@/features/review/pull-request/pull-request-status";
import { type CardLookup, type ChangeSet, findCard } from "@/lib/api";
import { errorMessage } from "@/lib/format";

export type CardStatus = { kind: "loading" } | CardLookup | { kind: "error"; key: string | null; message: string };

/**
 * The first card key in a text, such as ABC-123 in "feature/ABC-123-login": what `[A-Z][A-Z0-9]+-\d+` matches, as
 * the core finds it (`branches::card`).
 */
export function issueKeyOf(text: string): string | null {
  return /[A-Z][A-Z0-9]+-\d+/.exec(text)?.[0] ?? null;
}

/**
 * The pull request title a card key is looked for in when the branch has none: undefined while the pull request is
 * still being looked for, null when there is none.
 */
function titleOf(status: PullRequestStatus): string | null | undefined {
  if (status.kind === "loading") return undefined;
  return status.kind === "linked" ? status.pullRequest.title : null;
}

/**
 * The card of a change set, read from Jira (only read) when it arrives: at once when the branch name holds a key,
 * else once the pull request is found or known to be missing, so its title can be looked in. Reload reads it again.
 */
export function useCard(changeSet: ChangeSet | null, pullRequest: PullRequestStatus) {
  const [found, setFound] = useState<{ changeSet: ChangeSet; title: string | null; status: CardStatus } | null>(null);
  const [attempt, setAttempt] = useState(0);
  const branch = changeSet?.target.kind === "branch" ? changeSet.target.branch : null;
  const branchKey = branch === null ? null : issueKeyOf(branch);
  // With a key in the branch, the title does not matter.
  const title = branchKey !== null ? null : titleOf(pullRequest);

  useEffect(() => {
    if (changeSet === null || title === undefined) return;
    if (changeSet.target.kind === "working_tree") {
      setFound({ changeSet, title, status: { kind: "working_tree" } });
      return;
    }
    let cancelled = false;
    setFound({ changeSet, title, status: { kind: "loading" } });
    findCard(changeSet.target, title).then(
      (lookup) => {
        if (!cancelled) setFound({ changeSet, title, status: lookup });
      },
      (cause: unknown) => {
        if (cancelled) return;
        const key = branchKey ?? (title === null ? null : issueKeyOf(title));
        setFound({ changeSet, title, status: { kind: "error", key, message: errorMessage(cause) } });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [changeSet, title, branchKey, attempt]);

  const retry = useCallback(() => setAttempt((n) => n + 1), []);
  const status: CardStatus =
    found !== null && found.changeSet === changeSet && found.title === title ? found.status : { kind: "loading" };
  return { status, retry };
}
