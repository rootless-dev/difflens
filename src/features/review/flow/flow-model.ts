// Pure mapping from a view of the flow graph to what the canvas draws: sized function and group boxes, open groups
// holding their boxes, one line per link, and which of them stay lit while something is focused.

import type { Edge, Node } from "@xyflow/react";

import { baseName } from "@/features/review/file-tree";
import {
  type Box,
  type FlowView,
  type Focus,
  type Group,
  type Layout,
  linkLit,
  litUnits,
  type Point,
  type Route,
  type Unit,
} from "@/features/review/flow/flow-groups";
import { type Link, linkLabel, linkTitle } from "@/features/review/flow/flow-links";
import { i18n } from "@/i18n";
import type { FlowNode, FlowNodeState } from "@/lib/api";

export interface StateLook {
  /** Background class of the state bar and of legend and list dots. */
  dot: string;
}

export const NODE_STATE: Record<FlowNodeState, StateLook> = {
  new: { dot: "bg-add" },
  modified: { dot: "bg-mod" },
  deleted: { dot: "bg-del" },
  untouched: { dot: "bg-faint" },
  external: { dot: "bg-faint" },
};

/** A function's state as the legend and the popover name it: "New", "External package". */
export const nodeStateLabel = (state: FlowNodeState) => i18n.t(`flow.nodeState.${state}`);

const CHANGED: ReadonlySet<FlowNodeState> = new Set(["new", "modified", "deleted"]);

export const isChanged = (node: FlowNode) => CHANGED.has(node.state);

/** Said of a box with no link in the current view. Never suggests the code is unused: another view may link it. */
export const isolatedNote = () => i18n.t("flow.isolatedNote");

// Boxes. Sizes are fixed before layout so ELK and the canvas agree without measuring the DOM. They are computed
// from the text in the language shown, so a longer translation widens the box; the layout runs again on a language
// switch (see FlowWorkspace).
export const NODE_HEIGHT = 62;
export const EXTERNAL_HEIGHT = 44;
export const GROUP_HEIGHT = 62;
/** Added under a box with no link, for the line that says so. */
export const ISOLATED_EXTRA = 16;
/** Added under an explained function, for the two lines of its explanation it shows. */
export const EXPLAINED_EXTRA = 32;
/** Room after a function's name for its Explain button. */
const EXPLAIN_BUTTON_ROOM = 18;
/** Room above the boxes an open group holds, for its header. */
export const CONTAINER_HEADER = 40;
export const CONTAINER_PADDING = 14;
const MIN_WIDTH = 148;
const MAX_WIDTH = 260;
const GROUP_MIN_WIDTH = 168;
const GROUP_MAX_WIDTH = 300;
const EXTERNAL_MIN_WIDTH = 124;
// IBM Plex Mono advances 0.6em: 6.9px at the 11.5px of function names, 6.6px at 11px.
const NAME_CHAR = 6.9;
const EXTERNAL_NAME_CHAR = 6.6;
// IBM Plex Sans at 11px averages about 6px a character.
const DETAIL_CHAR = 6;
const PADDING = 26;
// The icon before a group's name and the toggle after it.
const GROUP_CHROME = 44;

/** Last segment of a slash-separated container, as the node shows it: "internal/refund" reads "refund". */
export function shortContainer(container: string): string {
  const trimmed = container.replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) || container;
}

/**
 * Second line of a node: its package, then its first entry point and how many more there are.
 * An external package node, whose name is the package, says how many of its functions are called instead.
 */
export function nodeDetail(node: FlowNode): string {
  if (node.state === "external") return i18n.t("flow.functions", { count: node.externalFunctions });
  const [first, ...rest] = node.entryPoints;
  const parts = [shortContainer(node.container)];
  if (first) parts.push(rest.length > 0 ? `${first.label} +${rest.length}` : first.label);
  return parts.join(" · ");
}

export function nodeSize(node: FlowNode, isolated = false, explained = false): { width: number; height: number } {
  const extra = (isolated ? ISOLATED_EXTRA : 0) + (explained && node.state !== "external" ? EXPLAINED_EXTRA : 0);
  if (node.state === "external") {
    const text = Math.max(node.name.length * EXTERNAL_NAME_CHAR, nodeDetail(node).length * DETAIL_CHAR);
    return {
      width: clamp(Math.ceil(text + PADDING), EXTERNAL_MIN_WIDTH, MAX_WIDTH),
      height: EXTERNAL_HEIGHT + extra,
    };
  }
  const text = Math.max(
    node.name.length * NAME_CHAR + 14 + EXPLAIN_BUTTON_ROOM,
    nodeDetail(node).length * DETAIL_CHAR,
    isolated ? isolatedNote().length * DETAIL_CHAR : 0,
  );
  return { width: clamp(Math.ceil(text + PADDING), MIN_WIDTH, MAX_WIDTH), height: NODE_HEIGHT + extra };
}

/** Name a group box shows: the package, or the file's path. */
export const groupName = (group: Group) => (group.kind === "package" ? group.container : group.path);

