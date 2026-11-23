import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { reviewRefOf } from "@/features/review/review-state/review-status";
import {
  addComment,
  addGeneralComment,
  type ChangeSet,
  deleteComment,
  draftSignalComment,
  editComment,
  type FunctionSpan,
  getReviewState,
  replyToComment,
  restoreComment,
  type ReviewRef,
  type ReviewState,
  setCommentResolved,
  setFileReviewed,
  setFunctionReviewed,
  type SignalDraft,
  type SignalDraftRequest,
} from "@/lib/api";
import { errorMessage } from "@/lib/format";

/** Each resolves to whether it was saved; a failure is kept in `error` for the screen to show. */
export interface ReviewActions {
  setFileReviewed: (path: string, reviewed: boolean) => Promise<boolean>;
  setFunctionReviewed: (span: FunctionSpan, reviewed: boolean) => Promise<boolean>;
  addComment: (path: string, side: "old" | "new", line: number, body: string) => Promise<boolean>;
  /** A comment on the pull request as a whole, on no file or line. */
  addGeneralComment: (body: string) => Promise<boolean>;
  reply: (rootId: string, body: string) => Promise<boolean>;
  edit: (commentId: string, body: string) => Promise<boolean>;
  remove: (commentId: string) => Promise<boolean>;
  /** Takes back the delete of a published comment before it is published. */
  restore: (commentId: string) => Promise<boolean>;
  resolve: (commentId: string, resolved: boolean) => Promise<boolean>;
  /** A draft written from a model finding or a static check; resolves to it, or null when it could not be written. */
  draftFromSignal: (request: SignalDraftRequest) => Promise<SignalDraft | null>;
}

export interface ReviewStateHandle {
  /** null until first read. A new change set (after Reload) keeps the last state on screen until its own arrives. */
  state: ReviewState | null;
  /** Why the last read or save failed; cleared by the next one that works. */
  error: string | null;
  /** A save is under way; the state is read again once it is done. */
  busy: boolean;
  actions: ReviewActions;
  /** Reads the state again, for changes made elsewhere (comments brought in from the pull request). */
  refresh: () => void;
}

/**
 * The review state of a change set (marks, comments, the last review), read again after every save and whenever the
 * change set or the functions asked about change. Function statuses are only known for `functions`, the changed
 * functions of the flow graph once it is there.
 */
export function useReviewState(
  projectId: string,
  changeSet: ChangeSet | null,
  functions: readonly FunctionSpan[],
): ReviewStateHandle {
  const [state, setState] = useState<ReviewState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Only the answer to the latest read is kept, whichever arrives last.
  const latest = useRef(0);
  // The functions of the last read, asked about again by the reads that follow a save.
  const asked = useRef(functions);

  const read = useCallback(
    async (current: ChangeSet) => {
      const request = ++latest.current;
      try {
        const data = await getReviewState(projectId, reviewRefOf(current), [...asked.current]);
        if (request !== latest.current) return;
        setState(data);
        setError(null);
      } catch (cause: unknown) {
        if (request === latest.current) setError(errorMessage(cause));
      }
    },
    [projectId],
  );

  useEffect(() => {
    asked.current = functions;
    if (changeSet !== null) void read(changeSet);
  }, [changeSet, functions, read]);

  useEffect(
    () => () => {
      // A read still on its way when the screen closes is ignored.
      latest.current++;
    },
    [],
  );

  const run = useCallback(
    async (save: (review: ReviewRef) => Promise<unknown>): Promise<boolean> => {
      if (changeSet === null) return false;
      setBusy(true);
      let failure: string | null = null;
      try {
        await save(reviewRefOf(changeSet));
      } catch (cause: unknown) {
        failure = errorMessage(cause);
      }
      // Read again either way: a save that failed may have found the state changed under it.
      await read(changeSet);
      if (failure !== null) setError(failure);
      setBusy(false);
      return failure === null;
    },
    [changeSet, read],
  );

  const actions = useMemo<ReviewActions>(
    () => ({
      setFileReviewed: (path, reviewed) => run((review) => setFileReviewed(projectId, review, path, reviewed)),
      setFunctionReviewed: (span, reviewed) =>
        run((review) => setFunctionReviewed(projectId, review, span, reviewed)),
      addComment: (path, side, line, body) => run((review) => addComment(projectId, review, path, side, line, body)),
      addGeneralComment: (body) => run((review) => addGeneralComment(projectId, review, body)),
      reply: (rootId, body) => run(() => replyToComment(projectId, rootId, body)),
      edit: (commentId, body) => run(() => editComment(projectId, commentId, body)),
      remove: (commentId) => run(() => deleteComment(projectId, commentId)),
      restore: (commentId) => run(() => restoreComment(projectId, commentId)),
      resolve: (commentId, resolved) => run(() => setCommentResolved(projectId, commentId, resolved)),
      draftFromSignal: async (request) => {
        const written: { draft: SignalDraft | null } = { draft: null };
        const saved = await run(async (review) => {
          written.draft = await draftSignalComment(projectId, review, request);
        });
        return saved ? written.draft : null;
      },
    }),
    [projectId, run],
  );

  const refresh = useCallback(() => {
    if (changeSet !== null) void read(changeSet);
  }, [changeSet, read]);

  return { state, error, busy, actions, refresh };
}
