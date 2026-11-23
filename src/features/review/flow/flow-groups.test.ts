import { describe, expect, it } from "vitest";

import {
  absoluteBox,
  buildView,
  FILE_LEVEL_LIMITS,
  groupFunctions,
  initialLevel,
  isOpen,
  litUnits,
  moveBoxes,
  NOTHING_OPEN,
  reveal,
  toggleGroup,
} from "@/features/review/flow/flow-groups";
import { stateSummary } from "@/features/review/flow/flow-links";
import type { FlowEdge, FlowNode } from "@/lib/api";
import { makeFlowEdge, makeFlowGraph, makeFlowNode, REFUND_IDS as id, refundFlowGraph } from "@/test/flow-fixtures";

const graph = refundFlowGraph();
const grouping = groupFunctions(graph.nodes);
const group = (groupId: string) => grouping.groups.get(groupId)!;

describe("grouping", () => {
  it("groups functions by container into packages, and by file inside them; externals stay apart", () => {
    expect(grouping.packages.map((pkg) => [pkg.id, pkg.files.length, pkg.changed])).toEqual([
      ["package:internal/notify", 1, 0],
      ["package:internal/payment", 3, 2],
      ["package:internal/refund", 4, 4],
      ["package:internal/storage", 3, 2],
    ]);
    expect(grouping.files).toHaveLength(11);
    expect(grouping.externals.map((node) => node.id)).toEqual([id.gateway]);
    const service = grouping.fileOf.get(id.request)!;
    expect(service).toMatchObject({
      id: "file:internal/refund/service.go",
      path: "internal/refund/service.go",
      container: "internal/refund",
      changed: 1,
      additions: 28,
      deletions: 0,
    });
    expect(grouping.packageOf.get(id.request)?.id).toBe("package:internal/refund");
    const refund = group("package:internal/refund");
    expect([refund.additions, refund.deletions]).toEqual([66, 25]);
  });

  it("keeps a file shared by two packages apart in each", () => {
    const shared = groupFunctions([
      makeFlowNode("a", { container: "p", file: "x.go" }),
      makeFlowNode("b", { container: "p_test", file: "x.go" }),
    ]);
    expect(shared.files.map((file) => file.id)).toEqual(["file:x.go#p", "file:x.go#p_test"]);
  });
});

describe("expansion", () => {
  it("opens and closes one group, leaving the others as they are", () => {
    const refund = group("package:internal/refund");
    const service = group("file:internal/refund/service.go");
    const opened = toggleGroup(toggleGroup(NOTHING_OPEN, refund), service);
    expect(isOpen(opened, refund)).toBe(true);
    expect(isOpen(opened, service)).toBe(true);
    expect(isOpen(opened, group("package:internal/payment"))).toBe(false);
    const closed = toggleGroup(opened, refund);
    expect(isOpen(closed, refund)).toBe(false);
    expect(isOpen(closed, service)).toBe(true);
    expect(isOpen(NOTHING_OPEN, refund)).toBe(false);
  });

  it("opens what holds a function so it is drawn, and nothing at the function level", () => {
    const shown = reveal(NOTHING_OPEN, grouping, "packages", id.request);
    expect([...shown.packages]).toEqual(["package:internal/refund"]);
    expect([...shown.files]).toEqual(["file:internal/refund/service.go"]);
    expect(reveal(shown, grouping, "packages", id.request)).toBe(shown);
    expect([...reveal(NOTHING_OPEN, grouping, "files", id.request).packages]).toEqual([]);
    expect(reveal(NOTHING_OPEN, grouping, "functions", id.request)).toBe(NOTHING_OPEN);
    expect(reveal(NOTHING_OPEN, grouping, "packages", id.gateway)).toBe(NOTHING_OPEN);
  });
});

