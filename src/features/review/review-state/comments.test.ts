import { describe, expect, it } from "vitest";

import {
  authorOf,
  isOpen,
  lineKey,
  lineThreadsByFile,
  parseLineKey,
  threadsByLine,
  threadsOf,
} from "@/features/review/review-state/comments";
import { makeComment, makeGeneralComment } from "@/test/review-fixtures";

describe("comment threads", () => {
  it("groups replies under their first comment, oldest first, and drops orphans", () => {
    const comments = [
      makeComment("a"),
      makeComment("b", { line: 4, currentLine: 4 }),
      makeComment("a1", { parentId: "a" }),
      makeComment("a2", { parentId: "a" }),
      makeComment("lost", { parentId: "gone" }),
    ];
    expect(threadsOf(comments).map((thread) => [thread.root.id, thread.replies.map((reply) => reply.id)])).toEqual([
      ["a", ["a1", "a2"]],
      ["b", []],
    ]);
  });

  it("places threads under the line they sit on now, for one file and the sides shown", () => {
    const threads = threadsOf([
      makeComment("moved", { line: 2, currentLine: 5 }),
      makeComment("same line", { line: 5, currentLine: 5 }),
      makeComment("old side", { side: "old", currentLine: 5 }),
      makeComment("other file", { path: "main.go" }),
      makeComment("file gone", { currentLine: null }),
    ]);
    const both = threadsByLine(threads, "go.mod", new Set(["old", "new"]));
    expect([...both].map(([key, at]) => [key, at.map((thread) => thread.root.id)])).toEqual([
      ["new:5", ["moved", "same line"]],
      ["old:5", ["old side"]],
    ]);
    expect([...threadsByLine(threads, "go.mod", new Set(["new"])).keys()]).toEqual(["new:5"]);
  });

  it("groups line threads by file in the file list's order, each by line, and tells open ones apart", () => {
    const threads = threadsOf([
      makeGeneralComment("general"),
      makeComment("late", { path: "b.go", line: 30, currentLine: 30 }),
      makeComment("early", { path: "b.go", line: 40, currentLine: 3 }),
      makeComment("first file", { path: "a.go" }),
      makeComment("outside", { path: "z.go", currentLine: null }),
      makeComment("also outside", { path: "c.go", currentLine: null }),
    ]);
    expect(
      lineThreadsByFile(threads, ["b.go", "a.go"]).map((group) => [
        group.path,
        group.threads.map((thread) => thread.root.id),
      ]),
    ).toEqual([
      ["b.go", ["early", "late"]],
      ["a.go", ["first file"]],
      ["c.go", ["also outside"]],
      ["z.go", ["outside"]],
    ]);
    const [open, resolved, deleted] = threadsOf([
      makeComment("open"),
      makeComment("resolved", { resolved: true }),
      makeComment("deleted", { pendingChange: "delete" }),
    ]);
    expect([isOpen(open), isOpen(resolved), isOpen(deleted)]).toEqual([true, false, false]);
  });

  it("keys lines by side and number", () => {
    expect(lineKey("old", 12)).toBe("old:12");
    expect(parseLineKey("old:12")).toEqual({ side: "old", line: 12 });
    expect(parseLineKey(lineKey("new", 3))).toEqual({ side: "new", line: 3 });
  });

  it("names the user's own comments You, published ones included", () => {
    expect(authorOf(makeComment("mine"))).toBe("You");
    expect(authorOf(makeComment("published", { remoteAuthorId: "{me}", remoteAuthorName: "Dana" }))).toBe("You");
    expect(
      authorOf(makeComment("theirs", { remoteAuthorId: "u-9", remoteAuthorName: "Bruno", mine: false })),
    ).toBe("Bruno");
  });
});
