// Layered left-to-right layout of a view with ELK, open groups laid out around the boxes they hold. ELK takes about
// half a second on 300 functions, long enough to freeze the screen, so it runs in a web worker.

import type { ELK, ElkExtendedEdge, ElkNode } from "elkjs/lib/elk-api";

import type { Box, FlowView, Layout, Route, Unit } from "@/features/review/flow/flow-groups";
import { type Link, linkLabel } from "@/features/review/flow/flow-links";
import { CONTAINER_HEADER, CONTAINER_PADDING, linkLabelSize, unitSize } from "@/features/review/flow/flow-model";

const LAYERED = {
  "elk.algorithm": "layered",
  "elk.direction": "RIGHT",
  "elk.layered.nodePlacement.strategy": "BRANDES_KOEPF",
};

// Keeps the order given below, so a caller's callees read top to bottom in the order of the calls. Only on the
// root: ELK 0.12 fails on it inside open groups, and on some graphs with open groups at all (then it is dropped).
const MODEL_ORDER = { "elk.layered.considerModelOrder.strategy": "NODES_AND_EDGES" };

const ROOT_OPTIONS = {
  ...LAYERED,
  // Links between boxes of different open groups are laid out with the whole canvas.
  "elk.hierarchyHandling": "INCLUDE_CHILDREN",
  // Lines go around the boxes, in straight runs, and every route comes back on the whole canvas.
  "elk.edgeRouting": "ORTHOGONAL",
  "elk.json.edgeCoords": "ROOT",
  "elk.spacing.edgeNode": "14",
  "elk.spacing.edgeEdge": "8",
  "elk.layered.spacing.edgeNodeBetweenLayers": "14",
  "elk.layered.spacing.edgeEdgeBetweenLayers": "8",
  "elk.layered.spacing.nodeNodeBetweenLayers": "72",
  "elk.spacing.nodeNode": "22",
  "elk.spacing.componentComponent": "40",
};

const CONTAINER_OPTIONS = {
  ...LAYERED,
  "elk.alignment": "LEFT",
  "elk.layered.spacing.nodeNodeBetweenLayers": "56",
  "elk.spacing.nodeNode": "16",
  "elk.spacing.componentComponent": "24",
  "elk.padding": `[top=${CONTAINER_HEADER},left=${CONTAINER_PADDING},bottom=${CONTAINER_PADDING},right=${CONTAINER_PADDING}]`,
  "elk.nodeSize.constraints": "MINIMUM_SIZE",
};

// Boxes of one column share their left edge rather than their centre.
const NODE_OPTIONS = { "elk.alignment": "LEFT" };
const FIRST_LAYER = { ...NODE_OPTIONS, "elk.layered.layering.layerConstraint": "FIRST" };

/**
 * Boxes in reading order: breadth first from the boxes holding entry points, each box's targets by the position of
 * their first relation, then whatever is not reached, in the view's order.
 */
export function readingOrder(view: FlowView): string[] {
  const targets = new Map<string, string[]>();
  const links = [...view.links].sort((a, b) => firstPosition(a) - firstPosition(b));
  for (const link of links) {
    const list = targets.get(link.from);
    if (list) list.push(link.to);
    else targets.set(link.from, [link.to]);
  }
  const seen = new Set<string>();
  // Doubles as the breadth-first queue: everything visited is appended once, in order.
  const order: string[] = [];
  const visit = (id: string | undefined) => {
    if (id === undefined || seen.has(id) || !view.unitById.has(id)) return;
    seen.add(id);
    order.push(id);
  };
  for (const entry of view.graph.entryPoints) visit(view.unitOf.get(entry));
  for (let i = 0; i < order.length; i++) {
    for (const target of targets.get(order[i]) ?? []) visit(target);
  }
  for (const unit of view.units) visit(unit.id);
  return order;
}

