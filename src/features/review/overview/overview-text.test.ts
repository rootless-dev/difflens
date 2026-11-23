import { describe, expect, it } from "vitest";

import { issueKeyOf } from "@/features/review/card/use-card";
import {
  leftOutLines,
  LOOK_LIMIT,
  lookFirst,
  measuredRows,
  shapeRows,
  sinceOf,
  suggestions,
  tallyText,
} from "@/features/review/overview/overview-text";
import { reviewSignals } from "@/features/review/signals/signals";
import type { ChangeShape, Criterion, CriterionVerdict } from "@/lib/api";
import { makeChangedFile } from "@/test/fixtures";
import { makeFlowGraph, makeFlowNode } from "@/test/flow-fixtures";
import { makeFinding } from "@/test/insights-fixtures";
import { makeComment, makeReviewState } from "@/test/review-fixtures";
import {
  makeDuplicate,
  makeLocatedCheck,
  makeMutant,
  makeMutationRun,
  makeQualityFunction,
  makeQualityReport,
  makeTestCase,
  makeTestRun,
} from "@/test/tests-fixtures";

const files = [
  makeChangedFile("internal/refund/service.go", { status: "added" }),
  makeChangedFile("internal/refund/validate.go", { status: "added" }),
  makeChangedFile("internal/refund/service_test.go", { status: "added" }),
];

function criterion(verdict: CriterionVerdict): Criterion {
  return { criterion: "x", verdict, reasoning: "", path: null, line: null, located: false };
}

const SHAPE: ChangeShape = {
  filesAdded: 3,
  filesDeleted: 1,
  directoriesNew: ["internal/refund"],
  directoriesModified: [".", "internal/payment"],
  manifests: [
    { path: "go.mod", status: "modified" },
    { path: "go.sum", status: "modified" },
  ],
  migrations: [{ path: "migrations/0003_refunds.sql", status: "added" }],
};

describe("the card key", () => {
  it("is the first one in a text, as the core finds it", () => {
    expect(issueKeyOf("feature/ABC-200")).toBe("ABC-200");
    expect(issueKeyOf("ABC-178-login-page")).toBe("ABC-178");
    expect(issueKeyOf("fix/xAB2-17_typo")).toBe("AB2-17");
    expect(issueKeyOf("hotfix/PAY-1-and-OPS-2")).toBe("PAY-1");
    expect(issueKeyOf("feature/abc-178")).toBeNull();
    expect(issueKeyOf("feature/A-178")).toBeNull();
    expect(issueKeyOf("feature/ABC178")).toBeNull();
  });
});

describe("the criteria tally", () => {
  it("counts each verdict in the prototype's words, leaving out what is none", () => {
    const criteria = (["met", "met", "partly_met", "partly_met", "not_met", "no_evidence"] as const).map(criterion);
    expect(tallyText(criteria)).toBe("2 met, 2 partly met, 1 not met, 1 without evidence");
    expect(tallyText([criterion("not_met")])).toBe("1 not met");
  });
});

