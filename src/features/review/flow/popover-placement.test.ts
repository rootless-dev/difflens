import { describe, expect, it } from "vitest";

import { pinnedEdge, placePopover, POPOVER_WIDTH } from "@/features/review/flow/popover-placement";

const bounds = { width: 1000, height: 600 };
const popover = { width: 380, height: 280 };

describe("popover placement", () => {
  it("opens to the right of the node with the caret at its middle", () => {
    expect(placePopover({ x: 100, y: 200, width: 148, height: 62 }, bounds, popover)).toEqual({
      left: 266,
      top: 191,
      side: "right",
      caret: 40,
    });
  });

  it("flips to the left near the right edge", () => {
    expect(placePopover({ x: 700, y: 200, width: 148, height: 62 }, bounds, popover)).toMatchObject({
      left: 302,
      side: "left",
    });
  });

  it("flips a two-column popover to the left when it does not fit on the right", () => {
    expect(placePopover({ x: 850, y: 200, width: 148, height: 62 }, bounds, { width: 760, height: 280 })).toMatchObject({
      left: 72,
      side: "left",
    });
  });

  it("moves over the node, inside the canvas and without a caret side, when it fits on neither", () => {
    const narrow = { width: 500, height: 600 };
    expect(placePopover({ x: 60, y: 200, width: 148, height: 62 }, narrow, popover)).toMatchObject({
      left: 112,
      side: "over",
    });
    expect(placePopover({ x: 300, y: 200, width: 148, height: 62 }, narrow, popover)).toMatchObject({
      left: 8,
      side: "over",
    });
  });

  it("keeps a popover wider than the canvas at its left margin", () => {
    const tiny = { width: 300, height: 600 };
    expect(placePopover({ x: 100, y: 200, width: 148, height: 62 }, tiny, popover)).toMatchObject({
      left: 8,
      side: "over",
    });
  });

  it("decides the side by the width the popover widens to, not the one it opens at", () => {
    const node = { x: 300, y: 200, width: 148, height: 62 };
    // One column fits on the right of this node; two fit on neither side, so a popover about to widen opens over it.
    expect(placePopover(node, bounds, { width: POPOVER_WIDTH[1], height: 280 })).toMatchObject({ side: "right" });
    expect(placePopover(node, bounds, { width: POPOVER_WIDTH[2], height: 280 })).toMatchObject({ side: "over" });
    const nearRight = { x: 850, y: 200, width: 148, height: 62 };
    const wide = placePopover(nearRight, bounds, { width: POPOVER_WIDTH[2], height: 280 });
    expect(wide).toMatchObject({ side: "left", left: 72 });
  });

  it("pins a popover on the left by its right edge, beside the node, and any other by its left edge", () => {
    const nearRight = { x: 850, y: 200, width: 148, height: 62 };
    const left = placePopover(nearRight, bounds, { width: POPOVER_WIDTH[2], height: 280 });
    // The right edge sits a gap away from the node, whatever width the popover has on its way there.
    expect(pinnedEdge(left, POPOVER_WIDTH[2], bounds.width)).toEqual({ edge: "right", offset: 1000 - (850 - 18) });
    const right = placePopover({ x: 100, y: 200, width: 148, height: 62 }, bounds, popover);
    expect(pinnedEdge(right, POPOVER_WIDTH[1], bounds.width)).toEqual({ edge: "left", offset: 266 });
    const over = placePopover({ x: 300, y: 200, width: 148, height: 62 }, { width: 500, height: 600 }, popover);
    expect(pinnedEdge(over, POPOVER_WIDTH[1], 500)).toEqual({ edge: "left", offset: 8 });
  });

  it("keeps inside the canvas vertically and moves the caret to stay on the node", () => {
    const low = placePopover({ x: 100, y: 560, width: 148, height: 62 }, bounds, popover);
    expect(low.top).toBe(312);
    expect(low.caret).toBe(264);
    const high = placePopover({ x: 100, y: -20, width: 148, height: 62 }, bounds, popover);
    expect(high.top).toBe(8);
    expect(high.caret).toBe(16);
  });
});
