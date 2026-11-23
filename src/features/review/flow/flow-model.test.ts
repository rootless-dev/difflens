import { describe, expect, it } from "vitest";

import { buildView, groupFunctions, NOTHING_OPEN, toggleGroup } from "@/features/review/flow/flow-groups";
import {
  EXTERNAL_HEIGHT,
  GROUP_HEIGHT,
  groupDetail,
  groupSize,
  ISOLATED_EXTRA,
  linkGeometry,
  routeGeometry,
  NODE_HEIGHT,
  nodeDetail,
  nodeSize,
  shortContainer,
  toFlowEdges,
  toFlowNodes,
  unitName,
} from "@/features/review/flow/flow-model";
import { makeFlowNode, REFUND_IDS as id, refundFlowGraph } from "@/test/flow-fixtures";

const graph = refundFlowGraph();
const grouping = groupFunctions(graph.nodes);
const byId = (nodeId: string) => graph.nodes.find((node) => node.id === nodeId)!;

describe("node text and size", () => {
  it("shows the last segment of a container", () => {
    expect(shortContainer("internal/refund")).toBe("refund");
    expect(shortContainer("github.com/acme/gateway")).toBe("gateway");
    expect(shortContainer("com.acme.refund")).toBe("com.acme.refund");
    expect(shortContainer("pkg/")).toBe("pkg");
  });

  it("puts the first entry point after the package, counting the others", () => {
    expect(nodeDetail(byId(id.create))).toBe("refund · POST /refunds");
    expect(nodeDetail(byId(id.request))).toBe("refund");
    const twoRoutes = makeFlowNode("x", {
      container: "internal/api",
      entryPoints: [
        { kind: "http_route", label: "GET /a" },
        { kind: "http_route", label: "GET /b" },
      ],
    });
    expect(nodeDetail(twoRoutes)).toBe("api · GET /a +1");
  });

  it("sizes nodes from their text within bounds, external ones lower, isolated ones taller", () => {
    expect(nodeSize(byId(id.validate))).toEqual({ width: 148, height: NODE_HEIGHT });
    expect(nodeSize(makeFlowNode("x", { name: "VeryLongReceiverName.AndAnEvenLongerMethodNameThanThat" }))).toEqual({
      width: 260,
      height: NODE_HEIGHT,
    });
    expect(nodeSize(byId(id.gateway))).toEqual({ width: 178, height: EXTERNAL_HEIGHT });
    expect(nodeSize(makeFlowNode("external:fmt", { name: "fmt", state: "external", externalFunctions: 1 }))).toEqual({
      width: 124,
      height: EXTERNAL_HEIGHT,
    });
    expect(nodeSize(byId(id.validate), true)).toEqual({ width: 218, height: NODE_HEIGHT + ISOLATED_EXTRA });
  });

  it("names an external package and how many of its functions are called", () => {
    expect(nodeDetail(byId(id.gateway))).toBe("2 functions");
    expect(nodeDetail(makeFlowNode("external:fmt", { name: "fmt", state: "external", externalFunctions: 1 }))).toBe(
      "1 function",
    );
  });
});

describe("group text and size", () => {
  it("counts a package's files and changed functions, and a file's, with the unchanged ones apart", () => {
    expect(groupDetail(grouping.groups.get("package:internal/refund")!)).toBe("4 files · 4 changed functions");
    expect(groupDetail(grouping.groups.get("package:internal/payment")!)).toBe("3 files · 2 changed functions · 1 unchanged");
    expect(groupDetail(grouping.groups.get("file:internal/refund/service.go")!)).toBe("1 changed function");
  });

  it("sizes a group from its name within bounds", () => {
    // "1 file · 0 changed functions · 1 unchanged" is the longest line.
    expect(groupSize(grouping.groups.get("package:internal/notify")!)).toEqual({ width: 278, height: GROUP_HEIGHT });
    expect(groupSize(grouping.groups.get("file:internal/storage/queries.sql.go")!).width).toBe(284);
    const deep = groupFunctions([makeFlowNode("x", { file: "internal/platform/httpserver/middleware/authentication.go" })]);
    expect(groupSize(deep.files[0]).width).toBe(300);
  });
});

