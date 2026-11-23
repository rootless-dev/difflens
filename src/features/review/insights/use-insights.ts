import { useCallback, useEffect, useRef, useState } from "react";

import { reviewRefOf } from "@/features/review/review-state/review-status";
import {
  cancelModelRun,
  type ChangeSet,
  type FindingVerdict,
  getInsights,
  type Insights,
  type InsightsAnalysis,
  type InsightsPlan,
  isApiError,
  type Language,
  planInsights,
  planInsightsTranslation,
  type ReviewRef,
  runInsightsBatch,
  setFindingFixed,
  setFindingVerdict,
  startInsights,
  translateInsights,
  type TranslationPlan,
} from "@/lib/api";
import { errorMessage } from "@/lib/format";

export type PlanState =
  /** Not asked for: the Insights tab has not been opened since the screen opened or since Reload. */
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ready"; plan: InsightsPlan }
  | { status: "error"; message: string };

/** Calls of an analysis being asked now. */
export interface Running {
  analysisId: string;
  total: number;
  /** 0-based indexes of the calls in flight. */
  active: number[];
  /** Cancel was clicked: the calls in flight are being stopped and no other starts. */
  stopping: boolean;
}

export interface InsightsHandle {
  /** null until first read. */
  insights: Insights | null;
  plan: PlanState;
  running: Running | null;
  /** Why the last read, start or save failed; cleared by the next that works. */
  error: string | null;
  /** A verdict, or a finding marked fixed or reopened, is being saved. */
  saving: boolean;
  /** Starts an analysis and asks each of its calls: the only way, with retry, that a model is asked. */
  analyze: () => void;
  cancel: () => void;
  /** Asks one call of the latest analysis again (one that failed or did not run). */
  retry: (index: number) => void;
  retryPlan: () => void;
  /** Reads the findings and the plan again, as after a full analysis ran the calls in the core. Asks no model. */
  refresh: () => void;
  setVerdict: (findingId: string, verdict: FindingVerdict) => void;
  /** Marks a finding fixed by the user, or opens it again (`fixed` false). Asks no model. */
  setFixed: (findingId: string, fixed: boolean) => void;
  /** What translating the findings into `language` would send. Asks no model. */
  planTranslation: (language: Language) => Promise<TranslationPlan>;
  /** Asks the model one call of a translation, after the click on Translate; keeps what it answers. */
  translateCall: (language: Language, findingIds: string[], runId: string) => Promise<void>;
}

/**
 * Insights of a change set: its findings and latest analysis are read (never analyzed) once it is there; what Analyze
 * would send is worked out once `requested` (the tab was opened). The model is only asked by analyze and retry, which
 * run the calls of an analysis one at a time for Claude Code and up to three at once for the APIs.
 */
