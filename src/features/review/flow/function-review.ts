import { createContext } from "react";

import type { FlowNode, MarkStatus } from "@/lib/api";

/** Whether the functions of the graph were marked as reviewed, and how to mark one. */
export interface FunctionReview {
  /** null for a function that is not marked. */
  statusOf: (id: string) => MarkStatus | null;
  /** Marks a changed function, or unmarks a reviewed one; null while marking is not possible yet. */
  toggle: ((node: FlowNode) => void) | null;
  /** A save is under way. */
  busy: boolean;
}

export const FunctionReviewContext = createContext<FunctionReview>({
  statusOf: () => null,
  toggle: null,
  busy: false,
});
