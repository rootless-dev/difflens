// Words for the review's signals, in the language shown: what a badge says to a screen reader, the rows of a
// function's limits, the limit marker of the Diff, what a duplicate is, what a mutant changed, and the comment a
// draft starts with (a model finding's is written by the model, in the core).

import { categoryName, severityName } from "@/features/review/insights/insights-text";
import { formatScore } from "@/features/review/quality/quality-text";
import {
  type Badge,
  findingEndLine,
  findingLine,
  type FunctionSignals,
} from "@/features/review/signals/signals";
import { i18n } from "@/i18n";
import type {
  CheckSeverity,
  CrossedLimit,
  Duplicate,
  DuplicateBlock,
  Finding,
  LocatedCheck,
  Mutant,
  MutationOperator,
  QualityLimits,
  Severity,
} from "@/lib/api";
import { formatNumber, formatPercent, intlLocale } from "@/lib/format";

/** The text colour of a severity. */
export const SEVERITY_TEXT: Record<Severity, string> = {
  high: "text-sev-high",
  medium: "text-sev-med",
  low: "text-sev-low",
};

/**
 * The parts of a badge as a screen reader hears them, in the badge's order: "4 model findings, the most severe of
 * high severity", "3 limits crossed", "1 static check finding", "no test runs it". A group counts the functions no
 * test runs.
 */
export function badgeParts(badge: Badge, group = false): string[] {
  const parts: string[] = [];
  if (badge.findings > 0 && badge.severity !== null) {
    parts.push(
      i18n.t("signals.badge.findings", {
        count: badge.findings,
        severity: severityName(badge.severity).toLowerCase(),
      }),
    );
  }
  if (badge.limits > 0) parts.push(i18n.t("signals.badge.limits", { count: badge.limits }));
  if (badge.checks > 0) parts.push(i18n.t("signals.badge.checks", { count: badge.checks }));
  if (badge.duplicates > 0) parts.push(i18n.t("signals.badge.duplicates", { count: badge.duplicates }));
  if (badge.mutants > 0) parts.push(i18n.t("signals.badge.mutants", { count: badge.mutants }));
  if (badge.untested > 0) {
    parts.push(
      group
        ? i18n.t("signals.badge.untestedFunctions", { count: badge.untested })
        : i18n.t("signals.badge.untested"),
    );
  }
  return parts;
}

/** "Correctness, line 91", "Testing, lines 84–90". */
export function findingPlace(finding: Finding): string {
  const start = findingLine(finding);
  const end = findingEndLine(finding);
  const category = categoryName(finding.category);
  return end > start
    ? i18n.t("signals.findings.lines", { category, start, end })
    : i18n.t("signals.findings.line", { category, line: start });
}

/** "Error", "Warning", "Info": a static check finding's severity, in the language shown. */
export function checkSeverityName(severity: CheckSeverity): string {
  return i18n.t(`signals.checkSeverity.${severity}`);
}

/** "staticcheck SA4006", the tool and its rule as the tool writes them. */
export function checkName(check: Pick<LocatedCheck, "tool" | "rule">): string {
  return check.rule ? `${check.tool} ${check.rule}` : check.tool;
}

export interface LimitRow {
  kind: "complexity" | "crap" | "coverage" | "untested";
  label: string;
  note: string | null;
  value: string;
  /** "/ limit 10", "/ minimum 80%". */
  bound: string;
  /** Crossed: drawn with the gauge, the value in the "over the limit" red. */
  over: boolean;
}

const crossedKind = (signals: FunctionSignals, kind: CrossedLimit["kind"]) =>
  signals.limits.some((limit) => limit.kind === kind);

/**
 * A function's three limits as the popover and the Diff list them: complexity (with its value before the branch),
 * CRAP and the coverage of its changed lines, each that was measured; the crossed ones marked. When no test runs the
 * function, that comes first, in place of the coverage.
 */
