import type { DiffMode } from "@/features/review/DiffToolbar";
import { i18n } from "@/i18n";

/**
 * Views of a review, in tab order, by id. Only those in ENABLED_VIEWS exist yet; the others are shown, disabled. The
 * ids stay English in code and tests; `viewLabel` gives the name shown on the tab.
 */
export const VIEWS = ["Overview", "Flow", "Diff", "Tests", "Quality", "Insights", "Run"] as const;

export type ReviewView = (typeof VIEWS)[number];

/**
 * Where a review opens, when it is opened on something other than its Overview (a branch row's shortcuts): a tab, and
 * for the Diff what it shows, its files or its comments.
 */
export interface ReviewOpening {
  view: ReviewView;
  diffMode?: DiffMode;
}

export const ENABLED_VIEWS: ReadonlySet<ReviewView> = new Set([
  "Overview",
  "Flow",
  "Diff",
  "Tests",
  "Quality",
  "Insights",
]);

const VIEW_LABEL = {
  Overview: "review.views.names.overview",
  Flow: "review.views.names.flow",
  Diff: "review.views.names.diff",
  Tests: "review.views.names.tests",
  Quality: "review.views.names.quality",
  Insights: "review.views.names.insights",
  Run: "review.views.names.run",
} as const satisfies Record<ReviewView, string>;

/** The name of a view as its tab shows it, in the language shown. */
export const viewLabel = (view: ReviewView): string => i18n.t(VIEW_LABEL[view]);

/** Title of what is not there yet, such as a disabled tab. */
export const laterStep = (): string => i18n.t("review.views.laterStep");
