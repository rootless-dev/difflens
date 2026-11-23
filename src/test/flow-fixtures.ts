// A review shaped like the prototype's refund example: POST /refunds goes through Handler.Create into
// Service.Request, which loads the payment, validates the amount, calls the payment gateway (outside the
// repository) and saves the refund. Two other entry points reach the changed Repo.Get.

import type { EntryPoint, FlowCall, FlowEdge, FlowGraph, FlowNode } from "@/lib/api";

const MODULE = "example.com/payments";

export function makeFlowNode(id: string, overrides: Partial<FlowNode> = {}): FlowNode {
  const name = overrides.name ?? id;
  return {
    id,
    name,
    qualifiedName: name,
    container: "internal/app",
    state: "untouched",
    file: "internal/app/app.go",
    startLine: 1,
    endLine: 10,
    additions: 0,
    deletions: 0,
    complexity: 1,
    previousComplexity: 1,
    generated: false,
    test: false,
    entryPoints: [],
    calls: [],
    externalFunctions: 0,
    ...overrides,
  };
}

export function makeFlowCall(calleeId: string, qualifiedName: string, overrides: Partial<FlowCall> = {}): FlowCall {
  return {
    calleeId,
    qualifiedName,
    line: 1,
    kind: "always",
    external: false,
    shown: true,
    added: false,
    removed: false,
    relation: "call",
    evidence: null,
    ...overrides,
  };
}

export function makeFlowEdge(from: string, to: string, overrides: Partial<FlowEdge> = {}): FlowEdge {
  return {
    from,
    to,
    position: 1,
    line: 1,
    kind: "always",
    resolution: "static",
    added: false,
    removed: false,
    relation: "call",
    evidence: null,
    ...overrides,
  };
}

export function makeFlowGraph(nodes: FlowNode[], edges: FlowEdge[], overrides: Partial<FlowGraph> = {}): FlowGraph {
  return {
    language: "go",
    analyzer: { name: "difflens-go", version: "0.1.0" },
    oldSha: "a1b2c3d4e5f6a7b8",
    newSha: "e7f3b10c9d8e7f6a",
    options: { includeTests: false, includeCallers: false, includeCallees: false },
    nodes,
    edges,
    hidden: { changedTests: 3, callers: 4, callees: 2, externalPackages: 1 },
    entryPoints: nodes.filter((node) => node.entryPoints.length > 0).map((node) => node.id),
    truncated: false,
    diagnostics: [],
    analyzedAt: "2026-11-16T18:00:00Z",
    ...overrides,
  };
}

const route = (label: string): EntryPoint => ({ kind: "http_route", label });

/** Function ids as the Go analyzer writes them: import path, then receiver and name. */
export const REFUND_IDS = {
  create: `${MODULE}/internal/refund.(*Handler).Create`,
  request: `${MODULE}/internal/refund.(*Service).Request`,
  validate: `${MODULE}/internal/refund.Validate`,
  legacy: `${MODULE}/internal/refund.legacyRefund`,
  get: `${MODULE}/internal/payment.(*Repo).Get`,
  showPayment: `${MODULE}/internal/payment.(*Handler).Show`,
  onSettled: `${MODULE}/internal/payment.(*Consumer).OnSettled`,
  save: `${MODULE}/internal/storage.(*RefundRepo).Save`,
  insertRefund: `${MODULE}/internal/storage.(*Queries).InsertRefund`,
  queryRow: `${MODULE}/internal/storage.(*DB).QueryRow`,
  notify: `${MODULE}/internal/notify.(*Notifier).Send`,
  gateway: "external:github.com/acme/gateway",
  /** Called by Service.Request but not drawn. */
  audit: `${MODULE}/internal/refund.(*Service).audit`,
} as const;

const id = REFUND_IDS;

