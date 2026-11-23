// The dupes command: the duplicated code a review introduces in a JavaScript
// / TypeScript tree (dupes.schema.json, whose description is the algorithm,
// steps 1 to 10). Reads the tree only. This file picks the files (step 1),
// splits them into production and test code (step 2) and runs the search once
// per scope; tokens.js reads the tokens, search.js finds the maximal pairs,
// groups.js turns them into the groups printed.

import { readFileSync, statSync } from "node:fs";
import { join } from "node:path";

import { isGenerated, isTestFile, sourceFiles } from "../files.js";
import { MAX_GROUPS, buildGroups, isNewSide } from "./groups.js";
import { candidateGroups, maximalPairs, touchedPrefix, windowKeys } from "./search.js";
import { tokenize } from "./tokens.js";

/** Files larger than this are left out (step 1). */
export const MAX_FILE_SIZE = 1 << 20;

/** The scopes of step 2, in the order their groups are printed (step 10). */
export const SCOPES = ["production", "test"];

/** The scope of the file at `rel` (step 2): test code as analyze classifies it, production code otherwise. */
export function scopeOf(rel) {
  return isTestFile(rel) ? "test" : "production";
}

const asciiLower = (text) => text.replace(/[A-Z]/g, (c) => c.toLowerCase());

/**
 * Whether the file at `rel` is one the --changed file's `exclude` (`{ dirs, extensions }`, or null) leaves out
 * (step 1): a folder of its path is named one of `dirs`, or its file name ends with one of `extensions`, both
 * ignoring ASCII case. The file name itself is never compared with `dirs`.
 */
export function isExcluded(rel, exclude) {
  if (exclude === null || exclude === undefined) return false;
  const parts = asciiLower(rel).split("/");
  const name = parts.pop();
  const dirs = new Set(exclude.dirs.map(asciiLower));
  if (parts.some((part) => dirs.has(part))) return true;
  return exclude.extensions.some((extension) => name.endsWith(asciiLower(extension)));
}

/** A flag per line (index 1 to `lineCount`), 1 for the lines `ranges` ([start, end], inclusive) add. */
export function addedLineFlags(ranges, lineCount) {
  const flags = new Uint8Array(lineCount + 2);
  for (const [start, end] of ranges) {
    for (let line = Math.max(1, start); line <= Math.min(end, lineCount); line++) flags[line] = 1;
  }
  return flags;
}

/**
 * The files of step 1, tokenized: `{ files, diagnostics, timedOut }`, each file `{ rel, ids, startLines,
 * endLines, splits, addedLines, touched }`. `added` maps a relative path to the line ranges the review adds;
 * the files `exclude` names (see isExcluded) are left out.
 */
export function readFiles(dir, added, { exclude = null, log, deadline = Infinity } = {}) {
  const files = [];
  const diagnostics = [];
  const rels = sourceFiles(dir, { deadline });
  for (const rel of rels) {
    if (Date.now() > deadline) return { files, diagnostics, timedOut: true };
    if (isExcluded(rel, exclude)) {
      log?.debug("file excluded by the changed file, left out", { file: rel });
      continue;
    }
    const abs = join(dir, rel);
    let text;
    try {
      if (statSync(abs).size > MAX_FILE_SIZE) {
        log?.debug("file too large, left out", { file: rel });
        continue;
      }
      text = readFileSync(abs, "utf8");
    } catch (error) {
      diagnostics.push(`${rel}: the file could not be read: ${error.message}`);
      continue;
    }
    if (isGenerated(rel, text.slice(0, 4096))) continue;
    let tokens;
    try {
      tokens = tokenize(text, rel);
    } catch (error) {
      diagnostics.push(`${rel}: the file could not be tokenized: ${error.message}`);
      continue;
    }
    const addedLines = addedLineFlags(added.get(rel) ?? [], tokens.lineCount);
    files.push({ rel, ...tokens, addedLines, touched: touchedPrefix(tokens.startLines, addedLines) });
  }
  // The walk itself stops at the deadline, without a word.
  return { files, diagnostics, timedOut: Date.now() > deadline };
}