describe("views", () => {
  it("draws one box per package, linking them by the relations between their functions", () => {
    const view = buildView(graph, grouping, "packages", NOTHING_OPEN);
    expect(view.units.map((unit) => unit.id)).toEqual([
      "package:internal/notify",
      "package:internal/payment",
      "package:internal/refund",
      "package:internal/storage",
      id.gateway,
    ]);
    expect(view.unitOf.get(id.request)).toBe("package:internal/refund");
    expect(view.links.map((link) => [link.id, stateSummary(link)])).toEqual([
      ["package:internal/refund->package:internal/payment", "1 added"],
      [`package:internal/refund->${id.gateway}`, "1 added"],
      ["package:internal/refund->package:internal/storage", "1 added"],
      ["package:internal/payment->package:internal/storage", "1 preserved"],
      ["package:internal/payment->package:internal/notify", "1 added"],
    ]);
    expect(view.isolated.size).toBe(0);
  });

  it("shows an open package's files in place, and an open file's functions in it", () => {
    let expansion = toggleGroup(NOTHING_OPEN, group("package:internal/refund"));
    expansion = toggleGroup(expansion, group("file:internal/refund/service.go"));
    const view = buildView(graph, grouping, "packages", expansion);

    const refund = view.unitById.get("package:internal/refund");
    expect(refund).toMatchObject({ kind: "group", open: true, parent: null });
    expect(view.unitById.get("file:internal/refund/handler.go")).toMatchObject({
      open: false,
      parent: "package:internal/refund",
    });
    expect(view.unitById.get(id.request)).toMatchObject({ kind: "function", parent: "file:internal/refund/service.go" });
    expect(view.unitById.get("package:internal/payment")).toMatchObject({ open: false });
    // Every open group comes before what it holds.
    const order = view.units.map((unit) => unit.id);
    expect(order.indexOf("package:internal/refund")).toBeLessThan(order.indexOf("file:internal/refund/service.go"));
    expect(order.indexOf("file:internal/refund/service.go")).toBeLessThan(order.indexOf(id.request));
    expect(view.links.map((link) => link.id)).toContain(`file:internal/refund/handler.go->${id.request}`);
    expect(view.links.map((link) => link.id)).toContain(`${id.request}->file:internal/refund/validate.go`);
    expect(view.key).toBe("packages\nfile:internal/refund/service.go\npackage:internal/refund");
  });

  it("draws files at the file level and every function at the function level", () => {
    const files = buildView(graph, grouping, "files", NOTHING_OPEN);
    expect(files.units.filter((unit) => unit.kind === "group")).toHaveLength(11);
    expect(files.links).toHaveLength(11);
    const functions = buildView(graph, grouping, "functions", toggleGroup(NOTHING_OPEN, group("package:internal/refund")));
    expect(functions.units).toHaveLength(graph.nodes.length);
    expect(functions.links).toHaveLength(graph.edges.length);
    expect(functions.key).toBe("functions");
  });

  it("finds the boxes with no link in the view", () => {
    const lone = makeFlowNode("lone", { state: "new", file: "internal/app/lone.go" });
    const withLone = makeFlowGraph([...graph.nodes, lone], graph.edges);
    const view = buildView(withLone, groupFunctions(withLone.nodes), "functions", NOTHING_OPEN);
    expect([...view.isolated]).toEqual(["lone"]);
    const byPackage = buildView(withLone, groupFunctions(withLone.nodes), "packages", NOTHING_OPEN);
    expect([...byPackage.isolated]).toEqual(["package:internal/app"]);
  });
});

describe("initial level", () => {
  /** `files` files in one package, file i calling file i+1 `links` times around. */
  function sized(files: number, links: number) {
    const nodes: FlowNode[] = Array.from({ length: files }, (_, n) =>
      makeFlowNode(`f${n}`, { state: "modified", file: `p/f${n}.go`, container: "p" }),
    );
    const edges: FlowEdge[] = [];
    for (let n = 0; n < links; n++) {
      // Distinct pairs of files: n to n+1+k, wrapping, for k rounds.
      const round = Math.floor(n / files);
      edges.push(makeFlowEdge(`f${n % files}`, `f${(n + 1 + round) % files}`));
    }
    return makeFlowGraph(nodes, edges);
  }

  it("opens by file up to 20 files and 40 links between files, by package beyond either", () => {
    expect(FILE_LEVEL_LIMITS).toEqual({ files: 20, links: 40 });
    expect(initialLevel(graph)).toBe("files");
    expect(initialLevel(sized(20, 40))).toBe("files");
    expect(initialLevel(sized(21, 0))).toBe("packages");
    expect(initialLevel(sized(20, 41))).toBe("packages");
  });

  it("counts links between files once per pair, not per relation", () => {
    const repeated = sized(2, 1);
    const many = makeFlowGraph(
      repeated.nodes,
      Array.from({ length: 50 }, (_, n) => makeFlowEdge("f0", "f1", { line: n + 1 })),
    );
    expect(initialLevel(many)).toBe("files");
  });
});

