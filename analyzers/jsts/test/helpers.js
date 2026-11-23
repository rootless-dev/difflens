// Shared by the analyzer's tests: the fixtures, running the command line, and throwaway copies.

import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
export const TESTDATA = join(ROOT, "testdata");
export const MAIN = join(ROOT, "src", "main.js");

export function fixture(name) {
  return join(TESTDATA, name);
}

/** Whether a fixture's dependencies are installed (npm install in its folder); tests needing them skip otherwise. */
export function installed(name, pkg) {
  return existsSync(join(fixture(name), "node_modules", pkg, "package.json"));
}

/**
 * The PATH the core gives the analyzer: the user's, not npm's (`npm test` puts node_modules/.bin first, where
 * the analyzer's own typescript would pass for a tsc of the user's).
 */
const PATH = (process.env.PATH ?? "")
  .split(":")
  .filter((dir) => !dir.includes("node_modules/.bin"))
  .join(":");

/** Runs the analyzer's command line; resolves to { code, stdout, stderr, json }. */
export function cli(args, options = {}) {
  const result = spawnSync(process.execPath, [MAIN, ...args], {
    encoding: "utf8",
    maxBuffer: 256 << 20,
    env: { ...process.env, PATH, DIFFLENS_LOG_LEVEL: "error", ...(options.env ?? {}) },
    cwd: options.cwd,
  });
  let json;
  try {
    json = JSON.parse(result.stdout);
  } catch {
    json = null;
  }
  return { code: result.status, stdout: result.stdout, stderr: result.stderr, json };
}

/** A throwaway copy of a fixture (node_modules included), as the core gives the test step; removed by `done`. */
export function copy(name) {
  const run = mkdtempSync(join(tmpdir(), "jsts-test-"));
  const tree = join(run, "tree");
  cpSync(fixture(name), tree, { recursive: true, verbatimSymlinks: true });
  return { run, tree, done: () => rmSync(run, { recursive: true, force: true }) };
}

/** The analysis of a fixture, memoised. */
const analyses = new Map();
export function analysisOf(name) {
  if (!analyses.has(name)) {
    const { code, json, stderr } = cli(["analyze", "--dir", fixture(name)]);
    if (code !== 0) throw new Error(`analyze ${name} failed: ${stderr}`);
    analyses.set(name, json);
  }
  return analyses.get(name);
}

export function fn(analysis, id) {
  return analysis.functions.find((f) => f.id === id);
}

export function callsFrom(analysis, caller) {
  return analysis.calls.filter((c) => c.caller === caller);
}

export function call(analysis, caller, callee, relation = "call") {
  return analysis.calls.filter((c) => c.caller === caller && c.callee === callee && c.relation === relation);
}