/** "3 files · 12 changed functions", "5 changed functions · 2 unchanged". */
export function groupDetail(group: Group): string {
  const functions =
    group.kind === "package"
      ? group.files.reduce((sum, file) => sum + file.functions.length, 0)
      : group.functions.length;
  const parts = group.kind === "package" ? [i18n.t("flow.files", { count: group.files.length })] : [];
  parts.push(i18n.t("flow.changedFunctions", { count: group.changed }));
  if (functions > group.changed) parts.push(i18n.t("flow.unchanged", { count: functions - group.changed }));
  return parts.join(" · ");
}

export function groupSize(group: Group, isolated = false): { width: number; height: number } {
  const text = Math.max(
    groupName(group).length * NAME_CHAR + GROUP_CHROME,
    groupDetail(group).length * DETAIL_CHAR,
    isolated ? isolatedNote().length * DETAIL_CHAR : 0,
  );
  return {
    width: clamp(Math.ceil(text + PADDING), GROUP_MIN_WIDTH, GROUP_MAX_WIDTH),
    height: GROUP_HEIGHT + (isolated ? ISOLATED_EXTRA : 0),
  };
}

// A link's label: IBM Plex Sans at 10px, medium, averages about 5.7px a character; padding, border and the cross
// of a removed link around it. The label never grows past LABEL_MAX_WIDTH, it truncates.
const LABEL_CHAR = 5.7;
const LABEL_CHROME = 16;
const LABEL_CROSS = 10;
const LABEL_HEIGHT = 16;
const LABEL_MAX_WIDTH = 260;

/** Size of a link's label, so the layout can leave room for it. */
export function linkLabelSize(link: Link): { width: number; height: number } {
  const text = linkLabel(link).length * LABEL_CHAR + LABEL_CHROME + (link.state === "removed" ? LABEL_CROSS : 0);
  return { width: Math.min(LABEL_MAX_WIDTH, Math.ceil(text)), height: LABEL_HEIGHT };
}

/** Size of a box that does not hold others. An open group is sized by the layout around what it holds. */
export function unitSize(unit: Unit, isolated: boolean, explained = false): { width: number; height: number } {
  return unit.kind === "function" ? nodeSize(unit.node, isolated, explained) : groupSize(unit.group, isolated);
}

export interface FunctionNodeData extends Record<string, unknown> {
  node: FlowNode;
  detail: string;
  /** The popover of this function is open. */
  selected: boolean;
  dimmed: boolean;
  isolated: boolean;
  /** Laid out with room for the lines of its explanation. */
  explained: boolean;
}

export interface GroupNodeData extends Record<string, unknown> {
  group: Group;
  detail: string;
  /** The popover of this file is open. */
  selected: boolean;
  dimmed: boolean;
  isolated: boolean;
}

export interface ContainerNodeData extends Record<string, unknown> {
  group: Group;
  detail: string;
  dimmed: boolean;
}

export type FunctionFlowNode = Node<FunctionNodeData, "function">;
// Not "group", which React Flow uses for a node type of its own.
export type GroupFlowNode = Node<GroupNodeData, "cluster">;
export type ContainerFlowNode = Node<ContainerNodeData, "container">;
export type CanvasNode = FunctionFlowNode | GroupFlowNode | ContainerFlowNode;

export interface LinkEdgeData extends Record<string, unknown> {
  link: Link;
  /** "Handler.Create → Service.Request", "service.go → repo.go". */
  ends: string;
  /** Tooltip: the counts, then each relation. */
  title: string;
  dimmed: boolean;
  /** Its list is open. */
  selected: boolean;
  /** Its line around the boxes; without one it is drawn as a curve between its ends. */
  route: Route | null;
}

export type LinkFlowEdge = Edge<LinkEdgeData, "link">;

/** The short name of a function, by id, for lists and tooltips. */
export function functionNames(view: FlowView): (id: string) => string {
  const names = new Map(view.graph.nodes.map((node) => [node.id, node.name]));
  return (id) => names.get(id) ?? id;
}

/** What a box is called: the function's short name, the package, or the file's name. */
export function unitName(view: FlowView, id: string): string {
  const unit = view.unitById.get(id);
  if (!unit) return id;
  if (unit.kind === "function") return unit.node.name;
  return unit.group.kind === "package" ? unit.group.container : baseName(unit.group.path) || unit.group.path;
}

