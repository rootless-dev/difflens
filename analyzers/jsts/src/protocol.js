// The documents of the analyzer protocol, version 1, as plain objects. The
// schemas are in analyzers/protocol/ of the DiffLens repository:
// analysis.schema.json (analyze), checks.schema.json (check) and
// tests.schema.json (prepare, test, the --selection file and the --progress
// file), dupes.schema.json (dupes and its --changed file), mutation.schema.json
// (mutate, its --coverage file and its --progress file). The core rejects a document with a field the schema does not list.

import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";

export const PROTOCOL = 1;
// Identity reported by --version and in every document.
export const NAME = "difflens-analyzer-jsts";
export const VERSION = "0.0.0";

/** Folders whose tsconfig.json describes third-party code, as the core skips them. */
const SKIPPED = new Set(["node_modules", "vendor", "target", "dist", "build"]);

/**
 * The language id to report for the tree at `dir`, as the core detects it: "typescript" when a tsconfig.json is
 * at the root or one folder down (hidden and dependency folders aside), "javascript" otherwise. The core accepts
 * either from this analyzer.
 */
export function languageOf(dir) {
  const has = (folder) => {
    try {
      return statSync(join(folder, "tsconfig.json")).isFile();
    } catch {
      return false;
    }
  };
  if (has(dir)) return "typescript";
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (!entry.isDirectory() || entry.name.startsWith(".") || SKIPPED.has(entry.name)) continue;
    if (has(join(dir, entry.name))) return "typescript";
  }
  return "javascript";
}

const identity = () => ({ name: NAME, version: VERSION });

/** What `analyze` prints (analysis.schema.json). */
export function analysis(language, { functions = [], calls = [], externals = [], diagnostics = [] } = {}) {
  return { protocol: PROTOCOL, language, analyzer: identity(), functions, calls, externals, diagnostics };
}

/** What `prepare` prints (tests.schema.json, $defs/prepare). */
export function prepare(writable = []) {
  return { protocol: PROTOCOL, writable };
}

/** What `test` prints (tests.schema.json). */
export function testRun(
  language,
  { race = false, tests = [], packages = [], coverage = null, races = [], diagnostics = [] } = {},
) {
  return { protocol: PROTOCOL, language, runner: identity(), race, tests, packages, coverage, races, diagnostics };
}

/** The --progress file (tests.schema.json, $defs/progress). */
export function progress({ packagesDone = 0, packagesTotal = 0, tests = 0, failed = 0, current = null } = {}) {
  return { protocol: PROTOCOL, packagesDone, packagesTotal, tests, failed, current };
}

/** What `check` prints (checks.schema.json). */
export function checkRun(language, { checks = [], diagnostics = [] } = {}) {
  return { protocol: PROTOCOL, language, runner: identity(), checks, diagnostics };
}

/** What `dupes` prints (dupes.schema.json). */
export function dupes(language, { minTokens, minLines, duplicates = [], diagnostics = [] }) {
  return { protocol: PROTOCOL, language, runner: identity(), minTokens, minLines, duplicates, diagnostics };
}

/** What `mutate` prints (mutation.schema.json). */
export function mutation(language, { engine, mutants = [], summary, diagnostics = [] }) {
  return { protocol: PROTOCOL, language, runner: identity(), engine, mutants, summary, diagnostics };
}

/** The --progress file of `mutate` (mutation.schema.json, $defs/progress). */
export function mutationProgress({ phase = "baseline", total = 0, done = 0, killed = 0, survived = 0, timeout = 0, error = 0, current = null } = {}) {
  return { protocol: PROTOCOL, phase, total, done, killed, survived, timeout, error, current };
}

/** One checker; status is "clean", "findings", "unavailable" or "failed". */
export function check(tool, status, message = null, findings = []) {
  return { tool, status, message, findings };
}
