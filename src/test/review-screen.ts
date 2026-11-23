// Helpers for tests that render the ReviewScreen. A review opens on the Overview, which shows what the flow and the
// quality report measured, so both are asked for as the review opens, along with the overview, its plan, the card
// and the last mutation run.

import { screen, within } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";
import { vi } from "vitest";

import { findCard, getFlow, getInsights, getMutationState, getOverview, getQuality, planOverview } from "@/lib/api";
import { makeFlowGraph } from "@/test/flow-fixtures";
import { NO_INSIGHTS } from "@/test/insights-fixtures";
import { makeOverview, makeOverviewPlan } from "@/test/overview-fixtures";
import { makeMutationState, makeQualityReport } from "@/test/tests-fixtures";

/**
 * Answers what a review asks for as it opens on the Overview, never a model: the card, the overview and what
 * Summarize would send, the flow, the quality report, the findings already written and the last mutation run. The
 * answers are neutral (no card key, nothing summarized, no changed function, no finding, no mutation run), so a test
 * about something else sees no count in a tab. Call it first in `beforeEach`; a test file about one of them sets its
 * own answer after it. The file's `vi.mock("@/lib/api")` must mock each of these functions.
 */
export function answerReviewOpening(branch = "feat/refund-flow") {
  vi.mocked(findCard).mockResolvedValue({ kind: "no_key", branch });
  vi.mocked(getOverview).mockResolvedValue(makeOverview());
  vi.mocked(planOverview).mockResolvedValue(makeOverviewPlan({ cardKey: null, cardComments: 0 }));
  vi.mocked(getFlow).mockResolvedValue({ kind: "graph", graph: makeFlowGraph([], []) });
  vi.mocked(getQuality).mockResolvedValue({ kind: "report", report: makeQualityReport({ functions: [], crossed: [] }) });
  vi.mocked(getInsights).mockResolvedValue(NO_INSIGHTS);
  vi.mocked(getMutationState).mockResolvedValue(makeMutationState());
}

export const reviewTab = (name: string | RegExp) =>
  within(screen.getByRole("tablist", { name: "Review views" })).getByRole("tab", { name });

/** Waits for the changed files of the review to be read: the Diff lists them even behind another tab. */
export const changedFilesRead = () => screen.findByRole("listbox", { name: "Changed files", hidden: true });

/**
 * Opens a tab of a review just rendered, once its changed files are read, the way a reviewer would: the review opens
 * on the Overview, so a test about another tab clicks it first.
 */
export async function openReviewTab(user: UserEvent, name: string | RegExp) {
  await changedFilesRead();
  await user.click(reviewTab(name));
}
