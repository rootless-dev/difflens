// What the review's signals say about each changed function and file, worked out once for the Overview, the Flow and
// the Diff: the limits a function crosses, whether no test executes it, the static check findings on its added lines
// and the duplicated code it introduces (both placed by the core), the mutants of its lines that survived the last
// mutation run (code the tests run without verifying it), and the model's findings inside it. Dismissed findings count
// nowhere; fixed ones (marked so by a later analysis or by the user) are not signals at all, and only the Insights tab
// shows them.

import { createContext } from "react";

import type {
  CheckSeverity,
  CrossedLimit,
  Duplicate,
  Finding,
  LocatedCheck,
  Mutant,
  MutationRun,
  QualityFunction,
  QualityLimits,
  QualityReport,
  Severity,
} from "@/lib/api";

/** Most severe first. */
export const SEVERITY_RANK: Record<Severity, number> = { high: 0, medium: 1, low: 2 };

/** A static check finding's severity, most serious first. */
export const CHECK_SEVERITY_RANK: Record<CheckSeverity, number> = { error: 0, warning: 1, info: 2 };

/** The order limits are listed in, everywhere. */
const LIMIT_ORDER: Record<CrossedLimit["kind"], number> = { complexity: 0, crap: 1, changed_lines_coverage: 2 };

export interface FunctionSignals {
  fn: QualityFunction;
  /**
   * The limits it crosses: complexity, CRAP, then the coverage of its changed lines. The coverage is left out when no
   * test executes the function: `untested` stands for it, so the function does not count twice.
   */
  limits: CrossedLimit[];
  untested: boolean;
  /** On its added lines, by line. */
  checks: LocatedCheck[];
  /** Duplicated code whose new block it holds, by line. */
  duplicates: Duplicate[];
  /** Mutants of its lines that survived, by line. */
  mutants: Mutant[];
  /** Not dismissed nor fixed, found inside it, most severe first. */
  findings: Finding[];
}

export interface FileSignals {
  path: string;
  /** Not dismissed nor fixed and found in the file, most severe first, then by line. */
  findings: Finding[];
  /** Dismissed, not fixed, and found in the file, by line: the Diff shows them folded. */
  dismissed: Finding[];
  /** On its added lines, by line, in a function or not. */
  checks: LocatedCheck[];
  /** Duplicated code whose new block is in the file, by line, in a function or not. */
  duplicates: Duplicate[];
  /** Mutants of its lines that survived, by line, in a function or not. */
  mutants: Mutant[];
  /** Its functions that cross a limit or that no test executes, by start line. */
  functions: FunctionSignals[];
}

export interface ReviewSignals {
  limits: QualityLimits | null;
  /** Only the functions with some signal, by id. */
  functions: ReadonlyMap<string, FunctionSignals>;
  /** Only the files with some signal, by path. */
  files: ReadonlyMap<string, FileSignals>;
  /** Every finding not dismissed and not fixed, most severe first, found or not. */
  findings: Finding[];
  checks: LocatedCheck[];
  /** Every duplicate of the report, by file and line. */
  duplicates: Duplicate[];
  /** Every mutant of the last mutation run that survived, by file and line. */
  mutants: Mutant[];
  /** The last completed mutation run, the survivors' source; null without one. */
  mutation: MutationRun | null;
}

export const NO_SIGNALS: ReviewSignals = {
  limits: null,
  functions: new Map(),
  files: new Map(),
  findings: [],
  checks: [],
  duplicates: [],
  mutants: [],
  mutation: null,
};

/** The line a finding starts at now, or as the model cited it when it was not found again. */
export function findingLine(finding: Finding): number {
  return finding.location.kind === "found" ? finding.location.line : finding.line;
}

export function findingEndLine(finding: Finding): number {
  return finding.location.kind === "found" ? finding.location.endLine : finding.endLine;
}

const bySeverity = (a: Finding, b: Finding) =>
  SEVERITY_RANK[a.severity] - SEVERITY_RANK[b.severity] || findingLine(a) - findingLine(b);

/** The innermost of `functions` in `path` holding `line`. */
function holding(functions: readonly QualityFunction[], path: string, line: number): QualityFunction | null {
  let best: QualityFunction | null = null;
  for (const fn of functions) {
    if (fn.file !== path || line < fn.startLine || line > fn.endLine) continue;
    if (best === null || fn.endLine - fn.startLine < best.endLine - best.startLine) best = fn;
  }
  return best;
}

/**
 * Every signal of the review, from the quality report (limits, untested, checks, duplicates), the last completed
 * mutation run (the mutants that survived) and the model's findings.
 */
