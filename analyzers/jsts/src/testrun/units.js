// Which test runner runs which tests: the "units" of a run. A unit is a
// package folder and its runner, found from its package.json in this order:
//
//   1. the runner its `test` script names: vitest, jest, mocha, or
//      `node --test` (also through tsx, ts-node or c8 / nyc);
//   2. a runner's configuration file in the folder: vitest.config.*,
//      jest.config.* (or a "jest" key), .mocharc.* (or a "mocha" key);
//   3. a runner among its dependencies: vitest, jest, mocha;
//   4. node --test, when a test file imports node:test.
//
// The root package is the one unit when it has a runner and is not only a
// workspace root delegating to its packages (a script such as
// `pnpm -r test` or `turbo run test`); otherwise every package of the
// workspace and every folder one down with a package.json is a unit of its
// own. A test file belongs to the deepest unit holding it.

import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join, relative } from "node:path";

import { isTestFile, readJson, walkedFolder } from "../files.js";

export const RUNNERS = ["vitest", "jest", "mocha", "node"];

const CONFIGS = {
  vitest: /^vitest\.(config|workspace|projects)\.[cm]?[jt]s$/,
  jest: /^jest\.config\.([cm]?[jt]s|json)$/,
  mocha: /^\.mocharc(\.(js|cjs|mjs|json|jsonc|yaml|yml))?$/,
};

