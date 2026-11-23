// The only module that touches CodeMirror. It is loaded lazily (React.lazy) so the editor and the
// language parsers stay out of the main bundle, and tests replace it with a stub.

import { LanguageDescription, type LanguageSupport } from "@codemirror/language";
import { getChunks, getOriginalDoc, MergeView, unifiedMergeView } from "@codemirror/merge";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView, lineNumbers } from "@codemirror/view";
import { type MouseEvent, type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";

import {
  type AnnotationSlots,
  annotations as annotationBlocks,
  clickedLine,
  type EditorRole,
  expandStretches,
  hidingStretches,
  type LaneMark,
  type LineAnnotation,
  type LineSide,
  setAnnotations,
  setLane,
  signalLane,
} from "@/features/review/diff-annotations";
import { clampLine, oldToNew, type ScrollTarget } from "@/features/review/diff-scroll";
import { editorTheme, syntaxTheme } from "@/features/review/editor-theme";
import { baseName } from "@/features/review/file-tree";
import { cn } from "@/lib/utils";

export type DiffLayout = "unified" | "split";

export type { LaneMark, LineAnnotation, LineSide };

export interface DiffEditorProps {
  oldText: string;
  newText: string;
  /** Language id from ChangedFile.language; the file name is tried first. */
  language: string | null;
  path: string;
  layout: DiffLayout;
  /** Collapse unchanged stretches into "N unchanged lines hidden" rows that expand on click. */
  collapseUnchanged: boolean;
  /** Line to bring to the top once the diff is built, and again whenever it changes. */
  scrollTo?: ScrollTarget | null;
  /** Lines with something drawn under them, such as comment threads; `renderAnnotation` draws it. */
  annotations?: readonly LineAnnotation[];
  renderAnnotation?: (key: string) => ReactNode;
  /** What the signal lane marks, on lines of the new side; no lane is drawn without any. */
  lane?: readonly LaneMark[];
  /**
   * A plain click on a line of code. Not called for a click that ends a text selection, a double click, or a click
   * on a link, a control, a drawn block or the "unchanged lines" bar, so selecting code to copy it still works.
   */
  onLineClick?: (side: LineSide, line: number) => void;
}

const NO_ANNOTATIONS: readonly LineAnnotation[] = [];
const NO_LANE: readonly LaneMark[] = [];

/** The blocks an editor of this role draws. */
const forRole = (list: readonly LineAnnotation[], role: EditorRole) =>
  role === "unified" ? list : list.filter((annotation) => annotation.side === role);

/**
 * The elements CodeMirror created for the blocks, by key, kept in React state so their content is rendered into
 * them with portals. A block's element is created again when it scrolls back into view; its height is measured again
 * whenever its content resizes.
 */
function useSlots(): { slots: AnnotationSlots; elements: ReadonlyMap<string, HTMLElement> } {
  const [elements, setElements] = useState<ReadonlyMap<string, HTMLElement>>(() => new Map());
  const slots = useMemo<AnnotationSlots>(() => {
    const observers = new Map<HTMLElement, ResizeObserver>();
    return {
      attach(key, element, view) {
        setElements((current) => new Map(current).set(key, element));
        if (typeof ResizeObserver !== "undefined") {
          const observer = new ResizeObserver(() => view.requestMeasure());
          observer.observe(element);
          observers.set(element, observer);
        }
      },
      detach(key, element) {
        observers.get(element)?.disconnect();
        observers.delete(element);
        setElements((current) => {
          // The element drawn anew may have been attached before this one was let go.
          if (current.get(key) !== element) return current;
          const next = new Map(current);
          next.delete(key);
          return next;
        });
      },
    };
  }, []);
  return { slots, elements };
}

/** The editors of a built diff: one for the unified layout, the old and new sides for side by side. */
type Built = { layout: "unified"; view: EditorView } | { layout: "split"; old: EditorView; new: EditorView };

// Leaves the line just below the sticky file header rather than flush against it.
const SCROLL_MARGIN = 48;

function scrollToLine(built: Built, target: ScrollTarget): void {
  let view: EditorView;
  let position: number;
  if (built.layout === "split") {
    view = target.side === "old" ? built.old : built.new;
    position = view.state.doc.line(clampLine(target.line, view.state.doc.lines)).from;
  } else {
    view = built.view;
    const { state } = view;
    if (target.side === "new") {
      position = state.doc.line(clampLine(target.line, state.doc.lines)).from;
    } else {
      const original = getOriginalDoc(state);
      const old = original.line(clampLine(target.line, original.lines)).from;
      const chunks = getChunks(state)?.chunks ?? [];
      position = Math.min(oldToNew(chunks, old), state.doc.length);
      const chunk = chunks.find((candidate) => old >= candidate.fromA && old < candidate.toA);
      if (chunk) {
        // The line is among removed lines, drawn as one block above `fromB`: once the scroll has drawn
        // that block, bring the line itself into view.
        const index = original.lineAt(old).number - original.lineAt(chunk.fromA).number;
        view.dispatch({ effects: EditorView.scrollIntoView(position, { y: "start", yMargin: SCROLL_MARGIN }) });
        requestAnimationFrame(() => {
          const blocks = [...view.dom.querySelectorAll<HTMLElement>(".cm-deletedChunk")];
          const block = blocks.find((element) => view.posAtDOM(element) === chunk.fromB);
          block?.children[index]?.scrollIntoView({ block: "center" });
        });
        return;
      }
    }
  }
  view.dispatch({ effects: EditorView.scrollIntoView(position, { y: "start", yMargin: SCROLL_MARGIN }) });
}

const COLLAPSE = { margin: 3, minSize: 4 };
// Caps the time spent diffing a large, heavily changed file; the result may then be less minimal.
const DIFF_CONFIG = { scanLimit: 500, timeout: 500 };

/**
 * Finds the parser for a file: by file name first (it tells .tsx from .ts), then by the language id.
 * Resolves to null when none is known; the diff is then shown without highlighting.
 */
async function loadLanguage(language: string | null, path: string): Promise<LanguageSupport | null> {
  const { languages } = await import("@codemirror/language-data");
  const description =
    LanguageDescription.matchFilename(languages, baseName(path)) ??
    (language ? LanguageDescription.matchLanguageName(languages, language, false) : null);
  return description ? description.load() : null;
}

/** CodeMirror's own words, as DiffLens says them; `$` is the count. */
interface Phrases {
  unchangedLines: string;
}

function sideExtensions(label: string, phrases: Phrases, support: LanguageSupport | null): Extension[] {
  return [
    lineNumbers(),
    EditorState.readOnly.of(true),
    EditorView.editable.of(false),
    EditorState.phrases.of({ "$ unchanged lines": phrases.unchangedLines }),
    EditorView.contentAttributes.of({ "aria-label": label }),
    editorTheme,
    syntaxTheme,
    support ?? [],
  ];
}

/**
 * Expands the collapsed unchanged lines that would hide a block, in every editor of the diff alike so the two sides
 * of the side-by-side view stay level.
 */
function reveal(built: Built, list: readonly LineAnnotation[]) {
  const editors = editorsOf(built);
  const hiding = editors.flatMap(([view, role]) => hidingStretches(view.state, role, forRole(list, role), COLLAPSE));
  if (hiding.length === 0) return;
  for (const [view] of editors) {
    const effects = expandStretches(view.state, hiding, COLLAPSE);
    if (effects.length > 0) view.dispatch({ effects });
  }
}

/** Each editor of a built diff with the side it shows. */
function editorsOf(built: Built): [EditorView, EditorRole][] {
  return built.layout === "unified"
    ? [[built.view, "unified"]]
    : [
        [built.old, "old"],
        [built.new, "new"],
      ];
}

export default function DiffEditor({
  oldText,
  newText,
  language,
  path,
  layout,
  collapseUnchanged,
  scrollTo = null,
  annotations = NO_ANNOTATIONS,
  renderAnnotation,
  lane = NO_LANE,
  onLineClick,
}: DiffEditorProps) {
  // The editor is built with the words of the language shown, and built again when it changes.
  const { t } = useTranslation();
  const host = useRef<HTMLDivElement>(null);
  // undefined while the parser loads, so the editor is built once, already highlighted.
  const [support, setSupport] = useState<LanguageSupport | null | undefined>(undefined);
  const built = useRef<Built | null>(null);
  // Read when the editor is (re)built, so a rebuild for another layout keeps the line in view.
  const scrollTarget = useRef(scrollTo);
  // Likewise the blocks: a rebuilt editor starts with the current ones, later changes are dispatched to it.
  const annotationList = useRef(annotations);
  // The lane too; whether there is one at all is part of how the editor is built.
  const laneList = useRef(lane);
  const hasLane = lane.length > 0;
  const { slots, elements } = useSlots();

  useEffect(() => {
    if (laneList.current === lane) return;
    laneList.current = lane;
    if (!built.current) return;
    const view = built.current.layout === "unified" ? built.current.view : built.current.new;
    // An editor built without a lane has no field to take the marks; it is built again when one arrives.
    if (hasLane) view.dispatch({ effects: setLane.of(lane) });
  }, [lane, hasLane]);

  useEffect(() => {
    scrollTarget.current = scrollTo;
    if (built.current && scrollTo) scrollToLine(built.current, scrollTo);
  }, [scrollTo]);

  useEffect(() => {
    if (annotationList.current === annotations) return;
    annotationList.current = annotations;
    if (!built.current) return;
    for (const [view, role] of editorsOf(built.current)) {
      view.dispatch({ effects: setAnnotations.of(forRole(annotations, role)) });
    }
    if (collapseUnchanged) reveal(built.current, annotations);
  }, [annotations, collapseUnchanged]);

  function onClick(event: MouseEvent<HTMLDivElement>) {
    if (!onLineClick || !built.current || event.detail > 1 || !(event.target instanceof Element)) return;
    // The click that ends a drag selecting text is not a click on a line.
    const selection = window.getSelection();
    if (selection && !selection.isCollapsed && host.current?.contains(selection.anchorNode)) return;
    for (const [view, role] of editorsOf(built.current)) {
      const line = clickedLine(view, role, event.target);
      if (line) {
        onLineClick(line.side, line.line);
        return;
      }
    }
  }

  useEffect(() => {
    let cancelled = false;
    setSupport(undefined);
    loadLanguage(language, path).then(
      (loaded) => {
        if (!cancelled) setSupport(loaded);
      },
      () => {
        if (!cancelled) setSupport(null);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [language, path]);

  useEffect(() => {
    const parent = host.current;
    if (!parent || support === undefined) return;
    const collapse = collapseUnchanged ? COLLAPSE : undefined;
    const phrases: Phrases = { unchangedLines: t("review.editor.unchangedLines") };

    const blocks = (role: EditorRole) => annotationBlocks(role, slots, forRole(annotationList.current, role));
    const laneOf = () => (hasLane ? signalLane(laneList.current) : []);

    if (layout === "unified") {
      const view = new EditorView({
        parent,
        state: EditorState.create({
          doc: newText,
          extensions: [
            sideExtensions(t("review.editor.changesTo", { path }), phrases, support),
            unifiedMergeView({
              original: oldText,
              mergeControls: false,
              gutter: true,
              syntaxHighlightDeletions: true,
              collapseUnchanged: collapse,
              diffConfig: DIFF_CONFIG,
            }),
            blocks("unified"),
            laneOf(),
          ],
        }),
      });
      return show({ layout: "unified", view }, () => view.destroy());
    }

    const view = new MergeView({
      parent,
      a: {
        doc: oldText,
        extensions: [sideExtensions(t("review.editor.oldVersion", { path }), phrases, support), blocks("old")],
      },
      b: {
        doc: newText,
        extensions: [
          sideExtensions(t("review.editor.newVersion", { path }), phrases, support),
          blocks("new"),
          laneOf(),
        ],
      },
      gutter: true,
      collapseUnchanged: collapse,
      diffConfig: DIFF_CONFIG,
    });
    return show({ layout: "split", old: view.a, new: view.b }, () => view.destroy());

    function show(editors: Built, destroy: () => void) {
      built.current = editors;
      if (collapse) reveal(editors, annotationList.current);
      if (scrollTarget.current) scrollToLine(editors, scrollTarget.current);
      return () => {
        built.current = null;
        destroy();
      };
    }
  }, [oldText, newText, path, layout, collapseUnchanged, support, slots, t, hasLane]);

  return (
    <div
      ref={host}
      data-layout={layout}
      onClick={onClick}
      className={cn(
        "h-full min-h-0 [&_.cm-mergeView]:h-full [&_.cm-mergeViewEditor+.cm-mergeViewEditor]:border-l",
        onLineClick &&
          "[&_.cm-deletedLine:hover]:shadow-[inset_2px_0_0_var(--primary)] [&_.cm-line:hover]:shadow-[inset_2px_0_0_var(--primary)]",
      )}
    >
      {[...elements].map(([key, element]) => createPortal(renderAnnotation?.(key), element, key))}
    </div>
  );
}
