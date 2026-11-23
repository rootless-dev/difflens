import { describe, expect, it } from "vitest";

import { fileSignalLayout } from "@/features/review/signals/diff-signals";
import {
  duplicateSignal,
  filesBadge,
  functionBadge,
  mutantSignal,
  reviewSignals,
} from "@/features/review/signals/signals";
import {
  badgeParts,
  checkDraftBody,
  duplicateDraftBody,
  duplicateKindText,
  duplicateSizeText,
  limitPill,
  limitRows,
  mutantChangeText,
  mutantDraftBody,
} from "@/features/review/signals/signals-text";
import { i18n } from "@/i18n";
import type { CrossedLimit, Finding, Mutant } from "@/lib/api";
import { makeFinding } from "@/test/insights-fixtures";
import {
  makeDuplicate,
  makeLocatedCheck,
  makeMutant,
  makeMutationRun,
  makeQualityFunction,
  makeQualityReport,
} from "@/test/tests-fixtures";

const SERVICE = "internal/refund/service.go";
const request = { id: "refund.Service.Request", qualifiedName: "refund.Service.Request" };
const helper = { id: "refund.helper", qualifiedName: "refund.helper" };

const found = (id: string, line: number, endLine: number, overrides: Partial<Finding> = {}) =>
  makeFinding(id, { line, endLine, location: { kind: "found", line, endLine, moved: false }, ...overrides });

const limit = (kind: CrossedLimit["kind"], fn: typeof request, value: number, bound: number): CrossedLimit => ({
  kind,
  function: fn,
  value,
  limit: bound,
});

/** Service.Request (44–71) holds helper (60–65); Notify is in another file and no test runs it. */
function report() {
  return makeQualityReport({
    functions: [
      makeQualityFunction("refund.Service.Request", {
        startLine: 44,
        endLine: 71,
        complexity: 14,
        previousComplexity: 9,
        state: "modified",
        changedLines: { covered: 12, coverable: 25 },
        crap: 42,
      }),
      makeQualityFunction("refund.helper", { startLine: 60, endLine: 65 }),
      makeQualityFunction("refund.Notify", {
        file: "internal/refund/notify.go",
        startLine: 3,
        endLine: 12,
        changedLines: { covered: 0, coverable: 9 },
        crap: 6,
        complexity: 2,
        untested: true,
      }),
    ],
    crossed: [
      // Out of order on purpose: listed complexity, CRAP, coverage.
      limit("changed_lines_coverage", request, 0.48, 0.8),
      limit("complexity", request, 14, 10),
      limit("crap", request, 42, 30),
      limit("changed_lines_coverage", { id: "refund.Notify", qualifiedName: "refund.Notify" }, 0, 0.8),
      { kind: "changed_lines_coverage", function: null, value: 0.6, limit: 0.8 },
    ],
    checkFindings: [
      makeLocatedCheck({ line: 50, function: request }),
      makeLocatedCheck({ tool: "go vet", rule: "printf", line: 90, function: null, message: "wrong type" }),
    ],
  });
}

const findings = () => [
  found("low", 62, 62, { severity: "low" }),
  found("high", 52, 56),
  found("medium", 45, 45, { severity: "medium" }),
  found("gone", 47, 47, { verdict: "dismissed" }),
  makeFinding("lost", { location: { kind: "not_found", problem: "code_changed" } }),
];

