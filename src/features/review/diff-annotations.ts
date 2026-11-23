// Blocks drawn under lines of the diff (comment threads, the comment composer) and the line a click lands on.
// Only DiffEditor imports this module, so CodeMirror stays out of the main bundle.

import { getChunks, getOriginalDoc, uncollapseUnchanged } from "@codemirror/merge";
import { type EditorState, type Extension, RangeSet, StateEffect, StateField } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, gutter, GutterMarker, WidgetType } from "@codemirror/view";

import { clampLine, oldToNew } from "@/features/review/diff-scroll";
import { BUG, COPY, GAUGE, iconMarkup, NO_TEST, WRENCH } from "@/features/review/signals/signal-icons";

export type LineSide = "old" | "new";

/** Something to draw under one line of one side. Its key names the block React renders into. */
export interface LineAnnotation {
  key: string;
  side: LineSide;
  line: number;
  /** Drawn at the end of the line instead of under it, such as the limit marker; only on the new side. */
  inline?: boolean;
}

/** What the signal lane marks on one line of the new side. */
export interface LaneMark {
  line: number;
  /** The first line of a function over a limit. */
  limit?: boolean;
  /** The first line of a function no test runs. */
  untested?: boolean;
  /** A static check finding is on the line. */
  check?: boolean;
  /** Duplicated code the review introduces starts on the line. */
  duplicate?: boolean;
  /** A mutant that survived the last mutation run is on the line. */
  mutant?: boolean;
  /** The line is one a model finding cites: where it falls in the cited lines. */
  range?: "start" | "middle" | "end" | "single";
}

/**
 * What one editor shows: the unified view (the new side as its document, the old side as deleted chunks), or one
 * side of the side-by-side view.
 */
export type EditorRole = "unified" | LineSide;

/** Where blocks are drawn: told when a block's element appears and goes, so React can render into it. */
export interface AnnotationSlots {
  attach: (key: string, element: HTMLElement, view: EditorView) => void;
  detach: (key: string, element: HTMLElement) => void;
}

/** A point in the document and the side of it a block goes: before the line there (0) or after the line (1). */
interface Anchor {
  pos: number;
  side: 0 | 1;
}

/** Where the block of a line goes in an editor, or null when that editor does not show that side. */
export function anchorOf(state: EditorState, role: EditorRole, side: LineSide, line: number): Anchor | null {
  if (role !== "unified") {
    if (role !== side) return null;
    return { pos: state.doc.line(clampLine(line, state.doc.lines)).to, side: 1 };
  }
  if (side === "new") return { pos: state.doc.line(clampLine(line, state.doc.lines)).to, side: 1 };
  const original = getOriginalDoc(state);
  const old = original.line(clampLine(line, original.lines)).from;
  const chunks = getChunks(state)?.chunks ?? [];
  const removed = chunks.find((chunk) => old >= chunk.fromA && old < chunk.toA);
  // A removed line is drawn in its chunk's block, before the new lines of the chunk: the note goes right after
  // that block (which sits at the same point with side -1).
  if (removed) return { pos: removed.fromB, side: 0 };
  const position = Math.min(oldToNew(chunks, old), state.doc.length);
  return { pos: state.doc.lineAt(position).to, side: 1 };
}

class AnnotationWidget extends WidgetType {
  constructor(
    readonly key: string,
    readonly slots: AnnotationSlots,
    readonly inline = false,
  ) {
    super();
  }

  eq(other: AnnotationWidget) {
    return other.key === this.key && other.slots === this.slots && other.inline === this.inline;
  }

  toDOM(view: EditorView) {
    const element = document.createElement(this.inline ? "span" : "div");
    element.className = this.inline ? "cm-annotation cm-annotation-inline" : "cm-annotation";
    element.dataset.key = this.key;
    this.slots.attach(this.key, element, view);
    return element;
  }

  destroy(element: HTMLElement) {
    this.slots.detach(this.key, element);
  }

