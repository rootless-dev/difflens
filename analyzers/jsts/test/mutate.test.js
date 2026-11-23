import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { before, describe, it } from "node:test";

import { parseOptions } from "../src/cli.js";
import { TestNames, testTitles } from "../src/mutate/names.js";
import { binaryOperator, operatorOf, statusOf } from "../src/mutate/operators.js";
import { candidateLines, codePointColumn, coveredRanges, escapeGlob, excerpt, lineStarts, matchKey, mutantId, mutatePatterns, sample, uniqueIds } from "../src/mutate/plan.js";
import { filePoints } from "../src/mutate/points.js";
import { failureOf, nodeTestCommand, strykerOptions } from "../src/mutate/stryker.js";
import { manifestHash } from "../src/mutate/tools.js";
import { Tree } from "../src/files.js";
import { cli, copy, installed } from "./helpers.js";
import { protocolSchema, validate } from "./schema.js";

const schema = protocolSchema("mutation.schema.json");

describe("mutation candidates", () => {
  it("are the added lines a covered block spans", () => {
    const covered = coveredRanges([
      { file: "a.ts", startLine: 1, endLine: 4, statements: 2, count: 3 },
      { file: "a.ts", startLine: 6, endLine: 9, statements: 2, count: 0 },
      { file: "b.ts", startLine: 1, endLine: 9, statements: 2, count: 0 },
    ]);
    const added = new Map([
      ["a.ts", [[3, 7]]],
      ["b.ts", [[1, 2]]],
      ["c.ts", [[1, 2]]],
    ]);
    const lines = candidateLines(added, covered);
    assert.deepEqual([...lines.keys()], ["a.ts"]);
    assert.deepEqual([...lines.get("a.ts")], [3, 4]);
  });

  it("count columns in Unicode characters, with every line break JavaScript knows", () => {
    assert.deepEqual(lineStarts(`a\r\nb\nc\rd${String.fromCharCode(0x2028)}e`), [0, 3, 5, 7, 9]);
    const text = 'x = "😀" + y';
    // y is UTF-16 column 11 (0-based), the 11th character: column 11 (1-based).
    assert.equal(codePointColumn(text, 0, 11), 11);
    assert.equal(codePointColumn(text, 0, 0), 1);
    assert.equal(excerpt("a >= b"), "a >= b");
    assert.equal(excerpt("{\n  x();\n}"), "{...");
    assert.equal(excerpt("x".repeat(70)), `${"x".repeat(57)}...`);
  });

  it("get stable ids from their function and their place in it", () => {
    const base = { file: "a.ts", functionId: "a.f", functionLine: 10, mutator: "EqualityOperator", original: "a >= b", mutated: "a > b", line: 12, column: 5 };
    assert.equal(mutantId(base), mutantId({ ...base }));
    assert.equal(mutantId(base), mutantId({ ...base, functionLine: 20, line: 22 }), "the function moved: same id");
    assert.notEqual(mutantId(base), mutantId({ ...base, mutated: "a < b" }));
    assert.notEqual(mutantId(base), mutantId({ ...base, line: 13 }));
    const twins = [{ id: "x" }, { id: "x" }, { id: "y" }];
    uniqueIds(twins);
    assert.deepEqual(twins.map((m) => m.id), ["x", "x-2", "y"]);
  });

  it("are sampled round-robin across functions, in source order, the same every time", () => {
    const m = (file, functionId, functionLine, line, column = 1) => ({ file, functionId, functionLine, line, column, endLine: line, endColumn: column + 1, mutator: "M", mutated: "x" });
    const all = [
      m("b.ts", "b.g", 1, 2),
      m("a.ts", "a.f", 5, 6),
      m("a.ts", "a.f", 5, 7),
      m("a.ts", "a.f", 5, 8),
      m("a.ts", null, null, 1),
      m("a.ts", null, null, 20),
      m("a.ts", "a.h", 10, 11),
      m("b.ts", "b.g", 1, 3),
    ];
    const none = sample(all, 8);
    assert.equal(none.sampled, false);
    assert.deepEqual(none.selected.map((x) => `${x.file}:${x.line}`), ["a.ts:1", "a.ts:6", "a.ts:7", "a.ts:8", "a.ts:11", "a.ts:20", "b.ts:2", "b.ts:3"]);
    const some = sample(all, 5);
    assert.equal(some.sampled, true);
    // Groups: a.ts top level (line 1), a.f (5), a.h (10), b.g; round one takes the first of each, round two
    // a.ts:20 (top level) first.
    assert.deepEqual(some.selected.map((x) => `${x.file}:${x.line}`), ["a.ts:1", "a.ts:6", "a.ts:11", "a.ts:20", "b.ts:2"]);
    assert.deepEqual(sample([...all].reverse(), 5).selected, some.selected);
  });

  it("become StrykerJS mutate ranges, file names read as they are", () => {
    assert.equal(escapeGlob("src/[id]/page (1).ts"), "src/[[]id]/page [(]1[)].ts");
    const patterns = mutatePatterns(
      [
        { file: "pkg/src/a.ts", start: { line: 4, column: 9 }, end: { line: 4, column: 26 } },
        { file: "pkg/src/a.ts", start: { line: 4, column: 9 }, end: { line: 4, column: 26 } },
        { file: "pkg/src/b.ts", start: { line: 2, column: 0 }, end: { line: 3, column: 1 } },
      ],
      (file) => file.slice("pkg/".length),
    );
    assert.deepEqual(patterns, ["src/a.ts:4:9-4:26", "src/b.ts:2:0-3:1"]);
    assert.equal(matchKey("a", "M", "x", { line: 1, column: 2 }, { line: 1, column: 3 }), ["a", "M", "x", 1, 2, 1, 3].join("\0"));
  });
});

