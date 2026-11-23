// The mutation engine (StrykerJS), kept out of the analyzer's own
// dependencies: `prepare --tools <folder>` installs the pinned manifest of
// ./engine (package.json and package-lock.json, exact versions) into
// <folder>/stryker, and `mutate --tools <folder>` runs it from there.
//
// The install is `npm ci --omit=dev --ignore-scripts --legacy-peer-deps`
// (no install scripts; the runner plugins' peer dependencies, vitest and
// mocha, are left out: the project's own are used, see preload.mjs) in a
// temporary folder inside <folder>, renamed to <folder>/stryker when complete.
// <folder>/stryker/.difflens-engine holds a hash of the manifest: an install
// with another hash is replaced, one with the same is left as it is. Two
// prepares at once each install into their own temporary folder; the second
// to finish finds the first one's install in place and drops its own.

import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { keepEnd, packageVersion, run, which } from "../process.js";

/** The folder of the engine's manifest in the analyzer's source. */
export const MANIFEST = join(dirname(fileURLToPath(import.meta.url)), "engine");
const FILES = ["package.json", "package-lock.json"];
const MARKER = ".difflens-engine";
const CORE = "@stryker-mutator/core";

/** The hash of the manifest: what an install is of. */
export function manifestHash(manifest = MANIFEST) {
  const hash = createHash("sha256");
  for (const name of FILES) hash.update(name).update("\0").update(readFileSync(join(manifest, name))).update("\0");
  return hash.digest("hex");
}

/** Whether `target` holds a complete install of the manifest whose hash is `hash`. */
function installedAt(target, hash) {
  try {
    return readFileSync(join(target, MARKER), "utf8").trim() === hash && existsSync(join(target, "node_modules", CORE, "package.json"));
  } catch {
    return false;
  }
}

/**
 * Installs the engine into `<folder>/stryker` unless it is there already (prepare --tools). Resolves to
 * { dir, version, installed } (installed: whether this call installed it). Throws when npm fails.
 */
export async function installEngine(folder, { log, deadline = Infinity, manifest = MANIFEST } = {}) {
  const root = resolve(folder);
  const target = join(root, "stryker");
  const hash = manifestHash(manifest);
  if (installedAt(target, hash)) {
    log?.info("mutation engine already installed", { dir: target });
    return { dir: target, version: engineVersion(target), installed: false };
  }
  mkdirSync(root, { recursive: true });
  const npm = which("npm");
  if (npm === null) throw new Error("npm is needed to install the mutation engine (StrykerJS), but it is not on the PATH");
  const temporary = mkdtempSync(join(root, ".stryker-"));
  try {
    for (const name of FILES) copyFileSync(join(manifest, name), join(temporary, name));
    const args = ["ci", "--omit=dev", "--ignore-scripts", "--legacy-peer-deps", "--no-audit", "--no-fund"];
    const started = Date.now();
    const result = await run(npm, args, {
      cwd: temporary,
      deadline,
      env: { ...process.env, CI: "1", NO_UPDATE_NOTIFIER: "1", npm_config_update_notifier: "false", FORCE_COLOR: "0" },
    });
    log?.info("mutation engine installed", { dir: target, exit: result.code, elapsed: `${Date.now() - started}ms` });
    if (result.timedOut) throw new Error("npm did not finish installing the mutation engine (StrykerJS) in time");
    if (result.error) throw new Error(`npm could not run: ${result.error.message}`);
    if (result.code !== 0) throw new Error(`npm ${args.join(" ")} failed installing the mutation engine:\n${keepEnd((result.stderr || result.stdout).trim(), 4096)}`);
    writeFileSync(join(temporary, MARKER), `${hash}\n`);
    for (let attempt = 0; attempt < 3; attempt++) {
      if (installedAt(target, hash)) {
        // Another prepare got there first.
        return { dir: target, version: engineVersion(target), installed: false };
      }
      if (existsSync(target)) {
        // An install of another manifest: moved aside, then removed.
        const old = `${temporary}-old`;
        try {
          renameSync(target, old);
          rmSync(old, { recursive: true, force: true });
        } catch {
          // Moved by another prepare meanwhile.
        }
      }
      try {
        renameSync(temporary, target);
        return { dir: target, version: engineVersion(target), installed: true };
      } catch (error) {
        if (!["EEXIST", "ENOTEMPTY", "EPERM"].includes(error.code)) throw error;
      }
    }
    throw new Error(`the mutation engine could not be put in place at ${target}`);
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

/** The installed version of StrykerJS in an install folder, or null. */
export function engineVersion(target) {
  return packageVersion(join(target, "node_modules", CORE));
}

/**
 * The engine `mutate` runs: { dir (real path of the install), version, bin (Stryker's command line script),
 * instrumenter (the file to import) }, or { missing } saying why there is none.
 */
export function findEngine(folder) {
  if (!folder) return { missing: "no --tools folder was given, so the mutation engine (StrykerJS) cannot be found; run prepare --tools <folder> first" };
  const target = join(resolve(folder), "stryker");
  if (!installedAt(target, manifestHash())) {
    return { missing: `the mutation engine (StrykerJS) is not installed in ${target}, or is not the version this analyzer pins; run prepare --tools ${folder} first` };
  }
  const dir = realpathSync(target);
  const require = createRequire(join(dir, "package.json"));
  const core = join(dir, "node_modules", CORE);
  return {
    dir,
    version: engineVersion(dir),
    bin: join(core, "bin", "stryker.js"),
    instrumenter: require.resolve("@stryker-mutator/instrumenter"),
  };
}
