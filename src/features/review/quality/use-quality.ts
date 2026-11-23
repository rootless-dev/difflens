import { useCallback, useEffect, useState } from "react";

import { type ChangeSet, getQuality, type QualityResult } from "@/lib/api";
import { errorMessage } from "@/lib/format";

export type QualityState =
  /** Not asked for yet: the Quality tab has not been opened since the screen opened or since Reload. */
  | { status: "idle" }
  /** startedAt is null while the change set itself is still loading. */
  | { status: "loading"; startedAt: number | null }
  /** pending: a newer report (after a test run) is on its way; this one stays on screen until it arrives. */
  | { status: "ready"; result: QualityResult; pending: boolean }
  | { status: "error"; message: string };

interface Loaded {
  changeSet: ChangeSet;
  state: QualityState;
}

/**
 * The quality report of a change set, asked for only once `requested` is true, and asked for again whenever
 * `testRuns` changes (a test run completed, so coverage and CRAP moved). Like useFlow, the result belongs to the
 * change set object it was computed for, and a refresh keeps the current report, marked pending.
 */
export function useQuality(projectId: string, changeSet: ChangeSet | null, requested: boolean, testRuns: number) {
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (!requested || changeSet === null) return;
    let cancelled = false;
    setLoaded((previous) =>
      previous?.changeSet === changeSet && previous.state.status === "ready" && previous.state.result.kind === "report"
        ? { changeSet, state: { ...previous.state, pending: true } }
        : { changeSet, state: { status: "loading", startedAt: Date.now() } },
    );
    getQuality(projectId, changeSet.oldSha, changeSet.newSha).then(
      (result) => {
        if (!cancelled) setLoaded({ changeSet, state: { status: "ready", result, pending: false } });
      },
      (error: unknown) => {
        if (!cancelled) setLoaded({ changeSet, state: { status: "error", message: errorMessage(error) } });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [projectId, changeSet, requested, attempt, testRuns]);

  const retry = useCallback(() => setAttempt((n) => n + 1), []);

  const state: QualityState = !requested
    ? { status: "idle" }
    : loaded !== null && loaded.changeSet === changeSet
      ? loaded.state
      : { status: "loading", startedAt: null };
  return { state, retry };
}

/** Limits crossed, for the tab's count; null until a report is there. */
export function crossedCount(state: QualityState): number | null {
  return state.status === "ready" && state.result.kind === "report" ? state.result.report.crossed.length : null;
}
