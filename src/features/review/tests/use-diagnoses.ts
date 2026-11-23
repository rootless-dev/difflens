import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { type ShownDiagnosis, shownDiagnosis } from "@/features/review/ai-text/versions";
import { reviewRefOf } from "@/features/review/review-state/review-status";
import { failureKey, failureSubject } from "@/features/review/tests/diagnosis-text";
import { type Failure, failureCount } from "@/features/review/tests/suite";
import type { TestsPanel } from "@/features/review/tests/use-tests";
import { useAppLanguage } from "@/i18n/language";
import {
  cancelTestDiagnosis,
  type ChangeSet,
  diagnoseTestFailure,
  getModelInUse,
  getRunningTestDiagnoses,
  getTestDiagnoses,
  isApiError,
  joinTestDiagnosis,
  type Language,
  type ModelInUse,
  cancelModelRun,
  planTestDiagnosesTranslation,
  type ReviewComment,
  type RunningDiagnosis,
  type TestDiagnosis,
  type TestRun,
  translateTestDiagnoses,
  type TranslationPlan,
  writeReviewComment,
} from "@/lib/api";
import { errorMessage } from "@/lib/format";

/** An explanation being written, asked for from this screen or one left since. */
export interface DiagnosisRun {
  key: string;
  /** Waiting for another explanation to end: the model takes this many calls at once. */
  queued: boolean;
  stopping: boolean;
}

/**
 * Whether the explanations already written could be read: while they are read, or when that failed, a failure without
 * one shown is not offered Explain the failure, so one explained before is never asked for (and paid for) again.
 */
export type SavedDiagnoses = { status: "reading" } | { status: "ready" } | { status: "failed"; message: string };

export interface DiagnosesHandle {
  /** What the model wrote about each failure, by key (see `failureKey`), in the version that reads in the language shown if any. */
  shown: ReadonlyMap<string, ShownDiagnosis>;
  saved: SavedDiagnoses;
  /** Reads the explanations already written again, after reading them failed. */
  retryRead: () => void;
  /** Who Explain the failure would ask (the test_diagnosis task's model), for its tooltip; null until read. */
  model: ModelInUse | null;
  runOf: (key: string) => DiagnosisRun | null;
  /** Why the last click on the failure `key` failed; cleared by its next click. */
  errorOf: (key: string) => string | null;
  /** Asks the model why `failure` failed: the only way an explanation of a failure is written. */
  explain: (failure: Failure) => void;
  cancel: (key: string) => void;
  /** What translating the explanations `keys` into `language` would send. Asks no model. */
  planTranslation: (language: Language, keys: string[]) => Promise<TranslationPlan>;
  /** Asks the model one call of a translation, after the click on Translate; keeps what it answers. */
  translateCall: (language: Language, keys: string[], runId: string) => Promise<void>;
  /** Who Write review comment would ask (the review_comment task's model), for its tooltip; null until read. */
  commentModel: ModelInUse | null;
  /** The review comment written from the explanation of `key`, while it is still there (not deleted). */
  commentOf: (key: string) => ReviewComment | null;
  /** Write review comment is being answered for `key`. */
  commentRunOf: (key: string) => { stopping: boolean } | null;
  /** Why the last Write review comment of `key` failed; cleared by its next click. */
  commentErrorOf: (key: string) => string | null;
  /** Asks the model for a review comment from the explanation of `key`, kept as a draft: the tab stays as it is. */
  writeComment: (key: string) => void;
  cancelComment: (key: string) => void;
  /** Shows the comment written from the explanation of `key` in the Diff. */
  viewComment: (key: string) => void;
}

/** The review's comments, which the comments written from explanations are among. */
export interface DiagnosisComments {
  /** null until the review state is read. */
  comments: readonly ReviewComment[] | null;
  /** A comment was written: read the review state again. */
  onWritten: () => void;
  /** Switches to the Diff tab and brings `comment` into view. */
  onView: (comment: ReviewComment) => void;
}

const NO_COMMENTS: DiagnosisComments = { comments: null, onWritten: () => {}, onView: () => {} };

const NO_SHOWN: ReadonlyMap<string, ShownDiagnosis> = new Map();
const READING: SavedDiagnoses = { status: "reading" };
const READY: SavedDiagnoses = { status: "ready" };

/** While an explanation is being written, how often the core is asked which ones still wait their turn. */
export const RUNNING_POLL_MS = 1000;

/** Why reading the explanations against `run` failed, on the attempt `attempt`. */
interface ReadFailure {
  changeSet: ChangeSet;
  run: TestRun;
  attempt: number;
  message: string;
}

