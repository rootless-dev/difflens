import type { Overview, OverviewPlan } from "@/lib/api";
import { makeModelInUse } from "@/test/insights-fixtures";

/** An overview read before the first Summarize: no summary, and a change of no particular shape. */
export function makeOverview(overrides: Partial<Overview> = {}): Overview {
  return {
    summary: null,
    shape: {
      filesAdded: 0,
      filesDeleted: 0,
      directoriesNew: [],
      directoriesModified: [],
      manifests: [],
      migrations: [],
    },
    leftOut: { skipped: [], truncated: [] },
    ...overrides,
  };
}

export function makeOverviewPlan(overrides: Partial<OverviewPlan> = {}): OverviewPlan {
  return {
    tokens: 12_400,
    files: 3,
    condensed: false,
    cardKey: "PAY-482",
    cardComments: 1,
    model: makeModelInUse(),
    ...overrides,
  };
}