/** The runner a test script names, with the arguments of a `node --test` command; null for none. */
export function runnerOfScript(script) {
  if (typeof script !== "string" || script.trim() === "") return null;
  // The first command that runs tests, so "tsc && vitest run" is vitest.
  for (const part of script.split(/&&|\|\||;/)) {
    const words = part.trim().split(/\s+/).filter((w) => !/^[A-Z_][A-Z0-9_]*=/.test(w));
    const at = (name) => words.findIndex((w) => w === name || w.endsWith(`/${name}`));
    if (at("vitest") >= 0) return { runner: "vitest" };
    if (at("jest") >= 0) return { runner: "jest" };
    if (at("mocha") >= 0 || at("_mocha") >= 0) return { runner: "mocha" };
    const test = words.indexOf("--test");
    if (test >= 0) {
      const host = words.findIndex((w) => ["node", "tsx", "ts-node", "bun"].includes(w.split("/").at(-1)));
      if (host >= 0 && host < test) {
        const flags = [];
        const patterns = [];
        const hostName = words[host].split("/").at(-1);
        if (hostName === "tsx") flags.push("--import", "tsx");
        if (hostName === "ts-node") flags.push("--require", "ts-node/register");
        for (let i = host + 1; i < words.length; i++) {
          const w = words[i].replace(/^['"]|['"]$/g, "");
          if (w === "--test") continue;
          if (w.startsWith("--test-")) continue;
          if (["--import", "--require", "-r", "--loader", "--experimental-loader"].includes(w) && i + 1 < words.length) {
            flags.push(w, words[++i].replace(/^['"]|['"]$/g, ""));
          } else if (w.startsWith("--")) {
            if (/^--(experimental|no-warnings|enable-source-maps|conditions|env-file|import=|require=|loader=)/.test(w)) flags.push(w);
          } else {
            patterns.push(w);
          }
        }
        return { runner: "node", flags, patterns };
      }
    }
  }
  return null;
}

/** The runner of the package at `dir` (package.json `pkg`), or null. */
export function runnerOf(dir, pkg, testFiles) {
  const script = runnerOfScript(pkg.scripts?.test);
  if (script) return { ...script, how: "the test script" };
  let entries;
  try {
    entries = readdirSync(dir);
  } catch {
    entries = [];
  }
  for (const [runner, pattern] of Object.entries(CONFIGS)) {
    if (entries.some((name) => pattern.test(name))) return { runner, how: "its configuration file" };
  }
  if (pkg.jest && typeof pkg.jest === "object") return { runner: "jest", how: "package.json" };
  if (pkg.mocha && typeof pkg.mocha === "object") return { runner: "mocha", how: "package.json" };
  const deps = { ...(pkg.dependencies ?? {}), ...(pkg.devDependencies ?? {}) };
  for (const runner of ["vitest", "jest", "mocha"]) if (deps[runner]) return { runner, how: "its dependencies" };
  for (const file of testFiles) {
    let text;
    try {
      text = readFileSync(file, "utf8");
    } catch {
      continue;
    }
    if (/from\s+["'](node:)?test["']|require\(\s*["'](node:)?test["']\s*\)/.test(text)) return { runner: "node", flags: [], patterns: [], how: "node:test imports" };
  }
  return null;
}

/** Whether a root script only delegates to the workspace's packages. */
function delegates(script) {
  return typeof script === "string" && /\b(turbo|nx|lerna|pnpm\s+(-r|--recursive|--filter)|yarn\s+workspaces|npm\s+(run\s+\S+\s+)?(--workspaces|-ws))\b/.test(script);
}

/** The workspace package folders a package.json lists (globs ending in * or ** expanded), without the root. */
export function workspaceFolders(root, pkg) {
  let patterns = Array.isArray(pkg.workspaces) ? pkg.workspaces : Array.isArray(pkg.workspaces?.packages) ? pkg.workspaces.packages : [];
  const pnpm = join(root, "pnpm-workspace.yaml");
  if (existsSync(pnpm)) {
    const text = readFileSync(pnpm, "utf8");
    patterns = [...patterns, ...[...text.matchAll(/^\s*-\s*["']?([^"'\n#]+?)["']?\s*$/gm)].map((m) => m[1])];
  }
  const out = new Set();
  for (const pattern of patterns) {
    if (typeof pattern !== "string" || pattern.startsWith("!")) continue;
    const parts = pattern.replace(/^\.\//, "").replace(/\/$/, "").split("/");
    const expand = (dir, i) => {
      if (i === parts.length) {
        if (existsSync(join(dir, "package.json"))) out.add(dir);
        return;
      }
      const part = parts[i];
      if (part === "*" || part === "**") {
        let entries;
        try {
          entries = readdirSync(dir, { withFileTypes: true });
        } catch {
          return;
        }
        for (const e of entries) {
          if (!e.isDirectory() || !walkedFolder(e.name)) continue;
          expand(join(dir, e.name), i + 1);
          if (part === "**") expand(join(dir, e.name), i);
        }
      } else {
        expand(join(dir, part), i + 1);
      }
    };
    expand(root, 0);
  }
  out.delete(root);
  return [...out];
}

/**
 * The units of the tree at `root` whose source files are `files` (relative): [{ dir, rel, runner, how, flags,
 * patterns, testFiles (absolute) }], and the test files no unit runs.
 */
export function findUnits(root, files) {
  const tests = files.filter(isTestFile).map((rel) => join(root, rel));
  const rootPkg = readJson(join(root, "package.json"));
  const candidates = [];
  if (rootPkg) {
    const own = runnerOf(root, rootPkg, tests);
    const isWorkspace = rootPkg.workspaces !== undefined || existsSync(join(root, "pnpm-workspace.yaml"));
    if (own && !(isWorkspace && (delegates(rootPkg.scripts?.test) || own.how !== "the test script"))) {
      return assign(root, [{ dir: root, ...own }], tests);
    }
    for (const dir of workspaceFolders(root, rootPkg)) candidates.push(dir);
    if (own && !delegates(rootPkg.scripts?.test)) candidates.push(root);
  }
  let entries;
  try {
    entries = readdirSync(root, { withFileTypes: true });
  } catch {
    entries = [];
  }
  for (const e of entries) {
    if (e.isDirectory() && walkedFolder(e.name) && existsSync(join(root, e.name, "package.json"))) candidates.push(join(root, e.name));
  }
  const units = [];
  for (const dir of [...new Set(candidates)].sort()) {
    const pkg = readJson(join(dir, "package.json")) ?? {};
    const own = runnerOf(dir, pkg, tests.filter((t) => t.startsWith(`${dir}/`)));
    if (own) units.push({ dir, ...own });
  }
  return assign(root, units, tests);
}

function assign(root, units, tests) {
  const byDepth = [...units].sort((a, b) => b.dir.length - a.dir.length);
  for (const unit of units) {
    unit.rel = relative(root, unit.dir) || ".";
    unit.testFiles = [];
    unit.flags ??= [];
    unit.patterns ??= [];
  }
  const orphans = [];
  for (const file of tests) {
    const unit = byDepth.find((u) => file.startsWith(`${u.dir}/`));
    if (unit) unit.testFiles.push(file);
    else orphans.push(file);
  }
  return { units, orphans };
}

/** Whether node --test runs `file` by default (its default patterns), TypeScript included where Node strips types. */
export function nodeRunsByDefault(rel, typescript) {
  const ext = typescript ? "[cm]?[jt]s" : "[cm]?js";
  const base = rel.split("/").at(-1);
  if (new RegExp(`^(.+[.\\-_])?test(-.+)?\\.${ext}$`).test(base)) return true;
  if (new RegExp(`\\.${ext}$`).test(base) && rel.split("/").slice(0, -1).includes("test")) return true;
  return false;
}
