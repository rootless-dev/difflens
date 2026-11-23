import { Handle, type NodeProps, Position } from "@xyflow/react";
import { createContext, useContext } from "react";

import { useTranslation } from "react-i18next";

import { WithCode } from "@/components/with-code";
import { ExplainButton } from "@/features/review/explanations/ExplainButton";
import { explanationLine, lineOutdated } from "@/features/review/explanations/explanation-text";
import { canExplain, ExplanationsContext, nodeRunKey } from "@/features/review/explanations/use-explanations";
import {
  type FunctionFlowNode,
  isChanged,
  isolatedNote,
  NODE_STATE,
  nodeStateLabel,
} from "@/features/review/flow/flow-model";
import { FunctionReviewContext } from "@/features/review/flow/function-review";
import { SignalBadge } from "@/features/review/signals/signal-marks";
import { type Badge, functionBadge, SignalsContext } from "@/features/review/signals/signals";
import { badgeParts } from "@/features/review/signals/signals-text";
import { i18n } from "@/i18n";
import type { FlowNode, MarkStatus } from "@/lib/api";
import { cn } from "@/lib/utils";

/** Opens the popover of a function, or closes it when it is already open. */
export const SelectNodeContext = createContext<(id: string) => void>(() => {});

// Edges attach to these; they are invisible and cannot start a connection.
const HANDLE_STYLE = { opacity: 0, pointerEvents: "none" } as const;
// React Flow turns pointer events off on nodes that cannot be selected or dragged; the button takes them back.
const BUTTON_STYLE = { pointerEvents: "auto" } as const;

/** "Service.Request, new, 28 added, 0 removed", read by screen readers in place of the box. */
export function describeNode(
  node: FlowNode,
  isolated = false,
  mark: MarkStatus | null = null,
  explanation: string | null = null,
  badge: Badge | null = null,
): string {
  const parts = [node.name, nodeStateLabel(node.state).toLowerCase()];
  if (node.state === "external") {
    parts.push(i18n.t("flow.node.functionsCalled", { count: node.externalFunctions }));
  }
  if (isChanged(node)) {
    parts.push(
      i18n.t("flow.linesAdded", { count: node.additions }),
      i18n.t("flow.linesRemoved", { count: node.deletions }),
    );
  }
  if (node.entryPoints.length > 0) {
    parts.push(i18n.t("flow.node.entryPoint", { entries: node.entryPoints.map((entry) => entry.label).join(", ") }));
  }
  if (node.generated) parts.push(i18n.t("flow.node.generated"));
  if (node.test) parts.push(i18n.t("flow.node.testCode"));
  if (isolated) parts.push(isolatedNote().toLowerCase());
  if (mark === "reviewed") parts.push(i18n.t("flow.node.reviewed"));
  if (mark === "reopened") parts.push(i18n.t("flow.node.reopened"));
  // The badge, said in its own order: the model's findings, limits, checks, no test.
  if (badge) parts.push(...badgeParts(badge));
  if (explanation) parts.push(`${i18n.t("flow.explain.nodeLabel")}: ${explanation}`);
  return parts.join(", ");
}

/**
 * On the box's corner: a tick for a function reviewed as it is, a dashed ring for one that changed after it was
 * marked (the prototype's `.tick` and `.reop`).
 */
export function ReviewBadge({ mark }: { mark: MarkStatus | null }) {
  if (mark === "reviewed") {
    return (
      <span
        aria-hidden="true"
        className="absolute -top-[7px] -right-[7px] grid size-4 place-items-center rounded-full border-[1.4px] border-add bg-background text-[10px] leading-none font-semibold text-add"
      >
        ✓
      </span>
    );
  }
  if (mark === "reopened") {
    return (
      <span
        aria-hidden="true"
        className="absolute -top-[7px] -right-[7px] size-4 rounded-full border-2 border-dashed border-primary bg-background"
      />
    );
  }
  return null;
}

/**
 * A function of the graph, as in the prototype: state bar, name, package, lines added and removed.
 * An external package is a lower dashed box at the boundary: the package and how many of its functions are called.
 */