describe("mutation operators and statuses", () => {
  it("read the operator of an expression", () => {
    assert.equal(binaryOperator("a >= b"), ">=");
    assert.equal(binaryOperator("total -= i"), "-=");
    assert.equal(binaryOperator("(x.y === z)"), "===");
    assert.equal(binaryOperator("f(x)"), null);
  });

  it("map StrykerJS's mutators to the schema's families", () => {
    assert.equal(operatorOf("EqualityOperator", "a >= b", "a > b"), "relational_boundary");
    assert.equal(operatorOf("EqualityOperator", "a < b", "a <= b"), "relational_boundary");
    assert.equal(operatorOf("EqualityOperator", "a >= b", "a < b"), "relational_negation");
    assert.equal(operatorOf("EqualityOperator", "a === b", "a !== b"), "relational_negation");
    assert.equal(operatorOf("ArithmeticOperator", "a + b", "a - b"), "arithmetic");
    assert.equal(operatorOf("LogicalOperator", "a && b", "a || b"), "logical");
    assert.equal(operatorOf("UpdateOperator", "i++", "i--"), "increment");
    assert.equal(operatorOf("AssignmentOperator", "t += i", "t -= i"), "increment");
    assert.equal(operatorOf("AssignmentOperator", "t *= i", "t /= i"), "arithmetic");
    assert.equal(operatorOf("AssignmentOperator", "t ??= i", "t &&= i"), "logical");
    assert.equal(operatorOf("BooleanLiteral", "!ok", "ok"), "negation_removal");
    assert.equal(operatorOf("BooleanLiteral", "true", "false"), "other");
    assert.equal(operatorOf("ConditionalExpression", "a > b", "false", "condition"), "negation_removal");
    assert.equal(operatorOf("ConditionalExpression", "a > b", "false", "return"), "return_value");
    assert.equal(operatorOf("ConditionalExpression", "a > b", "true", "return"), "other");
    assert.equal(operatorOf("ConditionalExpression", "a > b", "true"), "other");
    assert.equal(operatorOf("ArrowFunction", "() => x", "() => undefined"), "return_value");
    assert.equal(operatorOf("CallExpression", "save(x);", ";"), "call_removal");
    assert.equal(operatorOf("CallExpression", 'throw new Error("no");', ";"), "other");
    assert.equal(operatorOf("CallExpression", "f(x)", "void 0"), "other");
    assert.equal(operatorOf("MethodExpression", "s.trim()", "s"), "other");
  });

  it("map StrykerJS's statuses, leaving out the ignored", () => {
    assert.equal(statusOf("Killed"), "killed");
    assert.equal(statusOf("Survived"), "survived");
    assert.equal(statusOf("Timeout"), "timeout");
    assert.equal(statusOf("NoCoverage"), "no_coverage");
    assert.equal(statusOf("CompileError"), "error");
    assert.equal(statusOf("RuntimeError"), "error");
    assert.equal(statusOf("Ignored"), null);
    assert.equal(statusOf("Pending"), null);
  });
});

