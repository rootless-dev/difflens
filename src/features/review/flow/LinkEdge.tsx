import { EdgeLabelRenderer, type EdgeProps } from "@xyflow/react";
import { createContext, useContext } from "react";
import { useTranslation } from "react-i18next";

import { type LabelPart, linkLabelParts, RELATION_LOOK, STATE_LOOK } from "@/features/review/flow/flow-links";
import { linkGeometry, type LinkFlowEdge, routeGeometry } from "@/features/review/flow/flow-model";
import { cn } from "@/lib/utils";

/** Opens the list of a link's relations at a point of the canvas, or closes it when it is already open. */
export const SelectLinkContext = createContext<(id: string, at: { x: number; y: number }) => void>(() => {});

// The label is HTML over the canvas; React Flow gives it no pointer events of its own.
const LABEL_STYLE = { pointerEvents: "all" } as const;

/**
 * The relations between two boxes. Their type is the line style (calls solid, registrations dotted, references
 * sparsely dotted), their change state the colour (added accent, kept grey, removed red with a cross on the label),
 * so the two read together. The label says what they are, or for groups how many of each state; pressing it lists
 * them.
 */
export function LinkEdge({ source, target, sourceX, sourceY, targetX, targetY, data }: EdgeProps<LinkFlowEdge>) {
  const { t } = useTranslation();
  const select = useContext(SelectLinkContext);
  if (!data) return null;
  const { link, ends, title, dimmed, selected, route } = data;
  // The laid out route goes around the boxes; once a box is dragged off it, a curve between the ends stands in.
  const { path, label } = route
    ? routeGeometry(route)
    : linkGeometry(sourceX, sourceY, targetX, targetY, source === target);
  const state = STATE_LOOK[link.state];
  const parts = linkLabelParts(link);
  const text = parts.map((part) => part.text).join(" · ");
  const removed = link.state === "removed";

  return (
    <g
      data-testid={`link ${link.id}`}
      data-relation={link.relation}
      data-state={link.state}
      data-dimmed={dimmed || undefined}
      className={cn("transition-opacity", dimmed ? "opacity-20" : removed && "opacity-75")}
    >
      <title>{title}</title>
      {/* Wider invisible stroke, so the tooltip shows without aiming at a 1.4px line. */}
      <path d={path} fill="none" stroke="transparent" strokeWidth={12} />
      <path
        d={path}
        fill="none"
        strokeWidth={selected ? state.strokeWidth + 0.8 : state.strokeWidth}
        strokeDasharray={RELATION_LOOK[link.relation].dash}
        strokeLinecap={link.relation === "call" ? undefined : "round"}
        className={state.stroke}
      />
      <EdgeLabelRenderer>
        <button
          type="button"
          data-link-id={link.id}
          aria-label={removed ? t("flow.link.removedLabel", { ends, text }) : `${ends}: ${text}`}
          aria-haspopup="dialog"
          aria-expanded={selected}
          title={title}
          onClick={() => select(link.id, label)}
          style={{ ...LABEL_STYLE, transform: `translate(-50%, -50%) translate(${label.x}px, ${label.y}px)` }}
          className={cn(
            "nodrag nopan absolute top-0 left-0 flex max-w-[260px] cursor-pointer items-center gap-1 rounded-full border bg-background px-1.5 py-px font-sans text-[10px] leading-[14px] font-medium whitespace-nowrap text-muted-foreground outline-none hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring",
            link.state === "added" && "border-primary text-primary",
            removed && "border-del text-del",
            selected && "shadow-[0_0_0_1px_var(--primary)]",
            dimmed && "opacity-20",
          )}
        >
          {removed && (
            <span aria-hidden="true" className="font-bold">
              ×
            </span>
          )}
          <span className={cn("truncate", removed && "line-through")}>
            {link.state === "mixed" ? <MixedCounts parts={parts} /> : text}
          </span>
        </button>
      </EdgeLabelRenderer>
    </g>
  );
}

/** Counts of a link mixing states, the added ones in the accent colour, the removed ones in red. */
function MixedCounts({ parts }: { parts: readonly LabelPart[] }) {
  return (
    <>
      {parts.map((part, index) => (
        <span key={index}>
          {index > 0 && " · "}
          <span className={cn(part.state === "added" && "text-primary", part.state === "removed" && "text-del line-through")}>
            {part.text}
          </span>
        </span>
      ))}
    </>
  );
}