  // What happens inside (typing, buttons, selecting a comment's text) is not the editor's business.
  ignoreEvent() {
    return true;
  }
}

/** Replaces the blocks an editor draws. */
export const setAnnotations = StateEffect.define<readonly LineAnnotation[]>();

function build(
  state: EditorState,
  role: EditorRole,
  slots: AnnotationSlots,
  annotations: readonly LineAnnotation[],
): DecorationSet {
  const ranges = annotations.flatMap((annotation) => {
    if (annotation.inline) {
      if (annotation.side !== "new" || role === "old") return [];
      const pos = state.doc.line(clampLine(annotation.line, state.doc.lines)).to;
      const widget = new AnnotationWidget(annotation.key, slots, true);
      return [Decoration.widget({ widget, side: 1 }).range(pos)];
    }
    const anchor = anchorOf(state, role, annotation.side, annotation.line);
    if (!anchor) return [];
    const widget = new AnnotationWidget(annotation.key, slots);
    return [Decoration.widget({ widget, block: true, side: anchor.side }).range(anchor.pos)];
  });
  return Decoration.set(ranges, true);
}

class LaneMarker extends GutterMarker {
  constructor(readonly mark: LaneMark) {
    super();
  }

  eq(other: LaneMarker) {
    const [a, b] = [this.mark, other.mark];
    return (
      a.line === b.line &&
      a.limit === b.limit &&
      a.untested === b.untested &&
      a.check === b.check &&
      a.duplicate === b.duplicate &&
      a.mutant === b.mutant &&
      a.range === b.range
    );
  }

  toDOM() {
    const element = document.createElement("span");
    element.className = "cm-signal";
    element.setAttribute("aria-hidden", "true");
    if (this.mark.range) element.dataset.range = this.mark.range;
    // The most telling mark of the line: a limit, then no test, then a check, then duplicated code, then a mutant
    // that survived. The shapes are fixed strings.
    const [kind, icon] = this.mark.limit
      ? ["limit", GAUGE]
      : this.mark.untested
        ? ["untested", NO_TEST]
        : this.mark.check
          ? ["check", WRENCH]
          : this.mark.duplicate
            ? ["duplicate", COPY]
            : this.mark.mutant
              ? ["mutant", BUG]
              : [null, null];
    if (kind && icon) {
      element.dataset.mark = kind;
      element.innerHTML = iconMarkup(icon);
    }
    return element;
  }
}

/** Replaces the marks of the signal lane. */
export const setLane = StateEffect.define<readonly LaneMark[]>();

function laneMarkers(state: EditorState, marks: readonly LaneMark[]): RangeSet<GutterMarker> {
  const ranges = marks.map((mark) =>
    new LaneMarker(mark).range(state.doc.line(clampLine(mark.line, state.doc.lines)).from),
  );
  return RangeSet.of(ranges, true);
}

/** The signal lane of an editor showing the new side, starting with `initial`. */
export function signalLane(initial: readonly LaneMark[]): Extension {
  const field = StateField.define<RangeSet<GutterMarker>>({
    create: (state) => laneMarkers(state, initial),
    update(value, transaction) {
      for (const effect of transaction.effects) {
        if (effect.is(setLane)) return laneMarkers(transaction.state, effect.value);
      }
      return value.map(transaction.changes);
    },
  });
  return [
    field,
    gutter({
      class: "cm-signalLane",
      markers: (view) => view.state.field(field),
      initialSpacer: () => new LaneMarker({ line: 1 }),
    }),
  ];
}

/** The blocks of one editor, starting with `initial`. Goes after the merge extensions, whose chunks it reads. */
export function annotations(
  role: EditorRole,
  slots: AnnotationSlots,
  initial: readonly LineAnnotation[],
): Extension {
  const field = StateField.define<{ list: readonly LineAnnotation[]; decorations: DecorationSet }>({
    create: (state) => ({ list: initial, decorations: build(state, role, slots, initial) }),
    update(value, transaction) {
      let list = value.list;
      for (const effect of transaction.effects) {
        if (effect.is(setAnnotations)) list = effect.value;
      }
      return list === value.list
        ? value
        : { list, decorations: build(transaction.state, role, slots, list) };
    },
    provide: (own) => EditorView.decorations.from(own, (value) => value.decorations),
  });
  return field;
}