describe("where to look first", () => {
  const graph = makeFlowGraph(
    [
      makeFlowNode("refund.Service.Request", {
        file: "internal/refund/service.go",
        startLine: 44,
        endLine: 70,
        state: "new",
      }),
    ],
    [],
  );

  it("ranks high findings, failing tests, functions over a limit, then medium findings and untested functions", () => {
    const items = lookFirst({
      findings: [
        makeFinding("medium", { severity: "medium", title: "Medium one" }),
        makeFinding("high", { title: "High one" }),
        makeFinding("gone", { verdict: "dismissed", title: "Dismissed" }),
      ],
      run: makeTestRun({
        tests: [
          makeTestCase("TestRefund_Declined", {
            outcome: "fail",
            output: "=== RUN TestRefund_Declined\n    service_test.go:88: expected ErrDeclined\n",
            file: "internal/refund/service_test.go",
            line: 80,
          }),
        ],
      }),
      quality: makeQualityReport({
        functions: [
          makeQualityFunction("refund.Service.Request", { complexity: 11, changedLines: { covered: 12, coverable: 25 } }),
          // No test runs it: the core says so, from the run's coverage of all its lines.
          makeQualityFunction("refund.Notify", {
            file: "internal/refund/notify.go",
            startLine: 3,
            changedLines: { covered: 0, coverable: 6 },
            untested: true,
          }),
        ],
      }),
      graph,
      files,
    });
    expect(items.map((item) => item.title)).toEqual([
      "High one",
      "TestRefund_Declined fails",
      "Service.Request is complex and not well tested",
      "Medium one",
      "Notify is not run by any test",
    ]);
    expect(items).toHaveLength(LOOK_LIMIT);
    const [finding, test, limit, , untested] = items;
    // A finding inside a function of the graph opens in Flow.
    expect(finding).toMatchObject({ model: true, measured: false, location: "internal/refund/service.go:52" });
    expect(finding.jump).toEqual({ kind: "flow", functionId: "refund.Service.Request" });
    expect(test.detail).toBe("service_test.go:88: expected ErrDeclined");
    expect(test.jump).toEqual({
      kind: "diff",
      target: { path: "internal/refund/service_test.go", line: 80, side: "new" },
    });
    expect(limit.detail).toBe(
      "Complexity 11 with 48% of its changed lines covered, CRAP 34.2. The limits are 10 and 30.",
    );
    expect(limit.jump).toEqual({ kind: "flow", functionId: "refund.Service.Request" });
    // Not in the graph, not a changed file: nowhere to open.
    expect(untested.jump).toBeNull();
  });

  it("folds many failing tests into one item that opens the Tests tab", () => {
    const failing = ["TestA", "TestB", "TestC"].map((name) => makeTestCase(name, { outcome: "fail" }));
    const [item] = lookFirst({ findings: null, run: makeTestRun({ tests: failing }), quality: null, graph: null, files });
    expect(item.title).toBe("3 tests fail");
    expect(item.detail).toBe("TestA, TestB and 1 other.");
    expect(item.jump).toEqual({ kind: "tests" });
  });

  it("is empty with nothing measured or read", () => {
    expect(lookFirst({ findings: [], run: null, quality: null, graph: null, files })).toEqual([]);
  });

  it("puts a check the tool calls an error before its warnings, within the same class", () => {
    const request = { id: "refund.Service.Request", qualifiedName: "refund.Service.Request" };
    const quality = makeQualityReport({
      functions: [makeQualityFunction("refund.Service.Request")],
      crossed: [],
      checkFindings: [
        makeLocatedCheck({ tool: "ruff", rule: "F841", line: 50, function: request, message: "unused" }),
        makeLocatedCheck({ tool: "mypy", rule: "arg-type", line: 60, function: request, severity: "error", message: "bad arg" }),
        makeLocatedCheck({ tool: "ruff", rule: "E501", line: 3, function: null, severity: "info", class: "style", message: "long" }),
        makeLocatedCheck({ tool: "mypy", rule: "misc", line: 2, function: null, severity: "error", message: "broken" }),
      ],
    });

    const items = lookFirst({ findings: null, run: null, quality, graph: null, files });

    expect(items.map((item) => item.tool)).toEqual([
      { tool: "mypy", rule: "arg-type", severity: "error" },
      { tool: "mypy", rule: "misc", severity: "error" },
      { tool: "ruff", rule: "E501", severity: "info" },
    ]);
    expect(items[0].quote).toEqual({ line: 60, message: "bad arg" });
  });

  it("ranks a correctness check after failing tests and before limits, a style check last, one row per function", () => {
    const request = { id: "refund.Service.Request", qualifiedName: "refund.Service.Request" };
    const validate = { id: "refund.Validate", qualifiedName: "refund.Validate" };
    const quality = makeQualityReport({
      functions: [
        makeQualityFunction("refund.Service.Request", {
          complexity: 14,
          changedLines: { covered: 12, coverable: 25 },
          crap: 42,
        }),
        makeQualityFunction("refund.Validate", { file: "internal/refund/validate.go", startLine: 12 }),
        makeQualityFunction("refund.Notify", {
          file: "internal/refund/notify.go",
          startLine: 3,
          changedLines: { covered: 0, coverable: 6 },
          untested: true,
        }),
      ],
      crossed: [
        { kind: "complexity", function: request, value: 14, limit: 10 },
        { kind: "crap", function: request, value: 42, limit: 30 },
        { kind: "changed_lines_coverage", function: request, value: 0.48, limit: 0.8 },
      ],
      checkFindings: [
        makeLocatedCheck({ line: 60, function: request }),
        makeLocatedCheck({ line: 50, rule: "ST1005", class: "style", function: request, message: "style" }),
        makeLocatedCheck({
          rule: "ST1005",
          class: "style",
          file: "internal/refund/validate.go",
          line: 14,
          function: validate,
          message: "error strings should not be capitalized",
        }),
        makeLocatedCheck({
          tool: "go vet",
          rule: "printf",
          file: "internal/refund/validate.go",
          line: 2,
          function: null,
          message: "wrong type",
        }),
      ],
    });

    const items = lookFirst({
      findings: [makeFinding("low", { severity: "low", title: "Low one" }), makeFinding("medium", { severity: "medium", title: "Medium one" })],
      run: null,
      quality,
      graph,
      files,
    });

    expect(items.map((item) => item.title)).toEqual([
      // Its correctness check carries its limits and its style check with it: one row.
      "Service.Request has 2 findings of staticcheck and crosses 3 limits",
      "go vet reports a problem in validate.go",
      "Medium one",
      "Notify is not run by any test",
      "Low one",
    ]);
    const [fn, check, , untested] = items;
    expect(fn).toMatchObject({
      measured: true,
      model: false,
      tool: { tool: "staticcheck", rule: "SA4006" },
      limits: 3,
      untested: false,
      quote: { line: 60, message: "this value of err is never used" },
      location: "internal/refund/service.go:44",
      jump: { kind: "flow", functionId: "refund.Service.Request" },
    });
    expect(fn.detail).toBe(
      "Complexity 14, CRAP 42 and 48% of the changed lines covered; the limits are 10, 30 and 80%.",
    );
    expect(check).toMatchObject({
      quote: { line: 2, message: "wrong type" },
      location: "internal/refund/validate.go:2",
      jump: { kind: "diff", target: { path: "internal/refund/validate.go", line: 2, side: "new" } },
    });
    expect(untested).toMatchObject({ untested: true, limits: 0, detail: "None of its 6 changed lines ran in a test." });
    // Validate's style check ranks below everything: past the cut.
    expect(items.some((item) => item.title.includes("Validate"))).toBe(false);
  });
});

