import "@/test/elk";

import { describe, expect, it } from "vitest";

import { absoluteBox, buildView, groupFunctions, NOTHING_OPEN, toggleGroup } from "@/features/review/flow/flow-groups";
import { layoutFlow, layoutInput, readingOrder } from "@/features/review/flow/flow-layout";
import { CONTAINER_HEADER } from "@/features/review/flow/flow-model";
import type { FlowGraph } from "@/lib/api";
import { makeFlowEdge, makeFlowGraph, makeFlowNode, REFUND_IDS as id, refundFlowGraph } from "@/test/flow-fixtures";

const byFunction = (graph: FlowGraph) => buildView(graph, groupFunctions(graph.nodes), "functions", NOTHING_OPEN);

describe("layout input", () => {
  it("orders boxes breadth first from the entry points, targets by call position, the rest last", () => {
    expect(readingOrder(byFunction(refundFlowGraph()))).toEqual([
      id.create,
      id.showPayment,
      id.onSettled,
      id.request,
      id.legacy,
      id.get,
      id.notify,
      id.validate,
      id.gateway,
      id.save,
      id.queryRow,
      id.insertRefund,
    ]);
    const lone = makeFlowGraph([makeFlowNode("b"), makeFlowNode("a")], [], { entryPoints: [] });
    expect(readingOrder(byFunction(lone))).toEqual(["b", "a"]);
  });

  it("gives ELK sized boxes and one edge per link, leaving out links to itself", () => {
    const graph = makeFlowGraph(
      [makeFlowNode("a", { name: "A" }), makeFlowNode("b", { name: "B", state: "external", file: null })],
      [
        makeFlowEdge("a", "b", { position: 1 }),
        makeFlowEdge("a", "b", { position: 3, relation: "reference" }),
        makeFlowEdge("a", "a", { position: 2 }),
      ],
      { entryPoints: ["a"] },
    );
    const input = layoutInput(byFunction(graph));
    expect(input.layoutOptions).toMatchObject({
      "elk.algorithm": "layered",
      "elk.direction": "RIGHT",
      "elk.hierarchyHandling": "INCLUDE_CHILDREN",
      "elk.layered.considerModelOrder.strategy": "NODES_AND_EDGES",
    });
    expect(layoutInput(byFunction(graph), false).layoutOptions).not.toHaveProperty(
      "elk.layered.considerModelOrder.strategy",
    );
    expect(input.children).toEqual([
      {
        id: "a",
        width: 148,
        height: 62,
        layoutOptions: { "elk.alignment": "LEFT", "elk.layered.layering.layerConstraint": "FIRST" },
      },
      { id: "b", width: 124, height: 44, layoutOptions: { "elk.alignment": "LEFT" } },
    ]);
    expect(input.edges).toEqual([
      { id: "a->b", sources: ["a"], targets: ["b"], labels: [{ id: "a->b:label", text: "calls · passes as value", width: 148, height: 16 }] },
    ]);
  });

  it("nests the boxes of an open group in it, padded for its header", () => {
    const graph = refundFlowGraph();
    const grouping = groupFunctions(graph.nodes);
    const view = buildView(graph, grouping, "packages", toggleGroup(NOTHING_OPEN, grouping.groups.get("package:internal/refund")!));
    const input = layoutInput(view);
    const refund = input.children!.find((child) => child.id === "package:internal/refund")!;
    expect(refund.width).toBeUndefined();
    expect(refund.layoutOptions?.["elk.padding"]).toBe(`[top=${CONTAINER_HEADER},left=14,bottom=14,right=14]`);
    expect(refund.children!.map((child) => child.id)).toEqual([
      "file:internal/refund/handler.go",
      "file:internal/refund/service.go",
      "file:internal/refund/legacy.go",
      "file:internal/refund/validate.go",
    ]);
    expect(input.edges!.map((edge) => edge.id)).toContain("file:internal/refund/handler.go->file:internal/refund/service.go");
  });
});