/** The ELK graph of a view: sized boxes in reading order, open groups holding theirs, one edge per link. */
export function layoutInput(view: FlowView, modelOrder = true): ElkNode {
  const rank = new Map(readingOrder(view).map((id, index) => [id, index]));
  // An open group reads where the first box it holds does.
  for (const unit of [...view.units].reverse()) {
    if (unit.parent === null) continue;
    const own = rank.get(unit.id) ?? Infinity;
    if (own < (rank.get(unit.parent) ?? Infinity)) rank.set(unit.parent, own);
  }
  const targeted = new Set(view.links.filter((link) => link.from !== link.to).map((link) => link.to));
  const entries = new Set(view.graph.entryPoints.map((id) => view.unitOf.get(id)));
  const children = new Map<string | null, Unit[]>();
  for (const unit of view.units) {
    const siblings = children.get(unit.parent);
    if (siblings) siblings.push(unit);
    else children.set(unit.parent, [unit]);
  }
  const build = (parent: string | null): ElkNode[] =>
    [...(children.get(parent) ?? [])]
      .sort((a, b) => (rank.get(a.id) ?? 0) - (rank.get(b.id) ?? 0))
      .map((unit): ElkNode => {
        if (unit.kind === "group" && unit.open) {
          // At least as wide as the group's closed box, so its header reads.
          const minimum = `(${unitSize(unit, false).width + CONTAINER_PADDING * 2},0)`;
          return {
            id: unit.id,
            layoutOptions: { ...CONTAINER_OPTIONS, "elk.nodeSize.minimum": minimum },
            children: build(unit.id),
          };
        }
        // Entry points nothing calls start the canvas, in its first column; ELK would otherwise pull them right.
        const first = parent === null && entries.has(unit.id) && !targeted.has(unit.id);
        return {
          id: unit.id,
          ...unitSize(unit, view.isolated.has(unit.id), view.explained.has(unit.id)),
          layoutOptions: first ? FIRST_LAYER : NODE_OPTIONS,
        };
      });
  const edges: ElkExtendedEdge[] = view.links
    // Links to itself are drawn as a loop on the box and do not affect the layout.
    .filter((link) => link.from !== link.to)
    .sort((a, b) => (rank.get(a.from) ?? 0) - (rank.get(b.from) ?? 0) || firstPosition(a) - firstPosition(b))
    // The label is sized so the layout leaves room for it beside the line, clear of boxes and other labels. ELK
    // leaves a label without text unplaced, at 0,0.
    .map((link) => ({
      id: link.id,
      sources: [link.from],
      targets: [link.to],
      labels: [{ id: `${link.id}:label`, text: linkLabel(link), ...linkLabelSize(link) }],
    }));
  return {
    id: "root",
    layoutOptions: modelOrder ? { ...ROOT_OPTIONS, ...MODEL_ORDER } : ROOT_OPTIONS,
    children: build(null),
    edges,
  };
}

/** Position of the link's first relation among its caller's, so a caller's targets keep the order of its code. */
const firstPosition = (link: Link) => Math.min(...link.relations.map((relation) => relation.position));

/** Every box of an ELK result, placed relative to the box holding it, and every link's route on the whole canvas. */
export function layoutOf(result: ElkNode): Layout {
  const boxes = new Map<string, Box>();
  const visit = (node: ElkNode) => {
    for (const child of node.children ?? []) {
      boxes.set(child.id, { x: child.x ?? 0, y: child.y ?? 0, width: child.width ?? 0, height: child.height ?? 0 });
      visit(child);
    }
  };
  visit(result);
  const routes = new Map<string, Route>();
  for (const edge of result.edges ?? []) {
    const section = edge.sections?.[0];
    if (!section) continue;
    const label = edge.labels?.[0];
    routes.set(edge.id, {
      points: [section.startPoint, ...(section.bendPoints ?? []), section.endPoint].map(({ x, y }) => ({ x, y })),
      label:
        label?.x === undefined || label.y === undefined
          ? null
          : { x: label.x + (label.width ?? 0) / 2, y: label.y + (label.height ?? 0) / 2 },
    });
  }
  return { boxes, routes };
}

let engine: Promise<ELK> | null = null;

/** Replaces the web worker ELK, where there are no workers (tests use the main-thread bundle). */
export function setLayoutEngine(elk: ELK): void {
  engine = Promise.resolve(elk);
}

function elk(): Promise<ELK> {
  engine ??= Promise.all([import("elkjs/lib/elk-api.js"), import("elkjs/lib/elk-worker.min.js?url")]).then(
    ([{ default: Elk }, { default: workerUrl }]) => new Elk({ workerUrl }),
  );
  return engine;
}

// One layout per graph and view key, so going back to a level or closing a group again does not lay it out again.
const layouts = new WeakMap<object, Map<string, Promise<Layout>>>();

export function layoutFlow(view: FlowView): Promise<Layout> {
  let byKey = layouts.get(view.graph);
  if (!byKey) layouts.set(view.graph, (byKey = new Map()));
  let layout = byKey.get(view.key);
  if (!layout) {
    const cache = byKey;
    layout = elk()
      .then((instance) => instance.layout(layoutInput(view)).catch(() => instance.layout(layoutInput(view, false))))
      .then(layoutOf);
    // A failed layout can be tried again.
    layout.catch(() => cache.delete(view.key));
    cache.set(view.key, layout);
  }
  return layout;
}