describe("the model suggests changing", () => {
  const many = ["a", "b", "c", "d", "e", "f", "g", "h"].map((id, index) =>
    makeFinding(id, { severity: index < 2 ? "high" : index < 5 ? "medium" : "low", title: `Finding ${id}` }),
  );

  it("lists only the findings not already in where to look first, most severe first, five at most", () => {
    const signals = reviewSignals(null, [...many, makeFinding("gone", { verdict: "dismissed" })]);
    const look = lookFirst({ findings: many, run: null, quality: null, graph: null, files, signals });
    expect(look.map((item) => item.id)).toEqual(["finding:a", "finding:b", "finding:c", "finding:d", "finding:e"]);

    const listed = suggestions(signals, look.slice(0, 2));
    expect(listed?.items.map((finding) => finding.id)).toEqual(["c", "d", "e", "f", "g"]);
    expect(listed).toMatchObject({ left: 6, above: 2, total: 8 });
  });

  it("is absent when every finding is above, or there is none", () => {
    const signals = reviewSignals(null, many.slice(0, 2));
    const look = lookFirst({ findings: many.slice(0, 2), run: null, quality: null, graph: null, files, signals });
    expect(suggestions(signals, look)).toBeNull();
    expect(suggestions(reviewSignals(null, []), [])).toBeNull();
  });
});

