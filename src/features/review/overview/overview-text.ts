// What the Overview tab says, worked out from data the review already has: the criteria tally, where to look first,
// what was measured, the shape of the change and what changed since the last review. Nothing here asks anything.

import type { DiffTarget } from "@/features/review/flow/flow-facts";
import { formatScore } from "@/features/review/quality/quality-text";
import { COVERAGE_TARGET, coverageRatio, fileDiffTarget } from "@/features/review/ReviewPage";
import {
  CHECK_SEVERITY_RANK,
  duplicateSignal,
  findingLine,
  type FunctionSignals,
  type ReviewSignals,
  reviewSignals,
} from "@/features/review/signals/signals";
import { duplicateKindText, duplicateSizeText, rangeText } from "@/features/review/signals/signals-text";
import { mutationScore } from "@/features/review/tests/mutation-text";
import type {
  ChangedFile,
  ChangeShape,
  CheckClass,
  CheckSeverity,
  Criterion,
  CriterionVerdict,
  CrossedLimit,
  Duplicate,
  Finding,
  FlowGraph,
  LeftOut,
  LocatedCheck,
  QualityFunction,
  QualityReport,
  ReviewState,
  Severity,
  TestRun,
} from "@/lib/api";
import { i18n } from "@/i18n";
import { formatNumber, formatPercent, intlLocale } from "@/lib/format";

/** "Partly met", in the language shown. */
export function verdictLabel(verdict: CriterionVerdict): string {
  return i18n.t(`overview.verdict.${verdict}`);
}

const VERDICT_ORDER: readonly CriterionVerdict[] = ["met", "partly_met", "not_met", "no_evidence"];

/** "2 met, 2 partly met, 1 not met, 1 without evidence", leaving out what is none. */
export function tallyText(criteria: readonly Criterion[]): string {
  return VERDICT_ORDER.flatMap((verdict) => {
    const count = criteria.filter((criterion) => criterion.verdict === verdict).length;
    return count === 0 ? [] : [i18n.t(`overview.tally.${verdict}`, { count })];
  }).join(", ");
}

/** Said of every "No evidence", so it is not read as "missing". */
export function noEvidenceNote(): string {
  return i18n.t("overview.noEvidenceNote");
}

/** Where an item of "Where to look first" opens. */
export type Jump =
  | { kind: "flow"; functionId: string }
  | { kind: "diff"; target: DiffTarget }
  | { kind: "tests" };

export interface LookItem {
  id: string;
  title: string;
  detail: string;
  /** A static check's own message, untranslated, shown before the detail after "Line 96:". */
  quote: { line: number; message: string } | null;
  /** Written by the model, measured by DiffLens, or both. */
  model: boolean;
  measured: boolean;
  /** The finding's severity, for its chip. */
  severity: Severity | null;
  /** The most urgent static check, for its chip and its severity's tag. */
  tool: { tool: string; rule: string | null; severity: CheckSeverity } | null;
  /** Limits crossed, for the gauge chip; 0 for none. */
  limits: number;
  /** No test executes the function. */
  untested: boolean;
  /** Duplicated code, for its chip. */
  duplicate: boolean;
  /** "refund/service.go:52", shown as code; null when it has no single place. */
  location: string | null;
  jump: Jump | null;
}

export interface LookInputs {
  findings: readonly Finding[] | null;
  /** The last completed test run, if any. */
  run: TestRun | null;
  quality: QualityReport | null;
  graph: FlowGraph | null;
  files: readonly ChangedFile[];
  /** The signals of `quality` and `findings`, when already worked out. */
  signals?: ReviewSignals;
}

/** At most this many items in "Where to look first". */
export const LOOK_LIMIT = 5;

// Lower first: what hurts most, then what was measured over what was only read. A correctness check (go vet,
// staticcheck SA) is almost always a real defect and needs no model; a style check weighs less than anything.
const FINDING_RANK: Record<Severity, number> = { high: 0, medium: 4, low: 6 };
const FAILING_RANK = 1;
const CHECK_RANK: Record<CheckClass, number> = { correctness: 2, style: 7 };
/** Within its class, a check the tool calls an error comes before a warning, a warning before an info. */
const checkRank = (check: LocatedCheck) => CHECK_RANK[check.class] + CHECK_SEVERITY_RANK[check.severity] / 10;
const LIMIT_RANK = 3;
// Code that already exists elsewhere is measured, not read, and costs little to act on: with the limits.
const DUPLICATE_RANK = 3;
const UNTESTED_RANK = 5;