export function limitRows(signals: FunctionSignals, limits: QualityLimits): LimitRow[] {
  const { fn } = signals;
  const rows: LimitRow[] = [];
  const minimum = i18n.t("signals.limits.minimum", { value: formatPercent(limits.changedLinesCoverage) });
  if (signals.untested) {
    rows.push({
      kind: "untested",
      label: i18n.t("signals.limits.untested"),
      note: fn.changedLines ? i18n.t("signals.limits.untestedNote", { count: fn.changedLines.coverable }) : null,
      value: formatPercent(0),
      bound: minimum,
      over: true,
    });
  }
  if (fn.complexity !== null) {
    rows.push({
      kind: "complexity",
      label: i18n.t("signals.limits.complexity"),
      note:
        fn.previousComplexity !== null && fn.previousComplexity !== fn.complexity
          ? i18n.t("signals.limits.complexityBefore", { value: formatScore(fn.previousComplexity) })
          : null,
      value: formatScore(fn.complexity),
      bound: i18n.t("signals.limits.limit", { value: formatScore(limits.complexity) }),
      over: crossedKind(signals, "complexity"),
    });
  }
  if (fn.crap !== null) {
    rows.push({
      kind: "crap",
      label: i18n.t("signals.limits.crap"),
      note: i18n.t("signals.limits.crapNote"),
      value: formatScore(fn.crap),
      bound: i18n.t("signals.limits.limit", { value: formatScore(limits.crap) }),
      over: crossedKind(signals, "crap"),
    });
  }
  if (!signals.untested && fn.changedLines && fn.changedLines.coverable > 0) {
    rows.push({
      kind: "coverage",
      label: i18n.t("signals.limits.coverage"),
      note: i18n.t("signals.limits.coverageNote", {
        covered: fn.changedLines.covered,
        coverable: fn.changedLines.coverable,
      }),
      value: formatPercent(fn.changedLines.covered / fn.changedLines.coverable),
      bound: minimum,
      over: crossedKind(signals, "changed_lines_coverage"),
    });
  }
  return rows;
}

/** One crossed limit in the Diff's marker: its short name and value ("CC", "14"). */
export interface PillPart {
  name: string;
  value: string;
}

function limitValue(limit: CrossedLimit): string {
  return limit.kind === "changed_lines_coverage" ? formatPercent(limit.value) : formatScore(limit.value);
}

/** The Diff's limit marker: only the limits crossed, its accessible name and its tooltip. */
export function limitPill(signals: FunctionSignals): { parts: PillPart[]; label: string; title: string } {
  const short = { complexity: "pillComplexity", crap: "pillCrap", changed_lines_coverage: "pillCoverage" } as const;
  const spoken = { complexity: "valueComplexity", crap: "valueCrap", changed_lines_coverage: "valueCoverage" } as const;
  const told = { complexity: "titleComplexity", crap: "titleCrap", changed_lines_coverage: "titleCoverage" } as const;
  const parts = signals.limits.map((limit) => ({ name: i18n.t(`signals.diff.${short[limit.kind]}`), value: limitValue(limit) }));
  const values = signals.limits.map((limit) => i18n.t(`signals.diff.${spoken[limit.kind]}`, { value: limitValue(limit) }));
  const title = signals.limits
    .map((limit) =>
      i18n.t(`signals.diff.${told[limit.kind]}`, {
        value: limitValue(limit),
        limit: limit.kind === "changed_lines_coverage" ? formatPercent(limit.limit) : formatScore(limit.limit),
      }),
    )
    .join(" ");
  const label = i18n.t("signals.diff.pillLabel", {
    name: signals.fn.name,
    count: signals.limits.length,
    values: values.join(", "),
  });
  return { parts, label, title };
}

/** The draft a static check finding starts: the tool, its rule and its own message, untranslated. */
export function checkDraftBody(check: LocatedCheck): string {
  return check.rule
    ? i18n.t("signals.draft.check", { tool: check.tool, rule: check.rule, message: check.message })
    : i18n.t("signals.draft.checkNoRule", { tool: check.tool, message: check.message });
}

