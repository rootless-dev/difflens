// Where the signals of one file go in its diff, all on the new side: the blocks under a line (a model finding under
// the last line it cites, a static check under its line, duplicated code under the last line of its new block, a
// mutant that survived under the last line it changes), the marker at the end of the first line of a function over a
// limit or that no test runs, and the marks of the signal lane. Hidden kinds are left out; duplicated code and
// surviving mutants are shown and hidden with the static checks, as all three are measured.

import type { LaneMark } from "@/features/review/diff-annotations";
import {
  type FileSignals,
  findingEndLine,
  findingLine,
  type FunctionSignals,
} from "@/features/review/signals/signals";
import type { Duplicate, Finding, LocatedCheck, Mutant } from "@/lib/api";

/** A finding's bar runs along this many lines at most. */
const MAX_RANGE = 400;

export interface LineSignals {
  findings: Finding[];
  dismissed: Finding[];
  checks: LocatedCheck[];
  duplicates: Duplicate[];
  mutants: Mutant[];
}

export interface FileSignalLayout {
  /** The blocks under each line of the new side. */
  under: ReadonlyMap<number, LineSignals>;
  /** The functions whose first line carries the limit marker or the note that no test runs them. */
  starts: ReadonlyMap<number, FunctionSignals>;
  lane: LaneMark[];
}

export interface ShownSignals {
  findings: boolean;
  checks: boolean;
}

const EMPTY_LAYOUT: FileSignalLayout = { under: new Map(), starts: new Map(), lane: [] };

export function fileSignalLayout(file: FileSignals | undefined, shown: ShownSignals): FileSignalLayout {
  if (!file) return EMPTY_LAYOUT;
  const under = new Map<number, LineSignals>();
  const at = (line: number) => {
    let found = under.get(line);
    if (!found) {
      found = { findings: [], dismissed: [], checks: [], duplicates: [], mutants: [] };
      under.set(line, found);
    }
    return found;
  };
  const lane = new Map<number, LaneMark>();
  const mark = (line: number) => {
    let found = lane.get(line);
    if (!found) {
      found = { line };
      lane.set(line, found);
    }
    return found;
  };

  const starts = new Map<number, FunctionSignals>();
  for (const fn of file.functions) {
    if (fn.limits.length === 0 && !fn.untested) continue;
    starts.set(fn.fn.startLine, fn);
    const start = mark(fn.fn.startLine);
    if (fn.limits.length > 0) start.limit = true;
    if (fn.untested) start.untested = true;
  }
  if (shown.findings) {
    for (const finding of file.findings) {
      const start = findingLine(finding);
      const end = Math.min(Math.max(findingEndLine(finding), start), start + MAX_RANGE);
      at(end).findings.push(finding);
      for (let line = start; line <= end; line++) {
        const range = start === end ? "single" : line === start ? "start" : line === end ? "end" : "middle";
        const lineMark = mark(line);
        lineMark.range = lineMark.range === undefined || lineMark.range === range ? range : "middle";
      }
    }
    for (const finding of file.dismissed) at(Math.max(findingEndLine(finding), findingLine(finding))).dismissed.push(finding);
  }
  if (shown.checks) {
    for (const check of file.checks) {
      at(check.line).checks.push(check);
      mark(check.line).check = true;
    }
    for (const duplicate of file.duplicates) {
      at(Math.max(duplicate.block.endLine, duplicate.block.startLine)).duplicates.push(duplicate);
      mark(duplicate.block.startLine).duplicate = true;
    }
    for (const mutant of file.mutants) {
      at(Math.max(mutant.endLine, mutant.line)).mutants.push(mutant);
      mark(mutant.line).mutant = true;
    }
  }
  return { under, starts, lane: [...lane.values()].sort((a, b) => a.line - b.line) };
}
