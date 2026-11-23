export interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Placement {
  left: number;
  top: number;
  /**
   * Side of the node the popover is on; its caret points back at the node. "over" when it fits on neither side and
   * was moved over the node to stay inside the canvas: there is no caret then.
   */
  side: "right" | "left" | "over";
  /** Distance of the caret from the popover's top edge. */
  caret: number;
}

const GAP = 18;
/** Space kept between the popover and the canvas's edges. */
export const POPOVER_MARGIN = 8;

/** The popover's width with one column, and with a second one beside it. */
export const POPOVER_WIDTH = { 1: 380, 2: 760 } as const;

/**
 * Which edge of the popover is pinned, and where, given the width it is placed for. A popover to the left of its node
 * is pinned by its right edge, so while it widens it grows away from the node instead of over it; any other is pinned
 * by its left edge, and grows to the right.
 */
export function pinnedEdge(
  place: Placement,
  width: number,
  boundsWidth: number,
): { edge: "left" | "right"; offset: number } {
  if (place.side === "left") return { edge: "right", offset: boundsWidth - (place.left + width) };
  return { edge: "left", offset: place.left };
}

/**
 * Puts the popover to the right of the node, or to its left when it does not fit on the right
 * (and there is more room there), with its caret level with the node's middle. When it fits on
 * neither side, it is moved over the node rather than past the canvas's edge.
 */
export function placePopover(
  anchor: Box,
  bounds: { width: number; height: number },
  popover: { width: number; height: number },
): Placement {
  const rightLeft = anchor.x + anchor.width + GAP;
  const leftLeft = anchor.x - GAP - popover.width;
  const maxLeft = bounds.width - popover.width - POPOVER_MARGIN;
  const fitsRight = rightLeft <= maxLeft;
  const roomRight = bounds.width - rightLeft;
  const roomLeft = anchor.x - GAP;
  const preferred = fitsRight || roomRight >= roomLeft ? "right" : "left";
  const wanted = preferred === "right" ? rightLeft : leftLeft;
  const left = clamp(wanted, POPOVER_MARGIN, Math.max(POPOVER_MARGIN, maxLeft));
  const side = left === wanted ? preferred : "over";

  const middle = anchor.y + anchor.height / 2;
  const maxTop = bounds.height - popover.height - POPOVER_MARGIN;
  const top = clamp(middle - 40, POPOVER_MARGIN, Math.max(POPOVER_MARGIN, maxTop));
  const caret = clamp(middle - top, 16, Math.max(16, popover.height - 16));
  return { left, top, side, caret };
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}
