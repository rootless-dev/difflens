// The command line every DiffLens analyzer implements (see README.md):
//
//   difflens-analyzer-jsts analyze --dir <source tree> [--timeout 120s]
//   difflens-analyzer-jsts prepare --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--tools <folder>]
//   difflens-analyzer-jsts test --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--offline]
//       [--no-race] [--no-cover] [--selection <file>] [--progress <file>]
//   difflens-analyzer-jsts check --dir <source tree> [--timeout 5m] [--cache-dir <folder>]
//   difflens-analyzer-jsts dupes --dir <source tree> --changed <file> [--timeout 120s]
//       [--min-tokens 50] [--min-lines 6]
//   difflens-analyzer-jsts mutate --dir <source tree> --changed <file> --coverage <file> [--timeout 10m]
//       [--cache-dir <folder>] [--tools <folder>] [--max-mutants 200] [--jobs <n>]
//       [--min-mutant-timeout 10s] [--progress <file>] [--offline]
//   difflens-analyzer-jsts --version
//
// The commands themselves live in analyze/, check.js, prepare.js, testrun/,
// dupes/ and mutate/, loaded on demand; this file parses the command line,
// keeps the exit codes and prints the single document on stdout.
//
// --cache-dir is the core's folder for build output worth keeping between
// runs (tests.schema.json). prepare, test and check accept it and keep
// nothing there: every cache of this ecosystem worth having is keyed by
// absolute paths (Jest's by a hash of its rootDir, tsc's build info, ESLint's
// cache), and each run works on a copy at another path, so none would ever
// be reused. mutate keeps StrykerJS's incremental file there (mutate/).

import { readFileSync, renameSync, statSync, writeFileSync } from "node:fs";
import { availableParallelism } from "node:os";
import { dirname, join, resolve } from "node:path";
import { parseArgs } from "node:util";

import { createLogger } from "./log.js";
import * as protocol from "./protocol.js";

export const EXIT_OK = 0;
/** The command itself failed; stderr says why. */
export const EXIT_FAILED = 1;
/** Bad command line, or a selection or changed file that cannot be read. */
export const EXIT_USAGE = 2;

const { NAME, VERSION } = protocol;

const USAGE = `usage: ${NAME} analyze --dir <source tree> [--timeout 120s]
       ${NAME} prepare --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--tools <folder>]
       ${NAME} test --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--offline] [--no-race]
           [--no-cover] [--selection <file>] [--progress <file>]
       ${NAME} check --dir <source tree> [--timeout 5m] [--cache-dir <folder>]
       ${NAME} dupes --dir <source tree> --changed <file> [--timeout 120s]
           [--min-tokens 50] [--min-lines 6]
       ${NAME} mutate --dir <source tree> --changed <file> --coverage <file> [--timeout 10m]
           [--cache-dir <folder>] [--tools <folder>] [--max-mutants 200] [--jobs <n>]
           [--min-mutant-timeout 10s] [--progress <file>] [--offline]
       ${NAME} --version
`;

const DEFAULT_TIMEOUTS = { analyze: "120s", prepare: "10m", test: "10m", check: "5m", dupes: "120s", mutate: "10m" };

/** --jobs when not given: half the CPUs, at least 1. */
export function defaultJobs() {
  return Math.max(1, Math.floor(availableParallelism() / 2));
}

class UsageError extends Error {}

const UNIT_SECONDS = { ns: 1e-9, us: 1e-6, µs: 1e-6, ms: 1e-3, s: 1, m: 60, h: 3600 };

/** Seconds in a Go duration such as "120s" or "1m30s", the form --timeout takes. */
export function parseDuration(text) {
  const part = /(\d+(?:\.\d*)?|\.\d+)(ns|us|µs|ms|s|m|h)/y;
  let seconds = 0;
  part.lastIndex = 0;
  while (part.lastIndex < text.length) {
    const match = part.exec(text);
    if (match === null) throw new UsageError(`invalid duration ${JSON.stringify(text)}`);
    seconds += Number(match[1]) * UNIT_SECONDS[match[2]];
  }
  if (text === "") throw new UsageError('invalid duration ""');
  return seconds;
}

