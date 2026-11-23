import { useCallback, useEffect, useRef, useState } from "react";

import { failureCount } from "@/features/review/tests/suite";
import {
  type ChangeSet,
  cancelTests,
  getTestsProgress,
  getTestsState,
  type Isolation,
  type RunSelection,
  runTests,
  type TestProgress,
  type TestRun,
  type TestsState,
} from "@/lib/api";
import { errorMessage } from "@/lib/format";

/** How often a run started elsewhere (another window, or before a Reload) is checked on until it ends. */
export const RUN_POLL_MS = 2000;
/** How often the progress of a run of the whole suite is read. */
export const PROGRESS_POLL_MS = 1000;

/** How the next run goes, from whichever button: where, and with the race detector and coverage or not. */
export interface RunChoice {
  isolation: Isolation;
  race: boolean;
  coverage: boolean;
}

/** A run going on now. */
export interface Running {
  /** When this window started it or first saw it, for the elapsed time. */
  since: number;
  isolation: Isolation;
  /** What it runs; null for every test. */
  selection: RunSelection | null;
  /** Started by this window, so its runTests call tells when it ends; otherwise it is polled. */
  ours: boolean;
  cancelling: boolean;
}

export type TestsPanel =
  | { status: "loading" }
  | { status: "error"; message: string }
  | {
      status: "ready";
      /** The last run and the project's isolation choice, as getTestsState gave them, updated by the runs made here. */
      tests: TestsState;
      running: Running | null;
      /** How far a run of the whole suite got, while it goes on. */
      progress: TestProgress | null;
      /** Why the last attempt to run or cancel did not get an answer. */
      runError: string | null;
      choice: RunChoice;
    };

type ReadyPanel = Extract<TestsPanel, { status: "ready" }>;

interface Loaded {
  changeSet: ChangeSet;
  panel: TestsPanel;
}

function readyPanel(tests: TestsState): TestsPanel {
  return {
    status: "ready",
    tests,
    running: tests.running
      ? { since: Date.now(), isolation: tests.isolation, selection: tests.runningSelection, ours: false, cancelling: false }
      : null,
    progress: null,
    runError: null,
    choice: {
      isolation: tests.isolation,
      race: tests.run?.options.race ?? true,
      coverage: tests.run?.options.coverage ?? true,
    },
  };
}

/**
 * The tests of a change set: the last run, read without running anything as soon as the change set is there, and
 * the runs started from here, whole or of a selection. The state belongs to the change set object it was read for, so
 * a reloaded change set reads it again. `onRunCompleted` is called when a run reports, so what depends on its
 * coverage can refresh.
 */
export function useTests(projectId: string, changeSet: ChangeSet | null, onRunCompleted: () => void) {
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [attempt, setAttempt] = useState(0);
  const completed = useRef(onRunCompleted);
  useEffect(() => {
    completed.current = onRunCompleted;
  });

  useEffect(() => {
    if (changeSet === null) return;
    let cancelled = false;
    setLoaded({ changeSet, panel: { status: "loading" } });
    getTestsState(projectId, changeSet.oldSha, changeSet.newSha).then(
      (tests) => {
        if (!cancelled) setLoaded({ changeSet, panel: readyPanel(tests) });
      },
      (error: unknown) => {
        if (!cancelled) setLoaded({ changeSet, panel: { status: "error", message: errorMessage(error) } });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [projectId, changeSet, attempt]);

  /** Changes the panel of `owner` if it is still the one shown and ready. */
  const update = useCallback(
    (owner: ChangeSet, change: (panel: ReadyPanel) => TestsPanel) =>
      setLoaded((current) =>
        current?.changeSet === owner && current.panel.status === "ready"
          ? { changeSet: owner, panel: change(current.panel) }
          : current,
      ),
    [],
  );

  const finish = useCallback(
    (owner: ChangeSet, run: TestRun) => {
      update(owner, (panel) => ({
        ...panel,
        tests: { ...panel.tests, run, running: false, runningSelection: null, isolation: run.isolation },
        running: null,
        progress: null,
        runError: null,
      }));
      if (run.status === "completed") completed.current();
    },
    [update],
  );

  const panel: TestsPanel =
    loaded !== null && loaded.changeSet === changeSet ? loaded.panel : { status: "loading" };
  const watching = panel.status === "ready" && panel.running !== null && !panel.running.ours;
  const wholeRunning = panel.status === "ready" && panel.running !== null && panel.running.selection === null;

  // A run this window did not start is checked on until it ends; then its result is read like on open.
  useEffect(() => {
    if (!watching || changeSet === null) return;
    let stopped = false;
    const timer = setInterval(() => {
      getTestsState(projectId, changeSet.oldSha, changeSet.newSha).then(
        (tests) => {
          if (stopped || tests.running) return;
          if (tests.run) {
            finish(changeSet, tests.run);
          } else {
            update(changeSet, (current) => ({ ...current, tests, running: null, progress: null }));
          }
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
  }, [watching, projectId, changeSet, finish, update]);

  // The progress of a run of the whole suite, while it goes on.
  useEffect(() => {
    if (!wholeRunning || changeSet === null) return;
    let stopped = false;
    const timer = setInterval(() => {
      getTestsProgress(projectId, changeSet.newSha).then(
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
  }, [wholeRunning, projectId, changeSet, update]);

  /** Runs every test, or only `selection`, as the panel's choice says. */
  const run = useCallback(
    (selection: RunSelection | null = null) => {
      if (changeSet === null || loaded?.changeSet !== changeSet || loaded.panel.status !== "ready") return;
      const owner = changeSet;
      const { isolation, race, coverage } = loaded.panel.choice;
      update(owner, (current) => ({
        ...current,
        tests: { ...current.tests, isolation },
        running: { since: Date.now(), isolation, selection, ours: true, cancelling: false },
        progress: null,
        runError: null,
      }));
      runTests(projectId, owner.oldSha, owner.newSha, isolation, { race, coverage }, selection).then(
        (result) => finish(owner, result),
        (error: unknown) =>
          update(owner, (current) => ({ ...current, running: null, progress: null, runError: errorMessage(error) })),
      );
    },
    [projectId, changeSet, loaded, update, finish],
  );

  const cancel = useCallback(() => {
    if (changeSet === null) return;
    const owner = changeSet;
    update(owner, (current) =>
      current.running ? { ...current, running: { ...current.running, cancelling: true }, runError: null } : current,
    );
    cancelTests(projectId, owner.oldSha, owner.newSha).catch((error: unknown) =>
      update(owner, (current) => ({
        ...current,
        running: current.running && { ...current.running, cancelling: false },
        runError: errorMessage(error),
      })),
    );
  }, [projectId, changeSet, update]);

  /** Changes how the next run goes. */
  const choose = useCallback(
    (change: Partial<RunChoice>) => {
      if (changeSet === null) return;
      update(changeSet, (current) => ({ ...current, choice: { ...current.choice, ...change } }));
    },
    [changeSet, update],
  );

  const retry = useCallback(() => setAttempt((n) => n + 1), []);

  return { panel, run, cancel, choose, retry };
}

/** Failures of the last completed run, for the tab's count (a test with failing subtests is one); null without one. */
export function failingCount(panel: TestsPanel): number | null {
  if (panel.status !== "ready" || panel.tests.run === null || panel.tests.run.status !== "completed") return null;
  return failureCount(panel.tests.run);
}
