import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import DiffEditor from "@/features/review/DiffEditor";

const OLD = "a\nb\nc\n";
const NEW = "a\nB\nc\n";

function renderEditor(props: Partial<Parameters<typeof DiffEditor>[0]> = {}) {
  return render(
    <DiffEditor
      oldText={OLD}
      newText={NEW}
      language={null}
      path="notes.txt"
      layout="unified"
      collapseUnchanged={false}
      {...props}
    />,
  );
}

const codeLines = (container: HTMLElement) => [...container.querySelectorAll<HTMLElement>(".cm-content .cm-line")];

describe("diff editor notes and line clicks", () => {
  it("renders the notes of a line inside the editor, under that line", async () => {
    const { container } = renderEditor({
      annotations: [{ key: "new:1", side: "new", line: 1 }],
      renderAnnotation: (key) => <p>Note for {key}</p>,
    });

    const note = await screen.findByText("Note for new:1");
    expect(note.closest(".cm-annotation")).not.toBeNull();
    expect(codeLines(container)[0].nextElementSibling).toBe(note.closest(".cm-annotation"));
  });

  it("reports a plain click on a line, on either layout, and nothing for a click inside a note", async () => {
    const onLineClick = vi.fn();
    const { container, rerender } = renderEditor({
      onLineClick,
      annotations: [{ key: "new:3", side: "new", line: 3 }],
      renderAnnotation: () => <button type="button">Inside the note</button>,
    });
    await waitFor(() => expect(codeLines(container)).not.toHaveLength(0));

    fireEvent.click(codeLines(container)[2], { detail: 1 });
    expect(onLineClick).toHaveBeenLastCalledWith("new", 3);
    fireEvent.click(await screen.findByRole("button", { name: "Inside the note" }), { detail: 1 });
    // A double click selects a word; it is not a request to comment.
    fireEvent.click(codeLines(container)[0], { detail: 2 });
    expect(onLineClick).toHaveBeenCalledTimes(1);

    rerender(
      <DiffEditor
        oldText={OLD}
        newText={NEW}
        language={null}
        path="notes.txt"
        layout="split"
        collapseUnchanged={false}
        onLineClick={onLineClick}
      />,
    );
    await waitFor(() => expect(container.querySelectorAll(".cm-mergeViewEditor")).toHaveLength(2));
    const [oldSide] = container.querySelectorAll<HTMLElement>(".cm-mergeViewEditor");
    fireEvent.click(oldSide.querySelectorAll(".cm-line")[1], { detail: 1 });
    expect(onLineClick).toHaveBeenLastCalledWith("old", 2);
  });

  it.each(["unified", "split"] as const)(
    "expands the unchanged lines that would hide a note, %s",
    async (layout) => {
      const lines = Array.from({ length: 40 }, (_, index) => `line ${index + 1}`);
      const changed = [...lines];
      changed[0] = "line one";
      const props = { oldText: lines.join("\n"), newText: changed.join("\n"), layout, collapseUnchanged: true };
      const plain = renderEditor(props);
      await waitFor(() => expect(plain.container.querySelector(".cm-collapsedLines")).not.toBeNull());
      plain.unmount();

      const { container } = renderEditor({
        ...props,
        annotations: [{ key: "new:30", side: "new", line: 30 }],
        renderAnnotation: () => <p>Note on line 30</p>,
      });

      expect(await screen.findByText("Note on line 30")).toBeInTheDocument();
      expect(container.querySelector(".cm-collapsedLines")).toBeNull();
    },
  );

  it("draws an inline block at the end of its line, and a click on it is not a click on the line", async () => {
    const onLineClick = vi.fn();
    const { container } = renderEditor({
      onLineClick,
      annotations: [{ key: "inline:2", side: "new", line: 2, inline: true }],
      renderAnnotation: () => <button type="button">CC 14</button>,
    });

    const marker = await screen.findByRole("button", { name: "CC 14" });
    expect(marker.closest(".cm-annotation-inline")?.closest(".cm-line")).toBe(codeLines(container)[1]);
    fireEvent.click(marker, { detail: 1 });
    expect(onLineClick).not.toHaveBeenCalled();
  });

  it.each(["unified", "split"] as const)("marks the signal lane of the new side, %s", async (layout) => {
    const { container, rerender } = renderEditor({
      layout,
      lane: [
        { line: 1, limit: true },
        { line: 2, check: true, range: "start" },
        { line: 3, range: "end" },
      ],
    });

    await waitFor(() => expect(container.querySelectorAll(".cm-signalLane")).toHaveLength(1));
    // Not the lane's spacer, which only gives it its width.
    const marks = () =>
      [...container.querySelectorAll<HTMLElement>(".cm-signalLane .cm-signal")].filter(
        (mark) => mark.closest<HTMLElement>(".cm-gutterElement")?.style.visibility !== "hidden",
      );
    await waitFor(() =>
      expect(marks().map((mark) => [mark.dataset.mark ?? "", mark.dataset.range ?? ""])).toEqual([
        ["limit", ""],
        ["check", "start"],
        ["", "end"],
      ]),
    );
    expect(marks()[0].querySelector("svg")).not.toBeNull();
    // Hiding the checks takes their mark away, in place.
    rerender(
      <DiffEditor
        oldText={OLD}
        newText={NEW}
        language={null}
        path="notes.txt"
        layout={layout}
        collapseUnchanged={false}
        lane={[{ line: 1, limit: true }]}
      />,
    );
    await waitFor(() => expect(marks().map((mark) => mark.dataset.mark ?? "")).toEqual(["limit"]));
  });

  it("draws no signal lane without marks", async () => {
    const { container } = renderEditor();
    await waitFor(() => expect(codeLines(container)).not.toHaveLength(0));
    expect(container.querySelector(".cm-signalLane")).toBeNull();
  });

  it("does not report the click that ends a text selection", async () => {
    const onLineClick = vi.fn();
    const { container } = renderEditor({ onLineClick });
    await waitFor(() => expect(codeLines(container)).not.toHaveLength(0));
    const line = codeLines(container)[1];
    const selection = window.getSelection()!;
    selection.selectAllChildren(line);

    fireEvent.click(line, { detail: 1 });

    expect(onLineClick).not.toHaveBeenCalled();
    selection.removeAllRanges();
  });
});