/** The options of `command` from its arguments; a UsageError for a bad command line. */
export function parseOptions(command, args) {
  const options = { dir: { type: "string", default: "" }, timeout: { type: "string", default: DEFAULT_TIMEOUTS[command] } };
  if (["prepare", "test", "check", "mutate"].includes(command)) options["cache-dir"] = { type: "string", default: "" };
  if (["prepare", "mutate"].includes(command)) options.tools = { type: "string", default: "" };
  if (command === "mutate") {
    Object.assign(options, {
      changed: { type: "string", default: "" },
      coverage: { type: "string", default: "" },
      "max-mutants": { type: "string", default: "200" },
      jobs: { type: "string", default: "" },
      "min-mutant-timeout": { type: "string", default: "10s" },
      progress: { type: "string", default: "" },
      offline: { type: "boolean", default: false },
    });
  }
  if (command === "test") {
    Object.assign(options, {
      offline: { type: "boolean", default: false },
      "no-race": { type: "boolean", default: false },
      "no-cover": { type: "boolean", default: false },
      selection: { type: "string", default: "" },
      progress: { type: "string", default: "" },
    });
  }
  if (command === "dupes") {
    Object.assign(options, {
      changed: { type: "string", default: "" },
      "min-tokens": { type: "string", default: "50" },
      "min-lines": { type: "string", default: "6" },
    });
  }
  let values;
  try {
    ({ values } = parseArgs({ args, options, strict: true, allowPositionals: false }));
  } catch (error) {
    throw new UsageError(`${command}: ${error.message}`);
  }
  if (values.dir === "" || parseDuration(values.timeout) <= 0) {
    throw new UsageError(
      `${command}: --dir is required, --timeout must be positive and no positional arguments are accepted`,
    );
  }
  if (command === "dupes") {
    if (values.changed === "") throw new UsageError("dupes: --changed is required");
    values.minTokens = minimum("--min-tokens", values["min-tokens"]);
    values.minLines = minimum("--min-lines", values["min-lines"]);
  }
  if (command === "mutate") {
    if (values.changed === "" || values.coverage === "") throw new UsageError("mutate: --changed and --coverage are required");
    values.maxMutants = minimum("--max-mutants", values["max-mutants"], command);
    values.jobs = values.jobs === "" ? defaultJobs() : minimum("--jobs", values.jobs, command);
    const seconds = parseDuration(values["min-mutant-timeout"]);
    if (seconds <= 0) throw new UsageError("mutate: --min-mutant-timeout must be positive");
    values.minMutantTimeoutMs = Math.round(seconds * 1000);
  }
  return values;
}

/** A whole-number flag (--min-tokens, --min-lines, --max-mutants, --jobs): at least 1. */
function minimum(flag, text, command = "dupes") {
  if (!/^\d+$/.test(text) || Number(text) < 1 || !Number.isSafeInteger(Number(text))) {
    throw new UsageError(`${command}: ${flag} must be a whole number of at least 1, not ${JSON.stringify(text)}`);
  }
  return Number(text);
}

/** The source tree to read; a missing one fails the command (exit 1). */
function tree(dir) {
  if (!statSync(dir, { throwIfNoEntry: false })?.isDirectory()) throw new Error(`${dir} is not a directory`);
  return resolve(dir);
}

/** The time by which a command must have answered: its --timeout from now, less a margin to print. */
function deadlineOf(options, margin = 2) {
  const seconds = parseDuration(options.timeout);
  return Date.now() + Math.max(seconds - Math.min(margin, seconds / 10), seconds / 2) * 1000;
}

/** Functions and calls of the tree (analysis.schema.json). Must not write inside --dir. */
async function analyze(options, log) {
  const dir = tree(options.dir);
  const started = Date.now();
  // Loaded here so that --version and usage errors do not pay for the compiler.
  const { analyzeTree } = await import("./analyze/index.js");
  const result = analyzeTree(dir, { log, deadline: deadlineOf(options) });
  log.info("analysis done", {
    elapsed: `${Date.now() - started}ms`,
    functions: result.functions.length,
    calls: result.calls.length,
    externals: result.externals.length,
    diagnostics: result.diagnostics.length,
  });
  return protocol.analysis(protocol.languageOf(dir), result);
}

/**
 * Fetches what the tests need, so the test step can run offline; never runs project code. With --tools, also
 * installs the mutation engine there when it is missing (mutate/tools.js).
 */
async function prepare(options, log) {
  const dir = tree(options.dir);
  const deadline = deadlineOf(options, 5);
  const { prepareTree } = await import("./prepare.js");
  const writable = await prepareTree(dir, { log, deadline });
  if (options.tools !== "") {
    const { installEngine } = await import("./mutate/tools.js");
    const engine = await installEngine(options.tools, { log, deadline });
    log.info("mutation engine ready", { dir: engine.dir, version: engine.version, installed: engine.installed });
  }
  log.info("prepare done", { writable: writable.length });
  return protocol.prepare(writable);
}