describe("layout", () => {
  it("lays callers out left of their callees, entry points in the first column", async () => {
    const view = byFunction(refundFlowGraph());
    const layout = await layoutFlow(view);

    expect(layout.boxes.size).toBe(view.units.length);
    const x = (nodeId: string) => layout.boxes.get(nodeId)!.x;
    expect(x(id.create)).toBeLessThan(x(id.request));
    expect(x(id.request)).toBeLessThan(x(id.get));
    expect(x(id.get)).toBeLessThan(x(id.queryRow));
    expect(x(id.showPayment)).toBe(x(id.create));
    expect(x(id.onSettled)).toBe(x(id.create));
    expect(await layoutFlow(byFunction(view.graph))).toBe(layout);
  });

  it("places the boxes of an open group inside it, below its header", async () => {
    const graph = refundFlowGraph();
    const grouping = groupFunctions(graph.nodes);
    const view = buildView(graph, grouping, "packages", toggleGroup(NOTHING_OPEN, grouping.groups.get("package:internal/refund")!));
    const layout = await layoutFlow(view);

    const refund = layout.boxes.get("package:internal/refund")!;
    const service = layout.boxes.get("file:internal/refund/service.go")!;
    expect(service.y).toBeGreaterThanOrEqual(CONTAINER_HEADER);
    expect(service.x + service.width).toBeLessThanOrEqual(refund.width);
    const absolute = absoluteBox(view, layout, "file:internal/refund/service.go")!;
    expect(absolute.x).toBe(refund.x + service.x);
  });

  it("routes every link from its source's side to its target's, through no other box", async () => {
    const graph = refundFlowGraph();
    const grouping = groupFunctions(graph.nodes);
    const views = [
      byFunction(graph),
      buildView(graph, grouping, "packages", toggleGroup(NOTHING_OPEN, grouping.groups.get("package:internal/refund")!)),
    ];
    for (const view of views) {
      const layout = await layoutFlow(view);
      const boxes = new Map(
        view.units.filter((unit) => unit.kind === "function" || !unit.open).map((unit) => [unit.id, absoluteBox(view, layout, unit.id)!]),
      );
      for (const link of view.links.filter((candidate) => candidate.from !== candidate.to)) {
        const route = layout.routes.get(link.id);
        expect(route, link.id).toBeDefined();
        const points = route!.points;
        const from = boxes.get(link.from)!;
        const to = boxes.get(link.to)!;
        expect(onEdge(points[0], from), `${link.id} starts on its source`).toBe(true);
        expect(onEdge(points[points.length - 1], to), `${link.id} ends on its target`).toBe(true);
        const label = route!.label;
        expect(label, link.id).not.toBeNull();
        const gaps = points.slice(1).map((point, i) => distanceToRun(label!, points[i], point));
        expect(Math.min(...gaps), `${link.id}'s label sits by its line`).toBeLessThan(16);
        for (const [id, box] of boxes) {
          if (id === link.from || id === link.to) continue;
          for (let i = 1; i < points.length; i++) {
            expect(crosses(points[i - 1], points[i], box), `${link.id} crosses ${id}`).toBe(false);
          }
        }
      }
    }
  });
});

type Point = { x: number; y: number };
type Rect = { x: number; y: number; width: number; height: number };

/** On the border of the box, give or take half a pixel. */
function onEdge(point: Point, box: Rect): boolean {
  const inX = point.x >= box.x - 0.5 && point.x <= box.x + box.width + 0.5;
  const inY = point.y >= box.y - 0.5 && point.y <= box.y + box.height + 0.5;
  const onSide = Math.abs(point.x - box.x) < 0.5 || Math.abs(point.x - box.x - box.width) < 0.5;
  const onTopOrBottom = Math.abs(point.y - box.y) < 0.5 || Math.abs(point.y - box.y - box.height) < 0.5;
  return inX && inY && (onSide || onTopOrBottom);
}

/** Distance from a point to a horizontal or vertical run. */
function distanceToRun(point: Point, a: Point, b: Point): number {
  const x = Math.min(Math.max(point.x, Math.min(a.x, b.x)), Math.max(a.x, b.x));
  const y = Math.min(Math.max(point.y, Math.min(a.y, b.y)), Math.max(a.y, b.y));
  return Math.hypot(point.x - x, point.y - y);
}

/** Whether a horizontal or vertical run passes through the inside of the box. */
function crosses(a: Point, b: Point, box: Rect): boolean {
  const [left, right] = [Math.min(a.x, b.x), Math.max(a.x, b.x)];
  const [top, bottom] = [Math.min(a.y, b.y), Math.max(a.y, b.y)];
  return right > box.x + 1 && left < box.x + box.width - 1 && bottom > box.y + 1 && top < box.y + box.height - 1;
}