describe("focus", () => {
  const view = buildView(graph, grouping, "files", NOTHING_OPEN);

  it("lights a box with the boxes it links to, through the box drawn for a function", () => {
    expect(litUnits(view, { kind: "file", id: "file:internal/payment/repo.go" })).toEqual(
      new Set([
        "file:internal/payment/repo.go",
        "file:internal/refund/service.go",
        "file:internal/payment/handler.go",
        "file:internal/payment/consumer.go",
        "file:internal/storage/db.go",
      ]),
    );
    expect(litUnits(view, { kind: "node", id: id.get })).toEqual(
      litUnits(view, { kind: "file", id: "file:internal/payment/repo.go" }),
    );
    expect(litUnits(view, null)).toBeNull();
  });

  it("lights the two ends of a link, and every box of a package", () => {
    const link = view.links[0];
    expect(litUnits(view, { kind: "link", id: link.id, at: { x: 0, y: 0 } })).toEqual(new Set([link.from, link.to]));
    expect(litUnits(view, { kind: "package", container: "internal/storage" })).toEqual(
      new Set([
        "file:internal/storage/db.go",
        "file:internal/storage/queries.sql.go",
        "file:internal/storage/refund_repo.go",
      ]),
    );
  });
});

describe("absolute boxes", () => {
  it("adds up the places of the open groups holding a box", () => {
    let expansion = toggleGroup(NOTHING_OPEN, group("package:internal/refund"));
    expansion = toggleGroup(expansion, group("file:internal/refund/service.go"));
    const view = buildView(graph, grouping, "packages", expansion);
    const layout = {
      boxes: new Map([
        ["package:internal/refund", { x: 100, y: 50, width: 400, height: 300 }],
        ["file:internal/refund/service.go", { x: 10, y: 40, width: 200, height: 120 }],
        [id.request, { x: 14, y: 40, width: 148, height: 62 }],
      ]),
      routes: new Map(),
    };
    expect(absoluteBox(view, layout, id.request)).toEqual({ x: 124, y: 130, width: 148, height: 62 });
    expect(absoluteBox(view, layout, "package:internal/refund")).toEqual({ x: 100, y: 50, width: 400, height: 300 });
    expect(absoluteBox(view, layout, id.create)).toBeNull();
  });
});

describe("moved boxes", () => {
  let expansion = toggleGroup(NOTHING_OPEN, group("package:internal/refund"));
  expansion = toggleGroup(expansion, group("file:internal/refund/service.go"));
  const view = buildView(graph, grouping, "packages", expansion);
  const toValidate = `${id.request}->file:internal/refund/validate.go`;
  const route = { points: [{ x: 272, y: 160 }, { x: 300, y: 160 }, { x: 300, y: 200 }, { x: 330, y: 200 }], label: { x: 300, y: 180 } };
  const layout = {
    boxes: new Map([
      ["package:internal/refund", { x: 100, y: 50, width: 400, height: 300 }],
      ["file:internal/refund/service.go", { x: 10, y: 40, width: 200, height: 120 }],
      ["file:internal/refund/validate.go", { x: 230, y: 130, width: 160, height: 62 }],
      [id.request, { x: 14, y: 40, width: 148, height: 62 }],
    ]),
    routes: new Map([[toValidate, route]]),
  };

  it("leaves the layout as it is when nothing moved", () => {
    expect(view.links.some((link) => link.id === toValidate)).toBe(true);
    expect(moveBoxes(view, layout, new Map())).toBe(layout);
  });

  it("places a moved box where it was dropped and drops the routes of its links", () => {
    const moved = moveBoxes(view, layout, new Map([[id.request, { x: 20, y: 70 }]]));
    expect(moved.boxes.get(id.request)).toEqual({ x: 20, y: 70, width: 148, height: 62 });
    expect(moved.routes.has(toValidate)).toBe(false);
  });

  it("shifts the routes of links whose ends moved together, with the group holding them", () => {
    const moved = moveBoxes(view, layout, new Map([["package:internal/refund", { x: 140, y: 20 }]]));
    expect(moved.routes.get(toValidate)).toEqual({
      points: route.points.map(({ x, y }) => ({ x: x + 40, y: y - 30 })),
      label: { x: 340, y: 150 },
    });
    expect(absoluteBox(view, moved, id.request)).toEqual({ x: 164, y: 100, width: 148, height: 62 });
  });
});
