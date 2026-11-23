// Which mutants a run tests (mutation.schema.json, rules 1, 4 and 7), as pure
// functions: the candidate lines, positions in Unicode characters, the stable
// ids, the round-robin sample, and the `mutate` ranges StrykerJS is given.

import { createHash } from "node:crypto";

/** Line ranges (inclusive) per file that the --coverage blocks say ran (count > 0). */
export function coveredRanges(blocks) {
  const covered = new Map();
  for (const block of blocks) {
    if (block.count <= 0) continue;
    const list = covered.get(block.file) ?? [];
    list.push([block.startLine, block.endLine]);
    covered.set(block.file, list);
  }
  return covered;
}

/**
 * The candidate lines per file (rule 1): added (`added`: file -> [start, end] ranges of --changed) and covered
 * (`covered`: file -> ranges that ran). A Map from file to a Set of lines; files without any are left out.
 */
export function candidateLines(added, covered) {
  const out = new Map();
  for (const [file, ranges] of added) {
    const ran = covered.get(file);
    if (!ran || ran.length === 0) continue;
    const lines = new Set();
    for (const [start, end] of ranges) {
      for (let line = start; line <= end; line++) {
        if (ran.some(([from, to]) => from <= line && line <= to)) lines.add(line);
      }
    }
    if (lines.size > 0) out.set(file, lines);
  }
  return out;
}

const LF = 10;
const CR = 13;
const LINE_SEPARATOR = 0x2028;
const PARAGRAPH_SEPARATOR = 0x2029;

/** The offsets (UTF-16) where the lines of `text` start, with the line breaks JavaScript knows. */
export function lineStarts(text) {
  const starts = [0];
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    if (c === CR) {
      if (text.charCodeAt(i + 1) === LF) i++;
      starts.push(i + 1);
    } else if (c === LF || c === LINE_SEPARATOR || c === PARAGRAPH_SEPARATOR) {
      starts.push(i + 1);
    }
  }
  return starts;
}

/** The number of Unicode characters (code points) in `text`. */
export function codePoints(text) {
  let n = 0;
  for (const _ of text) n++;
  return n;
}

/** The 1-based column, in code points, of UTF-16 column `column` (0-based) of the line starting at `start`. */
export function codePointColumn(text, start, column) {
  return codePoints(text.slice(start, start + column)) + 1;
}

/** A short form of a piece of source for a description: its first line, at most 60 characters. */
export function excerpt(text) {
  let end = 0;
  while (end < text.length && ![LF, CR, LINE_SEPARATOR, PARAGRAPH_SEPARATOR].includes(text.charCodeAt(end))) end++;
  const first = [...text.slice(0, end)];
  if (first.length > 60) return `${first.slice(0, 57).join("")}...`;
  return end < text.length ? `${first.join("")}...` : first.join("");
}

/**
 * The stable id of a mutant: a hash of its file, function, mutator, original and mutated text, and its position
 * inside its function (lines from the function's first line, and the column).
 */
export function mutantId({ file, functionId, functionLine, mutator, original, mutated, line, column }) {
  const position = `${line - (functionLine ?? 0)}:${column}`;
  return createHash("sha256").update([file, functionId ?? "", mutator, original, mutated, position].join("\0")).digest("hex").slice(0, 20);
}

/** Gives every mutant a unique id: a repeated one (two mutants alike in every way) gets -2, -3... */
export function uniqueIds(mutants) {
  const seen = new Map();
  for (const m of mutants) {
    const n = (seen.get(m.id) ?? 0) + 1;
    seen.set(m.id, n);
    if (n > 1) m.id = `${m.id}-${n}`;
  }
}

function cmp(a, b) {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Rule 7: by file, then line, then column (then the span and the change, for a stable order). */
export function compareMutants(a, b) {
  return (
    cmp(a.file, b.file) ||
    a.line - b.line ||
    a.column - b.column ||
    a.endLine - b.endLine ||
    a.endColumn - b.endColumn ||
    cmp(a.mutator, b.mutator) ||
    cmp(a.mutated, b.mutated)
  );
}

/**
 * Rule 4: when there are more mutants than `max`, those taken round-robin across functions (functions by file
 * then start line, the mutants outside any function as one group per file, placed by their first line; each
 * group's mutants in source order). Returns { selected (in the order of rule 7), sampled }.
 */
export function sample(mutants, max) {
  const ordered = [...mutants].sort(compareMutants);
  if (ordered.length <= max) return { selected: ordered, sampled: false };
  const groups = new Map();
  for (const m of ordered) {
    const key = m.functionId === null ? `file\0${m.file}` : `function\0${m.functionId}`;
    let group = groups.get(key);
    if (!group) {
      group = { file: m.file, line: m.functionId === null ? m.line : (m.functionLine ?? m.line), key, mutants: [] };
      groups.set(key, group);
    }
    group.mutants.push(m);
  }
  const order = [...groups.values()].sort((a, b) => cmp(a.file, b.file) || a.line - b.line || cmp(a.key, b.key));
  const taken = [];
  for (let round = 0; taken.length < max; round++) {
    let any = false;
    for (const group of order) {
      if (round >= group.mutants.length) continue;
      any = true;
      taken.push(group.mutants[round]);
      if (taken.length === max) break;
    }
    if (!any) break;
  }
  return { selected: taken.sort(compareMutants), sampled: true };
}

/**
 * `text` with the characters a glob gives a meaning to put in brackets, so StrykerJS (minimatch) reads a file
 * name as it is. Not with backslashes: StrykerJS turns every backslash of a pattern into "/".
 */
export function escapeGlob(text) {
  return text.replace(/[*?[{}()+@]/g, "[$&]");
}

/**
 * The `mutate` patterns of StrykerJS for the mutants (`relativeTo` gives a file relative to Stryker's working
 * folder; `start` / `end` are StrykerJS positions: line 1-based, column 0-based in UTF-16):
 * `file:line:column-endLine:endColumn`, one per span.
 */
export function mutatePatterns(mutants, relativeTo = (file) => file) {
  const out = new Set();
  for (const m of mutants) {
    out.add(`${escapeGlob(relativeTo(m.file))}:${m.start.line}:${m.start.column}-${m.end.line}:${m.end.column}`);
  }
  return [...out];
}

/** The key matching a mutant of the listing to a result of the run: file, mutator, replacement and span. */
export function matchKey(file, mutator, replacement, start, end) {
  return [file, mutator, replacement, start.line, start.column, end.line, end.column].join("\0");
}