describe("measured in this branch", () => {
  it("has rows only for what was measured", () => {
    expect(measuredRows(null, null)).toEqual([]);
    const rows = measuredRows(makeTestRun(), makeQualityReport());
    expect(rows).toEqual([
      { label: "Tests", value: "7 passing", bad: "1 failing", note: "4 added in this branch" },
      {
        label: "Coverage of changed lines",
        value: "67%",
        tone: "warn",
        note: "the target for changed lines is 80%",
      },
      {
        label: "Functions over a limit",
        value: "2",
        tone: "warn",
        note: "complexity 10, CRAP 30, coverage 80%; both are functions this branch adds",
      },
      { label: "Changed functions no test runs", value: "0", tone: "ok" },
      { label: "Static checks", value: "No findings", tone: "ok", note: "go vet and staticcheck" },
      { label: "Duplicated code", value: "None", tone: "ok", note: "copies of at least 50 tokens and 6 lines" },
    ]);
    const uncovered = measuredRows(makeTestRun({ coverage: null }), null);
    expect(uncovered.map((row) => row.label)).toEqual(["Tests"]);
  });

  it("counts the duplicated code the branch introduces", () => {
    const report = makeQualityReport({
      duplicates: [makeDuplicate(), makeDuplicate({ kind: "repeated_in_change" })],
    });
    const row = measuredRows(null, report).find((candidate) => candidate.label === "Duplicated code");
    expect(row).toEqual({ label: "Duplicated code", value: "2 blocks", tone: "warn", note: "copies of at least 50 tokens and 6 lines" });
  });

  it("counts mutation testing only once a run completed, and the mutants that survived", () => {
    const report = makeQualityReport();
    const label = (row: { label: string }) => row.label === "Mutation testing";
    expect(measuredRows(null, report).find(label)).toBeUndefined();
    const row = measuredRows(null, report, reviewSignals(report, null, makeMutationRun())).find(label);
    expect(row).toEqual({
      label: "Mutation testing",
      value: "1 of 4 killed",
      tone: "warn",
      note: "2 survived: changes no test caught",
    });
    const clean = makeMutationRun({ mutants: [makeMutant("m-1", { status: "killed", killedBy: "TestRequest" })] });
    expect(measuredRows(null, report, reviewSignals(report, null, clean)).find(label)).toEqual({
      label: "Mutation testing",
      value: "1 of 1 killed",
      tone: "ok",
      note: "none survived",
    });
  });
});

describe("duplicated code in where to look first", () => {
  it("is one measured item per duplicate, saying what it copies or how often it repeats", () => {
    const report = makeQualityReport({
      crossed: [],
      duplicates: [makeDuplicate(), makeDuplicate({ kind: "repeated_in_change", function: null })],
    });
    const items = lookFirst({ findings: null, run: null, quality: report, graph: null, files: [] });
    const duplicates = items.filter((item) => item.duplicate);
    expect(duplicates.map(({ title, detail, location, measured, model }) => ({ title, detail, location, measured, model })))
      .toEqual([
        {
          title: "Service.Request copies code that already exists",
          detail: "Copies internal/payment/money.go:40-47, 74 tokens, 8 lines.",
          location: "internal/refund/service.go:60-67",
          measured: true,
          model: false,
        },
        {
          title: "service.go repeats the same code",
          detail: "Repeated 3× in the change, 74 tokens, 8 lines.",
          location: "internal/refund/service.go:60-67",
          measured: true,
          model: false,
        },
      ]);
    expect(duplicates[0].id).toBe("duplicate:copies_existing:internal/refund/service.go:60:67");
  });
});