let runs = 0;

/** The completed run of the panel when it has failures to explain. */
function failingRun(panel: TestsPanel) {
  if (panel.status !== "ready") return null;
  const run = panel.tests.run;
  return run !== null && run.status === "completed" && failureCount(run) > 0 ? run : null;
}

/**
 * What was read, with what `fetched` (read later) lacks: an explanation written meanwhile (by a click, or a call
 * joined on coming back) that the read began too early to see, or a newer one of the same failure.
 */
function merged(current: readonly TestDiagnosis[], fetched: readonly TestDiagnosis[]): TestDiagnosis[] {
  const byKey = new Map(fetched.map((diagnosis) => [diagnosis.key, diagnosis]));
  for (const kept of current) {
    const read = byKey.get(kept.key);
    if (read === undefined || kept.createdAt > read.createdAt) byKey.set(kept.key, kept);
  }
  return [...byKey.values()];
}

/**
 * The explanations of a review's failed tests: read (never written) once `requested` (the Tests tab was opened) and a
 * completed run with failures is shown, again whenever another run reports, so each is read against the last run;
 * written only by `explain`, after a click on Explain the failure. Until the first read lands (`saved`), or when it
 * failed, no failure is offered Explain the failure as if it had never been explained.
 *
 * The calls are kept by the core, not by this screen: they wait their turn there (as many at once as the model takes,
 * one for Claude Code), go on when the review is left, and are found again on coming back, shown as being written
 * (with Stop) until what they write arrives. Asking twice for one failure joins the call going on.
 */
