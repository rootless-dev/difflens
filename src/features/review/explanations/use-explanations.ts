import { createContext, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { type ShownExplanation, shownExplanations } from "@/features/review/ai-text/versions";
import { reviewRefOf } from "@/features/review/review-state/review-status";
import { useAppLanguage } from "@/i18n/language";
import {
  cancelExplanation,
  type ChangeSet,
  type ExplanationLevel,
  type Explanations,
  explainFunctions,
  type FlowNode,
  type FunctionSpan,
  getExplanations,
  getModelInUse,
  isApiError,
  type Language,
  type ModelInUse,
  type ModelTask,
  planExplanationsTranslation,
  type PullRequestText,
  translateExplanations,
  type TranslationPlan,
} from "@/lib/api";
import { errorMessage } from "@/lib/format";

/**
 * One click on an Explain button, while the model has not answered: a function's (`node:<id>`) or a group's, which
 * explain in short, or a function's Explain in detail (`detail:<id>`).
 */
export interface ExplanationRun {
  key: string;
  ids: readonly string[];
  level: ExplanationLevel;
  stopping: boolean;
}

export interface ExplanationsHandle {
  /** What the model wrote about each function (both levels), by id, in the version that reads in the language shown if any. */
  shown: ReadonlyMap<string, ShownExplanation>;
  /** Who an Explain button would ask at each level (each its own task's model), for its tooltip; null until read. */
  models: Readonly<Record<ExplanationLevel, ModelInUse | null>>;
  /** The run explaining function `id` in short now, its own or its group's. */
  runOf: (id: string) => ExplanationRun | null;
  runByKey: (key: string) => ExplanationRun | null;
  /** Why the last click on button `key` failed; cleared by its next click. */
  errorOf: (key: string) => string | null;
  /** Asks the model to explain `ids` at `level` (short unless said): the only way an explanation is written from the Flow tab. */
  explain: (key: string, ids: readonly string[], level?: ExplanationLevel) => void;
  cancel: (key: string) => void;
  /** What translating the explanations into `language` would send. Asks no model. */
  planTranslation: (language: Language) => Promise<TranslationPlan>;
  /** Asks the model one call of a translation, after the click on Translate; keeps what it answers. */
  translateCall: (language: Language, functionIds: string[], runId: string) => Promise<void>;
}

const NO_SHOWN: ReadonlyMap<string, ShownExplanation> = new Map();
const NO_MODELS: Readonly<Record<ExplanationLevel, ModelInUse | null>> = { brief: null, detail: null };

/** The task whose model writes explanations at each level. */
const TASK_OF: Record<ExplanationLevel, ModelTask> = {
  brief: "brief_explanation",
  detail: "detailed_explanation",
};

/** No explanation and no button that works: outside a review (such as a test of the canvas alone). */
export const NO_EXPLANATIONS: ExplanationsHandle = {
  shown: NO_SHOWN,
  models: NO_MODELS,
  runOf: () => null,
  runByKey: () => null,
  errorOf: () => null,
  explain: () => {},
  cancel: () => {},
  planTranslation: () => Promise.reject(new Error("No review.")),
  translateCall: () => Promise.resolve(),
};

export const ExplanationsContext = createContext<ExplanationsHandle>(NO_EXPLANATIONS);

/** The key of a function's own Explain button. */
export const nodeRunKey = (id: string) => `node:${id}`;

/** The key of a group's Explain button. */
export const groupRunKey = (id: string) => `group:${id}`;

/** The key of a function's Explain in detail button. */
export const detailRunKey = (id: string) => `detail:${id}`;

/** A function the model can be asked about: one with a place in the code (not an external package). */
export function canExplain(node: FlowNode): boolean {
  return node.state !== "external" && node.file !== null && node.startLine !== null && node.endLine !== null;
}

/** Every function of the graph with a place in the code, so an explanation can tell its code changed since. */
export function explanationSpans(nodes: readonly FlowNode[]): FunctionSpan[] {
  return nodes.flatMap((node) =>
    canExplain(node) && node.file !== null && node.startLine !== null && node.endLine !== null
      ? [{ id: node.id, file: node.file, startLine: node.startLine, endLine: node.endLine, deleted: node.state === "deleted" }]
      : [],
  );
}

let runs = 0;

/**
 * The explanations of a review's functions: read (never written) once `requested` (the Flow tab was opened) and the
 * graph is there, read against the functions as the graph has them now; written only by `explain`, after a click on
 * a node's or a group's Explain button (short) or a function's Explain in detail. `refresh` reads them again, as after
 * a full analysis wrote some.
 */
export function useExplanations(
  projectId: string,
  changeSet: ChangeSet | null,
  spans: readonly FunctionSpan[] | null,
  requested: boolean,
  pullRequest: PullRequestText | null | undefined,
): ExplanationsHandle & { refresh: () => void } {
  const language = useAppLanguage();
  const [read, setRead] = useState<{ changeSet: ChangeSet; explanations: Explanations } | null>(null);
  const [models, setModels] = useState(NO_MODELS);
  const [running, setRunning] = useState<ReadonlyMap<string, ExplanationRun & { runId: string }>>(new Map());
  const [errors, setErrors] = useState<ReadonlyMap<string, string>>(new Map());
  const [attempt, setAttempt] = useState(0);
  const alive = useRef(true);
  // The latest spans, for an answer that arrives after the graph changed.
  const latestSpans = useRef(spans);
  useEffect(() => {
    latestSpans.current = spans;
  });

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    if (!requested || changeSet === null || spans === null) return;
    let cancelled = false;
    getExplanations(projectId, reviewRefOf(changeSet), [...spans]).then(
      (explanations) => {
        if (!cancelled) setRead({ changeSet, explanations });
      },
      () => {
        // Explanations are an addition to the graph: one that cannot be read leaves the boxes as they are.
      },
    );
    return () => {
      cancelled = true;
    };
  }, [projectId, changeSet, spans, requested, attempt]);

  useEffect(() => {
    if (!requested) return;
    let cancelled = false;
    for (const level of ["brief", "detail"] as const) {
      getModelInUse(TASK_OF[level]).then(
        (inUse) => {
          if (!cancelled) setModels((current) => ({ ...current, [level]: inUse }));
        },
        () => {
          // The tooltip names "the model chosen in Preferences" instead.
        },
      );
    }
    return () => {
      cancelled = true;
    };
  }, [requested, attempt]);

  const explanations = read !== null && read.changeSet === changeSet ? read.explanations : null;
  const shown = useMemo(
    () => (explanations === null ? NO_SHOWN : shownExplanations(explanations, language)),
    [explanations, language],
  );

  const keep = useCallback((owner: ChangeSet, next: Explanations) => {
    if (alive.current) setRead({ changeSet: owner, explanations: next });
  }, []);

  const explain = useCallback(
    (key: string, ids: readonly string[], level: ExplanationLevel = "brief") => {
      if (changeSet === null || ids.length === 0 || running.has(key)) return;
      const owner = changeSet;
      const runId = `explain-${++runs}`;
      setRunning((current) => new Map(current).set(key, { key, ids, level, stopping: false, runId }));
      setErrors((current) => {
        const next = new Map(current);
        next.delete(key);
        return next;
      });
      explainFunctions(projectId, reviewRefOf(owner), [...ids], level, pullRequest ?? null, runId)
        .then(
          (next) => keep(owner, next),
          (cause: unknown) => {
            if (!alive.current || (isApiError(cause) && cause.code === "cancelled")) return;
            setErrors((current) => new Map(current).set(key, errorMessage(cause)));
            // What other calls of a group wrote is kept: read it.
            setAttempt((n) => n + 1);
          },
        )
        .finally(() => {
          if (!alive.current) return;
          setRunning((current) => {
            const next = new Map(current);
            next.delete(key);
            return next;
          });
        });
    },
    [projectId, changeSet, pullRequest, running, keep],
  );

  const cancel = useCallback(
    (key: string) => {
      const run = running.get(key);
      if (!run) return;
      setRunning((current) => {
        const found = current.get(key);
        return found ? new Map(current).set(key, { ...found, stopping: true }) : current;
      });
      cancelExplanation(run.runId).catch(() => {
        // Already over.
      });
    },
    [running],
  );

  const runOf = useCallback(
    (id: string) => {
      const own = running.get(nodeRunKey(id));
      if (own) return own;
      for (const run of running.values()) {
        if (run.level === "brief" && run.ids.includes(id)) return run;
      }
      return null;
    },
    [running],
  );

  const runByKey = useCallback((key: string) => running.get(key) ?? null, [running]);
  const errorOf = useCallback((key: string) => errors.get(key) ?? null, [errors]);

  const planTranslation = useCallback(
    (target: Language) => {
      if (changeSet === null) return Promise.reject(new Error("No change set."));
      return planExplanationsTranslation(projectId, reviewRefOf(changeSet), target);
    },
    [projectId, changeSet],
  );

  const translateCall = useCallback(
    async (target: Language, functionIds: string[], runId: string) => {
      if (changeSet === null) return;
      const owner = changeSet;
      keep(owner, await translateExplanations(projectId, reviewRefOf(owner), target, functionIds, [...(latestSpans.current ?? [])], runId));
    },
    [projectId, changeSet, keep],
  );

  const refresh = useCallback(() => setAttempt((n) => n + 1), []);

  return useMemo(
    () => ({ shown, models, runOf, runByKey, errorOf, explain, cancel, planTranslation, translateCall, refresh }),
    [shown, models, runOf, runByKey, errorOf, explain, cancel, planTranslation, translateCall, refresh],
  );
}
