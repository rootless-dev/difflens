import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { PullRequestStatus } from "@/features/review/pull-request/pull-request-status";
import { reviewRefOf } from "@/features/review/review-state/review-status";
import {
  cancelModelRun,
  type ChangeSet,
  getOverview,
  type Language,
  type Overview,
  type OverviewPlan,
  planOverview,
  planOverviewTranslation,
  type PullRequestText,
  summarizeOverview,
  translateOverview,
  type TranslationPlan,
} from "@/lib/api";
import { errorMessage } from "@/lib/format";

export type OverviewPlanState =
  /** Not asked for: the Overview tab has not been opened, or the pull request is still being looked for. */
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ready"; plan: OverviewPlan }
  | { status: "error"; message: string };

export interface OverviewHandle {
  /** null until first read. */
  overview: Overview | null;
  plan: OverviewPlanState;
  /** Summarize was clicked and the model has not answered yet. */
  running: { stopping: boolean } | null;
  /** Why the last read or Summarize failed; cleared by the next that works. */
  error: string | null;
  /** Asks the model: the only way the Overview does. */
  summarize: () => void;
  cancel: () => void;
  retry: () => void;
  /** What translating the summary into `language` would send. Asks no model and not Jira. */
  planTranslation: (language: Language) => Promise<TranslationPlan>;
  /** Asks the model to translate the summary, after the click on Translate; keeps what it answers. */
  translate: (language: Language, runId: string) => Promise<void>;
}

/**
 * The pull request as the Overview sends it: undefined while it is being looked for, null when there is none.
 */
export function pullRequestText(status: PullRequestStatus): PullRequestText | null | undefined {
  if (status.kind === "loading") return undefined;
  if (status.kind !== "linked") return null;
  return { title: status.pullRequest.title, description: status.pullRequest.description };
}

/**
 * The Overview of a change set: the stored summary and what is derived from the change are read once `requested`
 * (the tab was opened), and what Summarize would send once the pull request is known too. Nothing here asks a model
 * except `summarize`.
 */
export function useOverview(
  projectId: string,
  changeSet: ChangeSet | null,
  requested: boolean,
  pullRequest: PullRequestText | null | undefined,
): OverviewHandle {
  const [read, setRead] = useState<{ changeSet: ChangeSet; overview: Overview } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [planned, setPlanned] = useState<{ key: unknown; state: OverviewPlanState } | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [running, setRunning] = useState<{ stopping: boolean } | null>(null);
  const runId = useRef<string | null>(null);
  const runs = useRef(0);
  const alive = useRef(true);
  const title = pullRequest === undefined ? undefined : (pullRequest?.title ?? null);
  const description = pullRequest === undefined ? undefined : (pullRequest?.description ?? null);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    if (!requested || changeSet === null) return;
    let cancelled = false;
    getOverview(projectId, reviewRefOf(changeSet)).then(
      (overview) => {
        if (cancelled) return;
        setRead({ changeSet, overview });
        setError(null);
      },
      (cause: unknown) => {
        if (!cancelled) setError(errorMessage(cause));
      },
    );
    return () => {
      cancelled = true;
    };
  }, [projectId, changeSet, requested, attempt]);

  // What a plan was worked out for: a new pull request object with the same text does not ask again.
  const key = useMemo(() => ({ changeSet, title, description, attempt }), [changeSet, title, description, attempt]);

  useEffect(() => {
    if (!requested || key.changeSet === null || key.title === undefined) return;
    let cancelled = false;
    const text = key.title === null ? null : { title: key.title, description: key.description ?? "" };
    setPlanned({ key, state: { status: "loading" } });
    planOverview(projectId, reviewRefOf(key.changeSet), text).then(
      (plan) => {
        if (!cancelled) setPlanned({ key, state: { status: "ready", plan } });
      },
      (cause: unknown) => {
        if (!cancelled) setPlanned({ key, state: { status: "error", message: errorMessage(cause) } });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [projectId, requested, key]);

  const summarize = useCallback(() => {
    if (changeSet === null || pullRequest === undefined || running !== null) return;
    const id = `overview-${++runs.current}`;
    runId.current = id;
    setRunning({ stopping: false });
    setError(null);
    summarizeOverview(projectId, reviewRefOf(changeSet), pullRequest, id).then(
      (overview) => {
        if (!alive.current) return;
        setRead({ changeSet, overview });
      },
      (cause: unknown) => {
        if (alive.current) setError(errorMessage(cause));
      },
    ).finally(() => {
      runId.current = null;
      if (alive.current) setRunning(null);
    });
  }, [projectId, changeSet, pullRequest, running]);

  const cancel = useCallback(() => {
    const id = runId.current;
    if (id === null) return;
    setRunning((current) => current && { stopping: true });
    cancelModelRun(id).catch(() => {
      // Already over.
    });
  }, []);

  const retry = useCallback(() => setAttempt((n) => n + 1), []);

  const planTranslation = useCallback(
    (language: Language) => {
      if (changeSet === null) return Promise.reject(new Error("No change set."));
      return planOverviewTranslation(projectId, reviewRefOf(changeSet), language);
    },
    [projectId, changeSet],
  );

  const translate = useCallback(
    async (language: Language, id: string) => {
      if (changeSet === null) return;
      const overview = await translateOverview(projectId, reviewRefOf(changeSet), language, id);
      if (alive.current) setRead({ changeSet, overview });
    },
    [projectId, changeSet],
  );

  const overview = read !== null && read.changeSet === changeSet ? read.overview : null;
  const plan: OverviewPlanState =
    !requested || title === undefined
      ? { status: "idle" }
      : planned !== null && planned.key === key
        ? planned.state
        : { status: "loading" };
  return { overview, plan, running, error, summarize, cancel, retry, planTranslation, translate };
}