/** "pkg/money.go:40-58", or "pkg/money.go:40" for one line. */
export function rangeText(block: Pick<DuplicateBlock, "file" | "startLine" | "endLine">): string {
  return block.endLine > block.startLine
    ? `${block.file}:${block.startLine}-${block.endLine}`
    : `${block.file}:${block.startLine}`;
}

/** "`a.go:1-9`, `b.go:3-11` and `c.go:5-13`", each place as code. */
function placesText(blocks: readonly DuplicateBlock[]): string {
  return new Intl.ListFormat(intlLocale(), { type: "conjunction" }).format(blocks.map((block) => `\`${rangeText(block)}\``));
}

/** How many times the block is there in the change, itself included, for repeated_in_change. */
export function repetitions(duplicate: Duplicate): number {
  return duplicate.others.length + 1;
}

/** What kind of duplicate it is, with code in backticks: "copies `pkg/money.go:40-58`", "repeated 3× in the change". */
export function duplicateKindText(duplicate: Duplicate): string {
  if (duplicate.kind === "repeated_in_change") {
    return i18n.t("signals.duplicates.repeated", { count: repetitions(duplicate) });
  }
  const place = `\`${rangeText(duplicate.others[0])}\``;
  return duplicate.others.length === 1
    ? i18n.t("signals.duplicates.copies", { place })
    : i18n.t("signals.duplicates.copiesMore", { place, count: duplicate.others.length - 1 });
}

/** "74 tokens, 8 lines": the size of the copy. */
export function duplicateSizeText(duplicate: Duplicate): string {
  return i18n.t("signals.duplicates.size", {
    tokens: formatNumber(duplicate.tokens),
    count: duplicate.lines,
  });
}

/** The draft duplicated code starts: where it already is, and to reuse it rather than keep a copy. */
export function duplicateDraftBody(duplicate: Duplicate): string {
  const places = placesText(duplicate.others);
  return duplicate.kind === "copies_existing"
    ? i18n.t("signals.draft.duplicate", { places })
    : i18n.t("signals.draft.repeated", { places, count: repetitions(duplicate) });
}

/** "Comparison boundary", "Call removed": the family of change a mutant makes, in the language shown. */
export function operatorName(operator: MutationOperator): string {
  return i18n.t(`signals.mutants.operator.${operator}`);
}

/** Code as one line: its runs of whitespace (newlines and indentation included) as one space. */
export function oneLine(code: string): string {
  return code.replace(/\s+/g, " ").trim();
}

/**
 * What a mutant changed, as shown in a sentence: the whole line as it is and as mutated when it spans one line the
 * core could read, otherwise only the piece replaced and what replaced it.
 */
export function mutantLines(mutant: Mutant): { before: string; after: string } {
  if (mutant.before.length === 1 && mutant.after.length === 1) {
    return { before: oneLine(mutant.before[0]), after: oneLine(mutant.after[0]) };
  }
  return { before: oneLine(mutant.original), after: oneLine(mutant.mutated) };
}

/**
 * The sentence of a mutant that survived or ran out of time, code in backticks: "`if balance >= amount {` became
 * `if balance > amount {` and no test failed", "Removing `s.notify(refund)` made no test fail".
 */
export function mutantChangeText(mutant: Mutant): string {
  const timeout = mutant.status === "timeout";
  if (oneLine(mutant.mutated) === "") {
    return i18n.t(timeout ? "signals.mutants.removalTimeout" : "signals.mutants.removal", {
      original: oneLine(mutant.original),
    });
  }
  return i18n.t(timeout ? "signals.mutants.changeTimeout" : "signals.mutants.change", mutantLines(mutant));
}

/** The draft a mutant that survived starts: which change no test notices, and the test that is missing, by operator. */
export function mutantDraftBody(mutant: Mutant): string {
  return i18n.t(`signals.draft.mutant.${mutant.operator}`, {
    original: oneLine(mutant.original),
    mutated: oneLine(mutant.mutated),
  });
}
