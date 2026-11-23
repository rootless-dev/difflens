import { createContext, type ReactNode, useCallback, useContext, useEffect, useMemo, useState } from "react";

import { targetKey } from "@/features/review/review-target";
import {
  cancelFullAnalysis,
  dismissFullAnalysis,
  type FullAnalysisEvent,
  type FullAnalysisJob,
  getFullAnalyses,
  listenToFullAnalyses,
  type PullRequestText,
  type ReviewRef,
  type ReviewTarget,
  startFullAnalysis,
} from "@/lib/api";

/** The full analyses of this run of the app, as the core reports them, and what can be done with them. */
export interface FullAnalysesHandle {
  /** In the order they were started. */
  jobs: FullAnalysisJob[];
  /** The latest full analysis of a review, kept until it is dismissed. */
  jobFor: (projectId: string, target: ReviewTarget) => FullAnalysisJob | null;
  /** Starts (or queues) a full analysis, after its confirmation was clicked: the only way one starts. */
  start: (projectId: string, review: ReviewRef, pullRequest: PullRequestText | null) => Promise<FullAnalysisJob>;
  cancel: (jobId: string) => void;
  /** Forgets a finished one, giving its review back. */
  dismiss: (jobId: string) => void;
}

const NONE: FullAnalysesHandle = {
  jobs: [],
  jobFor: () => null,
  start: () => Promise.reject(new Error("Full analyses are not available here.")),
  cancel: () => {},
  dismiss: () => {},
};

const FullAnalysesContext = createContext<FullAnalysesHandle>(NONE);

/** The full analyses, or none outside `FullAnalysesProvider`. */
export function useFullAnalyses(): FullAnalysesHandle {
  return useContext(FullAnalysesContext);
}

/** `event` applied to the jobs known: its state replaces the job's, its line goes at the end of its log. */
export function applyEvent(
  jobs: ReadonlyMap<string, FullAnalysisJob>,
  event: FullAnalysisEvent,
): Map<string, FullAnalysisJob> {
  const next = new Map(jobs);
  const log = next.get(event.job.id)?.log ?? [];
  next.set(event.job.id, { ...event.job, log: event.line ? [...log, event.line] : log });
  return next;
}

/** What the core listed, over what events already brought: of the two, the job with the longer log. */
export function mergeListed(
  jobs: ReadonlyMap<string, FullAnalysisJob>,
  listed: readonly FullAnalysisJob[],
): Map<string, FullAnalysisJob> {
  const next = new Map<string, FullAnalysisJob>();
  for (const job of listed) {
    const known = jobs.get(job.id);
    next.set(job.id, known && known.log.length > job.log.length ? known : job);
  }
  for (const [id, job] of jobs) {
    if (!next.has(id)) next.set(id, job);
  }
  return next;
}

export function sameReview(job: FullAnalysisJob, projectId: string, target: ReviewTarget): boolean {
  return job.projectId === projectId && targetKey(job.review.target) === targetKey(target);
}

/**
 * Follows every full analysis while the app runs: the ones the core already has, read once, then every change it
 * emits. Nothing here starts one; `start` is called by the confirmation's button.
 */
export function FullAnalysesProvider({ children }: { children: ReactNode }) {
  const [jobs, setJobs] = useState<ReadonlyMap<string, FullAnalysisJob>>(new Map());

  useEffect(() => {
    let stopped = false;
    let unlisten: (() => void) | null = null;
    listenToFullAnalyses((event) => {
      if (!stopped) setJobs((current) => applyEvent(current, event));
    }).then(
      (stop) => {
        if (stopped) stop();
        else unlisten = stop;
      },
      () => {
        // Outside the app (tests), there is nothing to listen to.
      },
    );
    getFullAnalyses().then(
      (listed) => {
        if (!stopped) setJobs((current) => mergeListed(current, listed));
      },
      () => {
        // The core keeps them; the next event brings them back.
      },
    );
    return () => {
      stopped = true;
      unlisten?.();
    };
  }, []);

  const start = useCallback(async (projectId: string, review: ReviewRef, pullRequest: PullRequestText | null) => {
    const job = await startFullAnalysis(projectId, review, pullRequest);
    setJobs((current) => {
      // A finished one of the same review is replaced by the new one.
      const next = new Map(
        [...current].filter(([id, other]) => id === job.id || !sameReview(other, projectId, review.target)),
      );
      const known = next.get(job.id);
      next.set(job.id, known && known.log.length > job.log.length ? known : job);
      return next;
    });
    return job;
  }, []);

  const cancel = useCallback((jobId: string) => {
    cancelFullAnalysis(jobId).catch(() => {
      // Already over.
    });
  }, []);

  const dismiss = useCallback((jobId: string) => {
    setJobs((current) => {
      const next = new Map(current);
      next.delete(jobId);
      return next;
    });
    dismissFullAnalysis(jobId).catch(() => {
      // Already gone.
    });
  }, []);

  const handle = useMemo<FullAnalysesHandle>(() => {
    const list = [...jobs.values()].sort((a, b) => a.queuedAt.localeCompare(b.queuedAt));
    return {
      jobs: list,
      jobFor: (projectId, target) =>
        list.filter((job) => sameReview(job, projectId, target)).at(-1) ?? null,
      start,
      cancel,
      dismiss,
    };
  }, [jobs, start, cancel, dismiss]);

  return <FullAnalysesContext value={handle}>{children}</FullAnalysesContext>;
}

/** Running or waiting: what the indicators count. */
export function isActive(job: FullAnalysisJob): boolean {
  return job.status !== "finished";
}
