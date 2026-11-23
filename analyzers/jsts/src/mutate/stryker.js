// One StrykerJS run: the configuration for a unit of the project (a folder
// and its runner, testrun/units.js) and the mutants chosen, and the run
// itself, Stryker's command line in a child process (its own process group,
// stopped with its workers at the deadline), its results read from the
// events of reporter.mjs as they come.
//
// Per runner: vitest, jest and mocha through their StrykerJS plugins with
// "perTest" coverage (each mutant runs the tests that reach it); node --test
// through Stryker's command runner (`node [flags] --test <test files>`, every
// test for every mutant, no coverage, so no killedBy). The plugins find the
// project's runner through preload.mjs, which also stops the run when
// `mutate` is stopped.
//
// Settings: inPlace (the tree is a throwaway copy: the files are instrumented
// where they are, no sandbox copy), ignoreStatic (a mutant that runs only when
// a module loads is not tested), disableTypeChecks for every file, the
// mutators of operators.js EXCLUDED_MUTATORS left out, timeoutFactor 3 and
// timeoutMS --min-mutant-timeout (Stryker stops a mutant after
// 3 x its tests' baseline time + --min-mutant-timeout + its own overhead),
// concurrency --jobs, incremental with its file in --cache-dir, no reporter
// but ours, no dashboard, no log file.

import { writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { run } from "../process.js";
import { EventFile } from "../testrun/collect.js";
import { shellQuote } from "../testrun/commands.js";
import { EXCLUDED_MUTATORS } from "./operators.js";

const HERE = dirname(fileURLToPath(import.meta.url));
export const REPORTER = join(HERE, "reporter.mjs");
export const PRELOAD = join(HERE, "preload.mjs");
/** The folder, inside the unit's folder, where Stryker keeps its backups and temporary files. */
export const TEMP_DIR = ".difflens-stryker";
const POLL = 200;

const PLUGINS = { vitest: "@stryker-mutator/vitest-runner", jest: "@stryker-mutator/jest-runner", mocha: "@stryker-mutator/mocha-runner" };

/** The command node --test runs as for Stryker's command runner, from the unit's folder. */
export function nodeTestCommand(unit) {
  const files = unit.testFiles.map((f) => relative(unit.dir, f));
  return [process.execPath, ...unit.flags, "--test", ...files].map(shellQuote).join(" ");
}

/**
 * The options of a run for `unit` on the `mutate` patterns: `jobs`, `timeoutMs` (--min-mutant-timeout),
 * `incrementalFile` (null without --cache-dir), `dryRunMinutes` (the time left for the baseline).
 */
export function strykerOptions(unit, mutate, { jobs, timeoutMs, incrementalFile, dryRunMinutes }) {
  const options = {
    mutate,
    plugins: [REPORTER],
    reporters: ["difflens-events"],
    concurrency: jobs,
    timeoutMS: timeoutMs,
    timeoutFactor: 3,
    dryRunTimeoutMinutes: dryRunMinutes,
    inPlace: true,
    tempDirName: TEMP_DIR,
    cleanTempDir: true,
    ignoreStatic: true,
    disableTypeChecks: true,
    checkers: [],
    mutator: { excludedMutations: EXCLUDED_MUTATORS },
    thresholds: { high: 80, low: 60, break: null },
    incremental: incrementalFile !== null,
    logLevel: "warn",
    fileLogLevel: "off",
    allowConsoleColors: false,
    warnings: false,
    allowEmpty: true,
  };
  if (incrementalFile !== null) options.incrementalFile = incrementalFile;
  if (unit.runner === "node") {
    // Without coverage, Stryker cannot tell static mutants apart (and refuses ignoreStatic): every mutant runs.
    Object.assign(options, { testRunner: "command", coverageAnalysis: "off", ignoreStatic: false, commandRunner: { command: nodeTestCommand(unit) } });
  } else {
    options.plugins.unshift(PLUGINS[unit.runner]);
    Object.assign(options, { testRunner: unit.runner, coverageAnalysis: "perTest" });
    if (unit.runner === "jest") options.jest = { enableFindRelatedTests: true };
    if (unit.runner === "mocha") options.mochaOptions = { spec: unit.testFiles.map((f) => relative(unit.dir, f)) };
  }
  return options;
}

const ANSI = new RegExp(`${String.fromCharCode(27)}\\[[0-9;]*m`, "g");
const LOG_LINE = /^\d\d:\d\d:\d\d \(\d+\) (TRACE|DEBUG|INFO|WARN|ERROR|FATAL) /;

/**
 * Why a run failed, from its output: StrykerJS's ERROR and FATAL log records (with the lines that continue
 * them), else the end of the output. At most `max` characters.
 */
export function failureOf(output, max = 4096) {
  const lines = output.replace(ANSI, "").split("\n");
  const kept = [];
  let keeping = false;
  for (const line of lines) {
    const record = LOG_LINE.exec(line);
    if (record) {
      keeping = record[1] === "ERROR" || record[1] === "FATAL";
      if (keeping) kept.push(line.replace(LOG_LINE, "").trim());
    } else if (keeping && line.trim() !== "" && !/^\s+at /.test(line)) {
      kept.push(line.trim());
    } else if (line.trim() === "" || /^\s*at /.test(line)) {
      keeping = keeping && line.trim() === "";
    }
  }
  const text = (kept.length > 0 ? kept.join("\n") : lines.filter((l) => l.trim() !== "").slice(-20).join("\n")).trim();
  return text.length > max ? `${text.slice(0, max - 3)}...` : text;
}

/**
 * Runs Stryker for `unit` with `options` (strykerOptions), the configuration and events in `scratch`. Calls
 * `onEvent` with each event of reporter.mjs as it comes; resolves to the child's result (process.js run).
 */
export async function runStryker(engine, unit, options, { scratch, deadline, offline, onEvent }) {
  const config = join(scratch, "stryker.config.json");
  const events = join(scratch, "events.ndjson");
  writeFileSync(config, JSON.stringify(options, null, 2));
  writeFileSync(events, "");
  const env = {
    ...process.env,
    NODE_OPTIONS: `${process.env.NODE_OPTIONS ?? ""} --import=${pathToFileURL(PRELOAD).href}`.trim(),
    DIFFLENS_MUTATE_PID: String(process.pid),
    DIFFLENS_STRYKER_TOOLS: engine.dir,
    DIFFLENS_STRYKER_PROJECT: unit.dir,
    DIFFLENS_STRYKER_EVENTS: events,
    NO_COLOR: "1",
    FORCE_COLOR: "0",
    NO_UPDATE_NOTIFIER: "1",
  };
  if (offline) env.npm_config_offline = "true";
  delete env.NODE_TEST_CONTEXT;
  const file = new EventFile(events);
  const drain = () => {
    for (const event of file.read()) onEvent(event);
  };
  const timer = setInterval(drain, POLL);
  try {
    return await run(process.execPath, [engine.bin, "run", config], { cwd: unit.dir, env, deadline, maxStdout: 1 << 20 });
  } finally {
    clearInterval(timer);
    drain();
  }
}
