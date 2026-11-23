// The check command: the project's own static checkers on the tree, read
// only (checks.schema.json). In this order:
//
// - tsc: `tsc --noEmit -p <tsconfig>` for every tsconfig.json of the tree
//   (and the configurations they reference; a solution configuration with
//   no files of its own is not run itself), with the typescript installed
//   for that configuration (node_modules/typescript in its folder or one
//   above), else a tsc on the PATH. Unavailable without either, or without a
//   tsconfig.json.
// - eslint: `eslint --format json .` where an ESLint configuration is (the
//   root, else each package of the workspace and each folder one down),
//   flat (eslint.config.*) or legacy (.eslintrc*, package.json
//   eslintConfig), with the eslint installed there, else one on the PATH.
//   Unavailable without a configuration or an eslint.
//
// The core checks a throwaway copy of the tree that `prepare` installed, so
// node_modules is there (pnpm's symbolic links included) and the project's
// own tsc, eslint and ESLint plugins are the ones that run. Both run under a
// read-only sandbox (sandbox.js): eslint loads the project's configuration
// and plugins, which are code. Incremental builds keep their build info in
// a temporary folder, never in the tree, and the caches tools keep by
// convention (find-cache-dir's CACHE_DIR, XDG_CACHE_HOME) go there too.
// Nothing is kept in --cache-dir: tsc's build info and ESLint's cache are
// keyed by absolute paths, and each run checks a copy at another path.

import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";

import { Tree, readJson, walkedFolder } from "./files.js";
import { keepEnd, major, packageBin, packageDir, packageVersion, run, which } from "./process.js";
import * as protocol from "./protocol.js";
import { readOnly } from "./sandbox.js";
import { workspaceFolders } from "./testrun/units.js";

export const TOOL_TSC = "tsc";
export const TOOL_ESLINT = "eslint";

/** Codes tsc reports about project references that were not built: the build's business, not the code's. */
const REFERENCE_CODES = new Set(["TS6305", "TS6306", "TS6307", "TS6308", "TS6310", "TS6377", "TS6379"]);

const FLAT_CONFIGS = ["eslint.config.js", "eslint.config.mjs", "eslint.config.cjs", "eslint.config.ts", "eslint.config.mts", "eslint.config.cts"];
const LEGACY_CONFIGS = [".eslintrc.js", ".eslintrc.cjs", ".eslintrc.yaml", ".eslintrc.yml", ".eslintrc.json", ".eslintrc"];

