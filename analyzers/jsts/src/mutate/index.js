// The mutate command (mutation.schema.json): mutation testing of the lines a
// review adds, with StrykerJS installed by `prepare --tools` (tools.js).
//
//   1. Candidate lines: added (--changed) and covered (--coverage, count > 0),
//      in the project's own source files (not tests, not generated).
//   2. The mutants on them, listed by StrykerJS's own instrumenter in this
//      process (nothing runs), placed and filtered by points.js.
//   3. The cap (rule 4, plan.js sample), deterministic.
//   4. One Stryker run per unit (a folder and its runner, testrun/units.js)
//      holding chosen mutants, with `mutate` ranges of exactly their spans
//      (stryker.js); results come from reporter.mjs as they are known, and
//      only the chosen mutants are reported (a range may hold others).
//   5. With --cache-dir: Stryker's incremental mode, its file
//      <cache-dir>/stryker-incremental.json (one per unit folder), reuses
//      results (`cached`: the mutants Stryker planned as early results), and
//      <cache-dir>/difflens-mutants.json keeps the durations Stryker does not
//      keep for reused results.

import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";
import { pathToFileURL } from "node:url";

import { Tree, isGenerated, isTestFile, sourceFiles } from "../files.js";
import { findUnits } from "../testrun/units.js";
import { TestNames } from "./names.js";
import { EXCLUDED_MUTATORS, statusOf } from "./operators.js";
import { candidateLines, coveredRanges, matchKey, mutatePatterns, sample, uniqueIds } from "./plan.js";
import { filePoints } from "./points.js";
import { failureOf, runStryker, strykerOptions } from "./stryker.js";
import { findEngine } from "./tools.js";

export const ENGINE_NAME = "StrykerJS";
const MAX_FILE_SIZE = 1 << 20;
const DURATIONS_FILE = "difflens-mutants.json";
const MAX_DURATIONS = 20000;

/** A logger for StrykerJS's instrumenter: its warnings go to ours, the rest is dropped. */
function instrumenterLogger(log) {
  const no = () => false;
  const drop = () => {};
  return {
    isTraceEnabled: no,
    isDebugEnabled: no,
    isInfoEnabled: no,
    isWarnEnabled: () => true,
    isErrorEnabled: () => true,
    isFatalEnabled: () => true,
    trace: drop,
    debug: drop,
    info: drop,
    warn: (message) => log.warn(`instrumenter: ${message}`),
    error: (message) => log.warn(`instrumenter: ${message}`),
    fatal: (message) => log.warn(`instrumenter: ${message}`),
  };
}

/** The incremental file of a unit's runs in --cache-dir. */
export function incrementalFileOf(cacheDir, unitRel) {
  if (unitRel === ".") return join(cacheDir, "stryker-incremental.json");
  return join(cacheDir, `stryker-incremental-${createHash("sha256").update(unitRel).digest("hex").slice(0, 12)}.json`);
}

function readDurations(cacheDir) {
  if (!cacheDir) return new Map();
  try {
    const kept = JSON.parse(readFileSync(join(cacheDir, DURATIONS_FILE), "utf8"));
    return new Map(Object.entries(kept.durations ?? {}).filter(([, ms]) => Number.isInteger(ms) && ms >= 0));
  } catch {
    return new Map();
  }
}

function writeDurations(cacheDir, durations) {
  const entries = [...durations].slice(-MAX_DURATIONS);
  const file = join(cacheDir, DURATIONS_FILE);
  const temporary = `${file}.${process.pid}`;
  writeFileSync(temporary, JSON.stringify({ protocol: 1, durations: Object.fromEntries(entries) }));
  renameSync(temporary, file);
}

/** The deepest unit holding the file `abs`, or null. */
function unitOf(units, abs) {
  let best = null;
  for (const unit of units) if (abs.startsWith(`${unit.dir}/`) && (best === null || unit.dir.length > best.dir.length)) best = unit;
  return best;
}

/**
 * Lists the mutants of the candidate lines (steps 1 and 2): { mutants, diagnostics, leftOut }.
 */