describe("canvas nodes and edges", () => {
  const expansion = toggleGroup(NOTHING_OPEN, grouping.groups.get("file:internal/refund/service.go")!);
  const view = buildView(graph, grouping, "files", expansion);
  const toValidate = `${id.request}->file:internal/refund/validate.go`;
  const route = { points: [{ x: 362, y: 101 }, { x: 650, y: 101 }], label: { x: 506, y: 92 } };
  const layout = {
    boxes: new Map([
      ["file:internal/refund/service.go", { x: 200, y: 30, width: 400, height: 140 }],
      [id.request, { x: 14, y: 40, width: 148, height: 62 }],
    ]),
    routes: new Map([[toValidate, route]]),
  };

  it("maps closed groups, open ones and functions to their node types, nested in what holds them", () => {
    const nodes = toFlowNodes(view, layout, null);
    const service = nodes.find((node) => node.id === "file:internal/refund/service.go")!;
    expect(service).toMatchObject({ type: "container", position: { x: 200, y: 30 }, width: 400, height: 140 });
    const request = nodes.find((node) => node.id === id.request)!;
    // Measured up front: React Flow keeps where links attach to a box only when the box passed in anew says so.
    expect(request).toMatchObject({
      type: "function",
      parentId: "file:internal/refund/service.go",
      extent: "parent",
      position: { x: 14, y: 40 },
      measured: { width: 148, height: 62 },
    });
    const handler = nodes.find((node) => node.id === "file:internal/refund/handler.go")!;
    expect(handler).toMatchObject({ type: "cluster", width: 250, height: GROUP_HEIGHT });
    expect(handler.parentId).toBeUndefined();
  });

  it("dims what the focus does not touch, and marks the selected box", () => {
    const nodes = toFlowNodes(view, layout, { kind: "node", id: id.request });
    const data = (nodeId: string) => nodes.find((node) => node.id === nodeId)!.data;
    expect(data(id.request)).toMatchObject({ selected: true, dimmed: false });
    expect(data("file:internal/refund/validate.go")).toMatchObject({ dimmed: false });
    expect(data("file:internal/notify/notifier.go")).toMatchObject({ dimmed: true });
    expect(data("file:internal/refund/service.go")).toMatchObject({ dimmed: false });

    const edges = toFlowEdges(view, layout, { kind: "node", id: id.request });
    const edge = (edgeId: string) => edges.find((candidate) => candidate.id === edgeId)!;
    expect(edge(`${id.request}->file:internal/refund/validate.go`).data).toMatchObject({ dimmed: false });
    expect(edge("file:internal/payment/consumer.go->file:internal/notify/notifier.go").data).toMatchObject({
      dimmed: true,
    });
  });

  it("maps every link to an edge between its boxes, added ones on top, with their ends named", () => {
    const edges = toFlowEdges(view, layout, null);
    expect(edges).toHaveLength(view.links.length);
    const validate = edges.find((edge) => edge.target === "file:internal/refund/validate.go")!;
    expect(validate).toMatchObject({ id: toValidate, type: "link", source: id.request, zIndex: 1 });
    expect(validate.data?.route).toBe(route);
    expect(edges.find((edge) => edge.id !== toValidate)?.data?.route).toBeNull();
    expect(validate.data?.ends).toBe("Service.Request → validate.go");
    expect(validate.data?.title).toContain("Service.Request → Validate, line 49: calls, only on some paths, added.");
    expect(edges.find((edge) => edge.target === "file:internal/storage/db.go")?.zIndex).toBe(0);
    expect(unitName(view, id.gateway)).toBe("github.com/acme/gateway");
  });
});

describe("link geometry", () => {
  it("curves from the source's right to the target's left with the label half way", () => {
    const { path, label } = linkGeometry(0, 0, 200, 100, false);
    expect(path).toBe("M0,0 C100,0 100,100 200,100");
    expect(label).toEqual({ x: 100, y: 50 });
  });

  it("loops a link to itself over the node", () => {
    expect(linkGeometry(150, 30, 0, 30, true).path).toBe("M150,30 C198,-40 -48,-40 0,30");
  });

  it("draws a routed link in straight runs with rounded bends, its label where the layout left room", () => {
    const route = {
      points: [{ x: 0, y: 0 }, { x: 40, y: 0 }, { x: 40, y: 100 }, { x: 44, y: 100 }],
      label: { x: 60, y: 50 },
    };
    expect(routeGeometry(route)).toEqual({
      path: "M0,0 L34,0 Q40,0 40,6 L40,98 Q40,100 42,100 L44,100",
      label: { x: 60, y: 50 },
    });
  });

  it("puts the label of a routed link half way along it when the layout placed none", () => {
    const route = { points: [{ x: 0, y: 0 }, { x: 40, y: 0 }, { x: 40, y: 80 }], label: null };
    expect(routeGeometry(route).label).toEqual({ x: 40, y: 20 });
  });
});
