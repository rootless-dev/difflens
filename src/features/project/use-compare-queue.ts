import { useEffect, useState, useSyncExternalStore } from "react";

import { CompareQueue, type CompareTarget, compareKey } from "@/features/project/compare-queue";
import { compareBranches } from "@/lib/api";

/** Comparisons running at the same time; git is the bottleneck, not the UI. */
const CONCURRENCY = 4;

/**
 * Compares `targets` (the rows on screen) with limited concurrency and re-renders as results
 * arrive. Results are kept for the life of the page, so the queue is created once per project.
 */
export function useCompareQueue(projectId: string, targets: CompareTarget[]): CompareQueue {
  const [queue] = useState(
    () =>
      new CompareQueue(
        ({ branch, base }) => compareBranches(projectId, branch, base),
        CONCURRENCY,
      ),
  );
  useSyncExternalStore(queue.subscribe, queue.getVersion);

  const wantedKey = targets.map(compareKey).join("\n");
  useEffect(() => {
    queue.want(targets);
    // `targets` changes identity on every render; its content is what wantedKey tracks.
  }, [queue, wantedKey]);

  return queue;
}
