// The words around an explanation of a function: the line its box shows, who an Explain button asks, and what a
// group's button explains.
import type { ShownExplanation } from "@/features/review/ai-text/versions";
import { canExplain } from "@/features/review/explanations/use-explanations";
import type { Group } from "@/features/review/flow/flow-groups";
import { modelName } from "@/features/review/insights/insights-text";
import { i18n } from "@/i18n";
import type { ModelInUse } from "@/lib/api";

/**
 * What a function's box shows of its explanation: the short one whole (the box keeps its first lines), or, for a
 * function explained only in detail (as every explanation written before short ones was), the first sentence of what
 * changed, or of what it is for for a function of the context. The prompts ask for that sentence to stand on its own.
 */
export function explanationLine(explanation: ShownExplanation): string {
  if (explanation.brief) return explanation.brief.text.trim();
  const detail = explanation.detail;
  if (!detail) return "";
  const text = (detail.kind === "changed" ? detail.whatChanged : detail.purpose).trim();
  const match = /^(.+?[.!?])(?:\s|$)/s.exec(text);
  return match ? match[1] : text;
}

/** The code changed since the text a function's box shows was written. */
export function lineOutdated(explanation: ShownExplanation): boolean {
  return explanation.brief ? explanation.brief.outdated : (explanation.detail?.outdated ?? false);
}

/** Who an Explain button asks, as its tooltip names it. */
export function explainerName(model: ModelInUse | null): string {
  return model ? modelName(model) : i18n.t("flow.explain.modelUnknown");
}

/** Every function of a group the model can be asked about, in the graph's order. */
export function groupFunctionIds(group: Group): string[] {
  const functions = group.kind === "file" ? group.functions : group.files.flatMap((file) => file.functions);
  return functions.filter(canExplain).map((node) => node.id);
}