/**
 * The stretches of unchanged lines `collapseUnchanged` hides in an editor, worked out as @codemirror/merge does
 * (it does not expose them): from `margin` lines after a chunk to `margin` lines before the next, when that leaves at
 * least `minSize` lines.
 */
function collapsedStretches(state: EditorState, margin: number, minSize: number): { from: number; to: number }[] {
  const merge = getChunks(state);
  if (!merge) return [];
  const sideA = merge.side === "a";
  const stretches: { from: number; to: number }[] = [];
  let previous = 1;
  for (let index = 0; ; index++) {
    const chunk = index < merge.chunks.length ? merge.chunks[index] : null;
    const from = index ? previous + margin : 1;
    const to = chunk ? state.doc.lineAt(sideA ? chunk.fromA : chunk.fromB).number - 1 - margin : state.doc.lines;
    if (to - from + 1 >= minSize) stretches.push({ from: state.doc.line(from).from, to: state.doc.line(to).to });
    if (!chunk) break;
    previous = state.doc.lineAt(Math.min(state.doc.length, sideA ? chunk.toA : chunk.toB)).number;
  }
  return stretches;
}

export interface Collapse {
  margin: number;
  minSize: number;
}

/**
 * Which collapsed stretches, counted from the top, would hide one of the blocks of an editor. Both editors of the
 * side-by-side view collapse the same stretches in the same order, so the count is the same on either side.
 */
export function hidingStretches(
  state: EditorState,
  role: EditorRole,
  list: readonly LineAnnotation[],
  collapse: Collapse,
): number[] {
  const stretches = collapsedStretches(state, collapse.margin, collapse.minSize);
  return list.flatMap((annotation) => {
    const anchor = anchorOf(state, role, annotation.side, annotation.line);
    if (!anchor) return [];
    const index = stretches.findIndex(({ from, to }) => anchor.pos >= from && anchor.pos <= to);
    return index < 0 ? [] : [index];
  });
}

/** Effects that expand those stretches of an editor, so a comment on an unchanged line stays in sight. */
export function expandStretches(state: EditorState, indices: Iterable<number>, collapse: Collapse): StateEffect<number>[] {
  const stretches = collapsedStretches(state, collapse.margin, collapse.minSize);
  return [...new Set(indices)].flatMap((index) => {
    const stretch = stretches[index];
    return stretch ? [uncollapseUnchanged.of(stretch.from)] : [];
  });
}

/**
 * The line a click landed on, or null when it was not on a line of code: on a block drawn under a line, on the
 * "unchanged lines" bar, on a link or a control.
 */
export function clickedLine(view: EditorView, role: EditorRole, target: Element): { side: LineSide; line: number } | null {
  if (!view.contentDOM.contains(target)) return null;
  if (target.closest(".cm-annotation, .cm-collapsedLines, a, button, input, textarea")) return null;
  const removed = target.closest<HTMLElement>(".cm-deletedLine");
  if (removed) {
    const block = removed.closest<HTMLElement>(".cm-deletedChunk");
    if (!block || role !== "unified") return null;
    const at = view.posAtDOM(block);
    const chunk = (getChunks(view.state)?.chunks ?? []).find((candidate) => candidate.fromB === at);
    if (!chunk) return null;
    const index = [...block.children].filter((child) => child.classList.contains("cm-deletedLine")).indexOf(removed);
    return { side: "old", line: getOriginalDoc(view.state).lineAt(chunk.fromA).number + index };
  }
  const line = target.closest<HTMLElement>(".cm-line");
  if (!line) return null;
  return { side: role === "old" ? "old" : "new", line: view.state.doc.lineAt(view.posAtDOM(line)).number };
}