/** A maximal pair as groups.js reads it: its two sides' lines, which are new, and its tokens. */
function sidesOf(files, pair) {
  const side = (file, first, length) => ({
    file: file.rel,
    startLine: file.startLines[first],
    endLine: file.endLines[first + length - 1],
  });
  const a = files[pair.a];
  const b = files[pair.b];
  const sides = [side(a, pair.p, pair.length), side(b, pair.q, pair.length)];
  return {
    sides,
    fresh: [isNewSide(sides[0].startLine, sides[0].endLine, a.addedLines), isNewSide(sides[1].startLine, sides[1].endLine, b.addedLines)],
    tokens: pair.length,
  };
}

/**
 * Whether a maximal pair (as sidesOf gives it) is kept: both of its sides span at least `minLines` lines (step 6)
 * and one of them is new (step 7).
 */
export function keepsPair(pair, minLines) {
  return pair.sides.every((s) => s.endLine - s.startLine + 1 >= minLines) && (pair.fresh[0] || pair.fresh[1]);
}

/**
 * Steps 4 to 7 on the files of one scope: the maximal pairs kept, as sidesOf gives them, and whether the search
 * ran out of time.
 */
function searchScope(files, { minTokens, minLines, log, deadline }) {
  if (!files.some((file) => file.touched[file.touched.length - 1] > 0)) return { kept: [], timedOut: false };
  const keys = files.map((file) => windowKeys(file.ids, file.splits, minTokens));
  const groups = candidateGroups(files, keys, minTokens);
  const { pairs, timedOut } = maximalPairs(files, groups, minTokens, deadline);
  log?.debug("pairs found", { files: files.length, candidateGroups: groups.length, pairs: pairs.length });
  const kept = pairs.map((pair) => sidesOf(files, pair)).filter((pair) => keepsPair(pair, minLines));
  return { kept, timedOut };
}

/**
 * The duplicates the change adds to the tree at `dir` (absolute): `{ duplicates, diagnostics }`. `added` maps
 * relative paths to the [start, end] line ranges added; `exclude` is the --changed file's `exclude` (or null).
 * The search runs once per scope, on that scope's files only, production first; each scope reports at most
 * `maxGroups` groups. A search past `deadline` reports what it found: the scopes not searched yet are skipped.
 */
export function findDuplicates(
  dir,
  added,
  { minTokens = 50, minLines = 6, exclude = null, maxGroups = MAX_GROUPS, log, deadline = Infinity } = {},
) {
  const started = Date.now();
  const read = readFiles(dir, added, { exclude, log, deadline });
  const { files, diagnostics } = read;
  let timedOut = read.timedOut;
  const duplicates = [];
  let pairs = 0;
  for (const scope of SCOPES) {
    if (timedOut) break;
    const scoped = files.filter((file) => scopeOf(file.rel) === scope);
    const found = searchScope(scoped, { minTokens, minLines, log, deadline });
    timedOut = found.timedOut;
    pairs += found.kept.length;
    const built = buildGroups(found.kept, maxGroups);
    for (const group of built.duplicates) duplicates.push({ kind: group.kind, scope, ...group });
    if (built.dropped > 0) {
      diagnostics.push(`${built.dropped} more ${scope} duplicate groups were found and left out: at most ${maxGroups} per scope are reported`);
    }
  }
  if (timedOut) {
    diagnostics.push(
      `the search ran out of time after ${((Date.now() - started) / 1000).toFixed(1)}s; the duplicates found until then are reported`,
    );
  }
  log?.info("duplicates found", {
    files: files.length,
    pairs,
    groups: duplicates.length,
    elapsed: `${Date.now() - started}ms`,
  });
  return { duplicates, diagnostics };
}