/** Mutation testing of the lines the review adds (mutation.schema.json), with the engine of --tools. */
async function mutate(options, log) {
  const started = Date.now();
  const dir = tree(options.dir);
  const { added } = readChanged(options.changed, "mutate");
  const blocks = readCoverage(options.coverage);
  const progressFile = options.progress;
  const progress = progressFile === "" ? null : (state) => writeProgress(progressFile, protocol.mutationProgress(state));
  const { runMutation } = await import("./mutate/index.js");
  const result = await runMutation(dir, {
    log,
    started,
    deadline: deadlineOf(options, 5),
    added,
    blocks,
    cacheDir: options["cache-dir"] === "" ? "" : resolve(options["cache-dir"]),
    tools: options.tools,
    maxMutants: options.maxMutants,
    jobs: options.jobs,
    minMutantTimeoutMs: options.minMutantTimeoutMs,
    progress,
    offline: options.offline,
  });
  log.info("mutation done", { ...result.summary, diagnostics: result.diagnostics.length });
  return protocol.mutation(protocol.languageOf(dir), result);
}

/** Runs the project's tests once (tests.schema.json). Failing tests are results, not a failure. */
async function runTests(options, log) {
  const dir = tree(options.dir);
  const selection = readSelection(options.selection);
  const progressFile = options.progress;
  const progress = progressFile === "" ? null : (counts) => writeProgress(progressFile, protocol.progress(counts));
  const { runTests: run } = await import("./testrun/index.js");
  // --no-race is accepted and has nothing to turn off: JavaScript has no race detector.
  const result = await run(dir, {
    log,
    deadline: deadlineOf(options, 5),
    cover: !options["no-cover"],
    selection,
    progress,
  });
  log.info("test run done", { tests: result.tests.length, packages: result.packages.length, partial: selection !== null });
  return protocol.testRun(protocol.languageOf(dir), { race: false, ...result });
}

/** Static checkers on the tree (checks.schema.json): the project's tsc and eslint, read only. */
async function check(options, log) {
  const dir = tree(options.dir);
  const { runChecks } = await import("./check.js");
  const { checks, diagnostics } = await runChecks(dir, { log, deadline: deadlineOf(options) });
  log.info("checks done", { checks: checks.map((c) => `${c.tool}: ${c.status}`).join(", ") });
  return protocol.checkRun(protocol.languageOf(dir), { checks, diagnostics });
}

/** Duplicated code the review introduces (dupes.schema.json). Must not write inside --dir. */
async function dupes(options, log) {
  const dir = tree(options.dir);
  const { added, exclude } = readChanged(options.changed);
  const { findDuplicates } = await import("./dupes/index.js");
  const { minTokens, minLines } = options;
  const result = findDuplicates(dir, added, { minTokens, minLines, exclude, log, deadline: deadlineOf(options) });
  return protocol.dupes(protocol.languageOf(dir), { minTokens, minLines, ...result });
}

/**
 * The --changed file (dupes.schema.json, $defs/changed): `{ added, exclude }`. `added` maps a relative path to
 * the [start, end] line ranges added; `exclude` is the optional `exclude` object (`{ dirs, extensions }`, both
 * string arrays), or null when the file has none. Only dupes uses `exclude`; mutate reads the same file and
 * ignores it. A file that cannot be read or does not match the schema is a usage error.
 */
export function readChanged(file, command = "dupes") {
  let changed;
  try {
    changed = JSON.parse(readFileSync(file, "utf8"));
  } catch (error) {
    throw new UsageError(`${command}: cannot read the changed lines: ${error.message}`);
  }
  const fail = (why) => {
    throw new UsageError(`${command}: cannot read the changed lines: ${why}`);
  };
  if (!isObject(changed)) fail("not a JSON object");
  const { exclude, ...required } = changed;
  onlyKeys(required, ["protocol", "files"], "the document", fail);
  if (changed.protocol !== protocol.PROTOCOL) fail(`protocol ${JSON.stringify(changed.protocol)} is not ${protocol.PROTOCOL}`);
  if (!Array.isArray(changed.files)) fail("files is not an array");
  if (exclude !== undefined) {
    if (!isObject(exclude)) fail("exclude is not an object");
    onlyKeys(exclude, ["dirs", "extensions"], "exclude", fail);
    for (const key of ["dirs", "extensions"]) {
      if (!Array.isArray(exclude[key]) || !exclude[key].every((item) => typeof item === "string")) {
        fail(`exclude.${key} is not an array of strings`);
      }
    }
  }
  const added = new Map();
  changed.files.forEach((entry, i) => {
    const where = `files[${i}]`;
    if (!isObject(entry)) fail(`${where} is not an object`);
    onlyKeys(entry, ["path", "added"], where, fail);
    if (typeof entry.path !== "string") fail(`${where}.path is not a string`);
    if (!Array.isArray(entry.added)) fail(`${where}.added is not an array`);
    for (const lines of entry.added) {
      if (!Array.isArray(lines) || lines.length !== 2 || !isLine(lines[0]) || !isLine(lines[1])) {
        fail(`${where}.added holds ${JSON.stringify(lines)}, not a [start, end] pair of lines`);
      }
    }
    added.set(entry.path, [...(added.get(entry.path) ?? []), ...entry.added]);
  });
  return { added, exclude: exclude === undefined ? null : { dirs: exclude.dirs, extensions: exclude.extensions } };
}