export function FunctionNode({ data }: NodeProps<FunctionFlowNode>) {
  const { t } = useTranslation();
  const select = useContext(SelectNodeContext);
  const review = useContext(FunctionReviewContext);
  const explanations = useContext(ExplanationsContext);
  const { signals } = useContext(SignalsContext);
  const { node, detail, selected, dimmed, isolated, explained } = data;
  const external = node.state === "external";
  const changed = isChanged(node);
  const mark = changed ? review.statusOf(node.id) : null;
  const badge = changed ? functionBadge(signals.functions.get(node.id)) : null;
  const explainable = canExplain(node);
  const explanation = explainable ? (explanations.shown.get(node.id) ?? null) : null;
  // Shown only on a box laid out with room for it.
  const line = explained && explanation ? explanationLine(explanation) || null : null;
  const outdated = explanation !== null && lineOutdated(explanation);

  return (
    <div className="relative h-full w-full">
      <Handle type="target" position={Position.Left} isConnectable={false} style={HANDLE_STYLE} />
      <button
        type="button"
        data-node-id={node.id}
        data-state={node.state}
        data-dimmed={dimmed || undefined}
        aria-label={describeNode(node, isolated, mark, line, badge)}
        aria-haspopup="dialog"
        aria-expanded={selected}
        title={node.qualifiedName}
        onClick={() => select(node.id)}
        style={BUTTON_STYLE}
        className={cn(
          "relative flex h-full w-full cursor-pointer flex-col justify-center rounded-[7px] border py-1.5 pr-2.5 pl-3 text-left leading-[15px] outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring",
          external ? "border-dashed bg-card" : "bg-secondary",
          selected && "border-primary shadow-[0_0_0_1px_var(--primary)]",
          dimmed
            ? "opacity-30"
            : node.state === "untouched" && !selected
              ? "opacity-60"
              : node.state === "deleted" && !selected && "opacity-55",
        )}
      >
        {!external && (
          <span
            aria-hidden="true"
            className={cn("absolute top-2.5 bottom-2.5 left-0 w-[3px] rounded-[1.5px]", NODE_STATE[node.state].dot)}
          />
        )}
        <span
          className={cn(
            "truncate font-mono font-medium",
            external ? "text-[11px] text-muted-foreground" : "text-[11.5px]",
            explainable && "pr-5",
            node.state === "deleted" && "line-through decoration-del",
          )}
        >
          {node.name}
        </span>
        <span className="truncate text-[11px] text-muted-foreground">{detail}</span>
        {(changed || node.generated || node.test) && (
          <span className="flex items-center gap-1.5 font-mono text-[11px]">
            {changed && (
              <>
                <span className="text-add">+{node.additions}</span>
                <span className="text-del">−{node.deletions}</span>
              </>
            )}
            {(node.generated || node.test) && (
              <span className="ml-auto rounded-[4px] border px-1 font-sans text-[10px] leading-[13px] text-faint">
                {node.generated ? t("flow.node.generatedTag") : t("flow.node.testTag")}
              </span>
            )}
          </span>
        )}
        {isolated && <span className="truncate text-[11px] text-faint italic">{t("flow.isolatedNote")}</span>}
        {line && (
          <span
            data-explanation={outdated ? "outdated" : "current"}
            className={cn("mt-0.5 line-clamp-2 text-[11px] leading-[15px]", outdated ? "text-faint" : "text-ai")}
          >
            <WithCode text={line} />
          </span>
        )}
        <ReviewBadge mark={mark} />
        {badge && <SignalBadge badge={badge} />}
      </button>
      {explainable && (
        <ExplainButton
          runKey={nodeRunKey(node.id)}
          ids={[node.id]}
          name={node.name}
          outdated={outdated}
          className={cn("absolute top-1.5 right-1.5", dimmed && "opacity-30")}
        />
      )}
      <Handle type="source" position={Position.Right} isConnectable={false} style={HANDLE_STYLE} />
    </div>
  );
}
