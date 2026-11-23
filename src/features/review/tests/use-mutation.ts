import { useCallback, useEffect, useState } from "react";

import { PROGRESS_POLL_MS, RUN_POLL_MS } from "@/features/review/tests/use-tests";
import {
  cancelMutation,
  type ChangeSet,
  getMutationProgress,
  getMutationState,
  type MutationProgress,
  type MutationRun,
  type MutationState,
  runMutation,
} from "@/lib/api";
import { errorMessage } from "@/lib/format";

/** A mutation run going on now. */
export interface MutationRunning {
  /** When this window started it or first saw it, for the elapsed time. */
  since: number;
  /** Started by this window, so its runMutation call tells when it ends; otherwise it is polled. */
  ours: boolean;
  cancelling: boolean;
}

export type MutationPanel =
  | { status: "loading" }
  | { status: "error"; message: string }
  | {
      status: "ready";
      /** The last completed run and why none can run, as getMutationState gave them, updated by the runs made here. */
      state: MutationState;
      running: MutationRunning | null;
      /** How far the run going on got; null before the analyzer says. */
      progress: MutationProgress | null;
      /** The last run started here when it ended without completing (failed, timed out, cancelled), until another. */
      ended: MutationRun | null;
      /** Why the last attempt to run or cancel did not get an answer. */
      runError: string | null;
    };

type ReadyPanel = Extract<MutationPanel, { status: "ready" }>;

interface Loaded {
  changeSet: ChangeSet;
  panel: MutationPanel;
}

const seen = (): MutationRunning => ({ since: Date.now(), ours: false, cancelling: false });

/**
 * Mutation testing of a change set, kept above the tabs so a run goes on while the reviewer is elsewhere: the last
 * completed run and why none can run, read without running anything as soon as the change set is there and again
 * when `testRuns` changes (a test run with coverage is what makes a run possible); and the runs started from here,
 * only on click. A run started elsewhere (another window, or before a Reload) is checked on until it ends. Like
 * useTests, the state belongs to the change set object it was read for.
 */
export function useMutation(projectId: string, changeSet: ChangeSet | null, testRuns: number) {
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (changeSet === null) return;
    let cancelled = false;
    // Read again for the same change set, what is shown stays until the answer; a run going on is kept.
    setLoaded((previous) => (previous?.changeSet === changeSet ? previous : { changeSet, panel: { status: "loading" } }));
    getMutationState(projectId, changeSet.oldSha, changeSet.newSha).then(
      (state) => {
        if (cancelled) return;
        setLoaded((previous) => {
          const before = previous?.changeSet === changeSet && previous.panel.status === "ready" ? previous.panel : null;
          // A run started here is followed by its own call; one started elsewhere by what the core says now.
          const running = before?.running?.ours ? before.running : state.running ? (before?.running ?? seen()) : null;
          return {
            changeSet,
            panel: {
              status: "ready",
              state: { ...state, running: running !== null },
              running,
              progress: running ? (before?.progress ?? null) : null,
              ended: before?.ended ?? null,
              runError: null,
            },
          };
        });
      },
      (error: unknown) => {
        if (!cancelled) setLoaded({ changeSet, panel: { status: "error", message: errorMessage(error) } });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [projectId, changeSet, attempt, testRuns]);

  /** Changes the panel of `owner` if it is still the one shown and ready. */
  const update = useCallback(
    (owner: ChangeSet, change: (panel: ReadyPanel) => MutationPanel) =>
      setLoaded((current) =>
        current?.changeSet === owner && current.panel.status === "ready"
          ? { changeSet: owner, panel: change(current.panel) }
          : current,
      ),
    [],
  );

  /** A run ended: a completed one replaces the one kept; another is only told about, the completed one staying. */
  const finish = useCallback(
    (owner: ChangeSet, run: MutationRun) =>
      update(owner, (panel) => ({
        ...panel,
        state: { ...panel.state, run: run.status === "completed" ? run : panel.state.run, running: false },
        running: null,
        progress: null,
        ended: run.status === "completed" ? null : run,
        runError: null,
      })),
    [update],
  );

  const panel: MutationPanel =
    loaded !== null && loaded.changeSet === changeSet ? loaded.panel : { status: "loading" };
  const runningNow = panel.status === "ready" && panel.running !== null;
  const watching = panel.status === "ready" && panel.running !== null && !panel.running.ours;

  // A run this window did not start is checked on until it ends; then its result is read like on open.
  useEffect(() => {
    if (!watching || changeSet === null) return;
    let stopped = false;
    const timer = setInterval(() => {
      getMutationState(projectId, changeSet.oldSha, changeSet.newSha).then(
        (state) => {
          if (stopped || state.running) return;
          update(changeSet, (current) => ({ ...current, state, running: null, progress: null }));
        },
        () => {
          // Asked again at the next tick.
        },
      );
    }, RUN_POLL_MS);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [watching, projectId, changeSet, update]);

  // How far the run got, while it goes on.
  useEffect(() => {
    if (!runningNow || changeSet === null) return;
    let stopped = false;
    const timer = setInterval(() => {
      getMutationProgress(projectId, changeSet.newSha).then(
        (progress) => {
          if (!stopped && progress) {
            update(changeSet, (current) => (current.running ? { ...current, progress } : current));
          }
        },
        () => {
          // Asked again at the next tick.
        },
      );
    }, PROGRESS_POLL_MS);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [runningNow, projectId, changeSet, update]);

  /** Runs mutation testing on the change set: only from a click. */
  const run = useCallback(() => {
    if (changeSet === null || loaded?.changeSet !== changeSet || loaded.panel.status !== "ready") return;
    const owner = changeSet;
    update(owner, (current) => ({
      ...current,
      state: { ...current.state, running: true },
      running: { since: Date.now(), ours: true, cancelling: false },
      progress: null,
      ended: null,
      runError: null,
    }));
    runMutation(projectId, owner.oldSha, owner.newSha).then(
      (result) => finish(owner, result),
      (error: unknown) =>
        update(owner, (current) => ({
          ...current,
          state: { ...current.state, running: false },
          running: null,
          progress: null,
          runError: errorMessage(error),
        })),
    );
  }, [projectId, changeSet, loaded, update, finish]);

  const cancel = useCallback(() => {
    if (changeSet === null) return;
    const owner = changeSet;
    update(owner, (current) =>
      current.running ? { ...current, running: { ...current.running, cancelling: true }, runError: null } : current,
    );
    cancelMutation(projectId, owner.newSha).catch((error: unknown) =>
      update(owner, (current) => ({
        ...current,
        running: current.running && { ...current.running, cancelling: false },
        runError: errorMessage(error),
      })),
    );
  }, [projectId, changeSet, update]);

  const retry = useCallback(() => setAttempt((n) => n + 1), []);

  return { panel, run, cancel, retry };
}

export type MutationHandle = ReturnType<typeof useMutation>;

/** The last completed mutation run, for the review's signals; null without one or while it is read. */
export function mutationRunOf(panel: MutationPanel): MutationRun | null {
  return panel.status === "ready" ? panel.state.run : null;
}
