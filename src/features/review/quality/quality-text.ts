// Wording of the Quality tab, from the report's numbers, in the language shown: the limits a function crosses, the
// review comment and the coding-agent prompt written from them (never by a model), and the checks' statuses.

import { i18n } from "@/i18n";
import type { CrossedLimit, QualityFunction, QualityReport, StaticCheck } from "@/lib/api";
import { formatPercent, intlLocale } from "@/lib/format";

/**
 * Complexity and CRAP as whole numbers when they are, else with one decimal, so 30.4 never reads as 30. The
 * decimal separator is the language's ("30,4" in Portuguese); no grouping, as a score is not a count.
 */
export function formatScore(value: number): string {
  const decimals = Number.isInteger(value) ? 0 : 1;
  return new Intl.NumberFormat(intlLocale(), {
    minimumFractionDigits: decimals,
    maximumFractionDigits: decimals,
    useGrouping: false,
  }).format(value);
}

/** A changed function over at least one limit, with the limits it crosses in the report's order. */
export interface OverLimit {
  fn: QualityFunction;
  crossings: CrossedLimit[];
}

/**
 * The report's functions split by whether they cross a limit, each part in the report's order (complexity first).
 * The limits are the core's: a function crosses one when `crossed` names it.
 */
export function attention(report: QualityReport): { over: OverLimit[]; within: QualityFunction[] } {
  const over: OverLimit[] = [];
  const within: QualityFunction[] = [];
  for (const fn of report.functions) {
    const crossings = report.crossed.filter((limit) => limit.function?.id === fn.id);
    if (crossings.length > 0) over.push({ fn, crossings });
    else within.push(fn);
  }
  return { over, within };
}

/** The coverage of the whole review's changed lines under its target, when it is. */
export function reviewCoverageCrossing(report: QualityReport): CrossedLimit | null {
  return report.crossed.find((limit) => limit.function === null) ?? null;
}

function values(limit: CrossedLimit): { value: string; limit: string } {
  return limit.kind === "changed_lines_coverage"
    ? { value: formatPercent(limit.value), limit: formatPercent(limit.limit) }
    : { value: formatScore(limit.value), limit: formatScore(limit.limit) };
}

const CROSSING_KEY = { complexity: "complexity", crap: "crap", changed_lines_coverage: "coverage" } as const;

/** "complexity 12 (limit 10)", "48% of the changed lines covered (target 80%)". */
export function crossingText(limit: CrossedLimit): string {
  return i18n.t(`quality.functions.crossing.${CROSSING_KEY[limit.kind]}`, values(limit));
}

export function reviewCoverageText(limit: CrossedLimit): string {
  const { value, limit: target } = values(limit);
  return i18n.t("quality.functions.reviewCoverage", { value, target });
}

/** The key the draft written for a function over a limit is linked to (`signals::QUALITY_SIGNAL` in the core). */
export function functionSignal(fn: QualityFunction): string {
  return `quality:${fn.file}:${fn.id}`;
}

function list(items: readonly string[]): string {
  return new Intl.ListFormat(intlLocale(), { style: "long", type: "conjunction" }).format(items);
}

/**
 * The review comment a function over a limit starts with, short and the same for the same numbers: its metrics
 * against their limits, then a direction. Complexity alone suggests splitting it, uncovered lines alone covering
 * them; CRAP, which grows with both, suggests both.
 */
export function functionDraftBody({ fn, crossings }: OverLimit): string {
  const metrics = crossings.map((limit) => i18n.t(`quality.draft.metric.${CROSSING_KEY[limit.kind]}`, values(limit)));
  const kinds = new Set(crossings.map((limit) => limit.kind));
  const split = kinds.has("complexity") || kinds.has("crap");
  const cover = kinds.has("changed_lines_coverage") || kinds.has("crap");
  const direction = split && cover ? "splitAndCover" : split ? "split" : "cover";
  return i18n.t("quality.draft.body", {
    name: fn.name,
    metrics: list(metrics),
    direction: i18n.t(`quality.draft.${direction}`),
  });
}