export function reviewSignals(
  report: QualityReport | null,
  findings: readonly Finding[] | null,
  mutation: MutationRun | null = null,
): ReviewSignals {
  const functions = report?.functions ?? [];
  const byId = new Map<string, FunctionSignals>();
  const entry = (fn: QualityFunction): FunctionSignals => {
    let found = byId.get(fn.id);
    if (!found) {
      found = { fn, limits: [], untested: fn.untested, checks: [], duplicates: [], mutants: [], findings: [] };
      byId.set(fn.id, found);
    }
    return found;
  };
  for (const fn of functions) {
    const limits = (report?.crossed ?? [])
      .filter((limit) => limit.function?.id === fn.id)
      .filter((limit) => !(fn.untested && limit.kind === "changed_lines_coverage"))
      .sort((a, b) => LIMIT_ORDER[a.kind] - LIMIT_ORDER[b.kind]);
    if (limits.length > 0 || fn.untested) entry(fn).limits = limits;
  }
  const checks = report?.checkFindings ?? [];
  const fnById = new Map(functions.map((fn) => [fn.id, fn]));
  for (const check of checks) {
    const fn = check.function ? fnById.get(check.function.id) : undefined;
    if (fn) entry(fn).checks.push(check);
  }
  const duplicates = report?.duplicates ?? [];
  for (const duplicate of duplicates) {
    const fn = duplicate.function ? fnById.get(duplicate.function.id) : undefined;
    if (fn) entry(fn).duplicates.push(duplicate);
  }
  // Only a completed run says which mutants survived; the core keeps no other.
  const mutants = mutation?.status === "completed" ? mutation.mutants.filter((mutant) => mutant.status === "survived") : [];
  for (const mutant of mutants) {
    const fn = mutant.function ? fnById.get(mutant.function.id) : undefined;
    if (fn) entry(fn).mutants.push(mutant);
  }
  const open =(findings ?? []).filter((finding) => finding.resolution === null);
  const shown = open.filter((finding) => finding.verdict !== "dismissed").sort(bySeverity);
  for (const finding of shown) {
    if (finding.location.kind !== "found") continue;
    const fn = holding(functions, finding.path, finding.location.line);
    if (fn) entry(fn).findings.push(finding);
  }

  const files = new Map<string, FileSignals>();
  const file = (path: string): FileSignals => {
    let found = files.get(path);
    if (!found) {
      found = { path, findings: [], dismissed: [], checks: [], duplicates: [], mutants: [], functions: [] };
      files.set(path, found);
    }
    return found;
  };
  for (const finding of open) {
    if (finding.location.kind !== "found") continue;
    const into = file(finding.path);
    if (finding.verdict === "dismissed") into.dismissed.push(finding);
    else into.findings.push(finding);
  }
  for (const check of checks) file(check.file).checks.push(check);
  for (const duplicate of duplicates) file(duplicate.block.file).duplicates.push(duplicate);
  for (const mutant of mutants) file(mutant.file).mutants.push(mutant);
  for (const signals of byId.values()) {
    if (signals.limits.length > 0 || signals.untested) file(signals.fn.file).functions.push(signals);
  }
  for (const into of files.values()) {
    into.findings.sort(bySeverity);
    into.dismissed.sort((a, b) => findingLine(a) - findingLine(b));
    into.functions.sort((a, b) => a.fn.startLine - b.fn.startLine);
  }
  return {
    limits: report?.limits ?? null,
    functions: byId,
    files,
    findings: shown,
    checks,
    duplicates,
    mutants,
    mutation: mutation?.status === "completed" ? mutation : null,
  };
}

/** What a box of the Flow (or a file header) shows: up to six parts, in this order, each only when there is one. */
export interface Badge {
  /** The most severe finding's severity, null without findings. */
  severity: Severity | null;
  findings: number;
  limits: number;
  checks: number;
  /** Duplicated code introduced. */
  duplicates: number;
  /** Mutants that survived. */
  mutants: number;
  /** Functions no test executes. */
  untested: number;
}

const EMPTY: Badge = { severity: null, findings: 0, limits: 0, checks: 0, duplicates: 0, mutants: 0, untested: 0 };

const isEmpty = (badge: Badge) =>
  badge.findings === 0 &&
  badge.limits === 0 &&
  badge.checks === 0 &&
  badge.duplicates === 0 &&
  badge.mutants === 0 &&
  badge.untested === 0;

function mostSevere(a: Severity | null, b: Severity | null): Severity | null {
  if (a === null) return b;
  if (b === null) return a;
  return SEVERITY_RANK[a] <= SEVERITY_RANK[b] ? a : b;
}

/** The badge of a function's box, or null when it has no signal. */
export function functionBadge(signals: FunctionSignals | undefined): Badge | null {
  if (!signals) return null;
  const badge: Badge = {
    severity: signals.findings[0]?.severity ?? null,
    findings: signals.findings.length,
    limits: signals.limits.length,
    checks: signals.checks.length,
    duplicates: signals.duplicates.length,
    mutants: signals.mutants.length,
    untested: signals.untested ? 1 : 0,
  };
  return isEmpty(badge) ? null : badge;
}

/**
 * The badge of a closed file or package: everything in its files, those outside any function included; the most
 * severe finding, and the sum of each part.
 */
export function filesBadge(files: readonly (FileSignals | undefined)[]): Badge | null {
  const badge = { ...EMPTY };
  for (const file of files) {
    if (!file) continue;
    badge.severity = mostSevere(badge.severity, file.findings[0]?.severity ?? null);
    badge.findings += file.findings.length;
    badge.checks += file.checks.length;
    badge.duplicates += file.duplicates.length;
    badge.mutants += file.mutants.length;
    for (const fn of file.functions) {
      badge.limits += fn.limits.length;
      if (fn.untested) badge.untested += 1;
    }
  }
  return isEmpty(badge) ? null : badge;
}

/** The key a draft written from a signal is linked to. */
export function findingSignal(finding: Finding): string {
  return `finding:${finding.id}`;
}

export function checkSignal(check: LocatedCheck): string {
  return `check:${check.tool}:${check.rule ?? ""}:${check.file}:${check.line}:${check.message}`;
}

export function duplicateSignal(duplicate: Duplicate): string {
  const { block } = duplicate;
  return `duplicate:${duplicate.kind}:${block.file}:${block.startLine}:${block.endLine}`;
}

export function mutantSignal(mutant: Mutant): string {
  return `mutant:${mutant.id}`;
}

/** What the Flow's boxes and popovers read besides the graph: the signals, and the tabs their sections link to. */
export interface SignalsHandle {
  signals: ReviewSignals;
  onOpenQuality: () => void;
  onOpenInsights: () => void;
  onOpenTests: () => void;
}

export const SignalsContext = createContext<SignalsHandle>({
  signals: NO_SIGNALS,
  onOpenQuality: () => {},
  onOpenInsights: () => {},
  onOpenTests: () => {},
});