export const refundNodes: FlowNode[] = [
  makeFlowNode(id.create, {
    name: "Handler.Create",
    qualifiedName: "refund.Handler.Create",
    container: "internal/refund",
    state: "new",
    file: "internal/refund/handler.go",
    startLine: 24,
    endLine: 41,
    additions: 18,
    complexity: 3,
    previousComplexity: null,
    entryPoints: [route("POST /refunds")],
    calls: [
      makeFlowCall(id.legacy, "refund.legacyRefund", { line: 30, kind: "conditional", removed: true }),
      makeFlowCall(id.request, "refund.Service.Request", { line: 33, added: true }),
    ],
  }),
  makeFlowNode(id.request, {
    name: "Service.Request",
    qualifiedName: "refund.Service.Request",
    container: "internal/refund",
    state: "new",
    file: "internal/refund/service.go",
    startLine: 44,
    endLine: 71,
    additions: 28,
    complexity: 11,
    previousComplexity: null,
    calls: [
      makeFlowCall(id.get, "payment.Repo.Get", { line: 45, added: true }),
      makeFlowCall(id.validate, "refund.Validate", { line: 49, kind: "conditional", added: true }),
      makeFlowCall(id.gateway, "github.com/acme/gateway", {
        line: 53,
        kind: "conditional",
        external: true,
        added: true,
      }),
      makeFlowCall(id.save, "storage.RefundRepo.Save", { line: 58, kind: "conditional", added: true }),
      makeFlowCall(id.audit, "refund.Service.audit", { line: 66, shown: false, added: true }),
    ],
  }),
  makeFlowNode(id.validate, {
    name: "Validate",
    qualifiedName: "refund.Validate",
    container: "internal/refund",
    state: "new",
    file: "internal/refund/validate.go",
    startLine: 12,
    endLine: 31,
    additions: 20,
    complexity: 4,
    previousComplexity: null,
  }),
  makeFlowNode(id.legacy, {
    name: "legacyRefund",
    qualifiedName: "refund.legacyRefund",
    container: "internal/refund",
    state: "deleted",
    file: "internal/refund/legacy.go",
    startLine: 10,
    endLine: 34,
    deletions: 25,
    complexity: 5,
    previousComplexity: 5,
  }),
  makeFlowNode(id.get, {
    name: "Repo.Get",
    qualifiedName: "payment.Repo.Get",
    container: "internal/payment",
    state: "modified",
    file: "internal/payment/repo.go",
    startLine: 27,
    endLine: 38,
    additions: 3,
    deletions: 3,
    complexity: 2,
    previousComplexity: 2,
  }),
  makeFlowNode(id.showPayment, {
    name: "Handler.Show",
    qualifiedName: "payment.Handler.Show",
    container: "internal/payment",
    file: "internal/payment/handler.go",
    startLine: 18,
    endLine: 33,
    complexity: 3,
    previousComplexity: 3,
    entryPoints: [route("GET /payments/{id}")],
  }),
  makeFlowNode(id.onSettled, {
    name: "Consumer.OnSettled",
    qualifiedName: "payment.Consumer.OnSettled",
    container: "internal/payment",
    state: "modified",
    file: "internal/payment/consumer.go",
    startLine: 30,
    endLine: 52,
    additions: 4,
    deletions: 1,
    complexity: 5,
    previousComplexity: 4,
    entryPoints: [{ kind: "message_handler", label: "payment.settled" }],
  }),
  makeFlowNode(id.save, {
    name: "RefundRepo.Save",
    qualifiedName: "storage.RefundRepo.Save",
    container: "internal/storage",
    state: "modified",
    file: "internal/storage/refund_repo.go",
    startLine: 18,
    endLine: 24,
    additions: 2,
    deletions: 2,
    complexity: 1,
    previousComplexity: 1,
  }),
  makeFlowNode(id.insertRefund, {
    name: "Queries.InsertRefund",
    qualifiedName: "storage.Queries.InsertRefund",
    container: "internal/storage",
    state: "modified",
    file: "internal/storage/queries.sql.go",
    startLine: 40,
    endLine: 52,
    additions: 6,
    deletions: 2,
    complexity: 2,
    previousComplexity: 2,
    generated: true,
  }),
  makeFlowNode(id.queryRow, {
    name: "DB.QueryRow",
    qualifiedName: "storage.DB.QueryRow",
    container: "internal/storage",
    file: "internal/storage/db.go",
    startLine: 61,
    endLine: 75,
    complexity: 3,
    previousComplexity: 3,
  }),
  makeFlowNode(id.notify, {
    name: "Notifier.Send",
    qualifiedName: "notify.Notifier.Send",
    container: "internal/notify",
    file: "internal/notify/notifier.go",
    startLine: 15,
    endLine: 29,
    complexity: 2,
    previousComplexity: 2,
  }),
  makeFlowNode(id.gateway, {
    name: "github.com/acme/gateway",
    qualifiedName: "github.com/acme/gateway",
    container: "github.com/acme/gateway",
    state: "external",
    file: null,
    startLine: null,
    endLine: null,
    complexity: null,
    previousComplexity: null,
    externalFunctions: 2,
  }),
];

export const refundEdges: FlowEdge[] = [
  makeFlowEdge(id.create, id.request, { position: 1, line: 33, added: true }),
  makeFlowEdge(id.create, id.legacy, { position: 2, line: 30, kind: "conditional", removed: true }),
  makeFlowEdge(id.request, id.get, { position: 1, line: 45, added: true }),
  makeFlowEdge(id.request, id.validate, { position: 2, line: 49, kind: "conditional", added: true }),
  makeFlowEdge(id.request, id.gateway, {
    position: 3,
    line: 53,
    kind: "conditional",
    resolution: "dynamic",
    added: true,
  }),
  makeFlowEdge(id.request, id.save, { position: 4, line: 58, kind: "conditional", added: true }),
  makeFlowEdge(id.get, id.queryRow, { position: 1, line: 29 }),
  makeFlowEdge(id.showPayment, id.get, { position: 1, line: 22 }),
  makeFlowEdge(id.onSettled, id.get, { position: 1, line: 34 }),
  makeFlowEdge(id.onSettled, id.notify, { position: 2, line: 47, kind: "async", added: true }),
  makeFlowEdge(id.save, id.insertRefund, { position: 1, line: 20, added: true }),
];

export function refundFlowGraph(overrides: Partial<FlowGraph> = {}): FlowGraph {
  return makeFlowGraph(refundNodes, refundEdges, {
    entryPoints: [id.create, id.showPayment, id.onSettled],
    ...overrides,
  });
}
