// The Flow tab once a graph has arrived: left panel, toolbar, notices and the React Flow canvas.
// Loaded lazily with React Flow and ELK; tests render it for real with src/test/react-flow.ts and src/test/elk.ts.

import "@xyflow/react/dist/base.css";
import "@/features/review/flow/flow.css";

import {
  Background,
  BackgroundVariant,
  ControlButton,
  Controls,
  MiniMap,
  type NodeChangeset,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
} from "@xyflow/react";
import { RotateCcw } from "lucide-react";
import { type RefObject, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { ErrorNote } from "@/components/page";
import { Button } from "@/components/ui/button";
import { ExplanationsBanner } from "@/features/review/explanations/ExplanationsBanner";
import { ExplanationsContext } from "@/features/review/explanations/use-explanations";
import type { DiffTarget } from "@/features/review/flow/flow-facts";
import {
  absoluteBox,
  type Box,
  buildView,
  containerOf,
  type Expansion,
  type FlowView,
  type Focus,
  groupFunctions,
  initialLevel,
  type Layout,
  type Level,
  moveBoxes,
  NOTHING_OPEN,
  type Point,
  reveal,
  toggleGroup,
} from "@/features/review/flow/flow-groups";
import { layoutFlow } from "@/features/review/flow/flow-layout";
import { type CanvasNode, isChanged, toFlowEdges, toFlowNodes } from "@/features/review/flow/flow-model";
import { FilePopover, FlowPopover, LinkPopover } from "@/features/review/flow/FlowPopover";
import { FlowSidebar } from "@/features/review/flow/FlowSidebar";
import { FlowToolbar } from "@/features/review/flow/FlowToolbar";
import { type Reveal, Spinner } from "@/features/review/flow/FlowView";
import { FunctionNode, SelectNodeContext } from "@/features/review/flow/FunctionNode";
import {
  ClusterNode,
  ContainerNode,
  type GroupActions,
  GroupActionsContext,
  groupBadge,
} from "@/features/review/flow/GroupNode";
import { LinkEdge, SelectLinkContext } from "@/features/review/flow/LinkEdge";
import { functionBadge, SignalsContext } from "@/features/review/signals/signals";
import { i18n } from "@/i18n";
import type { ChangedFile, FlowGraph, FlowOptions } from "@/lib/api";
import { coreText } from "@/lib/core-text";
import { errorMessage } from "@/lib/format";

const NODE_TYPES = { function: FunctionNode, cluster: ClusterNode, container: ContainerNode };
const EDGE_TYPES = { link: LinkEdge };
const FIT_VIEW = { padding: 0.12, maxZoom: 1 };
// Above this many boxes the minimap helps to find one's way; below, the whole canvas fits on screen.
const MINIMAP_FROM = 60;
const MOVE = { duration: 300 };
// Height of an open group's header, where its popover points.
const HEADER_HEIGHT = 34;

interface FlowWorkspaceProps {
  graph: FlowGraph;
  /** A graph with other options is on its way. */
  pending: boolean;
  options: FlowOptions;
  onOptionsChange: (options: FlowOptions) => void;
  files: readonly ChangedFile[];
  onViewDiff: (target: DiffTarget) => void;
  /** A function to show, opened from another tab; a new object shows it again. */
  reveal?: Reveal | null;
}

export default function FlowWorkspace(props: FlowWorkspaceProps) {
  return (
    <ReactFlowProvider>
      <Workspace {...props} />
    </ReactFlowProvider>
  );
}

interface Drawn {
  view: FlowView;
  layout: Layout;
}

type LayoutState =
  /** previous: the last view laid out, kept on screen while this one is laid out. */
  | { status: "loading"; previous: Drawn | null }
  | { status: "ready"; layout: Layout }
  | { status: "error"; message: string };

/**
 * The layout of `view`. Boxes and link labels are sized from their text, so the view is laid out again in a new
 * language; the old layout stays on screen meanwhile.
 */
function useLayout(view: FlowView | null, language: string): { layout: LayoutState; retry: () => void } {
  const [result, setResult] = useState<{
    view: FlowView;
    attempt: number;
    language: string;
    layout: LayoutState;
  } | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    if (view === null) return;
    let cancelled = false;
    layoutFlow(view).then(
      (layout) => {
        if (!cancelled) setResult({ view, attempt, language, layout: { status: "ready", layout } });
      },
      (error: unknown) => {
        if (!cancelled) {
          setResult({ view, attempt, language, layout: { status: "error", message: errorMessage(error) } });
        }
      },
    );
    return () => {
      cancelled = true;
    };
  }, [view, attempt, language]);
  const retry = useCallback(() => setAttempt((n) => n + 1), []);
  const current = result !== null && result.view === view && result.attempt === attempt && result.language === language;
  if (current) return { layout: result.layout, retry };
  const previous =
    result !== null && result.layout.status === "ready" ? { view: result.view, layout: result.layout.layout } : null;
  return { layout: { status: "loading", previous }, retry };
}

