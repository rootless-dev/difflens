import {
  ANALYSIS_STEPS,
  type FullAnalysisJob,
  type FullAnalysisPlan,
  type LogEvent,
  type LogLine,
  type ModelInUse,
  type ReviewTarget,
  type StepState,
} from "@/lib/api";
import { makeModelInUse, makePlan } from "@/test/insights-fixtures";
import { makeOverviewPlan } from "@/test/overview-fixtures";

const BRANCH: ReviewTarget = { kind: "branch", branch: "feat/refund-flow", base: "main" };

/** A full analysis of `feat/refund-flow` running in project p-1, every step in progress. */
export function makeJob(overrides: Partial<FullAnalysisJob> = {}): FullAnalysisJob {
  const steps: StepState[] = ANALYSIS_STEPS.map((step) => ({ step, status: "running", done: 0, total: 0, error: null }));
  return {
    id: "job-1",
    projectId: "p-1",
    projectName: "payments",
    review: { target: BRANCH, oldSha: "a1b2c3d", newSha: "e7f3b10" },
    status: "running",
    outcome: null,
    cancelling: false,
    steps,
    progress: 45,
    queuedAt: "2026-11-19T14:03:00Z",
    startedAt: "2026-11-19T14:03:00Z",
    finishedAt: null,
    log: [],
    ...overrides,
  };
}

export const logLine = (event: LogEvent, at = "2026-11-19T14:03:27Z"): LogLine => ({ ...event, at });

/** Every part on the same model unless `model` gives the model of the part. */
export function makeFullAnalysisPlan(
  overrides: Partial<FullAnalysisPlan> = {},
  model: Partial<Record<"insights" | "summary" | "explanations", ModelInUse>> = {},
): FullAnalysisPlan {
  const shared = makeModelInUse();
  return {
    tests: true,
    insights: makePlan({
      files: 60,
      tokens: 90_000,
      calls: [0, 1, 2].map(() => ({ paths: [], tokens: 30_000, truncated: [] })),
      model: model.insights ?? shared,
    }),
    summary: makeOverviewPlan({ tokens: 12_000, model: model.summary ?? shared }),
    explanations: {
      changed: 5,
      calls: [{ ids: ["a"], tokens: 8_000 }],
      tokens: 8_000,
      model: model.explanations ?? shared,
    },
    limit: 4,
    running: 0,
    ...overrides,
  };
}