const NO_MARKS = { quote: null, severity: null, tool: null, limits: 0, untested: false, duplicate: false } as const;

/**
 * The ranked list of "Where to look first": the model's findings that were not dismissed mixed with what DiffLens
 * measured (failing tests, static check findings, functions over a limit, changed functions no test runs), at most
 * `LOOK_LIMIT`. What was measured in one function is one item, ranked by its most urgent part.
 */
export function lookFirst({ findings, run, quality, graph, files, signals }: LookInputs): LookItem[] {
  const shown = signals ?? reviewSignals(quality, findings);
  const ranked: { rank: number; item: LookItem }[] = [];
  for (const finding of shown.findings) {
    const line = findingLine(finding);
    const found = finding.location.kind === "found";
    ranked.push({
      rank: FINDING_RANK[finding.severity],
      item: {
        ...NO_MARKS,
        id: `finding:${finding.id}`,
        title: finding.title,
        detail: finding.explanation,
        model: true,
        measured: false,
        severity: finding.severity,
        location: `${finding.path}:${line}`,
        jump: found ? placeJump(finding.path, line, graph, files) : null,
      },
    });
  }
  for (const item of failingItems(run, files)) ranked.push({ rank: FAILING_RANK, item });
  if (quality) {
    for (const signals of [...shown.functions.values()]) {
      const item = measuredItem(signals, graph, files);
      if (item) ranked.push(item);
    }
    for (const check of shown.checks) {
      if (check.function === null) ranked.push({ rank: checkRank(check), item: checkItem(check, files) });
    }
    for (const duplicate of shown.duplicates) {
      ranked.push({ rank: DUPLICATE_RANK, item: duplicateItem(duplicate, quality.functions, graph, files) });
    }
  }
  return ranked
    .map((entry, index) => ({ ...entry, index }))
    .sort((a, b) => a.rank - b.rank || a.index - b.index)
    .slice(0, LOOK_LIMIT)
    .map(({ item }) => item);
}

/** The model's findings that "Where to look first" left out, most severe first: the first `SUGGESTION_LIMIT`. */
export interface Suggestions {
  items: Finding[];
  /** How many were left out of "Where to look first". */
  left: number;
  /** How many are in "Where to look first". */
  above: number;
  /** Every finding not dismissed. */
  total: number;
}

export const SUGGESTION_LIMIT = 5;

/** "The model suggests changing": the findings not already in `look`; null when none is left. */
export function suggestions(signals: ReviewSignals, look: readonly LookItem[]): Suggestions | null {
  const listed = new Set(look.map((item) => item.id));
  const left = signals.findings.filter((finding) => !listed.has(`finding:${finding.id}`));
  if (left.length === 0) return null;
  return {
    items: left.slice(0, SUGGESTION_LIMIT),
    left: left.length,
    above: signals.findings.length - left.length,
    total: signals.findings.length,
  };
}

/** Joins "a", "b" and "c" as the language does. */
function listOf(items: readonly string[]): string {
  return new Intl.ListFormat(intlLocale(), { type: "conjunction" }).format(items);
}