async function listMutants(tree, engine, candidates, files, { log }) {
  const diagnostics = [];
  const known = new Set(files);
  const { Instrumenter } = await import(pathToFileURL(engine.instrumenter).href);
  const instrumenter = new Instrumenter(instrumenterLogger(log));
  const mutants = [];
  let leftOut = 0;
  for (const [rel, lines] of [...candidates].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))) {
    if (!known.has(rel) || isTestFile(rel)) continue;
    const abs = tree.abs(rel);
    let text;
    try {
      if (statSync(abs).size > MAX_FILE_SIZE) continue;
      text = readFileSync(abs, "utf8");
    } catch (error) {
      diagnostics.push(`${rel}: the file could not be read: ${error.message}`);
      continue;
    }
    if (isGenerated(rel, text.slice(0, 4096))) continue;
    let raw;
    try {
      const result = await instrumenter.instrument([{ name: abs, mutate: true, content: text }], {
        plugins: null,
        excludedMutations: EXCLUDED_MUTATORS,
        ignorers: [],
      });
      raw = result.mutants.filter((m) => !m.status);
    } catch (error) {
      diagnostics.push(`${rel}: the file could not be parsed for mutants: ${String(error?.message ?? error).split("\n")[0]}`);
      continue;
    }
    const points = filePoints(rel, text, raw, lines);
    mutants.push(...points.mutants);
    leftOut += points.leftOut;
  }
  uniqueIds(mutants);
  return { mutants, diagnostics, leftOut };
}

/** Counts of the progress file and the summary. */
function emptyCounts() {
  return { killed: 0, survived: 0, timeout: 0, error: 0, no_coverage: 0 };
}

/**
 * Runs the mutation testing of the tree at `dir`. `added` maps files to the line ranges --changed adds,
 * `blocks` are the --coverage blocks. Resolves to the fields of the mutation document.
 */