describe("the signals of a review", () => {
  it("place the findings in the innermost function, most severe first, leaving the dismissed out", () => {
    const signals = reviewSignals(report(), findings());

    expect(signals.functions.get(request.id)?.findings.map((finding) => finding.id)).toEqual(["high", "medium"]);
    expect(signals.functions.get(helper.id)?.findings.map((finding) => finding.id)).toEqual(["low"]);
    expect(signals.findings.map((finding) => finding.id)).toEqual(["high", "lost", "medium", "low"]);
    const file = signals.files.get(SERVICE);
    expect(file?.findings.map((finding) => finding.id)).toEqual(["high", "medium", "low"]);
    expect(file?.dismissed.map((finding) => finding.id)).toEqual(["gone"]);
  });

  it("leave the fixed findings out everywhere: not counted, not shown, not even folded", () => {
    const fixed = (id: string, kind: "model_checked" | "user_marked", overrides: Partial<Finding> = {}) =>
      found(id, 52, 52, {
        resolution: { kind, model: null, at: "2026-11-20T10:00:00Z", reason: null },
        ...overrides,
      });
    const signals = reviewSignals(report(), [
      ...findings(),
      fixed("fixed-by-model", "model_checked", { severity: "high" }),
      fixed("fixed-by-you", "user_marked", { verdict: "confirmed" }),
      fixed("fixed-and-dismissed", "user_marked", { verdict: "dismissed" }),
    ]);

    expect(signals.findings.map((finding) => finding.id)).toEqual(["high", "lost", "medium", "low"]);
    expect(signals.functions.get(request.id)?.findings.map((finding) => finding.id)).toEqual(["high", "medium"]);
    const file = signals.files.get(SERVICE);
    expect(file?.findings.map((finding) => finding.id)).toEqual(["high", "medium", "low"]);
    expect(file?.dismissed.map((finding) => finding.id)).toEqual(["gone"]);
    expect(filesBadge([file])?.findings).toBe(3);
  });

  it("keep a function's limits in order, its checks, and the coverage of one no test runs as untested", () => {
    const signals = reviewSignals(report(), null);

    const fn = signals.functions.get(request.id);
    expect(fn?.limits.map((crossed) => crossed.kind)).toEqual(["complexity", "crap", "changed_lines_coverage"]);
    expect(fn?.checks.map((check) => check.line)).toEqual([50]);
    const notify = signals.functions.get("refund.Notify");
    expect(notify?.untested).toBe(true);
    expect(notify?.limits).toEqual([]);
    // A check outside any function belongs to its file only.
    expect(signals.files.get(SERVICE)?.checks.map((check) => check.line)).toEqual([50, 90]);
    expect(signals.functions.has(helper.id)).toBe(false);
  });

  it("are empty without a report or findings", () => {
    const signals = reviewSignals(null, null);
    expect(signals.functions.size + signals.files.size + signals.findings.length + signals.checks.length).toBe(0);
    expect(signals.limits).toBeNull();
  });
});

describe("a badge", () => {
  it("has the parts of a function in a fixed order, untested as one, and says them", () => {
    const signals = reviewSignals(report(), findings());
    const badge = functionBadge(signals.functions.get(request.id));
    expect(badge).toEqual({ severity: "high", findings: 2, limits: 3, checks: 1, duplicates: 0, mutants: 0, untested: 0 });
    expect(badgeParts(badge!)).toEqual([
      "2 model findings, the most severe of high severity",
      "3 limits crossed",
      "1 static check finding",
    ]);
    const notify = functionBadge(signals.functions.get("refund.Notify"));
    expect(notify).toEqual({ severity: null, findings: 0, limits: 0, checks: 0, duplicates: 0, mutants: 0, untested: 1 });
    expect(badgeParts(notify!)).toEqual(["no test runs it"]);
    expect(functionBadge(undefined)).toBeNull();
  });

  it("of a group adds up its files, outside their functions too, and counts the functions no test runs", () => {
    const signals = reviewSignals(report(), findings());
    const badge = filesBadge([signals.files.get(SERVICE), signals.files.get("internal/refund/notify.go"), undefined]);
    expect(badge).toEqual({ severity: "high", findings: 3, limits: 3, checks: 2, duplicates: 0, mutants: 0, untested: 1 });
    expect(badgeParts(badge!, true)).toEqual([
      "3 model findings, the most severe of high severity",
      "3 limits crossed",
      "2 static check findings",
      "1 function no test runs",
    ]);
    expect(filesBadge([undefined])).toBeNull();
    expect(badgeParts({ severity: "low", findings: 1, limits: 1, checks: 0, duplicates: 0, mutants: 0, untested: 0 })).toEqual([
      "1 model finding, of low severity",
      "1 limit crossed",
    ]);
  });
});