function capitalized(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** One item for what was measured in a function: its checks, its limits and whether any test runs it. */
function measuredItem(
  signals: FunctionSignals,
  graph: FlowGraph | null,
  files: readonly ChangedFile[],
): { rank: number; item: LookItem } | null {
  const { fn, limits, untested } = signals;
  const checks = [...signals.checks].sort((a, b) => checkRank(a) - checkRank(b) || a.line - b.line);
  if (checks.length === 0 && limits.length === 0 && !untested) return null;
  const ranks = [
    ...checks.map(checkRank),
    ...(limits.length > 0 ? [LIMIT_RANK] : []),
    ...(untested ? [UNTESTED_RANK] : []),
  ];
  const first = checks[0];
  const tools = [...new Set(checks.map((check) => check.tool))];
  const name = fn.name;
  let title: string;
  if (first) {
    const checkPart =
      checks.length === 1
        ? i18n.t("overview.look.checkOne", { tool: first.tool })
        : i18n.t("overview.look.checkMany", { count: checks.length, tools: listOf(tools) });
    title =
      limits.length > 0
        ? i18n.t("overview.look.checksAndLimits", { name, checks: checkPart, count: limits.length })
        : untested
          ? i18n.t("overview.look.checksAndUntested", { name, checks: checkPart })
          : checks.length === 1
            ? i18n.t("overview.look.checkIn", { tool: first.tool, name })
            : i18n.t("overview.look.checksIn", { tools: listOf(tools), count: checks.length, name });
  } else if (limits.length > 0) {
    title = limitsTitle(signals);
  } else {
    title = i18n.t("overview.look.uncovered", { name });
  }
  const details = [
    limits.length > 0 ? limitsDetail(signals, first === undefined) : "",
    untested && (first !== undefined || limits.length > 0) ? i18n.t("overview.look.untestedToo") : "",
    untested && first === undefined && limits.length === 0 ? untestedDetail(signals) : "",
  ].filter((part) => part !== "");
  return {
    rank: Math.min(...ranks),
    item: {
      id: `function:${fn.id}`,
      title,
      detail: details.join(" "),
      quote: first ? { line: first.line, message: first.message } : null,
      model: false,
      measured: true,
      severity: null,
      tool: first ? { tool: first.tool, rule: first.rule, severity: first.severity } : null,
      limits: limits.length,
      untested,
      duplicate: false,
      location: `${fn.file}:${fn.startLine}`,
      jump: functionJump(fn, graph, files),
    },
  };
}

/** A static check finding on an added line outside any changed function. */
function checkItem(check: LocatedCheck, files: readonly ChangedFile[]): LookItem {
  const target = fileDiffTarget(check.file, check.line, files);
  return {
    ...NO_MARKS,
    id: `check:${check.tool}:${check.rule ?? ""}:${check.file}:${check.line}`,
    title: i18n.t("overview.look.checkInFile", { tool: check.tool, file: check.file.split("/").pop() ?? check.file }),
    detail: "",
    quote: { line: check.line, message: check.message },
    model: false,
    measured: true,
    tool: { tool: check.tool, rule: check.rule, severity: check.severity },
    location: `${check.file}:${check.line}`,
    jump: target ? { kind: "diff", target } : null,
  };
}

/** Duplicated code the review introduces: what it copies or how many times it is repeated, and its size. */
function duplicateItem(
  duplicate: Duplicate,
  functions: readonly QualityFunction[],
  graph: FlowGraph | null,
  files: readonly ChangedFile[],
): LookItem {
  const { block } = duplicate;
  const copies = duplicate.kind === "copies_existing";
  const name = duplicate.function
    ? (functions.find((fn) => fn.id === duplicate.function?.id)?.name ?? duplicate.function.qualifiedName)
    : null;
  const file = block.file.split("/").pop() ?? block.file;
  const title =
    name === null
      ? i18n.t(copies ? "overview.look.duplicateCopiesInFile" : "overview.look.duplicateRepeatsInFile", { file })
      : i18n.t(copies ? "overview.look.duplicateCopies" : "overview.look.duplicateRepeats", { name });
  // The detail is plain text here: the places lose their backticks.
  const what = capitalized(duplicateKindText(duplicate).replaceAll("`", ""));
  return {
    ...NO_MARKS,
    id: duplicateSignal(duplicate),
    title,
    detail: i18n.t("overview.look.duplicateDetail", { what, size: duplicateSizeText(duplicate) }),
    model: false,
    measured: true,
    duplicate: true,
    location: rangeText(block),
    jump: placeJump(block.file, block.startLine, graph, files),
  };
}

const has = (signals: FunctionSignals, kind: CrossedLimit["kind"]) =>
  signals.limits.find((limit) => limit.kind === kind) ?? null;

function limitsTitle(signals: FunctionSignals): string {
  const name = signals.fn.name;
  const complexity = has(signals, "complexity");
  const crap = has(signals, "crap");
  const coverage = has(signals, "changed_lines_coverage");
  if (!coverage && complexity && crap) return i18n.t("overview.look.complexAndUntested", { name });
  if (signals.limits.length > 1) return i18n.t("overview.look.overLimits", { name, count: signals.limits.length });
  if (complexity) return i18n.t("overview.look.overComplexity", { name });
  if (crap) return i18n.t("overview.look.risky", { name });
  return i18n.t("overview.look.belowCoverage", { name });
}

/**
 * What the limits measured. Complexity and CRAP alone keep the words they always had; with the coverage, or under a
 * static check, every value and every limit in one sentence.
 */
function limitsDetail(signals: FunctionSignals, alone: boolean): string {
  const { fn } = signals;
  const complexity = has(signals, "complexity");
  const crap = has(signals, "crap");
  const coverage = has(signals, "changed_lines_coverage");
  const ratio = fn.changedLines ? coverageRatio(fn.changedLines) : null;
  const covered = ratio === null ? null : formatPercent(ratio);
  if (alone && !coverage) {
    if (complexity && crap) {
      const values = {
        complexity: formatScore(complexity.value),
        crap: formatScore(crap.value),
        complexityLimit: formatScore(complexity.limit),
        crapLimit: formatScore(crap.limit),
      };
      return covered === null
        ? i18n.t("overview.look.complexAndUntestedDetail", values)
        : i18n.t("overview.look.complexAndUntestedDetailCovered", { ...values, covered });
    }
    if (complexity) {
      return i18n.t("overview.look.overComplexityDetail", {
        value: formatScore(complexity.value),
        limit: formatScore(complexity.limit),
      });
    }
    if (crap) {
      const values = { value: formatScore(crap.value), limit: formatScore(crap.limit) };
      return covered === null
        ? i18n.t("overview.look.riskyDetail", values)
        : i18n.t("overview.look.riskyDetailCovered", { ...values, covered });
    }
  }
  const values = signals.limits.map((limit) =>
    limit.kind === "complexity"
      ? i18n.t("overview.look.value.complexity", { value: formatScore(limit.value) })
      : limit.kind === "crap"
        ? i18n.t("overview.look.value.crap", { value: formatScore(limit.value) })
        : i18n.t("overview.look.value.coverage", { value: formatPercent(limit.value) }),
  );
  const bounds = signals.limits.map((limit) =>
    limit.kind === "changed_lines_coverage" ? formatPercent(limit.limit) : formatScore(limit.limit),
  );
  return i18n.t("overview.look.limitsDetail", {
    count: signals.limits.length,
    values: capitalized(listOf(values)),
    limits: listOf(bounds),
  });
}

function untestedDetail({ fn }: FunctionSignals): string {
  const coverable = fn.changedLines?.coverable ?? 0;
  return coverable > 0
    ? i18n.t("overview.look.uncoveredDetail", { count: coverable })
    : i18n.t("overview.look.neverRun");
}

/** A function of the graph holding `path:line` opens in Flow; else the line opens in Diff when the file is changed. */
function placeJump(path: string, line: number, graph: FlowGraph | null, files: readonly ChangedFile[]): Jump | null {
  const node = graph?.nodes.find(
    (candidate) =>
      candidate.file === path &&
      candidate.state !== "deleted" &&
      candidate.startLine !== null &&
      candidate.endLine !== null &&
      candidate.startLine <= line &&
      line <= candidate.endLine,
  );
  if (node) return { kind: "flow", functionId: node.id };
  const target = fileDiffTarget(path, line, files);
  return target ? { kind: "diff", target } : null;
}

function functionJump(fn: QualityFunction, graph: FlowGraph | null, files: readonly ChangedFile[]): Jump | null {
  if (graph?.nodes.some((node) => node.id === fn.id)) return { kind: "flow", functionId: fn.id };
  const target = fileDiffTarget(fn.file, fn.startLine, files);
  return target ? { kind: "diff", target } : null;
}

function failingItems(run: TestRun | null, files: readonly ChangedFile[]): LookItem[] {
  const failing = run?.tests.filter((test) => test.outcome === "fail") ?? [];
  if (failing.length > 2) {
    const [first, second] = failing;
    return [
      {
        ...NO_MARKS,
        id: "tests:failing",
        title: i18n.t("overview.look.failingTests", { count: failing.length }),
        detail: i18n.t("overview.look.failingTestsDetail", {
          first: first.name,
          second: second.name,
          count: failing.length - 2,
        }),
        model: false,
        measured: true,
        location: null,
        jump: { kind: "tests" },
      },
    ];
  }
  return failing.map((test) => {
    const output = test.output
      ?.split("\n")
      .map((line) => line.trim())
      .find((line) => line !== "" && !line.startsWith("==="));
    const target = test.file !== null && test.line !== null ? fileDiffTarget(test.file, test.line, files) : null;
    return {
      ...NO_MARKS,
      id: `test:${test.id}`,
      title: i18n.t("overview.look.testFails", { name: test.name }),
      detail: output
        ? truncate(output, 160)
        : test.exercises
          ? i18n.t("overview.look.exercises", { name: test.exercises.qualifiedName })
          : i18n.t("overview.look.inContainer", { container: test.container }),
      model: false,
      measured: true,
      location: test.file !== null && test.line !== null ? `${test.file}:${test.line}` : null,
      jump: target ? { kind: "diff", target } : { kind: "tests" },
    };
  });
}

function truncate(text: string, max: number): string {
  return text.length <= max ? text : `${text.slice(0, max - 1)}…`;
}

export interface MeasuredRow {
  label: string;
  /** The number, said plainly: "41 passing". */
  value: string;
  /** A second part in the problem colour: "1 failing". */
  bad?: string;
  tone?: "ok" | "warn" | "bad";
  note?: string;
}

/**
 * "Measured in this branch": only rows DiffLens has data for. Tests and coverage come from the last completed run;
 * functions over a limit, changed functions no test runs (once a run measured coverage) and the static checks of the
 * changed lines from the quality report, once there is one; mutation testing from its last completed run, when one
 * was asked for.
 */
export function measuredRows(
  run: TestRun | null,
  quality: QualityReport | null,
  signals: ReviewSignals = reviewSignals(quality, null),
): MeasuredRow[] {
  const rows: MeasuredRow[] = [];
  if (run) {
    const { passed, failed, added } = run.summary;
    rows.push({
      label: i18n.t("overview.measured.tests"),
      value: i18n.t("overview.measured.passing", { passed: formatNumber(passed) }),
      bad: failed > 0 ? i18n.t("overview.measured.failing", { failed: formatNumber(failed) }) : undefined,
      note: i18n.t("overview.measured.added", { count: added }),
    });
    const changed = run.coverage ? coverageRatio(run.coverage.changedLines) : null;
    if (changed !== null) {
      const target = quality?.limits.changedLinesCoverage ?? COVERAGE_TARGET;
      rows.push({
        label: i18n.t("overview.measured.coverage"),
        value: formatPercent(changed),
        tone: changed >= target ? "ok" : "warn",
        note: i18n.t("overview.measured.coverageTarget", { target: formatPercent(target) }),
      });
    }
  }
  if (quality) {
    const functions = [...signals.functions.values()];
    const over = functions.filter((fn) => fn.limits.length > 0);
    const added = over.filter((fn) => fn.fn.state === "new").length;
    const limits = i18n.t("overview.measured.limitsAre", {
      complexity: formatScore(quality.limits.complexity),
      crap: formatScore(quality.limits.crap),
      coverage: formatPercent(quality.limits.changedLinesCoverage),
    });
    const which =
      over.length === 0
        ? null
        : added === over.length
          ? over.length === 1
            ? i18n.t("overview.measured.oneAdded")
            : over.length === 2
              ? i18n.t("overview.measured.bothAdded")
              : i18n.t("overview.measured.allAdded")
          : added === 0
            ? i18n.t("overview.measured.noneNew")
            : i18n.t("overview.measured.someNew", { count: added });
    rows.push({
      label: i18n.t("overview.measured.overLimit"),
      value: formatNumber(over.length),
      tone: over.length > 0 ? "warn" : "ok",
      note: which === null ? limits : `${limits}; ${which}`,
    });
    // Only a run that measured coverage can tell a function no test runs.
    if (quality.testRunAt !== null) {
      const untested = functions.filter((fn) => fn.untested).length;
      rows.push({
        label: i18n.t("overview.measured.untested"),
        value: formatNumber(untested),
        tone: untested > 0 ? "warn" : "ok",
      });
    }
    const ran = quality.checks.filter((check) => check.status === "clean" || check.status === "findings");
    if (ran.length > 0) {
      const counts = new Map<string, number>();
      for (const check of signals.checks) counts.set(check.tool, (counts.get(check.tool) ?? 0) + 1);
      rows.push({
        label: i18n.t("overview.measured.checks"),
        value:
          signals.checks.length > 0
            ? i18n.t("overview.measured.checkFindings", { count: signals.checks.length })
            : i18n.t("overview.measured.noCheckFindings"),
        tone: signals.checks.length > 0 ? "warn" : "ok",
        note:
          signals.checks.length > 0
            ? [...counts]
                .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
                .map(([tool, count]) => `${tool} ${formatNumber(count)}`)
                .join(", ")
            : listOf(ran.map((check) => check.tool)),
      });
    }
    const duplicates = signals.duplicates.length;
    rows.push({
      label: i18n.t("overview.measured.duplication"),
      value:
        duplicates > 0
          ? i18n.t("overview.measured.duplicates", { count: duplicates })
          : i18n.t("overview.measured.noDuplicates"),
      tone: duplicates > 0 ? "warn" : "ok",
      note: i18n.t("overview.measured.duplicationLimits", {
        tokens: formatNumber(quality.limits.duplicationTokens),
        lines: formatNumber(quality.limits.duplicationLines),
      }),
    });
  }
  const mutation = signals.mutation;
  if (mutation) {
    const { killed, total } = mutationScore(mutation.summary);
    const survived = mutation.summary.survived;
    rows.push({
      label: i18n.t("overview.measured.mutation"),
      value:
        total > 0
          ? i18n.t("overview.measured.mutationScore", { killed: formatNumber(killed), total: formatNumber(total) })
          : i18n.t("overview.measured.noMutants"),
      tone: survived > 0 ? "warn" : "ok",
      note:
        survived > 0
          ? i18n.t("overview.measured.survived", { count: survived })
          : i18n.t("overview.measured.noneSurvived"),
    });
  }
  return rows;
}

export interface ShapeRow {
  label: string;
  value: string;
  /** What the value counts, by name. */
  title?: string;
}

/** "Shape of the change": entry points from the flow graph when there is one, the rest from the changed files. */
export function shapeRows(shape: ChangeShape, graph: FlowGraph | null): ShapeRow[] {
  const rows: ShapeRow[] = [];
  if (graph) {
    const states = new Map(graph.nodes.map((node) => [node.id, node.state]));
    const total = graph.entryPoints.length;
    const added = graph.entryPoints.filter((id) => states.get(id) === "new").length;
    rows.push({
      label: i18n.t("overview.shape.entryPoints"),
      value:
        total === 0
          ? i18n.t("overview.shape.noneReached")
          : counts(
              [
                i18n.t("overview.shape.entryNew", { count: added }),
                i18n.t("overview.shape.entryAffected", { count: total - added }),
              ],
              [added, total - added],
            ),
    });
  }
  const directories = [...shape.directoriesNew, ...shape.directoriesModified];
  if (directories.length > 0) {
    rows.push({
      label: i18n.t("overview.shape.directories"),
      value: counts(
        [
          i18n.t("overview.shape.directoriesNew", { count: shape.directoriesNew.length }),
          i18n.t("overview.shape.directoriesModified", { count: shape.directoriesModified.length }),
        ],
        [shape.directoriesNew.length, shape.directoriesModified.length],
      ),
      title: [
        ...shape.directoriesNew.map((directory) => i18n.t("overview.shape.newDirectory", { directory })),
        ...shape.directoriesModified,
      ].join("\n"),
    });
  }
  rows.push({
    label: i18n.t("overview.shape.files"),
    value:
      shape.filesAdded + shape.filesDeleted === 0
        ? i18n.t("overview.shape.noneAddedOrDeleted")
        : counts(
            [
              i18n.t("overview.shape.filesAdded", { count: shape.filesAdded }),
              i18n.t("overview.shape.filesDeleted", { count: shape.filesDeleted }),
            ],
            [shape.filesAdded, shape.filesDeleted],
          ),
  });
  const newMigrations = shape.migrations.filter((file) => file.status === "added").length;
  const changedMigrations = shape.migrations.length - newMigrations;
  rows.push({
    label: i18n.t("overview.shape.database"),
    value:
      shape.migrations.length === 0
        ? i18n.t("overview.shape.noMigration")
        : counts(
            [
              i18n.t("overview.shape.newMigrations", { count: newMigrations }),
              i18n.t("overview.shape.changedMigrations", { count: changedMigrations }),
            ],
            [newMigrations, changedMigrations],
          ),
    title: shape.migrations.map((file) => file.path).join("\n") || undefined,
  });
  rows.push({
    label: i18n.t("overview.shape.dependencies"),
    value:
      shape.manifests.length === 0
        ? i18n.t("overview.shape.noManifest")
        : i18n.t("overview.shape.manifestsChanged", {
            count: shape.manifests.length,
            names: names(shape.manifests.map((file) => file.path)),
          }),
    title: shape.manifests.map((file) => file.path).join("\n") || undefined,
  });
  return rows;
}

/** The parts whose count is not zero, joined: "1 new, 2 modified". */
function counts(parts: string[], values: number[]): string {
  return parts.filter((_, index) => values[index] > 0).join(", ");
}

/** Up to three file names, then how many more. */
function names(paths: readonly string[]): string {
  const base = paths.map((path) => path.split("/").pop() ?? path);
  if (base.length <= 3) return base.join(", ");
  return i18n.t("overview.shape.namesAndMore", { names: base.slice(0, 3).join(", "), more: base.length - 3 });
}

export interface Since {
  checkpointSha: string | null;
  /** RFC 3339. */
  at: string;
  /** null when nothing can be compared: the working tree, or the commit is gone. */
  commits: number | null;
  files: ChangedFile[];
  /** Why there is nothing to compare, when there is not. */
  note: string | null;
  reopened: number;
  /** The user's own unresolved threads whose line changed. */
  comments: number;
  /** Findings the user confirmed or dismissed whose code changed since. */
  findings: number;
}

/** "Since your last review", or null when there was no earlier review. */
export function sinceOf(state: ReviewState | null, findings: readonly Finding[] | null): Since | null {
  if (state?.checkpoint == null) return null;
  const { since } = state;
  const note =
    since.kind === "available"
      ? null
      : since.reason === "up_to_date"
        ? i18n.t("overview.since.upToDate")
        : since.reason === "checkpoint_missing"
          ? i18n.t("overview.since.checkpointMissing")
          : since.reason === "working_tree"
            ? i18n.t("overview.since.workingTree")
            : null;
  return {
    checkpointSha: state.checkpoint.sha,
    at: state.checkpoint.at,
    commits: since.kind === "available" ? since.commits : null,
    files: since.kind === "available" ? since.files : [],
    note,
    reopened: state.summary.reopenedFiles,
    comments: state.comments.filter(
      (comment) => comment.parentId === null && comment.mine && !comment.resolved && comment.outdated,
    ).length,
    findings: (findings ?? []).filter((finding) => finding.recheck).length,
  };
}

/** "3 files changed: service.go, validate.go and service_test.go", the names cut after three. */
export function changedFilesText(files: readonly ChangedFile[]): string {
  return i18n.t("overview.since.changedFiles", { count: files.length, names: names(files.map((file) => file.path)) });
}

/** A line of "Left out of the analysis": a file, shown as code, and what was left out of it. */
export interface LeftOutLine {
  path: string | null;
  text: string;
}

/** "Left out of the analysis": what a model reading the change would not see. */
export function leftOutLines(leftOut: LeftOut, condensed: boolean): LeftOutLine[] {
  const skipped = (reason: LeftOut["skipped"][number]["reason"]) => i18n.t(`overview.leftOut.skipped.${reason}`);
  return [
    ...leftOut.skipped.map((file) => ({ path: file.path, text: skipped(file.reason) })),
    ...leftOut.truncated.map((path) => ({ path, text: i18n.t("overview.leftOut.truncated") })),
    ...(condensed ? [{ path: null, text: i18n.t("overview.leftOut.condensed") }] : []),
  ];
}
