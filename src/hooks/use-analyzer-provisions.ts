import { useEffect, useState } from "react";

import { type AnalyzerProvision, getAnalyzerProvisions, listenToAnalyzerProvisions } from "@/lib/api";

/**
 * The analyzers being prepared now (their environment built the first time a project of their language needs it),
 * by analyzer id, in the order they started. Read once from the core, then kept up to date from its events: a build
 * that ends (ready or failed) leaves the list, since the request waiting for it then answers by itself.
 */
export function useAnalyzerProvisions(): AnalyzerProvision[] {
  const [preparing, setPreparing] = useState<ReadonlyMap<string, AnalyzerProvision>>(new Map());

  useEffect(() => {
    let stopped = false;
    let unlisten: (() => void) | null = null;
    listenToAnalyzerProvisions((event) => {
      if (!stopped) setPreparing((current) => applyProvision(current, event));
    }).then(
      (stop) => {
        if (stopped) stop();
        else unlisten = stop;
      },
      () => {
        // Outside the app (tests), there is nothing to listen to.
      },
    );
    getAnalyzerProvisions().then(
      (running) => {
        if (!stopped) setPreparing((current) => running.reduce(applyProvision, current));
      },
      () => {
        // Nothing known yet; the next event says it.
      },
    );
    return () => {
      stopped = true;
      unlisten?.();
    };
  }, []);

  return [...preparing.values()];
}

/** `current` with `event` applied: a build still preparing is kept with its latest step, a finished one dropped. */
export function applyProvision(
  current: ReadonlyMap<string, AnalyzerProvision>,
  event: AnalyzerProvision,
): ReadonlyMap<string, AnalyzerProvision> {
  const next = new Map(current);
  if (event.status === "preparing") next.set(event.analyzer, event);
  else next.delete(event.analyzer);
  return next;
}