export async function runMutation(dir, { log, deadline, started = Date.now(), added, blocks, cacheDir = "", tools = "", maxMutants = 200, jobs = 1, minMutantTimeoutMs = 10000, progress = null, offline = false }) {
  const engine = findEngine(tools);
  if (engine.missing) throw new Error(engine.missing);
  const tree = new Tree(dir);
  const diagnostics = [];
  const candidates = candidateLines(added, coveredRanges(blocks));
  const files = sourceFiles(tree.root, { deadline });
  const listed = await listMutants(tree, engine, candidates, files, { log });
  diagnostics.push(...listed.diagnostics);
  const { selected, sampled } = sample(listed.mutants, maxMutants);
  log.info("mutants listed", { candidates: listed.mutants.length, selected: selected.length, leftOut: listed.leftOut, files: candidates.size });

  const { units } = findUnits(tree.root, files);
  const byUnit = new Map();
  let noRunner = 0;
  for (const m of selected) {
    const unit = unitOf(units, tree.abs(m.file));
    if (unit === null) {
      noRunner++;
      continue;
    }
    const list = byUnit.get(unit) ?? [];
    list.push(m);
    byUnit.set(unit, list);
  }
  if (noRunner > 0) diagnostics.push(`${noRunner} mutant(s) were not tested: no test runner of the project runs the tests of their files`);

  const counts = emptyCounts();
  const state = { phase: "baseline", total: selected.length - noRunner, done: 0 };
  let lastProgress = "";
  const report = () => {
    if (!progress) return;
    const document = {
      phase: state.phase,
      total: state.total,
      done: state.done,
      killed: counts.killed,
      survived: counts.survived,
      timeout: counts.timeout,
      error: counts.error + counts.no_coverage,
      current: null,
    };
    const text = JSON.stringify(document);
    if (text === lastProgress) return;
    lastProgress = text;
    progress(document);
  };
  report();

  const durations = readDurations(cacheDir);
  if (cacheDir) mkdirSync(cacheDir, { recursive: true });
  const results = new Map();
  let ignored = 0;
  let untested = 0;
  const record = (m, result) => {
    results.set(m.id, result);
    state.done++;
    counts[result.status]++;
    if (!result.cached) durations.set(m.id, result.durationMs);
    report();
  };

  const ordered = [...byUnit].sort(([a], [b]) => (a.rel < b.rel ? -1 : a.rel > b.rel ? 1 : 0));
  for (const [unit, mutants] of ordered) {
    if (Date.now() > deadline - 1000) {
      untested += mutants.length;
      continue;
    }
    state.phase = "baseline";
    report();
    const byKey = new Map(mutants.map((m) => [matchKey(m.file, m.mutator, m.mutated, m.start, m.end), m]));
    const names = new TestNames(tree);
    const reused = new Set();
    let baselineDone = false;
    const onEvent = (event) => {
      if (event.e === "tests") {
        baselineDone = true;
        names.add(event.tests ?? []);
      } else if (event.e === "plan") {
        baselineDone = true;
        state.phase = "mutating";
        for (const p of event.plans ?? []) if (p.plan === "EarlyResult" && p.status && p.status !== "Ignored") reused.add(p.id);
        report();
      } else if (event.e === "mutant") {
        const rel = tree.rel(event.file);
        if (rel === null || !event.start || !event.end) return;
        const start = { line: event.start.line, column: event.start.column - 1 };
        const end = { line: event.end.line, column: event.end.column - 1 };
        const m = byKey.get(matchKey(rel, event.mutator, event.replacement ?? "", start, end));
        if (!m || results.has(m.id)) return;
        const status = statusOf(event.status);
        if (status === null) {
          ignored++;
          results.set(m.id, null);
          state.done++;
          report();
          return;
        }
        const cached = reused.has(event.id);
        const killer = status === "killed" && event.killedBy?.length > 0 ? names.nameOf(event.killedBy[0]) : null;
        const durationMs = cached ? (durations.get(m.id) ?? 0) : Math.max(0, Math.round(event.duration ?? 0));
        record(m, { status, killedBy: killer, durationMs, cached });
      }
    };
    const scratch = mkdtempSync(join(tmpdir(), "difflens-mutate-"));
    try {
      const patterns = mutatePatterns(mutants, (file) => relative(unit.dir, tree.abs(file)));
      const options = strykerOptions(unit, patterns, {
        jobs,
        timeoutMs: minMutantTimeoutMs,
        incrementalFile: cacheDir ? incrementalFileOf(cacheDir, unit.rel) : null,
        dryRunMinutes: Math.max(1, Math.ceil((deadline - Date.now()) / 60000)),
      });
      const unitStarted = Date.now();
      const result = await runStryker(engine, unit, options, { scratch, deadline, offline, onEvent });
      const missing = mutants.filter((m) => !results.has(m.id));
      log.info("stryker run", {
        dir: unit.rel,
        runner: unit.runner,
        exit: result.code,
        signal: result.signal,
        elapsed: `${Date.now() - unitStarted}ms`,
        mutants: mutants.length,
        missing: missing.length,
      });
      const output = failureOf(`${result.stdout}\n${result.stderr}`);
      if (result.timedOut) {
        untested += missing.length;
      } else if (result.error || result.code !== 0) {
        const why = result.error ? `it could not run: ${result.error.message}` : output || `it exited with ${result.code}`;
        if (!baselineDone) {
          diagnostics.push(`the tests of ${unit.rel} (${unit.runner}) fail or cannot run on the unmutated code, so ${missing.length} mutant(s) could not be tested: ${why}`);
        } else {
          diagnostics.push(`StrykerJS failed in ${unit.rel} (${unit.runner}), ${missing.length} mutant(s) could not be tested: ${why}`);
        }
        for (const m of missing) record(m, { status: "error", killedBy: null, durationMs: 0, cached: false });
      } else if (missing.length > 0) {
        diagnostics.push(`${missing.length} mutant(s) of ${unit.rel} were listed but StrykerJS did not test them`);
      }
    } finally {
      rmSync(scratch, { recursive: true, force: true });
    }
  }
  if (untested > 0) diagnostics.push(`the time ran out: ${untested} mutant(s) were not tested`);
  if (ignored > 0) diagnostics.push(`${ignored} mutant(s) were not tested: StrykerJS ignores mutants of code that runs only when a module loads (static mutants)`);
  if (cacheDir && existsSync(cacheDir)) {
    try {
      writeDurations(cacheDir, durations);
    } catch (error) {
      log.warn("mutant durations not kept", { error: error.message });
    }
  }

  // `selected` is in the order of rule 7 already.
  const mutants = [];
  for (const m of selected) {
    const result = results.get(m.id);
    if (!result) continue;
    mutants.push({
      id: m.id,
      file: m.file,
      line: m.line,
      column: m.column,
      endLine: m.endLine,
      endColumn: m.endColumn,
      original: m.original,
      mutated: m.mutated,
      operator: m.operator,
      description: m.description,
      function: m.functionId,
      status: result.status,
      killedBy: result.killedBy,
      durationMs: result.durationMs,
      cached: result.cached,
    });
  }
  const tally = (status) => mutants.filter((m) => m.status === status).length;
  const summary = {
    candidates: listed.mutants.length,
    sampled,
    maxMutants,
    killed: tally("killed"),
    survived: tally("survived"),
    timeout: tally("timeout"),
    error: tally("error"),
    noCoverage: tally("no_coverage"),
    cached: mutants.filter((m) => m.cached).length,
    durationMs: Date.now() - started,
  };
  return { engine: { name: ENGINE_NAME, version: engine.version }, mutants, summary, diagnostics };
}
