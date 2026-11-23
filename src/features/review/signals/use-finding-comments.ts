// The review comments a model writes from its findings, only on a click on Write comment in the Diff: one call per
// finding at a time, which can be stopped, and why the last one failed, kept for as long as the review is open (the
// Diff's files come and go). The comment itself is a draft of the review, read back with the review state.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { reviewRefOf } from "@/features/review/review-state/review-status";
import { cancelModelRun, type ChangeSet, isApiError, writeFindingComment } from "@/lib/api";
import { errorMessage } from "@/lib/format";

/** A comment being written from a finding. */
export interface FindingCommentRun {
  runId: string;
  stopping: boolean;
}

/** Why the last click on a finding's Write comment failed. */
export interface FindingCommentFailure {
  message: string;
  /** The Insights task has no model set up: Preferences is where to go. */
  needsModel: boolean;
}

export interface FindingComments {
  /** Asks the model for the finding's comment: the only way one is written. */
  write: (findingId: string) => void;
  cancel: (findingId: string) => void;
  runOf: (findingId: string) => FindingCommentRun | null;
  /** Cleared by the finding's next click. */
  errorOf: (findingId: string) => FindingCommentFailure | null;
  onOpenPreferences?: () => void;
}

let runs = 0;

/** `onWritten` reads the review state again, so the new draft shows on its line and its finding links to it. */
export function useFindingComments(
  projectId: string,
  changeSet: ChangeSet | null,
  onWritten: () => void,
  onOpenPreferences?: () => void,
): FindingComments {
  const [going, setGoing] = useState<ReadonlyMap<string, FindingCommentRun>>(() => new Map());
  const [errors, setErrors] = useState<ReadonlyMap<string, FindingCommentFailure>>(() => new Map());
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const write = useCallback(
    (findingId: string) => {
      if (changeSet === null || going.has(findingId)) return;
      const runId = `finding-comment-${++runs}`;
      setGoing((current) => new Map(current).set(findingId, { runId, stopping: false }));
      setErrors((current) => {
        if (!current.has(findingId)) return current;
        const next = new Map(current);
        next.delete(findingId);
        return next;
      });
      writeFindingComment(projectId, reviewRefOf(changeSet), findingId, runId)
        .then(
          () => {
            if (alive.current) onWritten();
          },
          (cause: unknown) => {
            if (!alive.current || (isApiError(cause) && cause.code === "cancelled")) return;
            const failure = {
              message: errorMessage(cause),
              needsModel: isApiError(cause) && cause.code === "model_not_configured",
            };
            setErrors((current) => new Map(current).set(findingId, failure));
          },
        )
        .finally(() => {
          if (!alive.current) return;
          setGoing((current) => {
            const next = new Map(current);
            next.delete(findingId);
            return next;
          });
        });
    },
    [projectId, changeSet, going, onWritten],
  );

  const cancel = useCallback(
    (findingId: string) => {
      const run = going.get(findingId);
      if (!run) return;
      setGoing((current) => new Map(current).set(findingId, { ...run, stopping: true }));
      cancelModelRun(run.runId).catch(() => {
        // Already over.
      });
    },
    [going],
  );

  const runOf = useCallback((findingId: string) => going.get(findingId) ?? null, [going]);
  const errorOf = useCallback((findingId: string) => errors.get(findingId) ?? null, [errors]);

  return useMemo(
    () => ({ write, cancel, runOf, errorOf, onOpenPreferences }),
    [write, cancel, runOf, errorOf, onOpenPreferences],
  );
}