describe("mutation points of a file", () => {
  const text = [
    "const LIMIT = 10 > 5;",
    "export function check(a, b, s) {",
    "  console.log(a > b);",
    "  if (a > b) return 1;",
    '  return "😀" === s && a >= b;',
    "}",
    "",
  ].join("\n");
  const at = (mutatorName, replacement, line, start, end) => ({ mutatorName, replacement, location: { start: { line, column: start }, end: { line, column: end } } });
  const raw = [
    at("EqualityOperator", "10 >= 5", 0, 14, 20),
    at("EqualityOperator", "a >= b", 2, 14, 19),
    at("ConditionalExpression", "true", 3, 6, 11),
    at("EqualityOperator", "a > b", 4, 23, 29),
  ];

  it("leave out top-level constants and logging calls, and place the rest in their function", () => {
    const { mutants, leftOut } = filePoints("src/x.js", text, raw, new Set([1, 2, 3, 4, 5, 6]));
    assert.equal(leftOut, 2);
    assert.deepEqual(
      mutants.map((m) => [m.line, m.column, m.endLine, m.endColumn, m.original, m.mutated, m.operator, m.functionId]),
      [
        [4, 7, 4, 12, "a > b", "true", "negation_removal", "src/x.check"],
        // "😀" is two UTF-16 units and one character: UTF-16 column 23 is the 23rd character, column 23.
        [5, 23, 5, 29, "a >= b", "a > b", "relational_boundary", "src/x.check"],
      ],
    );
    assert.deepEqual(mutants[1].start, { line: 5, column: 23 });
    assert.match(mutants[1].description, /^EqualityOperator: replace a >= b with a > b in check$/);
    assert.match(mutants[0].id, /^[0-9a-f]{20}$/);
  });

  it("take only the candidate lines", () => {
    const { mutants } = filePoints("src/x.js", text, raw, new Set([5]));
    assert.deepEqual(mutants.map((m) => m.line), [5]);
  });

  it("give a mutant of a whole arrow function to the function around it", () => {
    const source = "export function sum(xs) {\n  return xs.reduce((t, x) => t + x, 0);\n}\n";
    const { mutants } = filePoints("src/s.js", source, [at("ArrowFunction", "() => undefined", 1, 19, 34), at("ArithmeticOperator", "t - x", 1, 29, 34)], new Set([2]));
    assert.deepEqual(mutants.map((m) => [m.functionId, m.operator]), [
      ["src/s.sum", "return_value"],
      ["src/s.sum$1", "arithmetic"],
    ]);
  });
});