describe("the shape of the change", () => {
  it("counts entry points from the graph and the rest from the files", () => {
    const graph = makeFlowGraph(
      [
        makeFlowNode("refund.Handler", { state: "new", entryPoints: [{ kind: "http_route", label: "POST /refunds" }] }),
        makeFlowNode("payment.Handler", {
          state: "modified",
          entryPoints: [{ kind: "http_route", label: "GET /payments" }],
        }),
      ],
      [],
    );
    expect(shapeRows(SHAPE, graph)).toEqual([
      { label: "Entry points", value: "1 new, 1 affected" },
      { label: "Directories", value: "1 new, 2 modified", title: "internal/refund (new)\n.\ninternal/payment" },
      { label: "Files", value: "3 added, 1 deleted" },
      { label: "Database", value: "1 new migration", title: "migrations/0003_refunds.sql" },
      { label: "Dependencies", value: "go.mod, go.sum changed", title: "go.mod\ngo.sum" },
    ]);
  });

  it("leaves out what cannot be measured and says when nothing was touched", () => {
    const rows = shapeRows(
      { ...SHAPE, filesAdded: 0, filesDeleted: 0, manifests: [], migrations: [{ path: "db/migrate/1.rb", status: "modified" }] },
      null,
    );
    expect(rows.map((row) => row.label)).not.toContain("Entry points");
    expect(rows.find((row) => row.label === "Files")?.value).toBe("None added or deleted");
    expect(rows.find((row) => row.label === "Database")?.value).toBe("1 migration changed");
    expect(rows.find((row) => row.label === "Dependencies")?.value).toBe("No manifest touched");
  });
});

describe("since your last review", () => {
  it("is null without an earlier review", () => {
    expect(sinceOf(makeReviewState(), null)).toBeNull();
    expect(sinceOf(null, null)).toBeNull();
  });

  it("counts what changed, what was reopened, and the user's comments and classified findings on changed code", () => {
    const state = makeReviewState({
      checkpoint: { sha: "a41c9e2000000000", at: "2026-11-12T10:00:00Z" },
      since: { kind: "available", checkpointSha: "a41c9e2000000000", commits: 2, files },
      comments: [
        makeComment("mine", { outdated: true }),
        makeComment("resolved", { outdated: true, resolved: true }),
        makeComment("reply", { outdated: true, parentId: "mine" }),
      ],
      summary: { ...makeReviewState().summary, reopenedFiles: 2 },
    });
    const since = sinceOf(state, [makeFinding("a", { recheck: true }), makeFinding("b")]);
    expect(since).toMatchObject({ commits: 2, reopened: 2, comments: 1, findings: 1, note: null });
    expect(since?.files).toHaveLength(3);
  });

  it("says when nothing was added since", () => {
    const state = makeReviewState({
      checkpoint: { sha: "e7f3b10c9d8e7f6a", at: "2026-11-12T10:00:00Z" },
      since: { kind: "unavailable", reason: "up_to_date" },
    });
    expect(sinceOf(state, null)).toMatchObject({ commits: null, note: "Nothing was added to the branch since." });
  });
});

describe("left out of the analysis", () => {
  it("names the files not sent, those sent in part, and a condensed summary", () => {
    expect(
      leftOutLines(
        { skipped: [{ path: "assets/logo.png", reason: "binary" }], truncated: ["internal/big.go"] },
        true,
      ),
    ).toEqual([
      { path: "assets/logo.png", text: "is not sent to the model: it is binary." },
      { path: "internal/big.go", text: "is too large for one call: only its changed lines are sent." },
      {
        path: null,
        text: "The summary read a condensed change, too large to send whole: fewer lines around each change, and some files only in part.",
      },
    ]);
    expect(leftOutLines({ skipped: [], truncated: [] }, false)).toEqual([]);
  });
});
