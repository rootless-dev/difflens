// The test command: runs the project's tests once with the project's own
// runner (its node_modules, never the analyzer's), and reports them
// (tests.schema.json). See units.js for how runners are found, collect.js
// for how results are named, and coverage.js for coverage.
//
// Per runner (each through a reporter of ./reporters that streams results):
//
//   vitest   vitest run --reporter=<ours> [--coverage.enabled --coverage.provider=v8|istanbul
//            --coverage.reporter=json] [files] [-t pattern]
//   jest     jest --ci --watchman=false --reporters=<ours> --testLocationInResults
//            [--coverage --coverageReporters=json] [file patterns] [-t pattern]
//   node     node [flags of the test script] --test --test-reporter=<ours>
//            [--experimental-test-coverage] [files or the script's patterns] [--test-name-pattern=...]
//   mocha    [c8|nyc --reporter=json] mocha --reporter <ours> --exit [files] [--grep pattern]
//
// Coverage is measured with the runner's own provider: @vitest/coverage-v8
// or @vitest/coverage-istanbul for Vitest (unavailable when neither is
// installed), Jest's (always there), node's built-in coverage, and c8 or
// nyc for Mocha (unavailable without them). A selection runs the test
// files of the packages it names (a package is a folder), and for a package
// with only some tests, those tests by name pattern (the titles joined by
// spaces, as the runners match them); a subtest selects its first-level
// ancestor, and for node --test the whole top-level test. Every test and
// package carries the command that runs it again on its own (commands.js).
//
// --cache-dir is not used (see cli.js): Jest's cache folders are named after
// a hash of its absolute rootDir, so a copy at another path never reuses
// them, and the other runners keep nothing worth reusing.