export function toFlowNodes(view: FlowView, layout: Layout, focus: Focus | null): CanvasNode[] {
  const lit = litUnits(view, focus);
  return view.units.map((unit): CanvasNode => {
    const isolated = view.isolated.has(unit.id);
    const explained = view.explained.has(unit.id);
    const box: Box = layout.boxes.get(unit.id) ?? { x: 0, y: 0, ...unitSize(unit, isolated, explained) };
    const base = {
      id: unit.id,
      position: { x: box.x, y: box.y },
      width: box.width,
      height: box.height,
      // The size is known before drawing. Given as measured, React Flow keeps where a box's links attach when the
      // box is passed in anew (a drag passes them all in anew); without it the links of the box vanish.
      measured: { width: box.width, height: box.height },
      // A box dragged inside an open group stays in it.
      ...(unit.parent !== null ? { parentId: unit.parent, extent: "parent" as const } : {}),
    };
    const dimmed = lit !== null && !lit.has(unit.id);
    if (unit.kind === "function") {
      return {
        ...base,
        type: "function",
        data: {
          node: unit.node,
          detail: nodeDetail(unit.node),
          selected: focus?.kind === "node" && focus.id === unit.id,
          dimmed,
          isolated,
          explained,
        },
      };
    }
    if (unit.open) {
      // An open group is a backdrop: it fades only when a package other than its own is focused.
      return {
        ...base,
        type: "container",
        data: { group: unit.group, detail: groupDetail(unit.group), dimmed: focus?.kind === "package" && dimmed },
      };
    }
    return {
      ...base,
      type: "cluster",
      data: {
        group: unit.group,
        detail: groupDetail(unit.group),
        selected: focus?.kind === "file" && focus.id === unit.id,
        dimmed,
        isolated,
      },
    };
  });
}

export function toFlowEdges(view: FlowView, layout: Layout, focus: Focus | null): LinkFlowEdge[] {
  const lit = litUnits(view, focus);
  const name = functionNames(view);
  return view.links.map((link) => ({
    id: link.id,
    type: "link",
    source: link.from,
    target: link.to,
    // Added links are drawn above the rest so their highlight is never hidden.
    zIndex: link.state === "added" ? 1 : 0,
    data: {
      link,
      ends: `${unitName(view, link.from)} → ${unitName(view, link.to)}`,
      title: linkTitle(link, name),
      dimmed: focus !== null && lit !== null && !linkLit(link, view, focus, lit),
      selected: focus?.kind === "link" && focus.id === link.id,
      route: layout.routes.get(link.id) ?? null,
    },
  }));
}

// Bends of a routed line are rounded off by up to this much, less where the runs beside them are short.
const BEND_RADIUS = 6;

/** A routed line as an SVG path: straight runs through its points, each bend rounded off. */
export function routeGeometry(route: Route): LinkGeometry {
  const points = route.points;
  if (points.length === 0) return { path: "", label: route.label ?? { x: 0, y: 0 } };
  const parts = [`M${round(points[0].x)},${round(points[0].y)}`];
  for (let i = 1; i < points.length - 1; i++) {
    const [before, at, after] = [points[i - 1], points[i], points[i + 1]];
    const radius = Math.min(BEND_RADIUS, distance(before, at) / 2, distance(at, after) / 2);
    const start = towards(at, before, radius);
    const end = towards(at, after, radius);
    parts.push(`L${round(start.x)},${round(start.y)}`, `Q${round(at.x)},${round(at.y)} ${round(end.x)},${round(end.y)}`);
  }
  const last = points[points.length - 1];
  parts.push(`L${round(last.x)},${round(last.y)}`);
  return { path: parts.join(" "), label: route.label ?? halfWay(points) };
}

/** The point half way along a line through `points`. */
function halfWay(points: readonly Point[]): Point {
  const lengths = points.slice(1).map((point, index) => distance(points[index], point));
  let left = lengths.reduce((sum, length) => sum + length, 0) / 2;
  for (let i = 0; i < lengths.length; i++) {
    if (left <= lengths[i] && lengths[i] > 0) return towards(points[i], points[i + 1], left);
    left -= lengths[i];
  }
  return points[points.length - 1];
}

const distance = (a: Point, b: Point) => Math.hypot(b.x - a.x, b.y - a.y);

/** The point `length` from `from` on the way to `to`. */
function towards(from: Point, to: Point, length: number): Point {
  const total = distance(from, to);
  if (total === 0) return from;
  return { x: from.x + ((to.x - from.x) * length) / total, y: from.y + ((to.y - from.y) * length) / total };
}

const round = (value: number) => Math.round(value * 10) / 10;

export interface LinkGeometry {
  path: string;
  /** Centre of the label, half way along the line. */
  label: { x: number; y: number };
}

/** A curve from the source's right side to the target's left side. A link to itself loops over the node. */
export function linkGeometry(sx: number, sy: number, tx: number, ty: number, selfLoop: boolean): LinkGeometry {
  const [c1, c2] = selfLoop
    ? [
        { x: sx + 48, y: sy - 70 },
        { x: tx - 48, y: ty - 70 },
      ]
    : (() => {
        const reach = Math.max(Math.abs(tx - sx) / 2, 48);
        return [
          { x: sx + reach, y: sy },
          { x: tx - reach, y: ty },
        ];
      })();
  const path = `M${sx},${sy} C${c1.x},${c1.y} ${c2.x},${c2.y} ${tx},${ty}`;
  const t = 0.5;
  const u = 1 - t;
  const at = (p0: number, p1: number, p2: number, p3: number) =>
    u * u * u * p0 + 3 * u * u * t * p1 + 3 * u * t * t * p2 + t * t * t * p3;
  return { path, label: { x: at(sx, c1.x, c2.x, tx), y: at(sy, c1.y, c2.y, ty) } };
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}