const isObject = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const isLine = (value) => Number.isInteger(value) && value >= 1;

/** Fails with `fail` unless `value` has exactly the fields `keys`. */
function onlyKeys(value, keys, where, fail) {
  for (const key of Object.keys(value)) if (!keys.includes(key)) fail(`unknown field ${JSON.stringify(key)} in ${where}`);
  for (const key of keys) if (!Object.hasOwn(value, key)) fail(`${where} has no ${key}`);
}

/**
 * The --coverage file of mutate (mutation.schema.json, $defs/coverage): its blocks. A file that cannot be read
 * or does not match the schema is a usage error.
 */
export function readCoverage(file) {
  let coverage;
  try {
    coverage = JSON.parse(readFileSync(file, "utf8"));
  } catch (error) {
    throw new UsageError(`mutate: cannot read the coverage: ${error.message}`);
  }
  const fail = (why) => {
    throw new UsageError(`mutate: cannot read the coverage: ${why}`);
  };
  if (!isObject(coverage)) fail("not a JSON object");
  onlyKeys(coverage, ["protocol", "blocks"], "the document", fail);
  if (coverage.protocol !== protocol.PROTOCOL) fail(`protocol ${JSON.stringify(coverage.protocol)} is not ${protocol.PROTOCOL}`);
  if (!Array.isArray(coverage.blocks)) fail("blocks is not an array");
  const count = (value) => Number.isInteger(value) && value >= 0;
  coverage.blocks.forEach((block, i) => {
    const where = `blocks[${i}]`;
    if (!isObject(block)) fail(`${where} is not an object`);
    onlyKeys(block, ["file", "startLine", "endLine", "statements", "count"], where, fail);
    if (typeof block.file !== "string") fail(`${where}.file is not a string`);
    if (!isLine(block.startLine) || !isLine(block.endLine)) fail(`${where} has a line that is not a whole number of at least 1`);
    if (!count(block.statements) || !count(block.count)) fail(`${where} has a statements or count that is not a whole number`);
  });
  return coverage.blocks;
}

/** The --selection file (tests.schema.json, $defs/selection); "" selects everything. */
export function readSelection(file) {
  if (file === "") return null;
  let selection;
  try {
    selection = JSON.parse(readFileSync(file, "utf8"));
  } catch (error) {
    throw new UsageError(`test: cannot read the selection: ${error.message}`);
  }
  if (selection === null || typeof selection !== "object" || !Array.isArray(selection.packages)) {
    throw new UsageError("test: cannot read the selection: packages is missing");
  }
  return selection;
}

/** Replaces the --progress file whole, so the core never reads half of it. */
export function writeProgress(file, progress) {
  const temporary = join(dirname(resolve(file)), `.progress-${process.pid}`);
  writeFileSync(temporary, JSON.stringify(progress));
  renameSync(temporary, file);
}

const COMMANDS = { analyze, prepare, test: runTests, check, dupes, mutate };

/** Runs `args` (without the program) and resolves to the exit code. */
export async function main(args, stdout, stderr) {
  if (args.length > 0 && ["--version", "-version", "version"].includes(args[0])) {
    stdout.write(`${NAME} ${VERSION}\n`);
    return EXIT_OK;
  }
  if (args.length === 0 || !Object.hasOwn(COMMANDS, args[0])) {
    stderr.write(USAGE);
    return EXIT_USAGE;
  }
  const command = args[0];
  const log = createLogger(stderr, NAME, process.env.DIFFLENS_LOG_LEVEL);
  let options;
  try {
    options = parseOptions(command, args.slice(1));
  } catch (error) {
    if (!(error instanceof UsageError)) throw error;
    stderr.write(`${error.message}\n`);
    return EXIT_USAGE;
  }
  let document;
  try {
    document = await COMMANDS[command](options, log);
  } catch (error) {
    if (error instanceof UsageError) {
      stderr.write(`${error.message}\n`);
      return EXIT_USAGE;
    }
    log.error(`${command} failed`, { dir: options.dir, error: String(error?.message ?? error) });
    return EXIT_FAILED;
  }
  stdout.write(`${JSON.stringify(document)}\n`);
  return EXIT_OK;
}