import { mkdirSync, mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import { Tree, containerOf, rootContainerName, sourceFiles } from "../files.js";
import { keepEnd, packageBin, packageDir, packageVersion, run } from "../process.js";
import { EventFile, MAX_OUTPUT, Session, nameTitles } from "./collect.js";
import { namePattern, packageCommand, testCommand } from "./commands.js";
import { Coverage } from "./coverage.js";
import { findUnits, nodeRunsByDefault } from "./units.js";

export { namePattern };

const REPORTERS = join(dirname(fileURLToPath(import.meta.url)), "reporters");
const POLL = 200;

const regexEscape = (s) => s.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");

/**
 * The commands of the tests and packages of `units` (commands.js), as the Session asks for them: a test by its
 * file (absolute) and titles, a package by its container.
 */
export function unitCommands(units, tree, rootName) {
  const unitOf = new Map();
  const byContainer = new Map();
  for (const unit of units) {
    for (const file of unit.testFiles) {
      unitOf.set(file, unit);
      const c = containerOf(tree.rel(file), rootName);
      const entry = byContainer.get(c) ?? { unit, files: [] };
      if (entry.unit === unit) entry.files.push(relative(unit.dir, file));
      byContainer.set(c, entry);
    }
  }
  return {
    test(file, path, leaf) {
      const unit = unitOf.get(file);
      return unit ? testCommand(unit, relative(unit.dir, file), path, leaf) : null;
    },
    package(container) {
      const entry = byContainer.get(container);
      return entry ? packageCommand(entry.unit, entry.files) : null;
    },
  };
}

/** The title prefixes a package's selected tests stand for: a test whole, or each first-level subtest. */
export function selectedPrefixes(tests, wholeOnly = false) {
  const out = [];
  const seen = new Set();
  const add = (p) => {
    const key = JSON.stringify(p);
    if (!seen.has(key)) {
      seen.add(key);
      out.push(p);
    }
  };
  const whole = new Set(tests.filter((t) => t.subtests === null).map((t) => t.name));
  for (const t of tests) {
    const top = nameTitles(t.name);
    if (t.subtests === null || wholeOnly) {
      if (t.subtests === null || t.subtests.length > 0) add(top);
      continue;
    }
    if (whole.has(t.name)) continue;
    for (const sub of t.subtests) {
      const first = nameTitles(sub)[0];
      if (first !== "") add([...top, first]);
    }
  }
  return out;
}

/** Whether a test's titles fall under one of the prefixes. */
function under(prefixes) {
  return (path) => prefixes.some((p) => p.length <= path.length && p.every((title, i) => path[i] === title));
}

/** Runs the tests of the tree at `dir`. Resolves to the fields of the test document. */
export async function runTests(dir, { log, deadline, cover, selection, progress }) {
  const tree = new Tree(dir);
  const rootName = rootContainerName(tree.root);
  const files = sourceFiles(tree.root);
  const containers = new Set(files.map((rel) => containerOf(rel, rootName)));
  const diagnostics = [];
  const coverage = new Coverage(tree);
  const scratch = mkdtempSync(join(tmpdir(), "difflens-test-"));
  if (!cover) diagnostics.push("coverage was not measured: --no-cover was given");
  try {
    const { units, orphans } = findUnits(tree.root, files);
    const session = new Session({ tree, rootName, progress, commands: unitCommands(units, tree, rootName) });
    if (orphans.length > 0) {
      const rels = orphans.map((f) => tree.rel(f));
      diagnostics.push(
        `${orphans.length} test file(s) have no test runner (none is named by a test script, configured or a dependency), so they were not run: ${rels.slice(0, 5).join(", ")}${rels.length > 5 ? ", ..." : ""}`,
      );
    }
    if (units.length === 0 && orphans.length === 0) diagnostics.push("the project has no tests: no test runner and no test files were found");

    const empty = [];
    const plans = plan(units, selection, tree, rootName, containers, diagnostics, empty);
    // Packages reported: every folder of the project for a whole run; for a partial one, those it ran.
    const reported = new Set(selection === null ? containers : empty);
    for (const p of plans) session.expect(p.files ?? p.unit.testFiles);
    session.expectEmpty(reported);
    session.report(true);

    let stopped = false;
    let coverageUnavailable = [];
    for (const [n, p] of plans.entries()) {
      if (Date.now() > deadline) {
        diagnostics.push(`the tests of ${p.unit.rel} were not run: the time ran out`);
        stopped = true;
        continue;
      }
      const runner = runnerCommand(p, { tree, scratch: join(scratch, String(n)), cover });
      if (runner.missing) {
        diagnostics.push(`the tests of ${p.unit.rel} were not run: ${runner.missing}`);
        continue;
      }
      if (runner.coverage === null && cover) coverageUnavailable.push(`${p.unit.rel}: ${runner.coverageMissing}`);
      for (const c of p.containers) reported.add(c);
      const events = new EventFile(runner.events);
      const filter = p.prefixes ? under(p.prefixes) : null;
      const drain = () => {
        for (const event of events.read()) session.handle(event, filter);
      };
      const timer = setInterval(drain, POLL);
      const started = Date.now();
      const result = await run(runner.command, runner.args, {
        cwd: p.unit.dir,
        deadline,
        env: runner.env,
        maxStdout: 1 << 20,
        maxStderr: MAX_OUTPUT,
      });
      clearInterval(timer);
      drain();
      log.info("tests run", {
        dir: p.unit.rel,
        runner: p.unit.runner,
        version: runner.version,
        exit: result.code,
        signal: result.signal,
        elapsed: `${Date.now() - started}ms`,
        files: p.files?.length ?? "all",
      });
      const before = session.order.length;
      session.finishInvocation(result.timedOut);
      if (result.timedOut) {
        stopped = true;
        diagnostics.push(`the tests of ${p.unit.rel} were stopped: the time ran out`);
      }
      if (result.error) diagnostics.push(`${p.unit.runner} could not run in ${p.unit.rel}: ${result.error.message}`);
      const output = keepEnd(`${result.stderr}\n${result.stdout}`.trim(), MAX_OUTPUT);
      if (before === 0 && session.order.length === 0 && result.code !== 0 && !result.timedOut) {
        // The runner failed before any file: its configuration, or (Mocha) a file that does not load.
        diagnostics.push(`${p.unit.runner} failed in ${p.unit.rel} before running any test: ${keepEnd(output, 4096)}`);
        for (const file of p.files ?? p.unit.testFiles) {
          session.handle({ e: "end", file, error: output || `${p.unit.runner} exited with ${result.code}`, ms: 0 }, filter);
        }
      }
      for (const error of session.fatal.splice(0)) diagnostics.push(`${p.unit.runner} in ${p.unit.rel}: ${keepEnd(error, 4096)}`);
      if (runner.coverage && !result.timedOut) {
        if (runner.coverage.kind === "istanbul") {
          const file = join(runner.coverage.dir, "coverage-final.json");
          if (!coverage.addIstanbulFile(file)) diagnostics.push(`coverage of ${p.unit.rel} was not measured: ${p.unit.runner} wrote no coverage report`);
        }
      }
    }
    if (cover && session.coverageFiles.length > 0) coverage.addLines(session.coverageFiles);
    const { tests, packages } = session.results(reported);
    let coverageResult = null;
    if (cover) {
      if (stopped) diagnostics.push("the run was stopped by the time limit: results are partial and coverage was not measured");
      else if (coverage.measured) coverageResult = coverage.result();
      else if (coverageUnavailable.length === 0 && plans.length > 0) diagnostics.push("coverage was not measured: the test runner wrote no coverage report");
      for (const why of coverageUnavailable) diagnostics.push(`coverage was not measured for ${why}`);
      if (plans.length === 0 && cover) diagnostics.push("coverage was not measured: no tests ran");
    }
    return { tests, packages, coverage: coverageResult, diagnostics };
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

/**
 * The invocations of a run: per unit, everything (no selection); with a selection, one invocation of a unit
 * for the packages selected whole, and one per package with only some tests.
 */
function plan(units, selection, tree, rootName, containers, diagnostics, empty) {
  if (selection === null) {
    return units.map((unit) => ({
      unit,
      files: null,
      prefixes: null,
      containers: [...new Set(unit.testFiles.map((f) => containerOf(tree.rel(f), rootName)))],
    }));
  }
  const plans = [];
  const byContainer = new Map();
  for (const unit of units) {
    for (const file of unit.testFiles) {
      const c = containerOf(tree.rel(file), rootName);
      const list = byContainer.get(c) ?? [];
      list.push({ unit, file });
      byContainer.set(c, list);
    }
  }
  const whole = new Map();
  for (const selected of selection.packages) {
    const entries = byContainer.get(selected.package);
    if (!entries) {
      // A folder of the project without tests is reported as such; anything else was not found.
      if (containers.has(selected.package)) empty.push(selected.package);
      else diagnostics.push(`the package ${selected.package} was not found, so its tests were not run`);
      continue;
    }
    for (const { unit, file } of entries) {
      if (selected.tests === null) {
        const w = whole.get(unit) ?? { unit, files: [], prefixes: null, containers: new Set() };
        w.files.push(file);
        w.containers.add(selected.package);
        whole.set(unit, w);
      }
    }
    if (selected.tests !== null) {
      const byUnit = new Map();
      for (const { unit, file } of entries) {
        const list = byUnit.get(unit) ?? [];
        list.push(file);
        byUnit.set(unit, list);
      }
      for (const [unit, files] of byUnit) {
        const prefixes = selectedPrefixes(selected.tests, unit.runner === "node");
        if (prefixes.length === 0) continue;
        plans.push({ unit, files, prefixes, containers: [selected.package] });
      }
    }
  }
  return [...[...whole.values()].map((w) => ({ ...w, containers: [...w.containers] })), ...plans];
}

/**
 * The files or patterns node --test runs for a unit: those of its test script, a folder given there replaced
 * by the test files in it (Node 21 and later take globs and files, not folders); none for node's defaults.
 */
function nodePatterns(unit, tree) {
  const typescript = Number(process.versions.node.split(".")[0]) >= 23;
  const out = [];
  for (const pattern of unit.patterns) {
    const abs = join(unit.dir, pattern);
    let folder;
    try {
      folder = statSync(abs).isDirectory();
    } catch {
      folder = false;
    }
    if (!folder) {
      out.push(pattern);
      continue;
    }
    for (const file of unit.testFiles) {
      if (file.startsWith(`${abs.replace(/\/+$/, "")}/`) && nodeRunsByDefault(tree.rel(file), typescript)) out.push(relative(unit.dir, file));
    }
  }
  return out;
}

/** The command that runs an invocation: { command, args, env, events, coverage, version } or { missing }. */
function runnerCommand(p, { tree, scratch, cover }) {
  const { unit } = p;
  mkdirSync(scratch, { recursive: true });
  const events = join(scratch, "events.ndjson");
  const env = {
    ...process.env,
    DIFFLENS_TEST_EVENTS: events,
    NO_COLOR: "1",
    FORCE_COLOR: "0",
    NO_UPDATE_NOTIFIER: "1",
    npm_config_offline: "true",
  };
  delete env.NODE_TEST_CONTEXT;
  const covDir = join(scratch, "coverage");
  const relFiles = (p.files ?? []).map((f) => relative(unit.dir, f));
  const pattern = p.prefixes ? namePattern(p.prefixes) : null;
  const find = (name, bin = name) => {
    const dir = packageDir(name, unit.dir, tree.root);
    return dir ? { bin: packageBin(dir, bin), version: packageVersion(dir) } : null;
  };
  switch (unit.runner) {
    case "vitest": {
      const vitest = find("vitest");
      if (!vitest?.bin) return { missing: "vitest is not installed (node_modules/vitest)" };
      const args = [vitest.bin, "run", `--reporter=${join(REPORTERS, "vitest.mjs")}`, "--passWithNoTests", "--watch=false"];
      let coverage = null;
      let coverageMissing = null;
      if (cover) {
        const provider = ["v8", "istanbul"].find((name) => packageDir(`@vitest/coverage-${name}`, unit.dir, tree.root));
        if (provider) {
          // reportOnFailure: Vitest writes no coverage when a test fails, unless asked to.
          args.push(
            "--coverage.enabled=true",
            `--coverage.provider=${provider}`,
            "--coverage.reporter=json",
            `--coverage.reportsDirectory=${covDir}`,
            "--coverage.clean=true",
            "--coverage.reportOnFailure=true",
          );
          coverage = { kind: "istanbul", dir: covDir };
        } else {
          args.push("--coverage.enabled=false");
          coverageMissing = "neither @vitest/coverage-v8 nor @vitest/coverage-istanbul is installed";
        }
      } else args.push("--coverage.enabled=false");
      args.push(...relFiles);
      if (pattern) args.push("-t", pattern);
      return { command: process.execPath, args, env, events, coverage, coverageMissing, version: vitest.version };
    }
    case "jest": {
      const jest = find("jest") ?? find("jest-cli", "jest");
      if (!jest?.bin) return { missing: "jest is not installed (node_modules/jest)" };
      const args = [jest.bin, "--ci", "--watchman=false", `--reporters=${join(REPORTERS, "jest.cjs")}`, "--passWithNoTests", "--testLocationInResults"];
      let coverage = null;
      if (cover) {
        args.push("--coverage", "--coverageReporters=json", `--coverageDirectory=${covDir}`);
        coverage = { kind: "istanbul", dir: covDir };
      } else args.push("--coverage=false");
      for (const f of p.files ?? []) args.push(`${regexEscape(f)}$`);
      if (pattern) args.push("-t", pattern);
      return { command: process.execPath, args, env, events, coverage, version: jest.version };
    }
    case "mocha": {
      const mocha = find("mocha");
      if (!mocha?.bin) return { missing: "mocha is not installed (node_modules/mocha)" };
      const mochaArgs = [mocha.bin, "--reporter", join(REPORTERS, "mocha.cjs"), "--exit", ...relFiles];
      if (pattern) mochaArgs.push("--grep", pattern);
      if (cover) {
        const c8 = find("c8");
        const nyc = c8?.bin ? null : find("nyc");
        if (c8?.bin) {
          const args = [c8.bin, "--reporter=json", `--reports-dir=${covDir}`, `--temp-directory=${join(scratch, "c8")}`, process.execPath, ...mochaArgs];
          return { command: process.execPath, args, env, events, coverage: { kind: "istanbul", dir: covDir }, version: mocha.version };
        }
        if (nyc?.bin) {
          const args = [nyc.bin, "--reporter=json", `--report-dir=${covDir}`, `--temp-dir=${join(scratch, "nyc")}`, "--silent", process.execPath, ...mochaArgs];
          return { command: process.execPath, args, env, events, coverage: { kind: "istanbul", dir: covDir }, version: mocha.version };
        }
        return { command: process.execPath, args: mochaArgs, env, events, coverage: null, coverageMissing: "Mocha measures no coverage and neither c8 nor nyc is installed", version: mocha.version };
      }
      return { command: process.execPath, args: mochaArgs, env, events, coverage: null, version: mocha.version };
    }
    case "node": {
      const args = [...unit.flags, "--test", `--test-reporter=${join(REPORTERS, "node.mjs")}`, "--test-reporter-destination=stdout"];
      let coverage = null;
      let coverageMissing = null;
      if (cover) {
        if (Number(process.versions.node.split(".")[0]) >= 20) {
          args.push("--experimental-test-coverage");
          coverage = { kind: "events" };
        } else coverageMissing = `node ${process.version} has no test coverage`;
      }
      if (pattern) args.push(`--test-name-pattern=${pattern}`);
      if (p.files) args.push(...relFiles);
      else args.push(...nodePatterns(unit, tree));
      return { command: process.execPath, args, env, events, coverage, coverageMissing, version: process.version };
    }
    default:
      return { missing: `${unit.runner} is not a test runner this analyzer runs` };
  }
}