describe("the words of a function's limits", () => {
  it("list the three limits, the crossed ones marked, with what they were before", () => {
    const signals = reviewSignals(report(), null);
    const rows = limitRows(signals.functions.get(request.id)!, signals.limits!);
    expect(rows).toEqual([
      {
        kind: "complexity",
        label: "Cyclomatic complexity",
        note: "9 before this branch",
        value: "14",
        bound: "/ limit 10",
        over: true,
      },
      { kind: "crap", label: "CRAP", note: "joins complexity and coverage", value: "42", bound: "/ limit 30", over: true },
      {
        kind: "coverage",
        label: "Coverage of changed lines",
        note: "12 of 25 lines ran in the tests",
        value: "48%",
        bound: "/ minimum 80%",
        over: true,
      },
    ]);
    const untested = limitRows(signals.functions.get("refund.Notify")!, signals.limits!);
    expect(untested.map((row) => [row.kind, row.value, row.over])).toEqual([
      ["untested", "0%", true],
      ["complexity", "2", false],
      ["crap", "6", false],
    ]);
    expect(untested[0].note).toBe("coverage of its 9 changed lines");
  });

  it("make the Diff's marker of only the limits crossed", () => {
    const signals = reviewSignals(report(), null);
    const pill = limitPill(signals.functions.get(request.id)!);
    expect(pill.parts).toEqual([
      { name: "CC", value: "14" },
      { name: "CRAP", value: "42" },
      { name: "coverage", value: "48%" },
    ]);
    expect(pill.label).toBe(
      "Service.Request over 3 limits: cyclomatic complexity 14, CRAP 42, coverage of the changed lines 48%. Show the limits",
    );
    expect(pill.title).toBe(
      "Cyclomatic complexity 14, the limit is 10. CRAP 42, the limit is 30. Coverage of the changed lines 48%, the minimum is 80%.",
    );
  });
});

describe("a draft from a signal", () => {
  it("is the tool's message with its rule", () => {
    expect(checkDraftBody(makeLocatedCheck())).toBe(
      "staticcheck reports on this line (SA4006): “this value of err is never used”",
    );
    expect(checkDraftBody(makeLocatedCheck({ rule: null }))).toBe(
      "staticcheck reports on this line: “this value of err is never used”",
    );
  });
});

describe("duplicated code", () => {
  const copy = makeDuplicate();
  const repeated = makeDuplicate({ kind: "repeated_in_change" });

  it("counts on the function holding its block and on its file", () => {
    const signals = reviewSignals(makeQualityReport({ duplicates: [copy, repeated] }), null);
    const fn = signals.functions.get(request.id)!;
    expect(fn.duplicates).toEqual([copy, repeated]);
    expect(functionBadge(fn)?.duplicates).toBe(2);
    expect(signals.files.get(SERVICE)?.duplicates).toHaveLength(2);
    expect(filesBadge([signals.files.get(SERVICE)])?.duplicates).toBe(2);
    expect(signals.duplicates).toHaveLength(2);
    expect(badgeParts({ severity: null, findings: 0, limits: 0, checks: 0, duplicates: 2, mutants: 0, untested: 0 })).toEqual([
      "2 duplicated blocks",
    ]);
    // A block in no changed function still counts on its file.
    const loose = reviewSignals(makeQualityReport({ duplicates: [makeDuplicate({ function: null })] }), null);
    expect(loose.functions.get(request.id)?.duplicates ?? []).toEqual([]);
    expect(loose.files.get(SERVICE)?.duplicates).toHaveLength(1);
  });

  it("says what it copies or how often it repeats, and its size", () => {
    expect(duplicateKindText(copy)).toBe("copies `internal/payment/money.go:40-47`");
    expect(duplicateKindText(repeated)).toBe("repeated 3× in the change");
    const two = makeDuplicate({ others: [...copy.others, { ...copy.others[0], file: "internal/payment/fee.go" }] });
    expect(duplicateKindText(two)).toBe("copies `internal/payment/money.go:40-47` and 1 more place");
    expect(duplicateSizeText(copy)).toBe("74 tokens, 8 lines");
    expect(duplicateSignal(copy)).toBe("duplicate:copies_existing:internal/refund/service.go:60:67");
  });

  it("starts a draft saying where it already is, in the language shown", async () => {
    expect(duplicateDraftBody(copy)).toBe(
      "This already exists in `internal/payment/money.go:40-47`; reuse it instead of duplicating it.",
    );
    expect(duplicateDraftBody(repeated)).toBe(
      "This block is repeated 3 times in this change (also in `internal/refund/validate.go:20-27` and " +
        "`internal/refund/notify.go:5-12`); write it once and reuse it.",
    );
    await i18n.changeLanguage("pt-BR");
    expect(duplicateDraftBody(copy)).toBe(
      "Este trecho já existe em `internal/payment/money.go:40-47`; reutilize-o em vez de duplicar.",
    );
    expect(duplicateKindText(copy)).toBe("copia `internal/payment/money.go:40-47`");
    expect(duplicateKindText(repeated)).toBe("repetido 3× na mudança");
    await i18n.changeLanguage("en");
  });
});

