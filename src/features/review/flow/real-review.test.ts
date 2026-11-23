// Measures what the canvas would open with on a real review: the graph that the Rust harness
// `flow_graph::tests::sizes_of_a_real_review` writes as graph.json. Skipped unless DIFFLENS_GRAPH_JSON names that file:
//   DIFFLENS_GRAPH_JSON=/path/to/graph.json pnpm vitest run src/features/review/flow/real-review.test.ts

import "@/test/elk";

import { describe, expect, it } from "vitest";

import { absoluteBox, buildView, groupFunctions, initialLevel, NOTHING_OPEN } from "@/features/review/flow/flow-groups";
import { layoutFlow } from "@/features/review/flow/flow-layout";
import { linkLabel } from "@/features/review/flow/flow-links";
import type { FlowGraph } from "@/lib/api";

// The app's TypeScript setup has no Node types; this test only runs under vitest, in Node.
const env = (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env ?? {};
const file = env.DIFFLENS_GRAPH_JSON;
const NODE_FS = "node:fs";

describe.skipIf(!file)("a real review", () => {
  it("opens at the level the thresholds pick, with its groups and links", async () => {
    const fs = (await import(/* @vite-ignore */ NODE_FS)) as { readFileSync(path: string, encoding: "utf8"): string };
    const graph = JSON.parse(fs.readFileSync(file!, "utf8")) as FlowGraph;
    const grouping = groupFunctions(graph.nodes);
    const level = initialLevel(graph, grouping);
    const opened = buildView(graph, grouping, level, NOTHING_OPEN);
    const byFile = buildView(graph, grouping, "files", NOTHING_OPEN);
    const fileLinks = byFile.links.filter((link) => grouping.groups.has(link.from) && grouping.groups.has(link.to));

    console.log(
      [
        `functions ${graph.nodes.length}, files ${grouping.files.length}, packages ${grouping.packages.length}`,
        `links between files ${fileLinks.length}`,
        `initial level ${level}: ${opened.units.length} boxes, ${opened.links.length} links, ${opened.isolated.size} isolated`,
        ...opened.links.map((link) => `  ${link.from} -> ${link.to}: ${linkLabel(link)}`),
        ...[...opened.isolated].map((id) => `  isolated: ${id}`),
      ].join("\n"),
    );
    expect(opened.units.length).toBeGreaterThan(0);
  });

  it("routes the links of every level around the boxes", async () => {
    const fs = (await import(/* @vite-ignore */ NODE_FS)) as { readFileSync(path: string, encoding: "utf8"): string };
    const graph = JSON.parse(fs.readFileSync(file!, "utf8")) as FlowGraph;
    const grouping = groupFunctions(graph.nodes);
    for (const level of ["packages", "files", "functions"] as const) {
      const view = buildView(graph, grouping, level, NOTHING_OPEN);
      const layout = await layoutFlow(view);
      const boxes = view.units.map((unit) => [unit.id, absoluteBox(view, layout, unit.id)!] as const);
      const links = view.links.filter((link) => link.from !== link.to);
      const unrouted = links.filter((link) => !layout.routes.has(link.id));
      const crossings = links.flatMap((link) => {
        const points = layout.routes.get(link.id)?.points ?? [];
        return boxes
          .filter(([id, box]) => id !== link.from && id !== link.to && points.some((point, i) => i > 0 && crosses(points[i - 1], point, box)))
          .map(([id]) => `${link.id} through ${id}`);
      });
      const width = Math.max(...boxes.map(([, box]) => box.x + box.width));
      const height = Math.max(...boxes.map(([, box]) => box.y + box.height));
      console.log(
        `${level}: ${view.units.length} boxes, ${links.length} links, ${unrouted.length} unrouted, ` +
          `${crossings.length} crossing a box, canvas ${Math.round(width)} x ${Math.round(height)}`,
        ...crossings.map((crossing) => `\n  ${crossing}`),
      );
      expect(unrouted).toEqual([]);
      expect(crossings).toEqual([]);
    }
  }, 60_000);
});

type Point = { x: number; y: number };

/** Whether a horizontal or vertical run passes through the inside of the box. */
function crosses(a: Point, b: Point, box: { x: number; y: number; width: number; height: number }): boolean {
  const [left, right] = [Math.min(a.x, b.x), Math.max(a.x, b.x)];
  const [top, bottom] = [Math.min(a.y, b.y), Math.max(a.y, b.y)];
  return right > box.x + 1 && left < box.x + box.width - 1 && bottom > box.y + 1 && top < box.y + box.height - 1;
}