/** Runs every checker on the tree at `dir`; resolves to { checks, diagnostics }. */
export async function runChecks(dir, { log, deadline }) {
  const tree = new Tree(dir);
  const scratch = mkdtempSync(join(tmpdir(), "difflens-check-"));
  const diagnostics = [];
  try {
    const tsc = await checkTypes(tree, { log, deadline, scratch, diagnostics });
    const eslint = await lint(tree, { log, deadline, scratch, diagnostics });
    return { checks: [tsc, eslint], diagnostics };
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

/** The tsconfig.json files of the tree and those they reference, with whether each has files of its own. */
export function typeScriptProjects(root) {
  const found = [];
  const walk = (folder) => {
    let entries;
    try {
      entries = readdirSync(folder, { withFileTypes: true });
    } catch {
      return;
    }
    if (entries.some((e) => e.isFile() && e.name === "tsconfig.json")) found.push(join(folder, "tsconfig.json"));
    for (const e of entries) if (e.isDirectory() && walkedFolder(e.name)) walk(join(folder, e.name));
  };
  walk(root);
  const out = [];
  const seen = new Set();
  const queue = [...found];
  while (queue.length > 0) {
    const config = resolve(queue.shift());
    if (seen.has(config) || !existsSync(config)) continue;
    seen.add(config);
    const json = readJson(config) ?? {};
    for (const ref of Array.isArray(json.references) ? json.references : []) {
      if (typeof ref?.path !== "string") continue;
      const path = resolve(dirname(config), ref.path);
      queue.push(path.endsWith(".json") ? path : join(path, "tsconfig.json"));
    }
    const solution = Array.isArray(json.files) && json.files.length === 0 && json.include === undefined;
    out.push({ config, solution });
  }
  return out;
}

/** The typescript that checks `config`: { command, args (before tsc's own), version } or null. */
function findTypeScript(root, config) {
  const dir = packageDir("typescript", dirname(config), root);
  if (dir) {
    const bin = packageBin(dir, "tsc");
    if (bin) return { command: process.execPath, args: [bin], version: packageVersion(dir), where: "the project's" };
  }
  const global = which("tsc");
  return global ? { command: global, args: [], version: null, where: "the PATH's" } : null;
}

/** Whether a configuration builds incrementally (composite or incremental, its `extends` followed). */
async function incremental(config) {
  try {
    const ts = (await import("typescript")).default;
    const parsed = ts.getParsedCommandLineOfConfigFile(config, {}, { ...ts.sys, onUnRecoverableConfigFileDiagnostic: () => {} });
    return Boolean(parsed?.options.composite || parsed?.options.incremental);
  } catch {
    return false;
  }
}

const TSC_LINE = /^(.+?)\((\d+),(\d+)\): (error|warning|suggestion|message) (TS\d+): (.*)$/;
const TSC_GLOBAL = /^error (TS\d+): (.*)$/;

/** The protocol's severity of a tsc diagnostic category. */
const TSC_SEVERITY = { error: "error", warning: "warning", suggestion: "info", message: "info" };

/** The protocol's severity of an ESLint message: 2 is an error, 1 a warning, a fatal (parse) error an error. */
export function eslintSeverity(message) {
  if (message?.fatal) return "error";
  return message?.severity === 1 ? "warning" : "error";
}

/** Parses tsc's --pretty false output: findings, and the errors not about a file. */
export function parseTscOutput(output, cwd, tree) {
  const findings = [];
  const globals = [];
  let last = null;
  for (const line of output.split(/\r?\n/)) {
    const located = TSC_LINE.exec(line);
    if (located) {
      const [, file, lineNo, , category, code, message] = located;
      last = null;
      if (REFERENCE_CODES.has(code)) continue;
      const rel = tree.rel(resolve(cwd, file));
      if (rel === null || rel.split("/").includes("node_modules")) continue;
      last = { file: rel, line: Math.max(1, Number(lineNo)), message, rule: code, severity: TSC_SEVERITY[category] };
      findings.push(last);
      continue;
    }
    const global = TSC_GLOBAL.exec(line);
    if (global) {
      last = null;
      if (!REFERENCE_CODES.has(global[1])) globals.push(`${global[1]}: ${global[2]}`);
      continue;
    }
    if (last && /^\s+\S/.test(line)) last.message += `\n${line.trim()}`;
  }
  return { findings, globals };
}

async function checkTypes(tree, { log, deadline, scratch, diagnostics }) {
  const projects = typeScriptProjects(tree.root).filter((p) => !p.solution);
  if (projects.length === 0) return protocol.check(TOOL_TSC, "unavailable", "the project has no tsconfig.json, so it is not type-checked");
  const findings = [];
  const failures = [];
  let ran = 0;
  let missing = 0;
  for (const [n, { config }] of projects.entries()) {
    const rel = relative(tree.root, config);
    if (Date.now() > deadline) {
      diagnostics.push(`tsc: ${rel} was not checked: the time ran out`);
      continue;
    }
    const ts = findTypeScript(tree.root, config);
    if (!ts) {
      missing++;
      continue;
    }
    const args = [...ts.args, "--noEmit", "-p", config, "--pretty", "false"];
    if (await incremental(config)) args.push("--tsBuildInfoFile", join(scratch, `${n}.tsbuildinfo`));
    const wrapped = readOnly(ts.command, args, scratch);
    const cwd = dirname(config);
    const result = await run(wrapped.command, wrapped.args, { cwd, deadline, env: checkerEnv(scratch) });
    log.info("tsc done", { config: rel, exit: result.code, typescript: ts.version, sandboxed: wrapped.sandboxed });
    if (result.timedOut) {
      diagnostics.push(`tsc: ${rel} was not checked: the time ran out`);
      continue;
    }
    if (result.error) {
      failures.push(`${rel}: ${result.error.message}`);
      continue;
    }
    ran++;
    const parsed = parseTscOutput(result.stdout, cwd, tree);
    findings.push(...parsed.findings);
    if (result.code !== 0 && parsed.findings.length === 0) {
      failures.push(`${rel}: ${parsed.globals[0] ?? firstLine(result.stdout + result.stderr) ?? `tsc exited with ${result.code}`}`);
    }
    if (ts.where !== "the project's") diagnostics.push(`tsc: ${rel} was checked with the tsc on the PATH (the project has no node_modules/typescript)`);
  }
  if (ran === 0 && failures.length === 0) {
    if (missing > 0) {
      return protocol.check(TOOL_TSC, "unavailable", "typescript is not installed in the project (node_modules/typescript) and there is no tsc on the PATH");
    }
    return protocol.check(TOOL_TSC, "failed", "the time ran out before tsc could run");
  }
  if (missing > 0) diagnostics.push(`tsc: ${missing} configuration(s) were not checked: typescript is not installed for them`);
  const unique = dedupe(findings);
  if (unique.length > 0) {
    for (const f of failures) diagnostics.push(`tsc: ${f}`);
    return protocol.check(TOOL_TSC, "findings", null, unique);
  }
  if (failures.length > 0) return protocol.check(TOOL_TSC, "failed", keepEnd(failures.join("; "), 2000));
  return protocol.check(TOOL_TSC, "clean");
}

/**
 * Folders with an ESLint configuration: the root, else the packages of the workspace (package.json
 * `workspaces`, pnpm-workspace.yaml) and the folders one down that have one, sorted.
 */
export function eslintFolders(root) {
  const has = (dir) => {
    for (const name of FLAT_CONFIGS) if (existsSync(join(dir, name))) return { dir, flat: true };
    for (const name of LEGACY_CONFIGS) if (existsSync(join(dir, name))) return { dir, flat: false };
    const pkg = readJson(join(dir, "package.json"));
    if (pkg && typeof pkg.eslintConfig === "object") return { dir, flat: false };
    return null;
  };
  const top = has(root);
  if (top) return [top];
  const candidates = new Set(workspaceFolders(root, readJson(join(root, "package.json")) ?? {}));
  let entries;
  try {
    entries = readdirSync(root, { withFileTypes: true });
  } catch {
    entries = [];
  }
  for (const e of entries) if (e.isDirectory() && walkedFolder(e.name)) candidates.add(join(root, e.name));
  const out = [];
  for (const dir of [...candidates].sort()) {
    // A folder linted already lints the packages inside it.
    if (out.some((f) => dir.startsWith(`${f.dir}/`))) continue;
    const found = has(dir);
    if (found) out.push(found);
  }
  return out;
}

function findEslint(root, dir) {
  const pkgDir = packageDir("eslint", dir, root);
  if (pkgDir) {
    const bin = packageBin(pkgDir, "eslint");
    if (bin) return { command: process.execPath, args: [bin], version: packageVersion(pkgDir), local: true };
  }
  const global = which("eslint");
  return global ? { command: global, args: [], version: null, local: false } : null;
}

/** ESLint's JSON report as findings (files outside the tree dropped); null when it is not JSON. */
export function parseEslintOutput(stdout, tree) {
  let results;
  try {
    results = JSON.parse(stdout.slice(stdout.indexOf("[")));
  } catch {
    return null;
  }
  if (!Array.isArray(results)) return null;
  const findings = [];
  for (const result of results) {
    const rel = tree.rel(result.filePath);
    if (rel === null) continue;
    for (const m of result.messages ?? []) {
      findings.push({ file: rel, line: Math.max(1, m.line ?? 1), message: String(m.message ?? ""), rule: m.ruleId ?? null, severity: eslintSeverity(m) });
    }
  }
  return findings;
}

async function lint(tree, { log, deadline, scratch, diagnostics }) {
  const folders = eslintFolders(tree.root);
  if (folders.length === 0) return protocol.check(TOOL_ESLINT, "unavailable", "the project has no ESLint configuration");
  const findings = [];
  const failures = [];
  let ran = 0;
  let missing = 0;
  for (const { dir, flat } of folders) {
    const rel = relative(tree.root, dir) || ".";
    if (Date.now() > deadline) {
      diagnostics.push(`eslint: ${rel} was not linted: the time ran out`);
      continue;
    }
    const eslint = findEslint(tree.root, dir);
    if (!eslint) {
      missing++;
      continue;
    }
    const version = major(eslint.version);
    const env = checkerEnv(scratch);
    const args = [...eslint.args, "--format", "json", "--no-error-on-unmatched-pattern"];
    if (flat) {
      if (version > 0 && version < 9) env.ESLINT_USE_FLAT_CONFIG = "true";
    } else {
      if (version >= 10) {
        failures.push(`${rel}: ESLint ${eslint.version} no longer reads .eslintrc configurations`);
        continue;
      }
      if (version === 9) env.ESLINT_USE_FLAT_CONFIG = "false";
      args.push("--ext", ".js,.jsx,.ts,.tsx,.mjs,.cjs,.mts,.cts");
    }
    args.push(".");
    const wrapped = readOnly(eslint.command, args, scratch);
    const result = await run(wrapped.command, wrapped.args, { cwd: dir, deadline, env });
    log.info("eslint done", { dir: rel, exit: result.code, eslint: eslint.version, sandboxed: wrapped.sandboxed });
    if (result.timedOut) {
      diagnostics.push(`eslint: ${rel} was not linted: the time ran out`);
      continue;
    }
    if (result.error) {
      failures.push(`${rel}: ${result.error.message}`);
      continue;
    }
    const parsed = result.code === 0 || result.code === 1 ? parseEslintOutput(result.stdout, tree) : null;
    if (parsed === null) {
      failures.push(`${rel}: ${eslintError(result.stderr + result.stdout) ?? `eslint exited with ${result.code}`}`);
      continue;
    }
    ran++;
    findings.push(...parsed);
    if (!eslint.local) diagnostics.push(`eslint: ${rel} was linted with the eslint on the PATH (the project has no node_modules/eslint)`);
  }
  if (ran === 0 && failures.length === 0) {
    if (missing > 0) return protocol.check(TOOL_ESLINT, "unavailable", "eslint is not installed in the project (node_modules/eslint) and there is no eslint on the PATH");
    return protocol.check(TOOL_ESLINT, "failed", "the time ran out before eslint could run");
  }
  const unique = dedupe(findings);
  if (unique.length > 0) {
    for (const f of failures) diagnostics.push(`eslint: ${f}`);
    return protocol.check(TOOL_ESLINT, "findings", null, unique);
  }
  if (failures.length > 0) return protocol.check(TOOL_ESLINT, "failed", keepEnd(failures.join("; "), 2000));
  return protocol.check(TOOL_ESLINT, "clean");
}

/** The most telling line of ESLint's crash report ("Error: Cannot find package ..."). */
function eslintError(text) {
  const lines = text.split(/\r?\n/).map((l) => l.trim()).filter((l) => l !== "" && !l.startsWith("Oops!") && !l.startsWith("at "));
  return lines.find((l) => /error/i.test(l)) ?? lines[0] ?? null;
}

function firstLine(text) {
  return text.split(/\r?\n/).map((l) => l.trim()).find((l) => l !== "") ?? null;
}

function dedupe(findings) {
  const seen = new Set();
  const out = [];
  for (const f of findings) {
    const key = `${f.file}|${f.line}|${f.rule}|${f.severity}|${f.message}`;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(f);
  }
  out.sort((a, b) => (a.file < b.file ? -1 : a.file > b.file ? 1 : a.line - b.line));
  return out;
}

/**
 * The environment of a checker: no colours, no update checks, temporary files and the caches plugins keep by
 * convention in the scratch folder, the one place the sandbox lets them write. Tools that cache under
 * node_modules/.cache use find-cache-dir, which honours CACHE_DIR; others follow XDG_CACHE_HOME. jiti (which
 * loads TypeScript configurations) and @babel/register would write their caches next to the code, which the
 * sandbox refuses with an error, so theirs are off: they would not outlive the run anyway.
 */
export function checkerEnv(scratch) {
  const cache = join(scratch, "cache");
  mkdirSync(cache, { recursive: true });
  return {
    ...process.env,
    NO_COLOR: "1",
    FORCE_COLOR: "0",
    NO_UPDATE_NOTIFIER: "1",
    TMPDIR: scratch,
    CACHE_DIR: cache,
    XDG_CACHE_HOME: cache,
    JITI_FS_CACHE: "false",
    BABEL_DISABLE_CACHE: "1",
  };
}
