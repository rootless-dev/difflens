// The graph's functions grouped into files and packages, the level the canvas opens at, which groups are open,
// and the view that results: the boxes drawn, nested as they are open, and the links between them. Pure.

import { aggregateLinks, type Link } from "@/features/review/flow/flow-links";
import type { FlowGraph, FlowNode } from "@/lib/api";

export type Level = "packages" | "files" | "functions";

export const LEVELS: readonly Level[] = ["packages", "files", "functions"];

const CHANGED = new Set(["new", "modified", "deleted"]);

export interface FileGroup {
  kind: "file";
  /** "file:<path>", with "#<container>" when two packages share the path. */
  id: string;
  path: string;
  container: string;
  /** Its functions, in the graph's order. */
  functions: FlowNode[];
  /** New, modified and deleted functions in it. */
  changed: number;
  additions: number;
  deletions: number;
}

export interface PackageGroup {
  kind: "package";
  /** "package:<container>". */
  id: string;
  container: string;
  /** By path. */
  files: FileGroup[];
  changed: number;
  additions: number;
  deletions: number;
}

export type Group = FileGroup | PackageGroup;

export interface Grouping {
  /** By container. */
  packages: PackageGroup[];
  /** Every file, by path. */
  files: FileGroup[];
  /** External package nodes: one box each at every level, never grouped. */
  externals: FlowNode[];
  /** The file and the package of each function, by function id. */
  fileOf: ReadonlyMap<string, FileGroup>;
  packageOf: ReadonlyMap<string, PackageGroup>;
  /** Every group, by id. */
  groups: ReadonlyMap<string, Group>;
}

/** Groups functions by `FlowNode.container` into packages and, inside them, by `FlowNode.file` into files. */
export function groupFunctions(nodes: readonly FlowNode[]): Grouping {
  const byContainer = new Map<string, Map<string, FlowNode[]>>();
  const externals: FlowNode[] = [];
  const containersOfPath = new Map<string, Set<string>>();
  for (const node of nodes) {
    if (node.state === "external") {
      externals.push(node);
      continue;
    }
    const path = node.file ?? "";
    let files = byContainer.get(node.container);
    if (!files) byContainer.set(node.container, (files = new Map()));
    const list = files.get(path);
    if (list) list.push(node);
    else files.set(path, [node]);
    const containers = containersOfPath.get(path) ?? new Set<string>();
    containers.add(node.container);
    containersOfPath.set(path, containers);
  }

  const fileOf = new Map<string, FileGroup>();
  const packageOf = new Map<string, PackageGroup>();
  const groups = new Map<string, Group>();
  const packages: PackageGroup[] = [];
  const allFiles: FileGroup[] = [];
  for (const container of [...byContainer.keys()].sort()) {
    const files = [...byContainer.get(container)!.entries()]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([path, functions]): FileGroup => {
        const shared = (containersOfPath.get(path)?.size ?? 0) > 1;
        return {
          kind: "file",
          id: `file:${path}${shared ? `#${container}` : ""}`,
          path,
          container,
          functions,
          ...changeOf(functions),
        };
      });
    const group: PackageGroup = {
      kind: "package",
      id: `package:${container}`,
      container,
      files,
      changed: files.reduce((sum, file) => sum + file.changed, 0),
      additions: files.reduce((sum, file) => sum + file.additions, 0),
      deletions: files.reduce((sum, file) => sum + file.deletions, 0),
    };
    packages.push(group);
    groups.set(group.id, group);
    for (const file of files) {
      allFiles.push(file);
      groups.set(file.id, file);
      for (const node of file.functions) {
        fileOf.set(node.id, file);
        packageOf.set(node.id, group);
      }
    }
  }
  allFiles.sort((a, b) => a.path.localeCompare(b.path) || a.container.localeCompare(b.container));
  return { packages, files: allFiles, externals, fileOf, packageOf, groups };
}

function changeOf(functions: readonly FlowNode[]) {
  let changed = 0;
  let additions = 0;
  let deletions = 0;
  for (const node of functions) {
    if (CHANGED.has(node.state)) changed += 1;
    additions += node.additions;
    deletions += node.deletions;
  }
  return { changed, additions, deletions };
}

/** Which groups are open. A group that is not listed is closed. */
export interface Expansion {
  readonly packages: ReadonlySet<string>;
  readonly files: ReadonlySet<string>;
}

export const NOTHING_OPEN: Expansion = { packages: new Set(), files: new Set() };

export function isOpen(expansion: Expansion, group: Group): boolean {
  return (group.kind === "package" ? expansion.packages : expansion.files).has(group.id);
}

/** Opens a closed group or closes an open one; the other groups stay as they are. */
export function toggleGroup(expansion: Expansion, group: Group): Expansion {
  const key = group.kind === "package" ? "packages" : "files";
  const next = new Set(expansion[key]);
  if (next.has(group.id)) next.delete(group.id);
  else next.add(group.id);
  return { ...expansion, [key]: next };
}