function range(fn: QualityFunction): string {
  return `${fn.file}:${fn.startLine}-${fn.endLine}`;
}

/** The metrics and code of one function, as a prompt has them. */
function functionSection({ fn, crossings }: OverLimit): string {
  const parts: string[] = [];
  parts.push(`${i18n.t("quality.prompt.metrics")}\n${crossings.map((limit) => `- ${crossingText(limit)}`).join("\n")}`);
  if (fn.code.length === 0) {
    parts.push(i18n.t("quality.prompt.codeUnavailable", { where: range(fn) }));
  } else {
    const last = fn.startLine + fn.code.length - 1;
    const width = String(last).length;
    const code = fn.code.map((text, index) => `${String(fn.startLine + index).padStart(width)}  ${text}`).join("\n");
    const cut = last < fn.endLine ? `\n${i18n.t("quality.prompt.codeCut", { count: fn.code.length })}` : "";
    parts.push(`${i18n.t("quality.prompt.code", { where: range(fn) })}\n\`\`\`\n${code}\n\`\`\`${cut}`);
  }
  return parts.join("\n\n");
}

const PLACE = (fn: QualityFunction) => ({ name: fn.name, file: fn.file, start: fn.startLine, end: fn.endLine });

/**
 * A prompt for a coding agent to fix the functions over a limit: each one's file and lines, the limits it crosses
 * and its code, then what to keep and what to change. The instructions follow the language shown; the code goes in
 * as it is.
 */
export function functionsPrompt(items: readonly OverLimit[]): string {
  if (items.length === 1) {
    const [item] = items;
    return [i18n.t("quality.prompt.fixOne", PLACE(item.fn)), functionSection(item), i18n.t("quality.prompt.closing")].join(
      "\n\n",
    );
  }
  const sections = items.map(
    (item, index) => `${index + 1}. ${i18n.t("quality.prompt.function", PLACE(item.fn))}\n\n${functionSection(item)}`,
  );
  return [i18n.t("quality.prompt.fixMany", { count: items.length }), ...sections, i18n.t("quality.prompt.closing")].join(
    "\n\n",
  );
}

export function limitsText({ limits }: QualityReport): string {
  return i18n.t("quality.limits.summary", {
    complexity: formatScore(limits.complexity),
    crap: formatScore(limits.crap),
    coverage: formatPercent(limits.changedLinesCoverage),
  });
}

/** What the Checks view counts: every static check finding, and races when the race detector found some. */
export function checksCount(report: QualityReport): number {
  const findings = report.checks.reduce((total, check) => total + check.findings.length, 0);
  return findings + (report.raceDetector === "races" ? 1 : 0);
}

export type Tone = "ok" | "bad" | "faint";

export function checkStatus(check: StaticCheck): { text: string; tone: Tone } {
  switch (check.status) {
    case "clean":
      return { text: i18n.t("quality.checks.clean"), tone: "ok" };
    case "findings":
      return { text: i18n.t("quality.checks.findings", { count: check.findings.length }), tone: "bad" };
    case "unavailable":
      return { text: i18n.t("quality.checks.notInstalled"), tone: "faint" };
    case "failed":
      return { text: i18n.t("quality.checks.failed"), tone: "bad" };
  }
}

export function raceStatus(report: QualityReport): { text: string; tone: Tone; note: string | null } {
  switch (report.raceDetector) {
    case "clean":
      return { text: i18n.t("quality.checks.noRaces"), tone: "ok", note: null };
    case "races":
      return { text: i18n.t("quality.checks.racesFound"), tone: "bad", note: null };
    case "not_run":
      return {
        text: i18n.t("quality.checks.notRun"),
        tone: "faint",
        note: report.testRunAt === null ? i18n.t("quality.checks.needsRun") : i18n.t("quality.checks.wasOff"),
      };
  }
}
