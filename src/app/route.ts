import type { ReviewOpening } from "@/features/review/views";
import type { ReviewTarget, TerminalKind } from "@/lib/api";

/** Screens the app can show. Later slices add their own members to this union. */
export type Route =
  | { screen: "projects" }
  | { screen: "project"; projectId: string; projectName: string }
  /**
   * `opening`: the tab the review opens on; its Overview when absent. `terminal`: a session of that kind to open in
   * the review's terminal panel, expanded, as it opens (a branch's ⋮ menu).
   */
  | {
      screen: "review";
      projectId: string;
      projectName: string;
      target: ReviewTarget;
      opening?: ReviewOpening;
      terminal?: TerminalKind;
    }
  /** Apply to every project; `from` is the screen "Back" returns to. */
  | { screen: "preferences"; from: Route };

export const PROJECTS_ROUTE: Route = { screen: "projects" };