/** Opens what holds a function at this level so it is drawn as itself; the same object when nothing needs opening. */
export function reveal(expansion: Expansion, grouping: Grouping, level: Level, functionId: string): Expansion {
  const file = grouping.fileOf.get(functionId);
  const pkg = grouping.packageOf.get(functionId);
  if (level === "functions" || !file || !pkg) return expansion;
  const needsPackage = level === "packages" && !expansion.packages.has(pkg.id);
  const needsFile = !expansion.files.has(file.id);
  if (!needsPackage && !needsFile) return expansion;
  return {
    packages: needsPackage ? new Set([...expansion.packages, pkg.id]) : expansion.packages,
    files: needsFile ? new Set([...expansion.files, file.id]) : expansion.files,
  };
}

/** One box of the view. Open groups hold other boxes and are never the end of a link. */
export type Unit =
  | { kind: "function"; id: string; node: FlowNode; parent: string | null }
  | { kind: "group"; id: string; group: Group; open: boolean; parent: string | null };

export interface FlowView {
  graph: FlowGraph;
  level: Level;
  /** Level and open groups, the same for two views that draw the same boxes of one graph. */
  key: string;
  /** Every box, each open group before the boxes it holds. */
  units: Unit[];
  unitById: ReadonlyMap<string, Unit>;
  /** The box drawn for each function: itself, or the closed group holding it. */
  unitOf: ReadonlyMap<string, string>;
  links: Link[];
  /** Boxes (not open groups) with no link in this view. */
  isolated: ReadonlySet<string>;
  /** Function boxes drawn with the lines of their explanation, so taller. */
  explained: ReadonlySet<string>;
}

const NONE_EXPLAINED: ReadonlySet<string> = new Set();

/**
 * What the canvas draws for `graph` at `level` with `expansion` open:
 * - functions: every function on its own;
 * - files: one box per file, an open one holding its functions;
 * - packages: one box per package, an open one holding its files, open files holding their functions.
 * External packages are boxes of their own at every level.
 * The functions in `explained` that are drawn as boxes of their own show their explanation, so they are laid out
 * taller; the view's key tells them apart.
 */
export function buildView(
  graph: FlowGraph,
  grouping: Grouping,
  level: Level,
  expansion: Expansion,
  explained: ReadonlySet<string> = NONE_EXPLAINED,
): FlowView {
  const units: Unit[] = [];
  const unitOf = new Map<string, string>();
  const closedGroups = new Set<string>();

  const addFunction = (node: FlowNode, parent: string | null) => {
    units.push({ kind: "function", id: node.id, node, parent });
    unitOf.set(node.id, node.id);
  };
  const addFile = (file: FileGroup, parent: string | null) => {
    const open = expansion.files.has(file.id);
    units.push({ kind: "group", id: file.id, group: file, open, parent });
    if (open) {
      for (const node of file.functions) addFunction(node, file.id);
    } else {
      closedGroups.add(file.id);
      for (const node of file.functions) unitOf.set(node.id, file.id);
    }
  };

  if (level === "functions") {
    for (const node of graph.nodes) addFunction(node, null);
  } else {
    if (level === "files") {
      for (const file of grouping.files) addFile(file, null);
    } else {
      for (const pkg of grouping.packages) {
        const open = expansion.packages.has(pkg.id);
        units.push({ kind: "group", id: pkg.id, group: pkg, open, parent: null });
        if (open) {
          for (const file of pkg.files) addFile(file, pkg.id);
        } else {
          closedGroups.add(pkg.id);
          for (const file of pkg.files) for (const node of file.functions) unitOf.set(node.id, pkg.id);
        }
      }
    }
    for (const node of grouping.externals) addFunction(node, null);
  }

  const links = aggregateLinks(graph.edges, unitOf, closedGroups);
  const linked = new Set(links.flatMap((link) => [link.from, link.to]));
  const isolated = new Set(
    units.filter((unit) => !(unit.kind === "group" && unit.open) && !linked.has(unit.id)).map((unit) => unit.id),
  );
  const open =
    level === "functions"
      ? []
      : [...expansion.packages, ...expansion.files].filter((groupId) => grouping.groups.has(groupId));
  const drawnExplained = new Set(
    units.filter((unit) => unit.kind === "function" && explained.has(unit.id)).map((unit) => unit.id),
  );
  const explainedKey = drawnExplained.size > 0 ? ["explained", ...[...drawnExplained].sort()] : [];
  return {
    graph,
    level,
    key: [level, ...open.sort(), ...explainedKey].join("\n"),
    units,
    unitById: new Map(units.map((unit) => [unit.id, unit])),
    unitOf,
    links,
    isolated,
    explained: drawnExplained,
  };
}

/** Thresholds of the file level: above either, the canvas opens by package. */
export const FILE_LEVEL_LIMITS = { files: 20, links: 40 } as const;

/**
 * The level the canvas opens at: by file when the drawn functions span at most 20 files and at most 40 links
 * between files, otherwise by package.
 */
