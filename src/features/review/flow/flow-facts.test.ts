import { describe, expect, it } from "vitest";

import { diffTarget, fileDiffTarget, nodeFacts, relationDiffTarget } from "@/features/review/flow/flow-facts";
import { type FileGroup, groupFunctions } from "@/features/review/flow/flow-groups";
import { makeChangedFile } from "@/test/fixtures";
import { makeFlowEdge, makeFlowNode, REFUND_IDS as id, refundFlowGraph } from "@/test/flow-fixtures";

const graph = refundFlowGraph();
const node = (nodeId: string) => graph.nodes.find((candidate) => candidate.id === nodeId)!;

describe("node facts", () => {
  it("describes a new entry point function", () => {
    expect(nodeFacts(graph, node(id.create))).toEqual({
      title: "refund.Handler.Create",
      stateLabel: "New",
      lines: { additions: 18, deletions: 0 },
      location: "internal/refund/handler.go:24–41",
      summary: "Added in this review.",
      facts: [
        { label: "Cyclomatic complexity", value: "3" },
        { label: "Entry point", value: "POST /refunds" },
        { label: "Callers shown", value: "0" },
        { label: "Calls shown", value: "1, and 1 call removed" },
      ],
    });
  });

  it("gives the previous complexity of a modified function when it changed", () => {
    const facts = nodeFacts(graph, node(id.onSettled));
    expect(facts.facts[0]).toEqual({ label: "Cyclomatic complexity", value: "4 → 5" });
    expect(facts.facts[1]).toEqual({ label: "Entry point", value: "payment.settled" });
    expect(nodeFacts(graph, node(id.get)).facts).toEqual([
      { label: "Cyclomatic complexity", value: "2" },
      { label: "Callers shown", value: "3" },
      { label: "Calls shown", value: "1" },
    ]);
  });

  it("marks generated, deleted, untouched functions and external packages", () => {
    expect(nodeFacts(graph, node(id.insertRefund)).facts).toContainEqual({ label: "Generated code", value: "Yes" });

    const deleted = nodeFacts(graph, node(id.legacy));
    expect(deleted).toMatchObject({
      stateLabel: "Deleted",
      lines: { additions: 0, deletions: 25 },
      summary: "Removed in this review: it exists only on the old side.",
    });
    expect(deleted.facts).toContainEqual({ label: "Cyclomatic complexity", value: "5, on the old side" });
    expect(deleted.facts).toContainEqual({ label: "Callers shown", value: "0" });

    expect(nodeFacts(graph, node(id.queryRow))).toMatchObject({ stateLabel: "Untouched", lines: null });

    expect(nodeFacts(graph, node(id.gateway))).toEqual({
      title: "github.com/acme/gateway",
      stateLabel: "External package",
      lines: null,
      location: "github.com/acme/gateway",
      summary: "A package outside the repository. One box stands for every function of it the graph calls.",
      facts: [
        { label: "Callers shown", value: "1" },
        { label: "Functions called", value: "2" },
      ],
    });
  });

  it("lists every entry point and writes a one-line function without a range", () => {
    const facts = nodeFacts(
      graph,
      makeFlowNode("x", {
        file: "cmd/tool/main.go",
        startLine: 7,
        endLine: 7,
        entryPoints: [
          { kind: "main", label: "main" },
          { kind: "cli_command", label: "tool sync" },
        ],
      }),
    );
    expect(facts.location).toBe("cmd/tool/main.go:7");
    expect(facts.facts).toContainEqual({ label: "Entry points", value: "main, tool sync" });
  });
});

describe("diff target", () => {
  const files = [
    makeChangedFile("internal/refund/service.go", { status: "added" }),
    makeChangedFile("internal/refund/legacy.go", { status: "deleted" }),
    makeChangedFile("internal/storage/ledger.go", { status: "renamed", oldPath: "internal/storage/refund_repo.go" }),
  ];

  it("opens the new side at the function's first line", () => {
    expect(diffTarget(node(id.request), files)).toEqual({ path: "internal/refund/service.go", line: 44, side: "new" });
  });

  it("opens the old side of the file for a deleted function", () => {
    expect(diffTarget(node(id.legacy), files)).toEqual({ path: "internal/refund/legacy.go", line: 10, side: "old" });
  });

  it("finds a renamed file by its old path", () => {
    expect(diffTarget(node(id.save), files)).toEqual({ path: "internal/storage/ledger.go", line: 18, side: "new" });
  });

  it("has none for external packages and files the review does not change", () => {
    expect(diffTarget(node(id.gateway), files)).toBeNull();
    expect(diffTarget(node(id.queryRow), files)).toBeNull();
  });

  it("opens a file group at its first changed function", () => {
    const grouping = groupFunctions(graph.nodes);
    expect(fileDiffTarget(grouping.groups.get("file:internal/refund/service.go") as FileGroup, files)).toEqual({
      path: "internal/refund/service.go",
      line: 44,
      side: "new",
    });
    expect(fileDiffTarget(grouping.groups.get("file:internal/storage/db.go") as FileGroup, files)).toBeNull();
  });

  it("opens a relation at its line in the caller's file, on the old side when it was removed", () => {
    const callFrom = (from: string, line: number, removed = false) =>
      relationDiffTarget(makeFlowEdge(from, id.save, { line, removed }), graph, files);
    expect(callFrom(id.request, 58)).toEqual({ path: "internal/refund/service.go", line: 58, side: "new" });
    expect(callFrom(id.request, 50, true)).toEqual({ path: "internal/refund/service.go", line: 50, side: "old" });
    expect(callFrom(id.queryRow, 3)).toBeNull();
    expect(callFrom("unknown", 3)).toBeNull();
  });
});
