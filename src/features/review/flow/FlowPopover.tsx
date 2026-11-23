import { useViewport } from "@xyflow/react";
import { X } from "lucide-react";
import {
  type ReactNode,
  type RefObject,
  useContext,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { ExplanationColumn, ExplanationSection } from "@/features/review/explanations/ExplanationSection";
import { detailRunKey, ExplanationsContext, nodeRunKey } from "@/features/review/explanations/use-explanations";
import { baseName } from "@/features/review/file-tree";
import {
  type DiffTarget,
  diffTarget,
  fileDiffTarget,
  nodeFacts,
  relationDiffTarget,
} from "@/features/review/flow/flow-facts";
import type { FileGroup, FlowView } from "@/features/review/flow/flow-groups";
import {
  type Link,
  linkState,
  linkStateWord,
  relationLabel,
  stateSummary,
  typeSummary,
} from "@/features/review/flow/flow-links";
import { functionNames, isChanged, NODE_STATE, unitName } from "@/features/review/flow/flow-model";
import { FunctionReviewContext } from "@/features/review/flow/function-review";
import {
  type Box,
  pinnedEdge,
  placePopover,
  POPOVER_MARGIN,
  POPOVER_WIDTH,
} from "@/features/review/flow/popover-placement";
import { functionButtonLabel, spanOf } from "@/features/review/review-state/review-status";
import { FunctionSignalSections, showsLimits } from "@/features/review/signals/PopoverSignals";
import { SignalsContext } from "@/features/review/signals/signals";
import { useReducedMotion } from "@/hooks/use-reduced-motion";
import type { ChangedFile, FlowCall, FlowGraph, FlowNode } from "@/lib/api";
import { cn } from "@/lib/utils";

interface FrameProps {
  /** The anchor's box in flow coordinates: a node, or a point on a link. */
  box: Box;
  /** Element the popover is positioned in; its size bounds the placement. */
  bounds: RefObject<HTMLElement | null>;
  title: string;
  /** Re-placed when it changes, as its height may. */
  contentKey: string;
  /** What the popover is open on; another one is a fresh opening, narrow again. The content key when not given. */
  openKey?: string;
  onClose: () => void;
  header: ReactNode;
  children: ReactNode;
  /**
   * A second column beside the content, such as a function's explanation; the popover widens to twice its width for
   * it. Never given empty.
   */
  aside?: ReactNode;
  footer: ReactNode;
}

const COLUMN = "min-h-0 min-w-0 overflow-y-auto px-[18px] pt-3.5 pb-4";

/** Two frames: the popover is painted at its opening width once before it widens, so the widening is seen. */
function afterNextPaint(callback: () => void): () => void {
  let frame = requestAnimationFrame(() => {
    frame = requestAnimationFrame(callback);
  });
  return () => cancelAnimationFrame(frame);
}

/**
 * A dialog beside its anchor that follows it while the graph pans and zooms. It is never taller than the canvas: its
 * header and footer stay, and each column scrolls on its own.
 *
 * It always opens one column wide. With a second column, there at the opening or arriving later, it then widens away
 * from its anchor, and the column fades in once it has its room; with reduced motion it is simply as wide as it needs.
 */
function PopoverFrame({
  box,
  bounds,
  title,
  contentKey,
  openKey = contentKey,
  onClose,
  header,
  children,
  aside,
  footer,
}: FrameProps) {
  const { t } = useTranslation();
  const own = useRef<HTMLDivElement>(null);
  const second = useRef<HTMLDivElement>(null);
  const { x, y, zoom } = useViewport();
  const reducedMotion = useReducedMotion();
  const hasAside = aside !== undefined;

  // Whether this opening has widened yet, and whether its second column was there when it opened. The frame is not
  // remounted for another anchor, so both are taken again whenever what it is open on changes.
  const [opening, setOpening] = useState({ key: openKey, widened: false, asideAtOpen: hasAside });
  let current = opening;
  if (opening.key !== openKey) current = { key: openKey, widened: false, asideAtOpen: hasAside };
  else if (opening.widened && !hasAside) current = { ...opening, widened: false };
  if (current !== opening) setOpening(current);
  const wide = hasAside && (reducedMotion || current.widened);
  const animated = wide && !reducedMotion;
  const columns = wide ? 2 : 1;
  // Placed for the width it ends at, so it does not move while it widens, nor change sides halfway.
  const targetWidth = POPOVER_WIDTH[hasAside ? 2 : 1];

  useEffect(() => {
    if (!hasAside || wide) return;
    return afterNextPaint(() =>
      setOpening((now) => (now.key === openKey && !now.widened ? { ...now, widened: true } : now)),
    );
  }, [hasAside, wide, openKey]);

  // Positioned straight on the element before paint: it depends on its own measured height, once capped to the
  // canvas. Placed again once it has widened, as its header may be shorter then.
  const place = useRef(() => {});
  useLayoutEffect(() => {
    const popover = own.current;
    const area = bounds.current;
    if (!popover || !area) return;
    place.current = () => {
      const room = area.clientHeight - 2 * POPOVER_MARGIN;
      popover.style.maxHeight = room > 0 ? `${room}px` : "";
      const anchor = { x: box.x * zoom + x, y: box.y * zoom + y, width: box.width * zoom, height: box.height * zoom };
      const canvas = { width: area.clientWidth, height: area.clientHeight };
      const placement = placePopover(anchor, canvas, { width: targetWidth, height: popover.offsetHeight });
      const pinned = pinnedEdge(placement, targetWidth, canvas.width);
      popover.style.left = pinned.edge === "left" ? `${pinned.offset}px` : "auto";
      popover.style.right = pinned.edge === "right" ? `${pinned.offset}px` : "";
      popover.style.top = `${placement.top}px`;
      popover.style.setProperty("--caret", `${placement.caret}px`);
      popover.dataset.side = placement.side;
    };
    place.current();
  }, [box, bounds, x, y, zoom, contentKey, targetWidth, columns]);

  // The Explain button that asked for a first explanation goes once it arrives; the keyboard's place moves on to the
  // new column then, and only then: focus is never taken from anywhere it still is.
  const lastFocused = useRef<Element | null>(null);
  useEffect(() => {
    lastFocused.current = null;
  }, [openKey]);
  useLayoutEffect(() => {
    if (!wide || current.asideAtOpen) return;
    const lost = lastFocused.current;
    const active = document.activeElement;
    if (!lost || lost.isConnected || (active !== null && active !== document.body)) return;
    second.current?.querySelector<HTMLElement>("button:not(:disabled)")?.focus();
  }, [wide, current.asideAtOpen]);

  return (
    <div
      ref={own}
      role="dialog"
      aria-label={title}
      data-side="right"
      data-columns={columns}
      onFocus={(event) => {
        lastFocused.current = event.target;
      }}
      onTransitionEnd={(event) => {
        if (event.target === event.currentTarget && event.propertyName === "width") place.current();
      }}
      className={cn(
        "flow-pop nodrag nopan nowheel absolute top-0 left-0 z-10 flex cursor-default flex-col rounded-[12px] border border-faint bg-card text-sm shadow-[0_28px_70px_rgba(0,0,0,0.18)] dark:shadow-[0_28px_70px_rgba(0,0,0,0.5)]",
        // POPOVER_WIDTH. Only widening is animated: back to one column, for another anchor, it is narrow at once.
        wide ? "w-[760px]" : "w-[380px]",
        animated && "motion-safe:transition-[width] motion-safe:duration-250 motion-safe:ease-out",
      )}
    >
      <header className="flex-none border-b px-[18px] pt-4 pb-3.5">
        <div className="flex items-start gap-2.5">
          <h3 className="min-w-0 flex-1 font-mono text-[15px] font-medium tracking-[-0.01em] break-words">{title}</h3>
          <button
            type="button"
            aria-label={t("common.actions.close")}
            onClick={onClose}
            className="grid size-6 flex-none cursor-pointer place-items-center rounded-[6px] text-muted-foreground outline-none hover:bg-secondary hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
          >
            <X aria-hidden="true" className="size-3.5" />
          </button>
        </div>
        <p className="mt-1.5 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-muted-foreground">
          {header}
        </p>
      </header>
      {/* Two columns keep the widths they end at, inside the 378px within the frame's border and the 380px beside
          it: while the frame widens the second one is cut by its edge rather than squeezed, and nothing reflows. */}
      <div className="flex min-h-0 overflow-hidden">
        <div className={cn(COLUMN, wide ? "w-[378px] flex-none" : "flex-1 basis-0")}>{children}</div>
        {wide && (
          <div
            ref={second}
            data-column="aside"
            className={cn(
              COLUMN,
              "w-[380px] flex-none border-l",
              animated &&
                "motion-safe:animate-in motion-safe:fade-in motion-safe:duration-200 motion-safe:delay-250 motion-safe:fill-mode-backwards",
            )}
          >
            {aside}
          </div>
        )}
      </div>
      <footer className="flex flex-none items-center justify-between gap-3 border-t px-[18px] py-2.5 text-xs text-faint">
        {footer}
      </footer>
    </div>
  );
}

function Lines({ additions, deletions }: { additions: number; deletions: number }) {
  return (
    <span className="font-mono whitespace-nowrap">
      <span className="text-add">+{additions}</span> <span className="text-del">−{deletions}</span>
    </span>
  );
}

/**
 * The footer's way to a file's diff. It shrinks and cuts a long file name short instead of running past the edge of
 * the popover; the whole name is in its tooltip.
 */
function DiffButton({ path, onClick }: { path: string; onClick: () => void }) {
  const { t } = useTranslation();
  const label = t("flow.popover.viewDiffOf", { name: baseName(path) });
  return (
    <Button size="sm" title={label} className="min-w-0 shrink font-medium" onClick={onClick}>
      <span className="truncate">{label}</span>
    </Button>
  );
}

const LINK_BUTTON =
  "min-w-0 cursor-pointer truncate rounded-[3px] text-left font-mono underline decoration-faint underline-offset-2 outline-none hover:decoration-foreground focus-visible:outline-2 focus-visible:outline-ring";

interface NodePopoverProps {
  graph: FlowGraph;
  node: FlowNode;
  box: Box;
  bounds: RefObject<HTMLElement | null>;
  files: readonly ChangedFile[];
  /** Opens the popover of a function, opening what holds it when needed. */
  onSelectNode: (id: string) => void;
  onViewDiff: (target: DiffTarget) => void;
  onClose: () => void;
}

/**
 * Marks the function as reviewed, or unmarks it; it says which it is now. It keeps its whole label, so the diff
 * button beside it is the one that gives way.
 */
function MarkButton({ node }: { node: FlowNode }) {
  const review = useContext(FunctionReviewContext);
  const toggle = review.toggle;
  if (!toggle || spanOf(node) === null) return null;
  const status = review.statusOf(node.id);
  return (
    <Button
      variant="outline"
      size="sm"
      disabled={review.busy}
      onClick={() => toggle(node)}
      className={cn(
        "flex-none",
        status === "reviewed" && "border-add text-add not-disabled:hover:border-add not-disabled:hover:text-add",
        status === "reopened" &&
          "border-primary text-primary not-disabled:hover:border-primary not-disabled:hover:text-primary",
      )}
    >
      {functionButtonLabel(status)}
    </Button>
  );
}

/** Short summary of one function beside its node: what the analyzer measured, and a way to its diff. */
export function FlowPopover({ graph, node, box, bounds, files, onSelectNode, onViewDiff, onClose }: NodePopoverProps) {
  const { t } = useTranslation();
  // A function over a limit, or that no test runs, lists its complexity among its limits rather than in its facts.
  const signalsHandle = useContext(SignalsContext);
  const signals = isChanged(node) ? signalsHandle.signals.functions.get(node.id) : undefined;
  const facts = nodeFacts(graph, node, !showsLimits(signals, signalsHandle.signals.limits));
  const signalState = signals
    ? `${signals.limits.length}/${signals.untested}/${signals.checks.length}/${signals.duplicates.length}/${signals.mutants.length}/${signals.findings.length}`
    : "";
  const target = diffTarget(node, files);
  const explanations = useContext(ExplanationsContext);
  // The explanation opens in a column beside the rest once there is one; each function is an opening of its own.
  const explained = explanations.shown.has(node.id);
  // Its detailed part is shown only once asked for, and hidden again for every other function and every opening.
  const [details, setDetails] = useState({ nodeId: node.id, expanded: false });
  let expanded = details.expanded;
  if (details.nodeId !== node.id) {
    expanded = false;
    setDetails({ nodeId: node.id, expanded: false });
  }
  // An explanation (short or detailed) arriving, running or failing changes the popover's size.
  const shown = explanations.shown.get(node.id);
  const explanationState = [
    shown?.brief?.createdAt ?? "",
    shown?.detail?.createdAt ?? "",
    explanations.runOf(node.id) ? "running" : "",
    explanations.runByKey(detailRunKey(node.id)) ? "detailing" : "",
    explanations.errorOf(nodeRunKey(node.id)) ?? "",
    explanations.errorOf(detailRunKey(node.id)) ?? "",
    expanded ? "expanded" : "",
  ].join("|");
  return (
    <PopoverFrame
      box={box}
      bounds={bounds}
      title={facts.title}
      contentKey={`${node.id}|${explanationState}|${signalState}`}
      openKey={node.id}
      onClose={onClose}
      aside={
        explained ? (
          <ExplanationColumn
            node={node}
            expanded={expanded}
            onExpandedChange={(next) => setDetails({ nodeId: node.id, expanded: next })}
          />
        ) : undefined
      }
      header={
        <>
          <i aria-hidden="true" className={cn("size-2 flex-none rounded-[2px]", NODE_STATE[node.state].dot)} />
          <span>{facts.stateLabel}</span>
          {facts.lines && <Lines {...facts.lines} />}
          <code title={facts.location} className="ml-auto min-w-0 break-all text-faint">
            {facts.location}
          </code>
        </>
      }
      footer={
        <>
          {/* Gives way first, so the buttons keep as much of their labels as they can. */}
          <span className="min-w-0 shrink-[10] truncate">
            {t("flow.popover.measuredBy", { name: graph.analyzer.name, version: graph.analyzer.version })}
          </span>
          {target ? (
            <span className="flex min-w-0 items-center gap-2">
              <MarkButton node={node} />
              <DiffButton path={target.path} onClick={() => onViewDiff(target)} />
            </span>
          ) : (
            <span>
              {node.state === "external" ? t("flow.popover.outsideRepository") : t("flow.popover.fileNotChanged")}
            </span>
          )}
        </>
      }
    >
      <p>{facts.summary}</p>
      <dl className="mt-3 grid grid-cols-[1fr_auto] gap-x-3 gap-y-[7px]">
        {facts.facts.map((fact) => (
          <div key={fact.label} className="contents">
            <dt className="text-muted-foreground">{fact.label}</dt>
            <dd className="text-right font-mono text-xs leading-[1.6]">{fact.value}</dd>
          </div>
        ))}
      </dl>
      <FunctionSignalSections signals={signals} files={files} onViewDiff={onViewDiff} />
      <ExplanationSection node={node} />
      {node.calls.length > 0 && <CallList calls={node.calls} onSelectNode={onSelectNode} />}
    </PopoverFrame>
  );
}

/**
 * Every relation the function has, drawn or not, in the order they appear in its code. A callee drawn in the graph
 * opens its own popover; the others are marked "not shown".
 */
function CallList({ calls, onSelectNode }: { calls: readonly FlowCall[]; onSelectNode: (id: string) => void }) {
  const { t } = useTranslation();
  const id = useId();
  return (
    <section aria-labelledby={id} className="mt-3.5 border-t pt-3">
      <h4 id={id} className="mb-1.5 text-xs font-normal text-faint">
        {t("flow.popover.callsByLine")}
      </h4>
      <ol className="grid gap-1 text-xs">
        {calls.map((call) => (
          <li key={`${call.calleeId}:${call.line}:${call.relation}`} className="flex min-w-0 items-baseline gap-2">
            {call.shown ? (
              <button
                type="button"
                title={call.qualifiedName}
                onClick={() => onSelectNode(call.calleeId)}
                className={cn(LINK_BUTTON, call.removed && "line-through")}
              >
                {call.qualifiedName}
              </button>
            ) : (
              <span
                title={call.qualifiedName}
                className={cn("min-w-0 truncate font-mono text-muted-foreground", call.removed && "line-through")}
              >
                {call.qualifiedName}
              </span>
            )}
            <span className="flex-none whitespace-nowrap text-faint">{t("flow.popover.line", { line: call.line })}</span>
            <span className="ml-auto flex flex-none gap-1.5">
              {call.relation !== "call" && <span className="text-muted-foreground">{relationLabel(call)}</span>}
              {!call.shown && <span className="text-faint">{t("flow.popover.notShown")}</span>}
              {call.added && <span className="text-add">{t("flow.popover.added")}</span>}
              {call.removed && <span className="text-del">{t("flow.popover.removed")}</span>}
            </span>
          </li>
        ))}
      </ol>
    </section>
  );
}

interface FilePopoverProps {
  file: FileGroup;
  box: Box;
  bounds: RefObject<HTMLElement | null>;
  files: readonly ChangedFile[];
  onSelectNode: (id: string) => void;
  onViewDiff: (target: DiffTarget) => void;
  /** Opens the file in place, showing its functions; null when it is open. */
  onOpen: (() => void) | null;
  onClose: () => void;
}

/** A closed file's changed functions, each with a way to its diff. */
export function FilePopover({ file, box, bounds, files, onSelectNode, onViewDiff, onOpen, onClose }: FilePopoverProps) {
  const { t } = useTranslation();
  const id = useId();
  const changed = file.functions.filter(isChanged);
  const others = file.functions.length - changed.length;
  const target = fileDiffTarget(file, files);
  return (
    <PopoverFrame
      box={box}
      bounds={bounds}
      title={file.path}
      contentKey={file.id}
      onClose={onClose}
      header={
        <>
          <span>{t("flow.changedFunctions", { count: file.changed })}</span>
          <Lines additions={file.additions} deletions={file.deletions} />
          <code className="ml-auto min-w-0 break-all text-faint">{file.container}</code>
        </>
      }
      footer={
        <>
          {onOpen ? (
            <Button variant="secondary" size="sm" onClick={onOpen}>
              {t("flow.group.showFunctions")}
            </Button>
          ) : (
            <span />
          )}
          {target ? (
            <DiffButton path={target.path} onClick={() => onViewDiff(target)} />
          ) : (
            <span>{t("flow.popover.fileNotChanged")}</span>
          )}
        </>
      }
    >
      <section aria-labelledby={id}>
        <h4 id={id} className="mb-1.5 text-xs font-normal text-faint">
          {t("flow.popover.changedFunctions")}
        </h4>
        <ol className="grid gap-1 text-xs">
          {changed.map((node) => {
            const diff = diffTarget(node, files);
            return (
              <li key={node.id} className="flex min-w-0 items-center gap-2">
                <i aria-hidden="true" className={cn("size-2 flex-none rounded-[2px]", NODE_STATE[node.state].dot)} />
                <button
                  type="button"
                  title={node.qualifiedName}
                  onClick={() => onSelectNode(node.id)}
                  className={cn(LINK_BUTTON, node.state === "deleted" && "line-through")}
                >
                  {node.name}
                </button>
                <span className="ml-auto flex flex-none items-center gap-2">
                  <Lines additions={node.additions} deletions={node.deletions} />
                  {diff && (
                    <button
                      type="button"
                      aria-label={t("flow.popover.viewDiffOf", { name: node.name })}
                      onClick={() => onViewDiff(diff)}
                      className="cursor-pointer rounded-[4px] border px-1.5 text-[11px] leading-[16px] text-muted-foreground outline-none hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
                    >
                      {t("flow.popover.diff")}
                    </button>
                  )}
                </span>
              </li>
            );
          })}
        </ol>
        {others > 0 && (
          <p className="mt-2 text-xs text-faint">{t("flow.popover.unchangedAround", { count: others })}</p>
        )}
      </section>
    </PopoverFrame>
  );
}

interface LinkPopoverProps {
  view: FlowView;
  link: Link;
  box: Box;
  bounds: RefObject<HTMLElement | null>;
  files: readonly ChangedFile[];
  onViewDiff: (target: DiffTarget) => void;
  onClose: () => void;
}

const STATE_TEXT = { added: "text-primary", preserved: "text-muted-foreground", removed: "text-del" } as const;

/** The concrete relations a link stands for: from, to, line, type and state, each with a way to its line. */
export function LinkPopover({ view, link, box, bounds, files, onViewDiff, onClose }: LinkPopoverProps) {
  const { t } = useTranslation();
  const id = useId();
  const name = functionNames(view);
  const title = `${unitName(view, link.from)} → ${unitName(view, link.to)}`;
  return (
    <PopoverFrame
      box={box}
      bounds={bounds}
      title={title}
      contentKey={link.id}
      onClose={onClose}
      header={
        <>
          <span>{stateSummary(link)}</span>
          <span className="text-faint">{typeSummary(link)}</span>
        </>
      }
      footer={<span>{t("flow.popover.relationsFooter", { count: link.relations.length })}</span>}
    >
      <section aria-labelledby={id}>
        <h4 id={id} className="mb-1.5 text-xs font-normal text-faint">
          {t("flow.popover.relations")}
        </h4>
        <ol className="grid gap-1.5 text-xs">
          {link.relations.map((edge) => {
            const state = linkState(edge);
            const target = relationDiffTarget(edge, view.graph, files);
            return (
              <li key={`${edge.from}->${edge.to}:${edge.relation}`} className="grid min-w-0 gap-0.5">
                <span className={cn("min-w-0 truncate font-mono", state === "removed" && "line-through")}>
                  {name(edge.from)} → {name(edge.to)}
                </span>
                <span className="flex min-w-0 items-center gap-2 text-faint">
                  <span className="whitespace-nowrap">{t("flow.popover.line", { line: edge.line })}</span>
                  <span className="min-w-0 truncate text-muted-foreground">{relationLabel(edge)}</span>
                  <span className={cn("whitespace-nowrap", STATE_TEXT[state])}>{linkStateWord(state)}</span>
                  {target && (
                    <button
                      type="button"
                      aria-label={t("flow.popover.viewDiffAtLine", { line: edge.line, name: baseName(target.path) })}
                      onClick={() => onViewDiff(target)}
                      className="ml-auto flex-none cursor-pointer rounded-[4px] border px-1.5 text-[11px] leading-[16px] text-muted-foreground outline-none hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
                    >
                      {t("flow.popover.diff")}
                    </button>
                  )}
                </span>
              </li>
            );
          })}
        </ol>
      </section>
    </PopoverFrame>
  );
}