function Workspace({
  graph: latest,
  pending,
  options,
  onOptionsChange,
  files,
  onViewDiff,
  reveal: revealed = null,
}: FlowWorkspaceProps) {
  const grouping = useMemo(() => groupFunctions(latest.nodes), [latest]);
  // Opens by file or by package depending on the size of the first graph; the user's choice is kept after that.
  const [level, setLevel] = useState<Level>(() => initialLevel(latest, grouping));
  const [expansion, setExpansion] = useState<Expansion>(NOTHING_OPEN);
  const [focus, setFocus] = useState<Focus | null>(null);
  const empty = latest.nodes.length === 0;
  // Explained functions are drawn taller, with the lines of their explanation: a new one lays the view out again.
  const { shown: explanations } = useContext(ExplanationsContext);
  const explainedKey = useMemo(() => [...explanations.keys()].sort().join("\n"), [explanations]);
  const explained = useMemo(() => new Set(explainedKey === "" ? [] : explainedKey.split("\n")), [explainedKey]);
  const view = useMemo(
    () => (empty ? null : buildView(latest, grouping, level, expansion, explained)),
    [empty, latest, grouping, level, expansion, explained],
  );
  const { t, i18n: translation } = useTranslation();
  const { layout, retry } = useLayout(view, translation.language);
  // The view on the canvas: the latest once laid out, the previous one until then.
  const drawn: Drawn | null =
    view === null
      ? null
      : layout.status === "ready"
        ? { view, layout: layout.layout }
        : layout.status === "loading"
          ? layout.previous
          : null;
  const shownView = drawn?.view ?? null;
  const shownLayout = drawn?.layout ?? null;
  const graph = shownView?.graph ?? latest;
  const updating = pending || (drawn !== null && drawn.view !== view);
  const flow = useReactFlow();

  const centreOn = useCallback(
    (shown: FlowView, placed: Layout, id: string) => {
      const box = absoluteBox(shown, placed, id);
      if (!box) return;
      const zoom = Math.max(flow.getZoom(), 0.9);
      // Shifted right of centre, so the box stays clear of the popover opening beside it.
      void flow.setCenter(box.x + box.width / 2 + 190 / zoom, box.y + box.height / 2, { ...MOVE, zoom });
    },
    [flow],
  );

  // Kept apart from the render: where a toggled group was on the canvas, and a function waiting to be drawn.
  const anchor = useRef<{ id: string; box: Box } | null>(null);
  const revealing = useRef<string | null>(null);
  // The canvas fits the first view by itself. A new graph or level is fitted again; opening or closing a group
  // keeps the group under the pointer where it was.
  const fitted = useRef<{ graph: FlowGraph; level: Level } | null>(null);
  useEffect(() => {
    if (shownView === null || shownLayout === null) return;
    const previous = fitted.current;
    fitted.current = { graph: shownView.graph, level: shownView.level };
    if (previous !== null && (previous.graph !== shownView.graph || previous.level !== shownView.level)) {
      anchor.current = null;
      void flow.fitView({ ...FIT_VIEW, ...MOVE });
    } else if (anchor.current !== null) {
      const before = anchor.current.box;
      const after = absoluteBox(shownView, shownLayout, anchor.current.id);
      anchor.current = null;
      if (after) {
        const { x, y, zoom } = flow.getViewport();
        void flow.setViewport({ x: x - (after.x - before.x) * zoom, y: y - (after.y - before.y) * zoom, zoom });
      }
    }
    const target = revealing.current;
    if (target !== null && shownView.unitById.has(target)) {
      revealing.current = null;
      centreOn(shownView, shownLayout, target);
    }
  }, [shownView, shownLayout, flow, centreOn]);

  /** Opens the popover of a function, opening the groups that hold it first. */
  const showFunction = useCallback(
    (id: string) => {
      setFocus({ kind: "node", id });
      const next = reveal(expansion, grouping, level, id);
      if (next !== expansion) {
        revealing.current = id;
        setExpansion(next);
      } else if (shownView && shownLayout) {
        centreOn(shownView, shownLayout, id);
      }
    },
    [expansion, grouping, level, shownView, shownLayout, centreOn],
  );

  // A function opened from another tab (the Overview's "Open in Flow") is shown once the canvas has a view; each new
  // request shows it again.
  const shownReveal = useRef<Reveal | null>(null);
  useEffect(() => {
    if (revealed === null || revealed === shownReveal.current || shownView === null) return;
    if (!latest.nodes.some((node) => node.id === revealed.functionId)) return;
    shownReveal.current = revealed;
    showFunction(revealed.functionId);
  }, [revealed, shownView, latest, showFunction]);

  const toggle = useCallback(
    (groupId: string) => {
      const group = grouping.groups.get(groupId);
      if (!group) return;
      const box = shownView && shownLayout ? absoluteBox(shownView, shownLayout, groupId) : null;
      anchor.current = box ? { id: groupId, box } : null;
      setFocus((current) => (current?.kind === "package" ? current : null));
      setExpansion((current) => toggleGroup(current, group));
    },
    [grouping, shownView, shownLayout],
  );

  const changeLevel = (next: Level) => {
    setLevel(next);
    setExpansion(NOTHING_OPEN);
    setFocus(null);
  };

  const onPackage = (container: string) => {
    setFocus({ kind: "package", container });
    if (!shownView) return;
    const nodes = shownView.units.filter((unit) => containerOf(unit) === container).map((unit) => ({ id: unit.id }));
    void flow.fitView({ ...MOVE, nodes, padding: 0.25, maxZoom: 1.1 });
  };

  const context = {
    untouched: graph.nodes.some((node) => node.state === "untouched"),
    external: graph.nodes.some((node) => node.state === "external"),
  };
  // The legend of findings, limits and checks, only while some box on the canvas carries a badge.
  const { signals } = useContext(SignalsContext);
  const badged =
    shownView?.units.some((unit) =>
      unit.kind === "function"
        ? isChanged(unit.node) && functionBadge(signals.functions.get(unit.node.id)) !== null
        : !unit.open && groupBadge(unit.group, signals) !== null,
    ) ?? false;

  return (
    <>
      <FlowSidebar graph={graph} focus={focus} onEntry={showFunction} onPackage={onPackage} />
      <div className="flex min-h-0 min-w-0 flex-col">
        <FlowToolbar
          controls={{
            hidden: latest.hidden,
            options,
            onChange: onOptionsChange,
            pending: updating,
            level,
            onLevelChange: changeLevel,
            context,
          }}
          signals={badged}
        />
        <ExplanationsBanner />
        <Notices graph={graph} />
        {empty ? (
          <p className="px-4 py-4 text-muted-foreground" aria-busy={pending || undefined}>
            {emptyMessage(latest, options)}
          </p>
        ) : drawn ? (
          <Canvas
            view={drawn.view}
            layout={drawn.layout}
            updating={updating}
            files={files}
            focus={focus}
            onFocus={setFocus}
            onSelectNode={showFunction}
            onToggle={toggle}
            onViewDiff={onViewDiff}
          />
        ) : layout.status === "error" ? (
          <div className="px-4 py-4">
            <ErrorNote>{t("flow.workspace.layoutFailed", { message: layout.message })}</ErrorNote>
            <Button variant="secondary" size="sm" className="mt-2.5" onClick={retry}>
              {t("common.actions.tryAgain")}
            </Button>
          </div>
        ) : (
          <p role="status" className="flex items-center gap-2 px-4 py-4">
            <Spinner />
            {t("flow.workspace.layingOut", { count: graph.nodes.length })}
          </p>
        )}
      </div>
    </>
  );
}