export function useInsights(projectId: string, changeSet: ChangeSet | null, requested: boolean): InsightsHandle {
  const [insights, setInsights] = useState<Insights | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [planned, setPlanned] = useState<{ changeSet: ChangeSet; state: PlanState } | null>(null);
  const [planAttempt, setPlanAttempt] = useState(0);
  const [readAttempt, setReadAttempt] = useState(0);
  const [running, setRunning] = useState<Running | null>(null);
  const [saving, setSaving] = useState(false);
  const inflight = useRef(new Set<string>());
  const stopping = useRef(false);
  const runs = useRef(0);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      // Leaving the screen asks no further call; one in flight still answers and is kept.
      alive.current = false;
      stopping.current = true;
    };
  }, []);

  /** Keeps the newest picture of the review: answers to calls side by side can arrive out of order. */
  const apply = useCallback((next: Insights) => {
    if (!alive.current) return;
    setInsights((current) => (isOlder(next, current) ? current : next));
  }, []);

  useEffect(() => {
    if (changeSet === null) return;
    let cancelled = false;
    getInsights(projectId, reviewRefOf(changeSet)).then(
      (data) => {
        if (cancelled) return;
        setInsights(data);
        setError(null);
      },
      (cause: unknown) => {
        if (!cancelled) setError(errorMessage(cause));
      },
    );
    return () => {
      cancelled = true;
    };
  }, [projectId, changeSet, readAttempt]);

  useEffect(() => {
    if (!requested || changeSet === null) return;
    let cancelled = false;
    setPlanned({ changeSet, state: { status: "loading" } });
    planInsights(projectId, reviewRefOf(changeSet)).then(
      (plan) => {
        if (!cancelled) setPlanned({ changeSet, state: { status: "ready", plan } });
      },
      (cause: unknown) => {
        if (!cancelled) setPlanned({ changeSet, state: { status: "error", message: errorMessage(cause) } });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [projectId, changeSet, requested, planAttempt]);

  const runCalls = useCallback(
    async (review: ReviewRef, analysis: InsightsAnalysis, indexes: number[]) => {
      stopping.current = false;
      const queue = [...indexes];
      setRunning({ analysisId: analysis.id, total: analysis.batches.length, active: [], stopping: false });
      const setActive = (change: (active: number[]) => number[]) =>
        setRunning((current) => current && { ...current, active: change(current.active) });
      const worker = async () => {
        while (queue.length > 0 && !stopping.current) {
          const index = queue.shift()!;
          const runId = `insights-${analysis.id}-${index}-${++runs.current}`;
          inflight.current.add(runId);
          setActive((active) => [...active, index]);
          try {
            apply(await runInsightsBatch(projectId, review, analysis.id, index, runId));
          } catch (cause: unknown) {
            if (alive.current) setError(errorMessage(cause));
            // A newer analysis replaced this one: its other calls are gone too.
            if (isApiError(cause) && cause.code === "unknown_finding") stopping.current = true;
          } finally {
            inflight.current.delete(runId);
            setActive((active) => active.filter((other) => other !== index));
          }
        }
      };
      const workers = Math.max(1, Math.min(analysis.parallelCalls, queue.length));
      await Promise.all(Array.from({ length: workers }, worker));
      if (!alive.current) return;
      setRunning(null);
      // The answers settle into one picture.
      try {
        apply(await getInsights(projectId, review));
      } catch {
        // The last answer stays on screen.
      }
    },
    [projectId, apply],
  );

  const analyze = useCallback(() => {
    if (changeSet === null || running !== null) return;
    const review = reviewRefOf(changeSet);
    setError(null);
    void (async () => {
      let started: Insights;
      try {
        started = await startInsights(projectId, review);
      } catch (cause: unknown) {
        if (alive.current) setError(errorMessage(cause));
        return;
      }
      apply(started);
      const analysis = started.analysis;
      if (analysis) await runCalls(review, analysis, analysis.batches.map((batch) => batch.index));
    })();
  }, [projectId, changeSet, running, apply, runCalls]);

  const retry = useCallback(
    (index: number) => {
      const analysis = insights?.analysis;
      if (changeSet === null || running !== null || !analysis) return;
      setError(null);
      void runCalls(reviewRefOf(changeSet), analysis, [index]);
    },
    [changeSet, running, insights, runCalls],
  );

  const cancel = useCallback(() => {
    stopping.current = true;
    setRunning((current) => current && { ...current, stopping: true });
    for (const runId of inflight.current) {
      cancelModelRun(runId).catch(() => {
        // Already over.
      });
    }
  }, []);

  /** Saves a change the user made to a finding, and shows the findings it answers with. */
  const save = useCallback(
    (change: (review: ReviewRef) => Promise<Insights>) => {
      if (changeSet === null) return;
      setSaving(true);
      change(reviewRefOf(changeSet)).then(
        (next) => {
          apply(next);
          if (alive.current) setError(null);
        },
        (cause: unknown) => {
          if (alive.current) setError(errorMessage(cause));
        },
      ).finally(() => {
        if (alive.current) setSaving(false);
      });
    },
    [changeSet, apply],
  );

  const setVerdict = useCallback(
    (findingId: string, verdict: FindingVerdict) =>
      save((review) => setFindingVerdict(projectId, review, findingId, verdict)),
    [projectId, save],
  );

  const setFixed = useCallback(
    (findingId: string, fixed: boolean) => save((review) => setFindingFixed(projectId, review, findingId, fixed)),
    [projectId, save],
  );

  const retryPlan = useCallback(() => setPlanAttempt((n) => n + 1), []);

  const refresh = useCallback(() => {
    setReadAttempt((n) => n + 1);
    setPlanAttempt((n) => n + 1);
  }, []);

  const planTranslation = useCallback(
    (language: Language) => {
      if (changeSet === null) return Promise.reject(new Error("No change set."));
      return planInsightsTranslation(projectId, reviewRefOf(changeSet), language);
    },
    [projectId, changeSet],
  );

  const translateCall = useCallback(
    async (language: Language, findingIds: string[], runId: string) => {
      if (changeSet === null) return;
      apply(await translateInsights(projectId, reviewRefOf(changeSet), language, findingIds, runId));
    },
    [projectId, changeSet, apply],
  );

  const plan: PlanState = !requested
    ? { status: "idle" }
    : planned !== null && planned.changeSet === changeSet
      ? planned.state
      : { status: "loading" };
  return {
    insights,
    plan,
    running,
    error,
    saving,
    analyze,
    cancel,
    retry,
    retryPlan,
    refresh,
    setVerdict,
    setFixed,
    planTranslation,
    translateCall,
  };
}

/** Calls of the analysis that have answered or failed. */
function settled(insights: Insights): number {
  return insights.analysis?.batches.filter((batch) => batch.status !== "pending").length ?? 0;
}

/** `next` is an older picture of the same analysis than `current`: fewer of its calls had ended. */
function isOlder(next: Insights, current: Insights | null): boolean {
  return (
    current !== null &&
    next.analysis !== null &&
    current.analysis?.id === next.analysis.id &&
    settled(next) < settled(current)
  );
}

/** Open findings for the tab's count and the progress line, the fixed ones left out; null until read. */
export function findingCounts(insights: Insights | null): { total: number; classified: number } | null {
  if (insights === null) return null;
  const open = insights.findings.filter((finding) => finding.resolution === null);
  return {
    total: open.length,
    classified: open.filter((finding) => finding.verdict !== "pending").length,
  };
}
