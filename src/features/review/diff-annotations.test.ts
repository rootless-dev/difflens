import { unifiedMergeView } from "@codemirror/merge";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { describe, expect, it } from "vitest";

import { anchorOf, annotations, clickedLine, setAnnotations } from "@/features/review/diff-annotations";

// old:  a / b / c / d / e        new:  a / B / c / x / d / e
// Line 2 is replaced, x is added after c, nothing else moves.
const OLD = "a\nb\nc\nd\ne\n";
const NEW = "a\nB\nc\nx\nd\ne\n";

function unified() {
  return EditorState.create({ doc: NEW, extensions: [unifiedMergeView({ original: OLD, mergeControls: false })] });
}

const endOf = (state: EditorState, line: number) => state.doc.line(line).to;

describe("where a note under a line goes", () => {
  it("in the unified view, under a new line, or under the block of removed lines that holds an old one", () => {
    const state = unified();
    expect(anchorOf(state, "unified", "new", 4)).toEqual({ pos: endOf(state, 4), side: 1 });
    // Old line 2 was replaced: its note follows the removed block, drawn where the chunk starts on the new side.
    expect(anchorOf(state, "unified", "old", 2)).toEqual({ pos: state.doc.line(2).from, side: 0 });
    // Old line 4 ("d") is unchanged and is new line 5.
    expect(anchorOf(state, "unified", "old", 4)).toEqual({ pos: endOf(state, 5), side: 1 });
    // Past the end, kept on the last line.
    expect(anchorOf(state, "unified", "new", 99)).toEqual({ pos: endOf(state, state.doc.lines), side: 1 });
  });

  it("draws a block React can render into, and tells the line a click lands on", () => {
    const attached: string[] = [];
    const view = new EditorView({
      parent: document.body,
      state: EditorState.create({
        doc: NEW,
        extensions: [
          unifiedMergeView({ original: OLD, mergeControls: false }),
          annotations(
            "unified",
            { attach: (key) => attached.push(key), detach: () => {} },
            [{ key: "new:3", side: "new", line: 3 }],
          ),
        ],
      }),
    });
    try {
      const block = view.dom.querySelector<HTMLElement>(".cm-annotation");
      expect(block?.dataset.key).toBe("new:3");
      expect(attached).toEqual(["new:3"]);
      // The block sits after line 3, before line 4.
      const lines = [...view.contentDOM.querySelectorAll(".cm-line")];
      expect(lines[2].nextElementSibling).toBe(block);

      expect(clickedLine(view, "unified", lines[3])).toEqual({ side: "new", line: 4 });
      expect(clickedLine(view, "unified", block!)).toBeNull();
      const removed = view.contentDOM.querySelector(".cm-deletedChunk .cm-deletedLine");
      expect(removed).not.toBeNull();
      expect(clickedLine(view, "unified", removed!)).toEqual({ side: "old", line: 2 });

      // A removed line's note comes right after the block of removed lines.
      view.dispatch({ effects: setAnnotations.of([{ key: "old:2", side: "old", line: 2 }]) });
      const under = view.dom.querySelector<HTMLElement>(".cm-annotation");
      expect(under?.dataset.key).toBe("old:2");
      expect(under?.previousElementSibling).toHaveClass("cm-deletedChunk");

      view.dispatch({ effects: setAnnotations.of([]) });
      expect(view.dom.querySelector(".cm-annotation")).toBeNull();
    } finally {
      view.destroy();
    }
  });

  it("side by side, only in the editor of its own side", () => {
    const old = EditorState.create({ doc: OLD });
    expect(anchorOf(old, "old", "old", 2)).toEqual({ pos: endOf(old, 2), side: 1 });
    expect(anchorOf(old, "old", "new", 2)).toBeNull();
    const fresh = EditorState.create({ doc: NEW });
    expect(anchorOf(fresh, "new", "new", 4)).toEqual({ pos: endOf(fresh, 4), side: 1 });
    expect(anchorOf(fresh, "new", "old", 4)).toBeNull();
  });
});