/** Why a graph has no node: nothing but tests changed, and they are left out, or no function changed at all. */
export function emptyMessage(graph: FlowGraph, options: FlowOptions): string {
  if (!options.includeTests) {
    const tests = graph.hidden.changedTests;
    return tests > 0
      ? i18n.t("flow.workspace.includeTestsToSee", { count: tests })
      : i18n.t("flow.workspace.noChangeOutsideTests");
  }
  return i18n.t("flow.workspace.noChange", { analyzer: graph.analyzer.name });
}

function Notices({ graph }: { graph: FlowGraph }) {
  const { t } = useTranslation();
  if (!graph.truncated && graph.diagnostics.length === 0) return null;
  return (
    <div className="flex flex-none flex-col gap-1.5 border-b px-4 py-2 text-xs">
      {graph.truncated && (
        <p role="note" className="text-mod">
          {t("flow.workspace.truncated")}
        </p>
      )}
      {graph.diagnostics.length > 0 && (
        <details className="text-muted-foreground">
          {/* The analyzers' own words, listed as they gave them, and the core's for an analyzer left out. */}
          <summary className="cursor-pointer select-none">
            {t("flow.workspace.problems", { count: graph.diagnostics.length })}
          </summary>
          <ul className="mt-1.5 mb-1 grid gap-1 font-mono">
            {graph.diagnostics.map((diagnostic, index) => (
              <li key={index} className="break-words whitespace-pre-wrap">
                {coreText(diagnostic)}
              </li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}

interface CanvasProps {
  view: FlowView;
  layout: Layout;
  /** A newer graph or view is on its way: this one is shown faded and marked busy. */
  updating: boolean;
  files: readonly ChangedFile[];
  focus: Focus | null;
  onFocus: (focus: Focus | null) => void;
  /** Opens a function's popover and brings it into view, from the left panel or a list in a popover. */
  onSelectNode: (id: string) => void;
  onToggle: (groupId: string) => void;
  onViewDiff: (target: DiffTarget) => void;
}

/** The element that opened the popover of `focus`, to give the keyboard back to when it closes. */
function triggerOf(area: HTMLElement | null, focus: Focus): HTMLElement | undefined {
  if (!area || focus.kind === "package") return undefined;
  const attribute = { node: "nodeId", file: "groupId", link: "linkId" } as const;
  const selector = { node: "[data-node-id]", file: "[data-group-id]", link: "[data-link-id]" } as const;
  const key = attribute[focus.kind];
  const candidates = [...area.querySelectorAll<HTMLElement>(selector[focus.kind])];
  return candidates.find((element) => element.dataset[key] === focus.id);
}

const NOTHING_MOVED: ReadonlyMap<string, Point> = new Map();

// Boxes are dragged while this key is held (Cmd on a Mac, Ctrl elsewhere), so a plain press still opens them.
const MOVE_KEY = /Mac|iPhone|iPad/.test(navigator.userAgent) ? "metaKey" : "ctrlKey";
const MOVE_KEY_NAME = MOVE_KEY === "metaKey" ? "Cmd" : "Ctrl";

/** Whether the key that lets boxes be dragged is held. */
function useMoveKey(): boolean {
  const [held, setHeld] = useState(false);
  useEffect(() => {
    const update = (event: KeyboardEvent) => setHeld(event[MOVE_KEY]);
    const release = () => setHeld(false);
    window.addEventListener("keydown", update);
    window.addEventListener("keyup", update);
    window.addEventListener("blur", release);
    return () => {
      window.removeEventListener("keydown", update);
      window.removeEventListener("keyup", update);
      window.removeEventListener("blur", release);
    };
  }, []);
  return held;
}

function Canvas({ view, layout, updating, files, focus, onFocus, onSelectNode, onToggle, onViewDiff }: CanvasProps) {
  const { t } = useTranslation();
  const area = useRef<HTMLDivElement>(null);
  // Where the user dragged boxes to, for this layout only: a new view is laid out afresh.
  const [dragged, setDragged] = useState<{ layout: Layout; positions: ReadonlyMap<string, Point> } | null>(null);
  const moved = dragged !== null && dragged.layout === layout ? dragged.positions : NOTHING_MOVED;
  const placed = useMemo(() => moveBoxes(view, layout, moved), [view, layout, moved]);
  const nodes = useMemo(() => toFlowNodes(view, placed, focus), [view, placed, focus]);
  const edges = useMemo(() => toFlowEdges(view, placed, focus), [view, placed, focus]);
  const moving = useMoveKey();

  const onNodesChange = useCallback(
    (changes: NodeChangeset<CanvasNode>) => {
      const positions = [...changes].flatMap((change) =>
        change.type === "position" && change.position ? [[change.id, change.position] as const] : [],
      );
      if (positions.length === 0) return;
      setDragged((current) => {
        const next = new Map(current !== null && current.layout === layout ? current.positions : NOTHING_MOVED);
        for (const [id, { x, y }] of positions) next.set(id, { x, y });
        return { layout, positions: next };
      });
    },
    [layout],
  );

  const selectNode = useCallback(
    (id: string) => onFocus(focus?.kind === "node" && focus.id === id ? null : { kind: "node", id }),
    [focus, onFocus],
  );
  const groupActions = useMemo<GroupActions>(
    () => ({
      toggle: onToggle,
      select: (id) => onFocus(focus?.kind === "file" && focus.id === id ? null : { kind: "file", id }),
    }),
    [focus, onFocus, onToggle],
  );
  const selectLink = useCallback(
    (id: string, at: { x: number; y: number }) =>
      onFocus(focus?.kind === "link" && focus.id === id ? null : { kind: "link", id, at }),
    [focus, onFocus],
  );

  // Back to what opened the popover once it is closed, so the keyboard does not lose its place. Looked up a turn
  // later: React Flow takes the new links into its store in an effect of its own and draws their labels anew.
  const restore = useRef<Focus | null>(null);
  const close = useCallback(() => {
    restore.current = focus;
    onFocus(null);
  }, [focus, onFocus]);
  useEffect(() => {
    const closed = restore.current;
    if (focus !== null || closed === null) return;
    restore.current = null;
    const timer = setTimeout(() => triggerOf(area.current, closed)?.focus(), 0);
    return () => clearTimeout(timer);
  }, [focus]);

  // Escape closes the popover or the package focus; a press outside the canvas and the left panel does too.
  useEffect(() => {
    if (!focus) return;
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        close();
      }
    }
    function onPointerDown(event: PointerEvent) {
      const target = event.target;
      if (!(target instanceof Element)) return;
      if (area.current?.contains(target) || target.closest("[data-flow-sidebar]")) return;
      onFocus(null);
    }
    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("pointerdown", onPointerDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("pointerdown", onPointerDown);
    };
  }, [focus, close, onFocus]);

  return (
    <SelectNodeContext value={selectNode}>
      <GroupActionsContext value={groupActions}>
        <SelectLinkContext value={selectLink}>
          <div
            ref={area}
            aria-busy={updating || undefined}
            data-moving={moving || undefined}
            className="flow-canvas relative min-h-0 flex-1"
          >
            <ReactFlow<CanvasNode>
              nodes={nodes}
              edges={edges}
              nodeTypes={NODE_TYPES}
              edgeTypes={EDGE_TYPES}
              fitView
              fitViewOptions={FIT_VIEW}
              minZoom={0.1}
              maxZoom={2}
              zIndexMode="auto"
              nodesDraggable={moving}
              onNodesChange={onNodesChange}
              nodesConnectable={false}
              nodesFocusable={false}
              edgesFocusable={false}
              elementsSelectable={false}
              deleteKeyCode={null}
              selectionKeyCode={null}
              multiSelectionKeyCode={null}
              onPaneClick={() => onFocus(null)}
              aria-label={t("flow.workspace.canvas")}
            >
              <Background variant={BackgroundVariant.Dots} gap={22} size={1} />
              <Controls position="bottom-left" showInteractive={false}>
                {moved.size > 0 && (
                  <ControlButton
                    onClick={() => setDragged(null)}
                    aria-label={t("flow.workspace.putBack")}
                    title={t("flow.workspace.putBackTitle", { key: MOVE_KEY_NAME })}
                  >
                    {/* React Flow fills its control icons; this one is drawn with strokes. */}
                    <RotateCcw aria-hidden="true" style={{ fill: "none" }} />
                  </ControlButton>
                )}
              </Controls>
              {view.units.length >= MINIMAP_FROM && (
                <MiniMap<CanvasNode>
                  position="top-right"
                  pannable
                  zoomable
                  nodeClassName={(node) => (node.data.dimmed ? "opacity-30" : "") + " " + minimapFill(node)}
                />
              )}
              <Popover
                view={view}
                layout={placed}
                focus={focus}
                bounds={area}
                files={files}
                onSelectNode={onSelectNode}
                onToggle={onToggle}
                onViewDiff={onViewDiff}
                onClose={close}
              />
            </ReactFlow>
          </div>
        </SelectLinkContext>
      </GroupActionsContext>
    </SelectNodeContext>
  );
}

interface PopoverProps {
  view: FlowView;
  layout: Layout;
  focus: Focus | null;
  bounds: RefObject<HTMLDivElement | null>;
  files: readonly ChangedFile[];
  onSelectNode: (id: string) => void;
  onToggle: (groupId: string) => void;
  onViewDiff: (target: DiffTarget) => void;
  onClose: () => void;
}

/** The popover of the focused function, file or link, when it is drawn in this view. */
function Popover({ view, layout, focus, bounds, files, onSelectNode, onToggle, onViewDiff, onClose }: PopoverProps) {
  if (!focus || focus.kind === "package") return null;
  if (focus.kind === "link") {
    const link = view.links.find((candidate) => candidate.id === focus.id);
    if (!link) return null;
    return (
      <LinkPopover
        view={view}
        link={link}
        box={{ ...focus.at, width: 0, height: 0 }}
        bounds={bounds}
        files={files}
        onViewDiff={onViewDiff}
        onClose={onClose}
      />
    );
  }
  const unit = view.unitById.get(focus.id);
  const box = absoluteBox(view, layout, focus.id);
  if (!unit || !box) return null;
  if (unit.kind === "function") {
    return (
      <FlowPopover
        graph={view.graph}
        node={unit.node}
        box={box}
        bounds={bounds}
        files={files}
        onSelectNode={onSelectNode}
        onViewDiff={onViewDiff}
        onClose={onClose}
      />
    );
  }
  if (unit.group.kind !== "file") return null;
  return (
    <FilePopover
      file={unit.group}
      box={unit.open ? { ...box, height: HEADER_HEIGHT } : box}
      bounds={bounds}
      files={files}
      onSelectNode={onSelectNode}
      onViewDiff={onViewDiff}
      onOpen={unit.open ? null : () => onToggle(unit.id)}
      onClose={onClose}
    />
  );
}

function minimapFill(node: CanvasNode): string {
  if (node.type !== "function") return "fill-faint";
  switch (node.data.node.state) {
    case "new":
      return "fill-add";
    case "modified":
      return "fill-mod";
    case "deleted":
      return "fill-del";
    default:
      return "fill-faint";
  }
}
