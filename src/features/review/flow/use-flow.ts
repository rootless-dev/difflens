import { useCallback, useEffect, useState } from "react";

import { type ChangeSet, type FlowOptions, type FlowResult, getFlow } from "@/lib/api";
import { errorMessage } from "@/lib/format";

export const DEFAULT_FLOW_OPTIONS: FlowOptions = { includeTests: false, includeCallers: false, includeCallees: false };

export type FlowState =
  /** Not asked for yet: the Flow tab has not been opened since the screen opened or since Reload. */
  | { status: "idle" }
  /** startedAt is null while the change set itself is still loading. */
  | { status: "loading"; startedAt: number | null }
  /** pending: a graph with other options is on its way; this one stays on screen until it arrives. */
  | { status: "ready"; result: FlowResult; pending: boolean }
  | { status: "error"; message: string };

interface Loaded {
  changeSet: ChangeSet;
  state: FlowState;
}

/**
 * The flow of a change set built with `options`, asked for only once `requested` is true. The result belongs to
 * the change set object it was computed for, so a reloaded change set (a new object) never shows a stale flow.
 * Changing only the options keeps the current graph, marked pending, until the new one arrives.
 */
export function useFlow(projectId: string, changeSet: ChangeSet | null, requested: boolean, options: FlowOptions) {
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [attempt, setAttempt] = useState(0);
  const { includeTests, includeCallers, includeCallees } = options;

  useEffect(() => {
    if (!requested || changeSet === null) return;
    let cancelled = false;
    setLoaded((previous) =>
      previous?.changeSet === changeSet && previous.state.status === "ready" && previous.state.result.kind === "graph"
        ? { changeSet, state: { ...previous.state, pending: true } }
        : { changeSet, state: { status: "loading", startedAt: Date.now() } },
    );
    getFlow(projectId, changeSet.oldSha, changeSet.newSha, { includeTests, includeCallers, includeCallees }).then(
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
  }, [projectId, changeSet, requested, attempt, includeTests, includeCallers, includeCallees]);

  const retry = useCallback(() => setAttempt((n) => n + 1), []);

  const state: FlowState = !requested
    ? { status: "idle" }
    : loaded !== null && loaded.changeSet === changeSet
      ? loaded.state
      : { status: "loading", startedAt: null };
  return { state, retry };
}