describe("mutants that survived", () => {
  const run = makeMutationRun();

  it("count on the function holding them and on their file, the others not at all", () => {
    const signals = reviewSignals(makeQualityReport(), null, run);
    const fn = signals.functions.get(request.id)!;
    expect(fn.mutants.map((mutant) => mutant.id)).toEqual(["m-1", "m-2"]);
    expect(functionBadge(fn)?.mutants).toBe(2);
    expect(signals.files.get(SERVICE)?.mutants).toHaveLength(2);
    expect(filesBadge([signals.files.get(SERVICE)])?.mutants).toBe(2);
    expect(signals.mutants.map((mutant) => mutant.id)).toEqual(["m-1", "m-2"]);
    expect(signals.mutation).toBe(run);
    expect(badgeParts({ severity: null, findings: 0, limits: 0, checks: 0, duplicates: 0, mutants: 2, untested: 0 })).toEqual([
      "2 mutants no test caught",
    ]);
    // A mutant at top level still counts on its file; a run that did not complete says nothing.
    const loose = reviewSignals(makeQualityReport(), null, makeMutationRun({ mutants: [makeMutant("m-9", { function: null })] }));
    expect(loose.files.get(SERVICE)?.mutants).toHaveLength(1);
    expect(loose.functions.get(request.id)?.mutants ?? []).toEqual([]);
    const cancelled = reviewSignals(makeQualityReport(), null, makeMutationRun({ status: "cancelled" }));
    expect(cancelled.mutants).toEqual([]);
    expect(cancelled.mutation).toBeNull();
  });

  it("say what changed and that no test failed, and are keyed by their id", () => {
    const [boundary, removal, timeout] = run.mutants;
    expect(mutantChangeText(boundary)).toBe("`if balance >= amount {` became `if balance > amount {` and no test failed");
    expect(mutantChangeText(removal)).toBe("Removing `s.notify(refund)` made no test fail");
    expect(mutantChangeText(timeout)).toBe("`for i <= n {` became `for i < n {` and the tests ran out of time");
    // Without the lines, the pieces alone.
    expect(mutantChangeText(makeMutant("m-0", { before: [], after: [] }))).toBe(
      "`>=` became `>` and no test failed",
    );
    expect(mutantSignal(boundary)).toBe("mutant:m-1");
  });

  it("go under the last line they change, marked on their first, shown with the checks", () => {
    const signals = reviewSignals(makeQualityReport({ checkFindings: [] }), null, run);
    const layout = fileSignalLayout(signals.files.get(SERVICE), { findings: false, checks: true });
    expect(layout.under.get(52)?.mutants.map((mutant) => mutant.id)).toEqual(["m-1"]);
    expect(layout.under.get(58)?.mutants.map((mutant) => mutant.id)).toEqual(["m-2"]);
    expect(new Map(layout.lane.map((mark) => [mark.line, mark])).get(52)).toEqual({ line: 52, mutant: true });
    const hidden = fileSignalLayout(signals.files.get(SERVICE), { findings: true, checks: false });
    expect(hidden.under.get(52)).toBeUndefined();
  });
});

