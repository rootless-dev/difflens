import { LoaderCircle, RotateCw, Sparkles, Square } from "lucide-react";
import { useContext } from "react";
import { useTranslation } from "react-i18next";

import { explainerName } from "@/features/review/explanations/explanation-text";
import { ExplanationsContext } from "@/features/review/explanations/use-explanations";
import { cn } from "@/lib/utils";

// React Flow turns pointer events off on nodes that cannot be selected or dragged; the button takes them back.
const BUTTON_STYLE = { pointerEvents: "auto" } as const;

interface ExplainButtonProps {
  /** The run this button starts: a function's or a group's (see `nodeRunKey`, `groupRunKey`). */
  runKey: string;
  /** The functions it explains; a group's button explains them all in one call. */
  ids: readonly string[];
  /** What its labels name: the function's or the group's name. */
  name: string;
  /** A group's button, which names how many functions it explains. */
  group?: boolean;
  /** The function was explained and its code changed since: the button explains it again. */
  outdated?: boolean;
  className?: string;
}

/**
 * The small button on a box that asks the model to explain it (a model call, named in its tooltip, only on click).
 * While the model has not answered it is a spinner that stops the call when pressed; a function its group's button
 * is explaining shows that run, and stops it.
 */
export function ExplainButton({ runKey, ids, name, group = false, outdated = false, className }: ExplainButtonProps) {
  const { t } = useTranslation();
  const explanations = useContext(ExplanationsContext);
  const run = group ? explanations.runByKey(runKey) : (ids[0] !== undefined ? explanations.runOf(ids[0]) : null);
  const error = explanations.errorOf(runKey);
  // A box's or a group's button explains in short.
  const model = explainerName(explanations.models.brief);
  const base =
    "grid size-5 flex-none cursor-pointer place-items-center rounded-[5px] outline-none hover:bg-ai-bg hover:text-ai focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default";

  if (run) {
    const label = t("flow.explain.cancel", { name });
    return (
      <button
        type="button"
        aria-label={label}
        title={run.stopping ? t("flow.explain.stopping") : label}
        disabled={run.stopping}
        onClick={() => explanations.cancel(run.key)}
        style={BUTTON_STYLE}
        className={cn(base, "group/stop text-ai", className)}
      >
        <LoaderCircle aria-hidden="true" className="size-3.5 animate-spin group-hover/stop:hidden" />
        <Square aria-hidden="true" className="hidden size-3 fill-current group-hover/stop:block" />
      </button>
    );
  }

  const label = group
    ? t("flow.explain.groupTitle", { count: ids.length, name, model })
    : outdated
      ? t("flow.explain.againTitle", { name, model })
      : t("flow.explain.buttonTitle", { name, model });
  const Icon = outdated ? RotateCw : Sparkles;
  return (
    <button
      type="button"
      aria-label={label}
      title={error ? `${t("flow.explain.failed", { message: error })}\n${label}` : label}
      disabled={ids.length === 0}
      onClick={() => explanations.explain(runKey, ids)}
      style={BUTTON_STYLE}
      className={cn(base, error ? "text-del" : outdated ? "text-primary" : "text-faint", className)}
    >
      <Icon aria-hidden="true" className="size-3.5" />
    </button>
  );
}
