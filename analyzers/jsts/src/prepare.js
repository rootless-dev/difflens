// The prepare command: installs the project's dependencies into the run's
// copy of the tree, so that `test` finds the project's own test runner and
// can run without the network. It runs the project's package manager, by its
// lockfile, with install scripts off (no project or dependency code runs):
//
//   pnpm-lock.yaml        pnpm install --frozen-lockfile --ignore-scripts
//   yarn.lock (Berry)     yarn install --immutable --mode=skip-build
//   yarn.lock (classic)   yarn install --frozen-lockfile --ignore-scripts --non-interactive
//   bun.lock, bun.lockb   bun install --frozen-lockfile --ignore-scripts
//   package-lock.json     npm ci --ignore-scripts
//   none                  npm install --ignore-scripts --no-package-lock
//
// for the package at the root of the tree (a workspace root installs every
// package), else for each folder one down with a package.json. A package
// that already has node_modules, or depends on nothing, is left as it is.
// The package managers keep their caches outside the tree (~/.npm, the pnpm
// store, the yarn cache), which `test` only reads; so nothing needs to be
// writable for the test step outside its own folders.

import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join, relative } from "node:path";

import { readJson, walkedFolder } from "./files.js";
import { keepEnd, run, which } from "./process.js";

/** The folders to install: the root with a package.json, else the folders one down that have one. */
export function installRoots(root) {
  if (existsSync(join(root, "package.json"))) return [root];
  const out = [];
  let entries = [];
  try {
    entries = readdirSync(root, { withFileTypes: true });
  } catch {
    return out;
  }
  for (const e of entries) {
    if (e.isDirectory() && walkedFolder(e.name) && existsSync(join(root, e.name, "package.json"))) out.push(join(root, e.name));
  }
  return out;
}

/** Whether the package at `dir` (package.json `pkg`) asks for anything to be installed; a workspace root does. */
function hasDependencies(dir, pkg) {
  const any = (field) => pkg[field] && typeof pkg[field] === "object" && Object.keys(pkg[field]).length > 0;
  return any("dependencies") || any("devDependencies") || any("optionalDependencies") || pkg.workspaces !== undefined || existsSync(join(dir, "pnpm-workspace.yaml"));
}

/** The package manager and the command that installs the package at `dir` offline-ready: { tool, args } . */
export function installCommand(dir, pkg) {
  const declared = typeof pkg.packageManager === "string" ? pkg.packageManager.split("@")[0] : null;
  const has = (name) => existsSync(join(dir, name));
  if (has("pnpm-lock.yaml") || declared === "pnpm") {
    return { tool: "pnpm", args: ["install", "--frozen-lockfile", "--ignore-scripts", "--prefer-offline", "--config.confirmModulesPurge=false"] };
  }
  if (has("bun.lock") || has("bun.lockb") || declared === "bun") return { tool: "bun", args: ["install", "--frozen-lockfile", "--ignore-scripts"] };
  if (has("yarn.lock") || declared === "yarn") {
    const berry = has(".yarnrc.yml") || (has("yarn.lock") && readFileSync(join(dir, "yarn.lock"), "utf8").includes("__metadata:"));
    if (berry) return { tool: "yarn", args: ["install", "--immutable", "--mode=skip-build"], env: { YARN_ENABLE_SCRIPTS: "false" } };
    return { tool: "yarn", args: ["install", "--frozen-lockfile", "--ignore-scripts", "--non-interactive"] };
  }
  if (has("package-lock.json") || has("npm-shrinkwrap.json")) return { tool: "npm", args: ["ci", "--ignore-scripts", "--no-audit", "--no-fund"] };
  return { tool: "npm", args: ["install", "--ignore-scripts", "--no-audit", "--no-fund", "--no-package-lock"] };
}

/** Installs what the tests of the tree at `dir` need; resolves to the folders the test step must write (none). */
export async function prepareTree(dir, { log, deadline }) {
  for (const folder of installRoots(dir)) {
    const rel = relative(dir, folder) || ".";
    const pkg = readJson(join(folder, "package.json"));
    if (!pkg || typeof pkg !== "object") {
      log.warn("package.json is not valid JSON; nothing installed", { dir: rel });
      continue;
    }
    if (existsSync(join(folder, "node_modules"))) {
      log.info("dependencies already installed", { dir: rel });
      continue;
    }
    if (!hasDependencies(folder, pkg)) {
      log.info("no dependencies to install", { dir: rel });
      continue;
    }
    const { tool, args, env = {} } = installCommand(folder, pkg);
    let command = which(tool);
    let commandArgs = args;
    if (command === null && tool !== "npm") {
      // Corepack provides pnpm and yarn where it is installed.
      const corepack = which("corepack");
      if (corepack) {
        command = corepack;
        commandArgs = [tool, ...args];
      }
    }
    if (command === null) {
      throw new Error(`${rel}: ${tool} is needed to install the dependencies (${lockfileOf(tool)}), but it is not on the PATH`);
    }
    const started = Date.now();
    const result = await run(command, commandArgs, {
      cwd: folder,
      deadline,
      env: { ...process.env, ...env, CI: "1", NO_UPDATE_NOTIFIER: "1", npm_config_update_notifier: "false", FORCE_COLOR: "0" },
    });
    log.info("dependencies installed", { dir: rel, tool, exit: result.code, elapsed: `${Date.now() - started}ms` });
    if (result.timedOut) throw new Error(`${rel}: ${tool} did not finish installing the dependencies in time`);
    if (result.error) throw new Error(`${rel}: ${tool} could not run: ${result.error.message}`);
    if (result.code !== 0) {
      throw new Error(`${rel}: ${tool} ${commandArgs.join(" ")} failed:\n${keepEnd((result.stderr || result.stdout).trim(), 4096)}`);
    }
  }
  return [];
}

function lockfileOf(tool) {
  return { pnpm: "pnpm-lock.yaml", yarn: "yarn.lock", bun: "bun.lock" }[tool] ?? "package-lock.json";
}
