// Builds the TypeScript programs (and so type checkers) that cover the
// source files of a tree: one per tsconfig.json / jsconfig.json found in it
// (project references followed), and one more, with allowJs, for the files
// no configuration includes. Configurations are read as data (JSON with
// comments, `extends` followed); nothing of the project is executed.
//
// The tree usually has no node_modules (the core analyses a copy of a
// commit), so imports of dependencies do not resolve and their values have
// no types; imports of the packages of a workspace are resolved to their
// sources instead, from their package.json files.

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

import ts from "typescript";

import { readJson, walkedFolder } from "../files.js";

const CONFIG_NAMES = ["tsconfig.json", "jsconfig.json"];

/** Options for the files no configuration includes: any JavaScript or TypeScript, ESM or CommonJS. */
const FALLBACK_OPTIONS = {
  allowJs: true,
  checkJs: false,
  noEmit: true,
  target: ts.ScriptTarget.ESNext,
  module: ts.ModuleKind.Preserve,
  moduleResolution: ts.ModuleResolutionKind.Bundler,
  jsx: ts.JsxEmit.Preserve,
  esModuleInterop: true,
  allowSyntheticDefaultImports: true,
  resolveJsonModule: true,
  allowImportingTsExtensions: true,
  skipLibCheck: true,
  experimentalDecorators: true,
  strict: false,
};

/** Overrides for every program: nothing is emitted or built, and options TypeScript 6 deprecates still apply. */
const OVERRIDES = {
  noEmit: true,
  emitDeclarationOnly: false,
  composite: false,
  incremental: false,
  tsBuildInfoFile: undefined,
  skipLibCheck: true,
  ignoreDeprecations: "6.0",
  plugins: undefined,
};

/** The tsconfig.json and jsconfig.json files of the tree, the shallowest first. */
export function findConfigs(root) {
  const out = [];
  const walk = (dir, depth) => {
    let entries;
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const name of CONFIG_NAMES) {
      if (entries.some((e) => e.isFile() && e.name === name)) out.push({ file: join(dir, name), depth });
    }
    for (const entry of entries) {
      if (entry.isDirectory() && walkedFolder(entry.name)) walk(join(dir, entry.name), depth + 1);
    }
  };
  walk(root, 0);
  out.sort((a, b) => a.depth - b.depth || (a.file < b.file ? -1 : 1));
  return out.map((c) => c.file);
}

/** The packages of the tree by name, from their package.json files: name -> folder. */
export function workspacePackages(root) {
  const packages = new Map();
  const walk = (dir, depth) => {
    if (depth > 6) return;
    let entries;
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    if (entries.some((e) => e.isFile() && e.name === "package.json")) {
      const pkg = readJson(join(dir, "package.json"));
      // Unnamed packages are kept for their entry points, under a key no package name can have.
      const key = typeof pkg?.name === "string" ? pkg.name : `#${dir}`;
      if (pkg && typeof pkg === "object" && !packages.has(key)) packages.set(key, { dir, pkg });
    }
    for (const entry of entries) {
      if (entry.isDirectory() && walkedFolder(entry.name)) walk(join(dir, entry.name), depth + 1);
    }
  };
  walk(root, 0);
  return packages;
}

