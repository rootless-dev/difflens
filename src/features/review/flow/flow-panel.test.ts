import { describe, expect, it } from "vitest";

import { entryRows, packageRows } from "@/features/review/flow/flow-panel";
import { makeFlowGraph, makeFlowNode, REFUND_IDS as id, refundFlowGraph } from "@/test/flow-fixtures";

describe("left panel", () => {
  it("lists the entry points that reach a change, with their state and kind", () => {
    expect(entryRows(refundFlowGraph())).toEqual([
      { key: `${id.create}#0`, nodeId: id.create, label: "POST /refunds", state: "new", meta: "new" },
      { key: `${id.showPayment}#0`, nodeId: id.showPayment, label: "GET /payments/{id}", state: "untouched", meta: "" },
      { key: `${id.onSettled}#0`, nodeId: id.onSettled, label: "payment.settled", state: "modified", meta: "consumer" },
    ]);
  });

  it("gives each entry point of a function its own row and names an unlabelled one after the function", () => {
    const graph = makeFlowGraph(
      [
        makeFlowNode("a", {
          entryPoints: [
            { kind: "rpc_method", label: "Refunds.Create" },
            { kind: "scheduled_job", label: "nightly-refunds" },
          ],
        }),
        makeFlowNode("b", { name: "run", state: "new" }),
      ],
      [],
      { entryPoints: ["a", "b", "unknown"] },
    );
    expect(entryRows(graph).map((row) => [row.label, row.meta])).toEqual([
      ["Refunds.Create", "rpc"],
      ["nightly-refunds", "job"],
      ["run", "new · main"],
    ]);
  });

  it("counts changed functions per package, most changed first", () => {
    expect(packageRows(refundFlowGraph())).toEqual([
      { container: "internal/refund", changed: 4 },
      { container: "internal/payment", changed: 2 },
      { container: "internal/storage", changed: 2 },
    ]);
  });
});