export function initialLevel(graph: FlowGraph, grouping: Grouping = groupFunctions(graph.nodes)): Level {
  if (grouping.files.length > FILE_LEVEL_LIMITS.files) return "packages";
  const byFile = buildView(graph, grouping, "files", NOTHING_OPEN);
  const fileLinks = byFile.links.filter((link) => grouping.groups.has(link.from) && grouping.groups.has(link.to));
  return fileLinks.length > FILE_LEVEL_LIMITS.links ? "packages" : "files";
}

/** Package of a box, for focusing a package from the left panel. Null for external packages. */
export function containerOf(unit: Unit): string | null {
  if (unit.kind === "group") return unit.group.container;
  return unit.node.state === "external" ? null : unit.node.container;
}

/** What the canvas focuses: a function or a file (its popover is open), a link (its list is open), or a package. */
export type Focus =
  | { kind: "node"; id: string }
  | { kind: "file"; id: string }
  | { kind: "link"; id: string; at: { x: number; y: number } }
  | { kind: "package"; container: string };

/** Ids of the boxes that stay lit while `focus` is set, or null when nothing is focused. */
export function litUnits(view: FlowView, focus: Focus | null): Set<string> | null {
  if (!focus) return null;
  if (focus.kind === "package") {
    return new Set(view.units.filter((unit) => containerOf(unit) === focus.container).map((unit) => unit.id));
  }
  if (focus.kind === "link") {
    const link = view.links.find((candidate) => candidate.id === focus.id);
    return new Set(link ? [link.from, link.to] : []);
  }
  const id = view.unitOf.get(focus.id) ?? focus.id;
  const lit = new Set([id]);
  for (const link of view.links) {
    if (link.from === id) lit.add(link.to);
    if (link.to === id) lit.add(link.from);
  }
  return lit;
}

/** Whether a link stays lit while `focus` is set. */
export function linkLit(link: Link, view: FlowView, focus: Focus, lit: ReadonlySet<string>): boolean {
  if (focus.kind === "link") return link.id === focus.id;
  if (focus.kind === "package") return lit.has(link.from) || lit.has(link.to);
  const id = view.unitOf.get(focus.id) ?? focus.id;
  return link.from === id || link.to === id;
}

export interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Point {
  x: number;
  y: number;
}

/** The line of a link as laid out, on the whole canvas: from the source's side to the target's, around the boxes. */
export interface Route {
  /** Start, bends, end. */
  points: readonly Point[];
  /** Centre of the label, where the layout left room for it. */
  label: Point | null;
}

export interface Layout {
  /** Every box, placed relative to the open group holding it. */
  boxes: ReadonlyMap<string, Box>;
  /** By link id. A link to itself, or one the layout could not route, has none. */
  routes: ReadonlyMap<string, Route>;
}

/** A box placed on the whole canvas, adding up the places of the open groups holding it. */
export function absoluteBox(view: FlowView, layout: Layout, id: string): Box | null {
  const own = layout.boxes.get(id);
  if (!own) return null;
  let { x, y } = own;
  let parent = view.unitById.get(id)?.parent ?? null;
  while (parent !== null) {
    const box = layout.boxes.get(parent);
    if (!box) return null;
    x += box.x;
    y += box.y;
    parent = view.unitById.get(parent)?.parent ?? null;
  }
  return { x, y, width: own.width, height: own.height };
}

/**
 * The layout once the user has dragged some boxes (positions relative to the box holding them, as the canvas gives
 * them). A link keeps its route, shifted, when both its ends moved together, as when the open group holding both
 * was dragged; otherwise it loses it and is drawn as a plain curve between its ends.
 */
export function moveBoxes(view: FlowView, layout: Layout, moved: ReadonlyMap<string, Point>): Layout {
  if (moved.size === 0) return layout;
  const boxes = new Map(layout.boxes);
  for (const [id, position] of moved) {
    const box = boxes.get(id);
    if (box) boxes.set(id, { ...box, ...position });
  }
  const shift = (id: string): Point => {
    let x = 0;
    let y = 0;
    for (let unit: string | null = id; unit !== null; unit = view.unitById.get(unit)?.parent ?? null) {
      const before = layout.boxes.get(unit);
      const after = boxes.get(unit);
      if (!before || !after) continue;
      x += after.x - before.x;
      y += after.y - before.y;
    }
    return { x, y };
  };
  const routes = new Map<string, Route>();
  for (const link of view.links) {
    const route = layout.routes.get(link.id);
    if (!route) continue;
    const from = shift(link.from);
    const to = shift(link.to);
    if (Math.abs(from.x - to.x) > 0.5 || Math.abs(from.y - to.y) > 0.5) continue;
    const move = (point: Point) => ({ x: point.x + from.x, y: point.y + from.y });
    routes.set(link.id, { points: route.points.map(move), label: route.label && move(route.label) });
  }
  return { boxes, routes };
}