export function useTestDiagnoses(
  projectId: string,
  changeSet: ChangeSet | null,
  panel: TestsPanel,
  requested: boolean,
  review: DiagnosisComments = NO_COMMENTS,
): DiagnosesHandle {
  const language = useAppLanguage();
  const run = failingRun(panel);
  const [read, setRead] = useState<{ changeSet: ChangeSet; diagnoses: readonly TestDiagnosis[] } | null>(null);
  const [model, setModel] = useState<ModelInUse | null>(null);
  const [commentModel, setCommentModel] = useState<ModelInUse | null>(null);
  const [commentRuns, setCommentRuns] = useState<ReadonlyMap<string, { runId: string; stopping: boolean }>>(new Map());
  const [commentErrors, setCommentErrors] = useState<ReadonlyMap<string, string>>(new Map());
  const [running, setRunning] = useState<ReadonlyMap<string, DiagnosisRun>>(new Map());
  const [errors, setErrors] = useState<ReadonlyMap<string, string>>(new Map());
  const alive = useRef(true);
  /** The failures this screen waits for: a call it started, or one it joined. */
  const followed = useRef<Set<string>>(new Set());
  const modelRef = useRef<ModelInUse | null>(null);
  useEffect(() => {
    modelRef.current = model;
  }, [model]);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const [attempt, setAttempt] = useState(0);
  const [failure, setFailure] = useState<ReadFailure | null>(null);
  const retryRead = useCallback(() => setAttempt((n) => n + 1), []);

  const keep = useCallback((owner: ChangeSet, diagnosis: TestDiagnosis) => {
    if (!alive.current) return;
    setRead((current) => {
      const kept = current !== null && current.changeSet === owner ? current.diagnoses : [];
      return { changeSet: owner, diagnoses: [...kept.filter((other) => other.key !== diagnosis.key), diagnosis] };
    });
  }, []);

  const unfollow = useCallback((key: string) => {
    followed.current.delete(key);
    if (!alive.current) return;
    setRunning((current) => {
      if (!current.has(key)) return current;
      const next = new Map(current);
      next.delete(key);
      return next;
    });
  }, []);

  const failed = useCallback((key: string, cause: unknown) => {
    if (!alive.current || (isApiError(cause) && cause.code === "cancelled")) return;
    setErrors((current) => new Map(current).set(key, errorMessage(cause)));
  }, []);

  /**
   * Takes in the calls the core says are being written for `owner`: the ones this screen follows get their place in
   * the queue; the others (asked for before the review was left) are shown as being written and joined.
   */
  const follow = useCallback(
    (owner: ChangeSet, calls: readonly RunningDiagnosis[]) => {
      if (!alive.current) return;
      const joined = calls.filter((call) => !followed.current.has(call.key));
      for (const call of joined) followed.current.add(call.key);
      setRunning((current) => {
        let next: Map<string, DiagnosisRun> | null = null;
        for (const call of calls) {
          const known = current.get(call.key);
          if (known !== undefined && known.queued === call.queued) continue;
          if (known === undefined && !joined.includes(call)) continue;
          next ??= new Map(current);
          next.set(call.key, { key: call.key, queued: call.queued, stopping: known?.stopping ?? false });
        }
        return next ?? current;
      });
      for (const call of joined) {
        joinTestDiagnosis(projectId, reviewRefOf(owner), call.key)
          .then(
            (diagnosis) => {
              if (diagnosis !== null) keep(owner, diagnosis);
              // Ended before it could be joined: what it wrote is kept, read it.
              else if (alive.current) setAttempt((n) => n + 1);
            },
            (cause: unknown) => failed(call.key, cause),
          )
          .finally(() => unfollow(call.key));
      }
    },
    [projectId, keep, unfollow, failed],
  );

  const wanted = requested && changeSet !== null && run !== null;
  useEffect(() => {
    if (!wanted || changeSet === null || run === null) return;
    let cancelled = false;
    const review = reviewRefOf(changeSet);
    // The calls going on first, then what was written: a call that ends in between has kept what it wrote by then.
    getRunningTestDiagnoses(projectId, review)
      .then(
        (calls) => calls ?? [],
        () => [] as RunningDiagnosis[],
      )
      .then((calls) => {
        if (cancelled) return;
        follow(changeSet, calls);
        return getTestDiagnoses(projectId, review).then(
          (diagnoses) => {
            if (cancelled) return;
            setRead((current) => ({
              changeSet,
              diagnoses: current !== null && current.changeSet === changeSet ? merged(current.diagnoses, diagnoses) : diagnoses,
            }));
          },
          (cause: unknown) => {
            // Said over the failures, instead of offering to explain again what may already be explained.
            if (!cancelled) setFailure({ changeSet, run, attempt, message: errorMessage(cause) });
          },
        );
      });
    getModelInUse("test_diagnosis").then(
      (inUse) => {
        if (!cancelled) setModel(inUse);
      },
      () => {
        // The tooltip names "the model chosen in Preferences" instead.
      },
    );
    getModelInUse("review_comment").then(
      (inUse) => {
        if (!cancelled) setCommentModel(inUse);
      },
      () => {
        // Likewise.
      },
    );
    return () => {
      cancelled = true;
    };
    // Read again for each run reported: each explanation is read against it.
  }, [projectId, changeSet, run, wanted, attempt, follow]);

  // While some are being written, which still wait their turn, as the core has it.
  const writing = running.size > 0;
  useEffect(() => {
    if (!writing || changeSet === null) return;
    const owner = changeSet;
    const timer = setInterval(() => {
      getRunningTestDiagnoses(projectId, reviewRefOf(owner)).then(
        (calls) => follow(owner, calls ?? []),
        () => {
          // Asked again on the next tick.
        },
      );
    }, RUNNING_POLL_MS);
    return () => clearInterval(timer);
  }, [writing, projectId, changeSet, follow]);

  const diagnoses = read !== null && read.changeSet === changeSet ? read.diagnoses : null;
  const readFailed =
    failure !== null && failure.changeSet === changeSet && failure.run === run && failure.attempt === attempt
      ? failure.message
      : null;
  // What was read for an earlier run of this change set stays shown while the next run's read goes on.
  const saved = useMemo<SavedDiagnoses>(
    () => (readFailed !== null ? { status: "failed", message: readFailed } : diagnoses === null ? READING : READY),
    [readFailed, diagnoses],
  );
  const shown = useMemo(
    () =>
      diagnoses === null
        ? NO_SHOWN
        : new Map(diagnoses.map((diagnosis) => [diagnosis.key, shownDiagnosis(diagnosis, language)])),
    [diagnoses, language],
  );

  const explain = useCallback(
    (failure: Failure) => {
      if (changeSet === null) return;
      const owner = changeSet;
      const key = failureKey(failure);
      if (followed.current.has(key)) return;
      followed.current.add(key);
      setRunning((current) => {
        // Until the core says, waiting when the model already has as many calls as it takes.
        const queued = current.size >= Math.max(1, modelRef.current?.parallelCalls ?? 1);
        return new Map(current).set(key, { key, queued, stopping: false });
      });
      setErrors((current) => {
        if (!current.has(key)) return current;
        const next = new Map(current);
        next.delete(key);
        return next;
      });
      const { pkg, test } = failureSubject(failure);
      diagnoseTestFailure(projectId, reviewRefOf(owner), pkg, test, `diagnose-${++runs}`)
        .then(
          (diagnosis) => keep(owner, diagnosis),
          (cause: unknown) => failed(key, cause),
        )
        .finally(() => unfollow(key));
    },
    [projectId, changeSet, keep, failed, unfollow],
  );

  const cancel = useCallback(
    (key: string) => {
      if (changeSet === null || !running.has(key)) return;
      setRunning((current) => {
        const found = current.get(key);
        return found ? new Map(current).set(key, { ...found, stopping: true }) : current;
      });
      cancelTestDiagnosis(projectId, reviewRefOf(changeSet), key).catch(() => {
        // Already over.
      });
    },
    [projectId, changeSet, running],
  );

  const runOf = useCallback((key: string) => running.get(key) ?? null, [running]);
  const errorOf = useCallback((key: string) => errors.get(key) ?? null, [errors]);

  const planTranslation = useCallback(
    (target: Language, keys: string[]) => {
      if (changeSet === null) return Promise.reject(new Error("No change set."));
      return planTestDiagnosesTranslation(projectId, reviewRefOf(changeSet), target, keys);
    },
    [projectId, changeSet],
  );

  const translateCall = useCallback(
    async (target: Language, keys: string[], runId: string) => {
      if (changeSet === null) return;
      const owner = changeSet;
      const diagnoses = await translateTestDiagnoses(projectId, reviewRefOf(owner), target, keys, runId);
      if (alive.current) setRead({ changeSet: owner, diagnoses });
    },
    [projectId, changeSet],
  );

  const { comments, onWritten, onView } = review;
  const commentOf = useCallback(
    (key: string) => {
      const id = shown.get(key)?.commentId ?? null;
      if (id === null || comments === null) return null;
      // Deleted here (a published one waiting for the next publish to leave) counts as gone.
      return comments.find((comment) => comment.id === id && comment.pendingChange !== "delete") ?? null;
    },
    [shown, comments],
  );

  const writeComment = useCallback(
    (key: string) => {
      if (changeSet === null || commentRuns.has(key)) return;
      const owner = changeSet;
      const runId = `review-comment-${++runs}`;
      setCommentRuns((current) => new Map(current).set(key, { runId, stopping: false }));
      setCommentErrors((current) => {
        if (!current.has(key)) return current;
        const next = new Map(current);
        next.delete(key);
        return next;
      });
      writeReviewComment(projectId, reviewRefOf(owner), key, runId)
        .then(
          (written) => {
            if (!alive.current) return;
            setRead((current) =>
              current === null || current.changeSet !== owner
                ? current
                : {
                    changeSet: owner,
                    diagnoses: current.diagnoses.map((diagnosis) =>
                      diagnosis.key === written.key ? { ...diagnosis, commentId: written.commentId } : diagnosis,
                    ),
                  },
            );
            onWritten();
          },
          (cause: unknown) => {
            if (!alive.current || (isApiError(cause) && cause.code === "cancelled")) return;
            setCommentErrors((current) => new Map(current).set(key, errorMessage(cause)));
          },
        )
        .finally(() => {
          if (!alive.current) return;
          setCommentRuns((current) => {
            const next = new Map(current);
            next.delete(key);
            return next;
          });
        });
    },
    [projectId, changeSet, commentRuns, onWritten],
  );

  const cancelComment = useCallback(
    (key: string) => {
      const going = commentRuns.get(key);
      if (!going) return;
      setCommentRuns((current) => new Map(current).set(key, { ...going, stopping: true }));
      cancelModelRun(going.runId).catch(() => {
        // Already over.
      });
    },
    [commentRuns],
  );

  const viewComment = useCallback(
    (key: string) => {
      const comment = commentOf(key);
      if (comment) onView(comment);
    },
    [commentOf, onView],
  );

  const commentRunOf = useCallback((key: string) => commentRuns.get(key) ?? null, [commentRuns]);
  const commentErrorOf = useCallback((key: string) => commentErrors.get(key) ?? null, [commentErrors]);

  return useMemo(
    () => ({
      shown,
      saved,
      retryRead,
      model,
      runOf,
      errorOf,
      explain,
      cancel,
      planTranslation,
      translateCall,
      commentModel,
      commentOf,
      commentRunOf,
      commentErrorOf,
      writeComment,
      cancelComment,
      viewComment,
    }),
    [
      shown,
      saved,
      retryRead,
      model,
      runOf,
      errorOf,
      explain,
      cancel,
      planTranslation,
      translateCall,
      commentModel,
      commentOf,
      commentRunOf,
      commentErrorOf,
      writeComment,
      cancelComment,
      viewComment,
    ],
  );
}
