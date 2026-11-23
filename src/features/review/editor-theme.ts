import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { EditorView } from "@codemirror/view";
import { tags as t } from "@lezer/highlight";

// Every colour is a CSS variable from index.css, so one theme follows the app into dark and light
// without rebuilding the editor. The selectors also override the merge package's base theme,
// which has its own light and dark colours.

const MONO = '"IBM Plex Mono", ui-monospace, monospace';
const SANS = '"IBM Plex Sans", system-ui, sans-serif';
const LINE_PADDING = "0 16px 0 8px";

export const editorTheme = EditorView.theme({
  "&": {
    height: "100%",
    backgroundColor: "var(--background)",
    color: "var(--foreground)",
    fontSize: "12px",
  },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": { fontFamily: MONO, lineHeight: "1.65" },
  ".cm-content": { padding: "0 0 16px", caretColor: "transparent" },
  // The blocks under lines (comments and their text boxes) sit inside the content and would inherit the hidden
  // caret: someone typing a comment has to see where the text goes.
  ".cm-annotation": { caretColor: "auto" },
  ".cm-line": { padding: LINE_PADDING },
  ".cm-content ::selection, .cm-line::selection": { backgroundColor: "var(--primary-soft)" },

  ".cm-gutters": {
    backgroundColor: "var(--background)",
    color: "var(--faint)",
    border: "none",
  },
  ".cm-lineNumbers .cm-gutterElement": {
    minWidth: "44px",
    padding: "0 10px 0 0",
    textAlign: "right",
    userSelect: "none",
  },

  // Added lines: the new editor of the unified view, and the right editor side by side.
  "&.cm-merge-b .cm-changedLine": { backgroundColor: "var(--add-bg)" },
  "&.cm-merge-b .cm-changedText": { background: "var(--add-text-bg)" },
  // Removed lines: the left editor side by side, and the deleted chunks shown inline in the unified view.
  "&.cm-merge-a .cm-changedLine": { backgroundColor: "var(--del-bg)" },
  "&.cm-merge-a .cm-changedText": { background: "var(--del-text-bg)" },
  ".cm-deletedChunk": { backgroundColor: "var(--del-bg)", paddingLeft: "0" },
  ".cm-deletedChunk .cm-deletedLine": { padding: LINE_PADDING },
  ".cm-deletedChunk .cm-deletedText": { background: "var(--del-text-bg)" },
  ".cm-insertedLine, .cm-deletedLine, .cm-deletedLine del": { textDecoration: "none" },

  // The signal lane, between the numbers and the code: the gauge on the line a function over a limit starts at, the
  // flask on one no test runs, the wrench on a static check's line, all in ink; a thin bar in the model's colour
  // along the lines a finding cites.
  ".cm-signalLane .cm-gutterElement": { width: "16px", position: "relative", display: "grid", placeItems: "center" },
  ".cm-signal": { display: "grid", placeItems: "center", height: "100%", color: "var(--foreground)" },
  ".cm-signal svg": { display: "block" },
  ".cm-signal[data-range]::after": {
    content: '""',
    position: "absolute",
    left: "6px",
    top: "0",
    bottom: "0",
    width: "2px",
    background: "var(--ai)",
  },
  ".cm-signal[data-range=start]::after, .cm-signal[data-range=single]::after": { top: "4px" },
  ".cm-signal[data-range=end]::after, .cm-signal[data-range=single]::after": { bottom: "4px" },
  // Blocks drawn at the end of a line, such as the limit marker.
  ".cm-annotation-inline": { display: "inline-flex", alignItems: "center", gap: "6px", marginLeft: "24px" },

  ".cm-changeGutter": { width: "3px", paddingLeft: "0" },
  "&.cm-merge-b .cm-changedLineGutter": { background: "var(--add)" },
  "&.cm-merge-a .cm-changedLineGutter, .cm-deletedLineGutter": { background: "var(--del)" },

  // The "N unchanged lines hidden" row, as the prototype's fold row.
  ".cm-collapsedLines": {
    padding: "3px 16px",
    fontFamily: SANS,
    fontSize: "12px",
    color: "var(--faint)",
    background: "var(--card)",
    borderTop: "1px solid var(--border)",
    borderBottom: "1px solid var(--border)",
    cursor: "pointer",
  },
  ".cm-collapsedLines:hover": { color: "var(--foreground)" },
  ".cm-collapsedLines:before, .cm-collapsedLines:after": { content: "none" },
});

const highlightStyle = HighlightStyle.define([
  {
    tag: [t.keyword, t.modifier, t.controlKeyword, t.operatorKeyword, t.definitionKeyword, t.moduleKeyword],
    color: "var(--syntax-keyword)",
  },
  { tag: [t.string, t.special(t.string), t.regexp, t.character, t.escape], color: "var(--syntax-string)" },
  { tag: [t.number, t.bool, t.null, t.atom, t.constant(t.name), t.standard(t.name)], color: "var(--syntax-constant)" },
  { tag: [t.typeName, t.className, t.namespace, t.tagName], color: "var(--syntax-type)" },
  { tag: [t.comment, t.lineComment, t.blockComment, t.docComment], color: "var(--faint)", fontStyle: "italic" },
  { tag: [t.punctuation, t.operator, t.bracket], color: "var(--muted-foreground)" },
  { tag: t.invalid, color: "var(--del)" },
  { tag: t.heading, fontWeight: "600" },
  { tag: t.strong, fontWeight: "600" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.link, textDecoration: "underline" },
]);

export const syntaxTheme = syntaxHighlighting(highlightStyle);
