import { useCallback, useEffect, useRef, useState } from "react";

import type { PublishOutcome, PullRequestStatus } from "@/features/review/pull-request/pull-request-status";
import { reviewRefOf } from "@/features/review/review-state/review-status";
import { type ChangeSet, linkPullRequest, publishComments, setPullRequestVerdict, type Verdict } from "@/lib/api";
import { errorMessage } from "@/lib/format";

export interface PullRequestHandle {
  status: PullRequestStatus;
  publishing: boolean;
  /** What the last publish did; cleared when the change set changes. */
  outcome: PublishOutcome | null;
  /** Sends the review's unpublished changes, then reads the pull request and the review state again. */
  publish: () => Promise<PublishOutcome | null>;
  /** The verdict being given or taken back right now. */
  deciding: Verdict | null;
  /** Why the last verdict could not be given or taken back. */
  verdictError: string | null;
  /** Gives (given true) or takes back the user's verdict, then shows the pull request as the host has it. */
  setVerdict: (verdict: Verdict, given: boolean) => Promise<void>;
}

/**
 * The pull request of a change set, looked for when it arrives (on open and after Reload). Finding it brings its
 * comments into the review state, so `onCommentsChanged` is called to read that again. Nothing is ever sent from here
 * except by `publish`.
 */
export function usePullRequest(
  projectId: string,
  changeSet: ChangeSet | null,
  onCommentsChanged: () => void,
): PullRequestHandle {
  const [status, setStatus] = useState<PullRequestStatus>({ kind: "loading" });
  const [publishing, setPublishing] = useState(false);
  const [outcome, setOutcome] = useState<PublishOutcome | null>(null);
  const [deciding, setDeciding] = useState<Verdict | null>(null);
  const [verdictError, setVerdictError] = useState<string | null>(null);
  // Only the answer to the latest look is kept.
  const latest = useRef(0);
  const changed = useRef(onCommentsChanged);
  useEffect(() => {
    changed.current = onCommentsChanged;
  }, [onCommentsChanged]);

  /** Resolves to the kind of status found, or null when a later look took over. */
  const look = useCallback(
    async (current: ChangeSet): Promise<PullRequestStatus["kind"] | null> => {
      const request = ++latest.current;
      let next: PullRequestStatus;
      try {
        next = await linkPullRequest(projectId, reviewRefOf(current));
      } catch (cause: unknown) {
        next = { kind: "error", message: errorMessage(cause) };
      }
      if (request !== latest.current) return null;
      setStatus(next);
      if (next.kind === "linked") changed.current();
      return next.kind;
    },
    [projectId],
  );

  useEffect(() => {
    if (changeSet === null) return;
    setOutcome(null);
    setVerdictError(null);
    if (changeSet.target.kind === "working_tree") {
      // Nothing to look for: say so at once.
      latest.current++;
      setStatus({ kind: "working_tree" });
      return;
    }
    setStatus({ kind: "loading" });
    void look(changeSet);
  }, [changeSet, look]);

  useEffect(
    () => () => {
      // A look still on its way when the screen closes is ignored.
      latest.current++;
    },
    [],
  );

  const publish = useCallback(async (): Promise<PublishOutcome | null> => {
    if (changeSet === null) return null;
    setPublishing(true);
    setOutcome(null);
    let result: PublishOutcome;
    try {
      result = { kind: "report", report: await publishComments(projectId, reviewRefOf(changeSet)) };
    } catch (cause: unknown) {
      result = { kind: "error", message: errorMessage(cause) };
    }
    setOutcome(result);
    // What was published comes back from the pull request with its remote ids; what failed stays a draft. A look
    // that finds the pull request reads the review state again itself.
    if ((await look(changeSet)) !== "linked") changed.current();
    setPublishing(false);
    return result;
  }, [changeSet, look, projectId]);

  const setVerdict = useCallback(
    async (verdict: Verdict, given: boolean) => {
      if (changeSet === null) return;
      setDeciding(verdict);
      setVerdictError(null);
      const request = ++latest.current;
      try {
        const next = await setPullRequestVerdict(projectId, reviewRefOf(changeSet), verdict, given);
        if (request === latest.current) setStatus(next);
        // Comments were pulled again with the pull request.
        changed.current();
      } catch (cause: unknown) {
        setVerdictError(errorMessage(cause));
        // Whatever happened on the host, show it as it is now.
        if ((await look(changeSet)) !== "linked") changed.current();
      }
      setDeciding(null);
    },
    [changeSet, look, projectId],
  );

  return { status, publishing, outcome, publish, deciding, verdictError, setVerdict };
}