describe("killedBy names", () => {
  const source = [
    'import { describe, expect, it, test } from "vitest";',
    'describe("account", () => {',
    '  describe("a/b", () => {',
    '    it("works", () => {});',
    "  });",
    '  it(name, () => {});',
    "});",
    'test("alone", () => {});',
    "",
  ].join("\n");

  it("are the titles around a test, from the test file's syntax", () => {
    const tests = testTitles("src/a.test.ts", source);
    assert.deepEqual(
      tests.map((t) => [t.line, t.column, t.titles]),
      [
        [1, 0, ["account"]],
        [2, 2, ["account", "a/b"]],
        [3, 4, ["account", "a/b", "works"]],
        // As analyze reads it (describe(MyClass) prints the class's name); killedBy checks it against the runner.
        [5, 2, ["account", "name"]],
        [7, 0, ["alone"]],
      ],
    );
  });

  it("are written as the test command names tests, by position or else by full name", () => {
    const dir = mkdtempSync(join(tmpdir(), "jsts-names-"));
    try {
      writeFileSync(join(dir, "a.test.ts"), source);
      const names = new TestNames(new Tree(dir));
      const file = join(dir, "a.test.ts");
      names.add([
        { id: "1", name: "account a/b works", file, line: 3, column: 4 },
        { id: "2", name: "alone", file, line: null, column: null },
        { id: "3", name: "account dynamic", file, line: 5, column: 2 },
        { id: "4", name: "nowhere", file: null, line: null, column: null },
      ]);
      assert.equal(names.nameOf("1"), "account/a∕b/works");
      assert.equal(names.nameOf("2"), "alone");
      assert.equal(names.nameOf("3"), null);
      assert.equal(names.nameOf("4"), null);
      assert.equal(names.nameOf("unknown"), null);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("StrykerJS runs", () => {
  const unit = (runner) => ({ dir: "/t", rel: ".", runner, flags: ["--import", "tsx"], testFiles: ["/t/test/a.test.js", "/t/test/b c.test.js"] });

  it("use the project's runner, perTest coverage where the plugin has it", () => {
    const options = (runner) => strykerOptions(unit(runner), ["src/a.ts:1:0-1:5"], { jobs: 3, timeoutMs: 10000, incrementalFile: "/c/stryker-incremental.json", dryRunMinutes: 9 });
    const vitest = options("vitest");
    assert.equal(vitest.testRunner, "vitest");
    assert.equal(vitest.coverageAnalysis, "perTest");
    assert.equal(vitest.plugins[0], "@stryker-mutator/vitest-runner");
    assert.match(vitest.plugins[1], /reporter\.mjs$/);
    assert.deepEqual(vitest.reporters, ["difflens-events"]);
    assert.equal(vitest.concurrency, 3);
    assert.equal(vitest.timeoutMS, 10000);
    assert.equal(vitest.timeoutFactor, 3);
    assert.equal(vitest.incremental, true);
    assert.equal(vitest.incrementalFile, "/c/stryker-incremental.json");
    assert.equal(vitest.inPlace, true);
    assert.equal(vitest.ignoreStatic, true);
    assert.deepEqual(options("mocha").mochaOptions, { spec: ["test/a.test.js", "test/b c.test.js"] });
    assert.equal(options("jest").testRunner, "jest");
    const node = options("node");
    assert.equal(node.testRunner, "command");
    assert.equal(node.coverageAnalysis, "off");
    assert.equal(node.ignoreStatic, false);
    assert.equal(node.commandRunner.command, nodeTestCommand(unit("node")));
    assert.match(node.commandRunner.command, /'--import' 'tsx' '--test' 'test\/a.test.js' 'test\/b c.test.js'$/);
    const noCache = strykerOptions(unit("vitest"), [], { jobs: 1, timeoutMs: 1, incrementalFile: null, dryRunMinutes: 1 });
    assert.equal(noCache.incremental, false);
    assert.equal(noCache.incrementalFile, undefined);
  });

  it("say why they failed with StrykerJS's errors", () => {
    const output = [
      "10:47:45 (47578) INFO DryRunExecutor Starting initial test run.",
      "\u001b[91m10:47:45 (47578) ERROR DryRunExecutor One or more tests failed in the initial test run:\u001b[39m",
      "\tStack when empty fails on purpose",
      "",
      "10:47:45 (47578) ERROR Stryker There were failed tests in the initial test run.",
      "ConfigError: There were failed tests in the initial test run.",
      "    at DryRunExecutor.validateResultCompleted (file:///x.js:65:27)",
    ].join("\n");
    assert.equal(
      failureOf(output),
      "DryRunExecutor One or more tests failed in the initial test run:\nStack when empty fails on purpose\nStryker There were failed tests in the initial test run.\nConfigError: There were failed tests in the initial test run.",
    );
    assert.equal(failureOf("just this"), "just this");
  });

  it("are pinned by a manifest whose hash names the install", () => {
    assert.match(manifestHash(), /^[0-9a-f]{64}$/);
    const pkg = JSON.parse(readFileSync(new URL("../src/mutate/engine/package.json", import.meta.url), "utf8"));
    for (const version of Object.values(pkg.dependencies)) assert.match(version, /^\d+\.\d+\.\d+$/, "exact versions only");
    const analyzer = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
    assert.deepEqual(Object.keys(analyzer.dependencies), ["typescript"], "StrykerJS is not a dependency of the analyzer");
  });
});

describe("mutate command line", () => {
  it("takes its flags", () => {
    const options = parseOptions("mutate", ["--dir", "/t", "--changed", "/c", "--coverage", "/v", "--max-mutants", "5", "--jobs", "2", "--min-mutant-timeout", "1.5s", "--tools", "/x", "--cache-dir", "/k", "--offline", "--progress", "/p"]);
    assert.equal(options.maxMutants, 5);
    assert.equal(options.jobs, 2);
    assert.equal(options.minMutantTimeoutMs, 1500);
    assert.equal(options.tools, "/x");
    assert.equal(options.offline, true);
    const defaults = parseOptions("mutate", ["--dir", "/t", "--changed", "/c", "--coverage", "/v"]);
    assert.equal(defaults.maxMutants, 200);
    assert.ok(defaults.jobs >= 1);
    assert.equal(defaults.minMutantTimeoutMs, 10000);
    assert.equal(parseOptions("prepare", ["--dir", "/t", "--tools", "/x"]).tools, "/x");
    assert.throws(() => parseOptions("mutate", ["--dir", "/t", "--changed", "/c"]), /--coverage/);
    assert.throws(() => parseOptions("mutate", ["--dir", "/t", "--changed", "/c", "--coverage", "/v", "--max-mutants", "0"]), /max-mutants/);
    assert.throws(() => parseOptions("mutate", ["--dir", "/t", "--changed", "/c", "--coverage", "/v", "--jobs", "x"]), /jobs/);
    assert.throws(() => parseOptions("mutate", ["--dir", "/t", "--changed", "/c", "--coverage", "/v", "--min-mutant-timeout", "0s"]), /min-mutant-timeout/);
    assert.throws(() => parseOptions("test", ["--dir", "/t", "--tools", "/x"]), /tools/);
  });

  it("exits 2 for input files it cannot read, 1 without the engine", () => {
    const dir = mkdtempSync(join(tmpdir(), "jsts-mutate-cli-"));
    try {
      const changed = join(dir, "changed.json");
      const coverage = join(dir, "coverage.json");
      writeFileSync(changed, JSON.stringify({ protocol: 1, files: [{ path: "a.js", added: [[1, 2]] }] }));
      writeFileSync(coverage, JSON.stringify({ protocol: 1, blocks: [{ file: "a.js", startLine: 1, endLine: 2, statements: 1, count: 1 }] }));
      const args = (...more) => ["mutate", "--dir", dir, "--changed", changed, "--coverage", coverage, ...more];
      writeFileSync(join(dir, "bad.json"), JSON.stringify({ protocol: 1, blocks: [{ file: "a.js", startLine: 0, endLine: 2, statements: 1, count: 1 }] }));
      const bad = cli(["mutate", "--dir", dir, "--changed", changed, "--coverage", join(dir, "bad.json")]);
      assert.equal(bad.code, 2);
      assert.match(bad.stderr, /mutate: cannot read the coverage/);
      writeFileSync(join(dir, "extra.json"), JSON.stringify({ protocol: 1, blocks: [], more: 1 }));
      assert.equal(cli(["mutate", "--dir", dir, "--changed", changed, "--coverage", join(dir, "extra.json")]).code, 2);
      assert.equal(cli(["mutate", "--dir", dir, "--changed", join(dir, "missing.json"), "--coverage", coverage]).code, 2);
      const noEngine = cli(args("--tools", join(dir, "tools")));
      assert.equal(noEngine.code, 1);
      assert.match(noEngine.stderr, /not installed.*prepare --tools/);
      assert.equal(noEngine.stdout, "");
      assert.equal(cli(args()).code, 1, "no --tools");
      const excluding = join(dir, "excluding.json");
      writeFileSync(excluding, JSON.stringify({ protocol: 1, exclude: { dirs: ["locales"], extensions: [".po"] }, files: [] }));
      const withExclude = cli(["mutate", "--dir", dir, "--changed", excluding, "--coverage", coverage, "--tools", join(dir, "tools")]);
      assert.equal(withExclude.code, 1, `the exclude of --changed is accepted: ${withExclude.stderr}`);
      assert.match(withExclude.stderr, /not installed/);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

// The end-to-end tests need the fixture's dependencies (npm install in testdata/mutation-vitest) and StrykerJS,
// installed once (network) by `prepare --tools` into a folder of the system's temporary folder kept between runs.
const TOOLS = join(tmpdir(), "difflens-jsts-test-tools");

describe("mutate with StrykerJS and Vitest", { skip: !installed("mutation-vitest", "vitest") && "npm install in testdata/mutation-vitest" }, () => {
  let unavailable = null;
  before(() => {
    const { run, tree, done } = copy("mutation-vitest");
    try {
      const prepared = cli(["prepare", "--dir", tree, "--tools", TOOLS, "--timeout", "5m"]);
      if (prepared.code !== 0) unavailable = `StrykerJS could not be installed (network?): ${prepared.stderr.slice(-500)}`;
      void run;
    } finally {
      done();
    }
  });

  const inputs = (folder) => {
    const changed = join(folder, "changed.json");
    const coverage = join(folder, "coverage.json");
    writeFileSync(
      changed,
      JSON.stringify({
        protocol: 1,
        files: [
          { path: "src/account.ts", added: [[1, 12]] },
          { path: "src/loop.ts", added: [[1, 8]] },
          { path: "src/unused.ts", added: [[1, 6]] },
          { path: "src/account.test.ts", added: [[1, 21]] },
        ],
      }),
    );
    writeFileSync(
      coverage,
      JSON.stringify({
        protocol: 1,
        blocks: [
          { file: "src/account.ts", startLine: 3, endLine: 12, statements: 5, count: 2 },
          { file: "src/loop.ts", startLine: 2, endLine: 8, statements: 5, count: 2 },
          { file: "src/unused.ts", startLine: 2, endLine: 6, statements: 5, count: 0 },
          { file: "src/account.test.ts", startLine: 1, endLine: 21, statements: 9, count: 1 },
        ],
      }),
    );
    return { changed, coverage };
  };

  const mutate = (cacheDir, ...more) => {
    const { run, tree, done } = copy("mutation-vitest");
    try {
      const { changed, coverage } = inputs(run);
      const progress = join(run, "progress.json");
      const args = ["mutate", "--dir", tree, "--changed", changed, "--coverage", coverage, "--tools", TOOLS, "--jobs", "2", "--min-mutant-timeout", "1s", "--timeout", "3m", "--progress", progress, ...more];
      if (cacheDir) args.push("--cache-dir", cacheDir);
      const result = cli(args);
      assert.equal(result.code, 0, result.stderr);
      return { json: result.json, progress: existsSync(progress) ? JSON.parse(readFileSync(progress, "utf8")) : null };
    } finally {
      done();
    }
  };

  const cache = mkdtempSync(join(tmpdir(), "jsts-mutate-cache-"));
  let first = null;

  it("reports the survivor of a missed boundary, the killed with their test, a timeout, and skips uncovered and test lines", (t) => {
    if (unavailable) return t.skip(unavailable);
    const { json, progress } = mutate(cache);
    first = json;
    assert.deepEqual(validate(json, schema), []);
    assert.deepEqual(json.engine, { name: "StrykerJS", version: "10.1.1" });
    assert.equal(json.language, "typescript");
    assert.deepEqual(json.diagnostics, []);
    const files = new Set(json.mutants.map((m) => m.file));
    assert.deepEqual([...files], ["src/account.ts", "src/loop.ts"], "unused.ts is not covered; tests are not mutated");
    const survivor = json.mutants.find((m) => m.status === "survived");
    assert.deepEqual(
      [survivor.file, survivor.line, survivor.column, survivor.original, survivor.mutated, survivor.operator, survivor.function],
      ["src/account.ts", 4, 10, "balance >= amount", "balance > amount", "relational_boundary", "src/account.canWithdraw"],
    );
    assert.equal(json.mutants.filter((m) => m.status === "survived").length, 1);
    const negated = json.mutants.find((m) => m.mutated === "balance < amount");
    assert.equal(negated.status, "killed");
    assert.equal(negated.killedBy, "account/allows a withdrawal below the balance");
    const timeout = json.mutants.find((m) => m.mutated === "i--");
    assert.equal(timeout.status, "timeout");
    assert.equal(timeout.operator, "increment");
    assert.equal(timeout.function, "src/loop.sumTo");
    assert.ok(json.mutants.every((m) => !m.cached));
    assert.ok(json.mutants.filter((m) => m.status === "killed").every((m) => typeof m.killedBy === "string"));
    assert.deepEqual(
      { candidates: json.summary.candidates, sampled: json.summary.sampled, survived: json.summary.survived, timeout: json.summary.timeout, error: json.summary.error },
      { candidates: json.mutants.length, sampled: false, survived: 1, timeout: 1, error: 0 },
    );
    const sorted = [...json.mutants].sort((a, b) => (a.file < b.file ? -1 : a.file > b.file ? 1 : a.line - b.line || a.column - b.column));
    assert.deepEqual(json.mutants.map((m) => m.id), sorted.map((m) => m.id), "ordered by file, line, column");
    assert.equal(new Set(json.mutants.map((m) => m.id)).size, json.mutants.length);
    assert.deepEqual(validate(progress, schema.$defs.progress, schema), []);
    assert.deepEqual([progress.phase, progress.done, progress.total, progress.survived], ["mutating", json.mutants.length, json.mutants.length, 1]);
  });

  it("reuses the results kept in --cache-dir", (t) => {
    if (unavailable) return t.skip(unavailable);
    if (first === null) return t.skip("the first run did not complete");
    const { json } = mutate(cache);
    assert.deepEqual(validate(json, schema), []);
    assert.ok(json.mutants.every((m) => m.cached), "every result reused");
    assert.equal(json.summary.cached, json.mutants.length);
    const strip = (m) => ({ ...m, cached: undefined });
    assert.deepEqual(json.mutants.map(strip), first.mutants.map(strip), "same ids, results and kept durations");
    rmSync(cache, { recursive: true, force: true });
  });

  it("samples round-robin across functions above --max-mutants", (t) => {
    if (unavailable) return t.skip(unavailable);
    const { json } = mutate(null, "--max-mutants", "3");
    assert.deepEqual(validate(json, schema), []);
    assert.equal(json.summary.sampled, true);
    assert.equal(json.summary.maxMutants, 3);
    assert.ok(json.summary.candidates > 3);
    assert.deepEqual(
      json.mutants.map((m) => [m.function, m.line, m.column, m.mutated]),
      [
        ["src/account.canWithdraw", 4, 10, "false"],
        ["src/account.deposit", 8, 7, "false"],
        ["src/loop.sumTo", 4, 19, "false"],
      ],
    );
  });
});
