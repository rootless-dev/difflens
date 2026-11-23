import type {
  Finding,
  Insights,
  InsightsAnalysis,
  InsightsBatch,
  InsightsPlan,
  ModelInUse,
} from "@/lib/api";

export const NO_INSIGHTS: Insights = { analysis: null, findings: [] };

/** `problem` may be given as its English text: it becomes a model_not_configured error without a reason. */
export function makeModelInUse(
  overrides: Partial<Omit<ModelInUse, "problem">> & { problem?: ModelInUse["problem"] | string } = {},
): ModelInUse {
  const { problem = null, ...rest } = overrides;
  return {
    provider: "claude_cli",
    providerName: "Claude Code",
    model: "opus",
    ready: true,
    parallelCalls: 1,
    ...rest,
    problem: typeof problem === "string" ? { code: "model_not_configured", message: problem } : problem,
  };
}

export function makePlan(overrides: Partial<InsightsPlan> = {}): InsightsPlan {
  return {
    files: 5,
    skipped: [
      { path: "assets/logo.png", reason: "binary" },
      { path: "internal/legacy/old.go", reason: "deleted" },
    ],
    calls: [
      {
        paths: [
          "go.mod",
          "internal/payment/repo.go",
          "internal/refund/handler.go",
          "internal/refund/service.go",
          "internal/storage/ledger.go",
        ],
        tokens: 48_200,
        truncated: [],
      },
    ],
    tokens: 48_200,
    model: makeModelInUse(),
    ...overrides,
  };
}

export function makeBatch(index: number, overrides: Partial<InsightsBatch> = {}): InsightsBatch {
  return { index, paths: [`file-${index}.go`], tokens: 20_000, status: "done", error: null, ...overrides };
}

export function makeAnalysis(overrides: Partial<InsightsAnalysis> = {}): InsightsAnalysis {
  return {
    id: "an-1",
    headSha: "e7f3b10c9d8e7f6a",
    provider: "claude_cli",
    providerName: "Claude Code",
    model: "claude-opus-5-5",
    createdAt: "2026-11-19T10:00:00Z",
    parallelCalls: 1,
    batches: [makeBatch(0)],
    language: "en",
    ...overrides,
  };
}

/** The prototype's first finding: in service.go, cited at lines 52 to 56. */
export function makeFinding(id: string, overrides: Partial<Finding> = {}): Finding {
  return {
    id,
    source: {
      analysisId: "an-1",
      provider: "claude_cli",
      providerName: "Claude Code",
      model: "claude-opus-5-5",
      analyzedAt: "2026-11-19T10:00:00Z",
      headSha: "e7f3b10c9d8e7f6a",
      earlier: false,
    },
    title: "Refund can be paid out and never recorded",
    severity: "high",
    category: "correctness",
    path: "internal/refund/service.go",
    line: 52,
    endLine: 56,
    location: { kind: "found", line: 52, endLine: 56, moved: false },
    excerpt: [
      { line: 50, text: "\t\treturn err", cited: false },
      { line: 51, text: "\t}", cited: false },
      { line: 52, text: "\tref, err := s.gateway.Refund(ctx, p.ChargeID, amt)", cited: true },
      { line: 53, text: "\tif err != nil {", cited: true },
      { line: 54, text: '\t\treturn fmt.Errorf("gateway refund: %w", err)', cited: true },
      { line: 55, text: "\t}", cited: true },
      { line: 56, text: "\treturn s.refunds.Save(ctx, Refund{PaymentID: id, Ref: ref, Amount: amt})", cited: true },
      { line: 57, text: "}", cited: false },
    ],
    explanation:
      "The gateway refund runs first and the row is saved afterwards. If Save fails, the customer has the money back " +
      "and the system has no refund on record.",
    suggestedDirection:
      "Save the refund as pending before calling the gateway, then mark it completed or failed with its answer.",
    evidence: ["Line 52 asks the gateway to refund; line 56 stores the refund only after that."],
    related: ["No test makes Save fail, so this path has never been run."],
    verdict: "pending",
    verdictAt: null,
    recheck: false,
    resolution: null,
    language: "en",
    translations: [],
    ...overrides,
  };
}

/** Five findings like the prototype's: two high, two medium, one low. */
export function prototypeFindings(): Finding[] {
  return [
    makeFinding("f1"),
    makeFinding("f2", {
      title: "Retrying POST /refunds refunds twice",
      path: "internal/refund/handler.go",
      line: 31,
      endLine: 31,
      location: { kind: "found", line: 31, endLine: 31, moved: false },
    }),
    makeFinding("f3", {
      title: "Two concurrent refunds can exceed the balance",
      severity: "medium",
      category: "concurrency",
      path: "internal/payment/repo.go",
      line: 27,
      endLine: 27,
      location: { kind: "found", line: 27, endLine: 27, moved: false },
    }),
    makeFinding("f4", {
      title: "Partial refund over the limit has no test",
      severity: "medium",
      category: "tests",
      path: "internal/refund/validate.go",
      line: 19,
      endLine: 19,
      location: { kind: "not_found", problem: "not_changed" },
      excerpt: [],
    }),
    makeFinding("f5", {
      title: "Gateway error loses the charge id",
      severity: "low",
      category: "observability",
      line: 54,
      endLine: 54,
      location: { kind: "found", line: 54, endLine: 54, moved: false },
    }),
  ];
}

export function makeInsights(overrides: Partial<Insights> = {}): Insights {
  return { analysis: makeAnalysis(), findings: prototypeFindings(), ...overrides };
}