const SOURCE_GUESSES = ["src/index", "index", "lib/index", "source/index"];
const RESOLVABLE = [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"];

function existingFile(base) {
  if (isFile(base) && RESOLVABLE.some((ext) => base.endsWith(ext))) return base;
  const stem = base.replace(/\.(d\.ts|[cm]?[jt]sx?)$/, "");
  for (const ext of RESOLVABLE) if (isFile(stem + ext)) return stem + ext;
  for (const ext of RESOLVABLE) if (isFile(join(base, `index${ext}`))) return join(base, `index${ext}`);
  return null;
}

function isFile(path) {
  try {
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

/** The source file a built path of a package (dist/index.js) most likely comes from (src/index.ts). */
export function sourceOf(dir, built) {
  const rel = built.replace(/^\.\//, "");
  const direct = existingFile(join(dir, rel));
  if (direct) return direct;
  const parts = rel.split("/");
  if (["dist", "build", "lib", "out", "esm", "cjs", "types"].includes(parts[0])) {
    let rest = parts.slice(1);
    if (["esm", "cjs", "src"].includes(rest[0])) rest = rest.slice(1);
    for (const prefix of ["src", "lib", "source", ""]) {
      const found = existingFile(join(dir, prefix, ...rest));
      if (found) return found;
    }
  }
  return null;
}

/** Every string a package.json `exports` value maps to, with its subpath. */
export function exportTargets(exportsField, subpath = ".") {
  const out = [];
  const add = (value, path) => {
    if (typeof value === "string") out.push({ subpath: path, target: value });
    else if (Array.isArray(value)) for (const v of value) add(v, path);
    else if (value && typeof value === "object") {
      for (const [key, v] of Object.entries(value)) add(v, key.startsWith(".") ? key : path);
    }
  };
  add(exportsField, subpath);
  return out;
}

/** Resolves `specifier` (a workspace package or one of its subpaths) to a source file of the tree, or null. */
function resolveWorkspace(specifier, packages) {
  let name = specifier;
  let sub = "";
  const parts = specifier.split("/");
  if (specifier.startsWith("@") && parts.length >= 2) {
    name = parts.slice(0, 2).join("/");
    sub = parts.slice(2).join("/");
  } else {
    name = parts[0];
    sub = parts.slice(1).join("/");
  }
  const found = packages.get(name);
  if (!found) return null;
  const { dir, pkg } = found;
  if (sub === "") {
    const targets = [];
    if (pkg.exports !== undefined) {
      for (const t of exportTargets(pkg.exports)) if (t.subpath === ".") targets.push(t.target);
    }
    for (const field of ["source", "types", "typings", "module", "main"]) if (typeof pkg[field] === "string") targets.push(pkg[field]);
    for (const target of targets) {
      const file = sourceOf(dir, target);
      if (file) return file;
    }
    for (const guess of SOURCE_GUESSES) {
      const file = existingFile(join(dir, guess));
      if (file) return file;
    }
    return null;
  }
  if (pkg.exports !== undefined) {
    for (const t of exportTargets(pkg.exports)) {
      if (t.subpath === `./${sub}`) {
        const file = sourceOf(dir, t.target);
        if (file) return file;
      }
    }
  }
  return existingFile(join(dir, "src", sub)) ?? existingFile(join(dir, sub));
}

function extensionOf(file) {
  if (file.endsWith(".d.ts")) return ts.Extension.Dts;
  for (const ext of [".tsx", ".ts", ".mts", ".cts", ".jsx", ".js", ".mjs", ".cjs"]) if (file.endsWith(ext)) return ext;
  return ts.Extension.Js;
}

/**
 * A compiler host shared by every program of a run: source files are parsed once (with parent pointers) and
 * reused by the programs that include them, and module resolution falls back to the workspace's packages.
 */
function createHost(packages) {
  const base = ts.createCompilerHost(FALLBACK_OPTIONS, true);
  const cache = new Map();
  const host = {
    ...base,
    getSourceFile(fileName, languageVersionOrOptions, onError) {
      const settings =
        typeof languageVersionOrOptions === "object" ? languageVersionOrOptions : { languageVersion: languageVersionOrOptions };
      const key = `${fileName}|${settings.languageVersion}|${settings.impliedNodeFormat ?? ""}`;
      if (cache.has(key)) return cache.get(key);
      let text;
      try {
        text = readFileSync(fileName, "utf8");
      } catch (error) {
        onError?.(error.message);
        cache.set(key, undefined);
        return undefined;
      }
      const sf = ts.createSourceFile(fileName, text, languageVersionOrOptions, true);
      cache.set(key, sf);
      return sf;
    },
    writeFile() {},
  };
  host.resolveModuleNameLiterals = (literals, containingFile, redirectedReference, options) =>
    literals.map((literal) => {
      const specifier = literal.text;
      const resolved = ts.resolveModuleName(specifier, containingFile, options, host, undefined, redirectedReference);
      if (resolved.resolvedModule) return resolved;
      if (!specifier.startsWith(".") && !specifier.startsWith("/")) {
        const file = resolveWorkspace(specifier, packages);
        if (file) {
          return { resolvedModule: { resolvedFileName: file, extension: extensionOf(file), isExternalLibraryImport: false } };
        }
      }
      return resolved;
    });
  return host;
}

/**
 * The programs covering `files` (absolute paths of the tree's source files), and for each file the program
 * that analyses it. Problems reading a configuration are returned as warnings; the files it would have covered
 * fall to the fallback program.
 */
export function buildPrograms(root, files, { log, deadline = Infinity } = {}) {
  const packages = workspacePackages(root);
  const host = createHost(packages);
  const warnings = [];
  const pending = new Set(files);
  const owner = new Map();
  const programs = [];
  const queue = findConfigs(root);
  const seen = new Set();
  const parseHost = {
    useCaseSensitiveFileNames: ts.sys.useCaseSensitiveFileNames,
    readDirectory: ts.sys.readDirectory,
    fileExists: ts.sys.fileExists,
    readFile: ts.sys.readFile,
    getCurrentDirectory: () => root,
    onUnRecoverableConfigFileDiagnostic: () => {},
  };
  while (queue.length > 0 && pending.size > 0) {
    if (Date.now() > deadline) break;
    const config = resolve(queue.shift());
    if (seen.has(config)) continue;
    seen.add(config);
    let parsed;
    try {
      parsed = ts.getParsedCommandLineOfConfigFile(config, { ignoreDeprecations: "6.0" }, parseHost);
    } catch (error) {
      warnings.push({ file: config, message: `could not read ${config}: ${error.message}` });
      continue;
    }
    if (!parsed) continue;
    const configErrors = parsed.errors.filter(
      (d) => d.category === ts.DiagnosticCategory.Error && d.code !== 18003 && d.code !== 5101 && d.code !== 5107,
    );
    if (configErrors.length > 0) {
      warnings.push({ file: config, message: ts.flattenDiagnosticMessageText(configErrors[0].messageText, " ") });
    }
    for (const ref of parsed.projectReferences ?? []) {
      const path = resolve(dirname(config), ref.path);
      queue.push(existsSync(path) && statSync(path).isDirectory() ? join(path, "tsconfig.json") : path);
    }
    const rootNames = parsed.fileNames.filter((f) => pending.has(resolve(f)) || /\.d\.[cm]?ts$/.test(f));
    if (!rootNames.some((f) => pending.has(resolve(f)))) continue;
    const options = { ...parsed.options, ...OVERRIDES };
    if (config.endsWith("jsconfig.json")) options.allowJs = true;
    const program = ts.createProgram({ rootNames, options, host });
    programs.push(program);
    for (const file of [...pending]) {
      if (program.getSourceFile(file)) {
        owner.set(file, program);
        pending.delete(file);
      }
    }
    log?.debug("program built", { config, files: rootNames.length });
  }
  if (pending.size > 0) {
    const program = ts.createProgram({ rootNames: [...pending], options: { ...FALLBACK_OPTIONS }, host });
    programs.push(program);
    for (const file of pending) if (program.getSourceFile(file)) owner.set(file, program);
    log?.debug("program built", { config: null, files: pending.size });
  }
  return { programs, owner, warnings, packages };
}