describe("the draft a mutant that survived starts", () => {
  const draft = (overrides: Partial<Mutant>) => mutantDraftBody(makeMutant("m", overrides));

  it("names the change and the test missing, by operator", () => {
    expect(draft({})).toBe("No test fails if `>=` becomes `>` here; a test for this boundary is missing.");
    expect(draft({ operator: "relational_negation", original: "==", mutated: "!=" })).toBe(
      "No test fails if `==` becomes `!=` here; a test that checks this comparison is missing.",
    );
    expect(draft({ operator: "negation_removal", original: "!ok", mutated: "ok" })).toBe(
      "No test fails if `!ok` becomes `ok` here; a test that checks this condition is missing.",
    );
    expect(draft({ operator: "arithmetic", original: "+", mutated: "-" })).toBe(
      "No test fails if `+` becomes `-` here; a test that checks the result of this calculation is missing.",
    );
    expect(draft({ operator: "logical", original: "&&", mutated: "||" })).toBe(
      "No test fails if `&&` becomes `||` here; a test where only one of these conditions holds is missing.",
    );
    expect(draft({ operator: "return_value", original: "total", mutated: "0" })).toBe(
      "No test fails if the return becomes `0` here; a test that checks the value returned is missing.",
    );
    expect(draft({ operator: "call_removal", original: "s.notify(\n\trefund)", mutated: "" })).toBe(
      "No test fails if the call `s.notify( refund)` is removed; a test that checks its effect is missing.",
    );
    expect(draft({ operator: "increment", original: "++", mutated: "--" })).toBe(
      "No test fails if `++` becomes `--` here; a test that checks this count is missing.",
    );
    expect(draft({ operator: "other", original: "a", mutated: "b" })).toBe(
      "No test fails if `a` becomes `b` here; a test that checks this line is missing.",
    );
  });

  it("is written in the language shown", async () => {
    await i18n.changeLanguage("pt-BR");
    expect(draft({})).toBe("Nenhum teste falha se `>=` virar `>` aqui; falta um teste para esse limite.");
    expect(draft({ operator: "negation_removal", original: "!ok", mutated: "ok" })).toBe(
      "Nenhum teste falha se `!ok` virar `ok` aqui; falta um teste que verifique essa condição.",
    );
    expect(draft({ operator: "return_value", original: "total", mutated: "0" })).toBe(
      "Nenhum teste falha se o retorno virar `0` aqui; falta um teste que verifique o valor retornado.",
    );
    expect(draft({ operator: "call_removal", original: "x()", mutated: "" })).toBe(
      "Nenhum teste falha se a chamada `x()` for removida; falta um teste que verifique o efeito dela.",
    );
    expect(mutantChangeText(makeMutant("m"))).toBe(
      "`if balance >= amount {` virou `if balance > amount {` e nenhum teste falhou",
    );
    await i18n.changeLanguage("en");
  });
});

describe("the signals of a file in the Diff", () => {
  it("put a finding under the last line it cites and a check under its line, and mark the lane", () => {
    const signals = reviewSignals(report(), findings());
    const layout = fileSignalLayout(signals.files.get(SERVICE), { findings: true, checks: true });

    expect(layout.under.get(56)?.findings.map((finding) => finding.id)).toEqual(["high"]);
    expect(layout.under.get(47)?.dismissed.map((finding) => finding.id)).toEqual(["gone"]);
    expect(layout.under.get(50)?.checks).toHaveLength(1);
    expect(layout.starts.get(44)?.fn.id).toBe(request.id);
    const lane = new Map(layout.lane.map((mark) => [mark.line, mark]));
    expect(lane.get(44)).toEqual({ line: 44, limit: true });
    expect(lane.get(45)).toEqual({ line: 45, range: "single" });
    expect([52, 53, 56].map((line) => lane.get(line)?.range)).toEqual(["start", "middle", "end"]);
    expect(lane.get(50)).toEqual({ line: 50, check: true });
  });

  it("put duplicated code under the last line of its block, marked on its first, shown with the checks", () => {
    const signals = reviewSignals(makeQualityReport({ checkFindings: [], duplicates: [makeDuplicate()] }), null);
    const layout = fileSignalLayout(signals.files.get(SERVICE), { findings: false, checks: true });
    expect(layout.under.get(67)?.duplicates).toHaveLength(1);
    expect(new Map(layout.lane.map((mark) => [mark.line, mark])).get(60)).toEqual({ line: 60, duplicate: true });
    const hidden = fileSignalLayout(signals.files.get(SERVICE), { findings: true, checks: false });
    expect(hidden.under.get(67)).toBeUndefined();
  });

  it("leaves out the kinds hidden, never the limits", () => {
    const signals = reviewSignals(report(), findings());
    const layout = fileSignalLayout(signals.files.get(SERVICE), { findings: false, checks: false });
    expect([...layout.under.keys()]).toEqual([]);
    expect(layout.lane).toEqual([{ line: 44, limit: true }]);
    expect(fileSignalLayout(undefined, { findings: true, checks: true }).lane).toEqual([]);
  });
});
