import { describe, expect, it } from "vitest";

import { diffContent } from "@/features/review/diff-content";
import type { FileSide } from "@/lib/api";
import { absentSide, textSide } from "@/test/fixtures";

const side = (reason: FileSide["reason"], size = 100): FileSide => ({ text: null, reason, size });

describe("diffContent", () => {
  it("compares the two texts", () => {
    expect(diffContent({ old: textSide("a\n"), new: textSide("b\n") })).toEqual({
      kind: "text",
      oldText: "a\n",
      newText: "b\n",
    });
  });

  it("reads an absent side as empty, so an added file is all additions and a deleted one all removals", () => {
    expect(diffContent({ old: absentSide, new: textSide("new\n") })).toEqual({
      kind: "text",
      oldText: "",
      newText: "new\n",
    });
    expect(diffContent({ old: textSide("gone\n"), new: absentSide })).toEqual({
      kind: "text",
      oldText: "gone\n",
      newText: "",
    });
  });

  it("says when the content did not change, or the file is empty", () => {
    expect(diffContent({ old: textSide("same"), new: textSide("same") })).toEqual({
      kind: "same",
      message: "The content of this file did not change.",
    });
    expect(diffContent({ old: absentSide, new: textSide("") })).toEqual({ kind: "same", message: "The file is empty." });
  });

  it("explains a binary file", () => {
    expect(diffContent({ old: side("binary"), new: side("binary") }).kind).toBe("unavailable");
    expect(diffContent({ old: side("binary"), new: side("binary") })).toMatchObject({
      message: "This file is binary, so it is not shown.",
    });
    expect(diffContent({ old: absentSide, new: side("binary") })).toMatchObject({
      message: "This file is binary, so it is not shown.",
    });
  });

  it("explains a file over the size limit, with its size", () => {
    expect(diffContent({ old: textSide("small"), new: side("too_large", 3_250_000) })).toMatchObject({
      kind: "unavailable",
      message: "The new version is larger than 2 MB (3.1 MB), so it is not shown.",
    });
  });

  it("explains a file that is not UTF-8, naming each side when they differ", () => {
    expect(diffContent({ old: side("not_utf8"), new: textSide("now utf-8") })).toMatchObject({
      message: "The old version is not UTF-8 text, so it is not shown.",
    });
    expect(diffContent({ old: side("not_utf8"), new: side("binary") })).toMatchObject({
      message: "The old version is not UTF-8 text, so it is not shown. The new version is binary, so it is not shown.",
    });
  });
});
