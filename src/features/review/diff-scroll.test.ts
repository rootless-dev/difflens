import { describe, expect, it } from "vitest";

import { clampLine, oldToNew } from "@/features/review/diff-scroll";

// Old: 10 lines of 10 characters. New: line 3 replaced by two lines, line 7 removed.
const chunks = [
  { fromA: 20, toA: 30, fromB: 20, toB: 40 },
  { fromA: 60, toA: 70, fromB: 70, toB: 70 },
];

describe("old side positions in a unified diff", () => {
  it("is unchanged before the first chunk", () => {
    expect(oldToNew(chunks, 10)).toBe(10);
  });

  it("is the start of the chunk inside a changed chunk", () => {
    expect(oldToNew(chunks, 25)).toBe(20);
    expect(oldToNew(chunks, 60)).toBe(70);
  });

  it("is shifted by what the chunks before it added and removed", () => {
    expect(oldToNew(chunks, 40)).toBe(50);
    expect(oldToNew(chunks, 80)).toBe(80);
    expect(oldToNew([], 33)).toBe(33);
  });
});

describe("line clamping", () => {
  it("keeps a line inside the document", () => {
    expect(clampLine(0, 10)).toBe(1);
    expect(clampLine(12, 10)).toBe(10);
    expect(clampLine(4, 10)).toBe(4);
    expect(clampLine(3, 0)).toBe(1);
  });
});
