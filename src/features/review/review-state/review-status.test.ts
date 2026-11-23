import { describe, expect, it } from "vitest";

import {
  fileButtonLabel,
  fileMarks,
  functionButtonLabel,
  functionSpans,
  reviewedCount,
  reviewRefOf,
  sinceTitle,
  spanOf,
} from "@/features/review/review-state/review-status";
import { makeChangedFile, makeChangeSet } from "@/test/fixtures";
import { makeFlowNode } from "@/test/flow-fixtures";
import { makeReviewState } from "@/test/review-fixtures";

describe("review status", () => {
  it("names the button after what a press will do", () => {
    expect(fileButtonLabel(null)).toBe("Mark as reviewed");
    expect(fileButtonLabel("reviewed")).toBe("✓ Reviewed");
    expect(fileButtonLabel("reopened")).toBe("Changed since your review. Mark as reviewed again");
    expect(functionButtonLabel(null)).toBe("Mark as reviewed");
    expect(functionButtonLabel("reviewed")).toBe("✓ Reviewed");
    expect(functionButtonLabel("reopened")).toBe("Mark as reviewed again");
  });

  it("counts only the files of the change set reviewed as they are", () => {
    const changeSet = makeChangeSet([makeChangedFile("a.go"), makeChangedFile("b.go"), makeChangedFile("c.go")]);
    const marks = fileMarks(
      makeReviewState({
        files: [
          { path: "a.go", status: "reviewed", reviewedAt: "t" },
          { path: "b.go", status: "reopened", reviewedAt: "t" },
          { path: "gone.go", status: "reviewed", reviewedAt: "t" },
        ],
      }),
    );
    expect(reviewedCount(changeSet, marks)).toBe(1);
    expect(fileMarks(null).size).toBe(0);
  });

  it("refers to a review by its target and the sides its change set froze", () => {
    const changeSet = makeChangeSet([makeChangedFile("a.go")]);
    expect(reviewRefOf(changeSet)).toEqual({
      target: changeSet.target,
      oldSha: changeSet.oldSha,
      newSha: changeSet.newSha,
    });
  });

  it("lets only changed functions with a known place be marked", () => {
    const changed = makeFlowNode("f", { state: "modified", file: "a.go", startLine: 3, endLine: 9 });
    const deleted = makeFlowNode("d", { state: "deleted", file: "b.go", startLine: 1, endLine: 2 });
    expect(spanOf(changed)).toEqual({ id: "f", file: "a.go", startLine: 3, endLine: 9, deleted: false });
    expect(spanOf(deleted)?.deleted).toBe(true);
    expect(spanOf(makeFlowNode("u", { state: "untouched" }))).toBeNull();
    expect(spanOf(makeFlowNode("x", { state: "external", file: null, startLine: null, endLine: null }))).toBeNull();
    expect(spanOf(makeFlowNode("n", { state: "new", startLine: null }))).toBeNull();
    expect(functionSpans([changed, makeFlowNode("u"), deleted]).map((span) => span.id)).toEqual(["f", "d"]);
  });

  it("explains why Since last review cannot be chosen, and what it shows when it can", () => {
    const now = new Date("2026-11-19T12:00:00Z");
    expect(sinceTitle({ kind: "unavailable", reason: "working_tree" }, null)).toMatch(/Uncommitted changes/);
    expect(sinceTitle({ kind: "unavailable", reason: "no_checkpoint" }, null)).toMatch(/^Mark a file or a function/);
    expect(sinceTitle({ kind: "unavailable", reason: "up_to_date" }, null)).toMatch(/^Nothing new/);
    expect(sinceTitle({ kind: "unavailable", reason: "checkpoint_missing" }, null)).toMatch(/rewritten/);
    expect(
      sinceTitle(
        { kind: "available", checkpointSha: "a41c9e2ffff", commits: 2, files: [makeChangedFile("a.go")] },
        "2026-11-19T10:00:00Z",
        now,
      ),
    ).toBe("Only what changed from a41c9e2 (your last review, 2 h ago) to the head: 2 commits, 1 file");
  });
});
