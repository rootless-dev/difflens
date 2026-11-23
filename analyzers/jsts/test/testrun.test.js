import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, it } from "node:test";

import { Tree } from "../src/files.js";
import { Session, escapeTitle, isBuildFailure, nameTitles } from "../src/testrun/collect.js";
import { Coverage } from "../src/testrun/coverage.js";
import { packageCommand, shellQuote, testCommand } from "../src/testrun/commands.js";
import { namePattern, selectedPrefixes, unitCommands } from "../src/testrun/index.js";
import { findUnits, nodeRunsByDefault, runnerOfScript } from "../src/testrun/units.js";
import { sourceFiles } from "../src/files.js";
import { analysisOf, cli, copy, fixture, installed } from "./helpers.js";

describe("test runners", () => {
  it("are found from the test script first", () => {
    assert.equal(runnerOfScript("vitest run --coverage").runner, "vitest");
    assert.equal(runnerOfScript("tsc -p . && jest --ci").runner, "jest");
    assert.equal(runnerOfScript("NODE_ENV=test c8 mocha").runner, "mocha");
    assert.deepEqual(runnerOfScript("node --import tsx --test test/"), { runner: "node", flags: ["--import", "tsx"], patterns: ["test/"] });
    assert.deepEqual(runnerOfScript("tsx --test src/**/*.test.ts").flags, ["--import", "tsx"]);
    assert.equal(runnerOfScript("echo no tests"), null);
  });

  it("are found per fixture, with the test files they run", () => {
    const units = (name) => findUnits(fixture(name), sourceFiles(fixture(name)));
    assert.deepEqual(units("vitest-proj").units.map((u) => [u.rel, u.runner, u.testFiles.length]), [[".", "vitest", 2]]);
    assert.deepEqual(units("jest-proj").units.map((u) => [u.runner, u.testFiles.length]), [["jest", 2]]);
    assert.deepEqual(units("mocha-proj").units.map((u) => u.runner), ["mocha"]);
    assert.deepEqual(units("nodetest-proj").units.map((u) => u.runner), ["node"]);
    const tslib = units("tslib");
    assert.deepEqual(tslib.units, []);
    assert.equal(tslib.orphans.length, 1);
  });

  it("know node --test's default file patterns", () => {
    assert.ok(nodeRunsByDefault("test/a.js", false) && nodeRunsByDefault("src/a.test.mjs", false) && nodeRunsByDefault("x/test-a.cjs", false));
    assert.ok(!nodeRunsByDefault("src/a.test.ts", false) && nodeRunsByDefault("src/a.test.ts", true));
    assert.ok(!nodeRunsByDefault("src/a.js", true));
  });
});

describe("test names and selections", () => {
  it("keep a slash inside a title from reading as a level", () => {
    assert.equal(escapeTitle("a/b"), "a∕b");
    assert.deepEqual(nameTitles("x/a∕b"), ["x", "a/b"]);
  });

  it("select whole tests or their first-level subtests", () => {
    assert.deepEqual(
      selectedPrefixes([
        { name: "math", subtests: ["divide/by zero", "adds"] },
        { name: "whole", subtests: null },
        { name: "none", subtests: [] },
      ]),
      [["math", "divide"], ["math", "adds"], ["whole"]],
    );
    assert.deepEqual(selectedPrefixes([{ name: "math", subtests: ["adds"] }], true), [["math"]]);
    const pattern = new RegExp(namePattern([["math", "a.b"], ["x/y"]]));
    assert.ok(pattern.test("math a.b") && pattern.test("math a.b deeper") && pattern.test("x/y"));
    assert.ok(!pattern.test("math axb") && !pattern.test("math a.bc") && !pattern.test("x/yz"));
  });

  it("quote shell words in single quotes", () => {
    assert.equal(shellQuote("plain"), "'plain'");
    assert.equal(shellQuote("it's $HOME `x`"), "'it'\\''s $HOME `x`'");
  });

  it("give each runner's command for a test and for a package", () => {
    const root = { dir: "/t", rel: ".", flags: [] };
    const path = ["math", "it's (big)", "a/b"];
    assert.equal(
      testCommand({ ...root, runner: "vitest" }, "src/m.test.ts", path, true),
      "npx vitest run 'src/m.test.ts' -t '^(?:math it'\\''s \\(big\\) a\\/b)$'",
    );
    assert.equal(testCommand({ ...root, runner: "jest" }, "src/m.test.js", ["sum"], false), "npx jest 'src/m.test.js' -t '^(?:sum)(?: |$)'");
    assert.equal(testCommand({ ...root, runner: "mocha" }, "test/s.spec.js", ["Stack", "pops"], true), "npx mocha 'test/s.spec.js' --grep '^(?:Stack pops)$'");
    // node --test is given the top-level test: a parent that matches runs everything below it.
    assert.equal(
      testCommand({ ...root, runner: "node" }, "test/str.test.js", ["reverse", "keeps empty"], true),
      "node --test --test-name-pattern='^(?:reverse)$' 'test/str.test.js'",
    );
    assert.equal(packageCommand({ ...root, runner: "vitest" }, ["src/a.test.ts", "src/b c.test.ts"]), "npx vitest run 'src/a.test.ts' 'src/b c.test.ts'");
    assert.equal(packageCommand({ ...root, runner: "jest" }, ["src/a.test.js"]), "npx jest 'src/a.test.js'");
    assert.equal(packageCommand({ ...root, runner: "mocha" }, ["test/a.spec.js"]), "npx mocha 'test/a.spec.js'");
    assert.equal(packageCommand({ ...root, runner: "node" }, ["test/a.test.js"]), "node --test 'test/a.test.js'");
    assert.equal(packageCommand({ ...root, runner: "node" }, []), null);
    assert.equal(testCommand({ ...root, runner: "bun" }, "a.test.js", ["x"], true), null);
  });

  it("run a package one level down in its folder, through the loaders of its test script", () => {
    const tsx = { dir: "/t/api", rel: "api", runner: "node", flags: ["--import", "tsx"] };
    assert.equal(
      testCommand(tsx, "src/a.test.ts", ["routes", "GET /users"], true),
      "cd api && node --import tsx --test --test-name-pattern='^(?:routes)$' 'src/a.test.ts'",
    );
    assert.equal(packageCommand(tsx, ["src/a.test.ts"]), "cd api && node --import tsx --test 'src/a.test.ts'");
    const spaced = { dir: "/t/my app", rel: "my app", runner: "vitest", flags: [] };
    assert.equal(packageCommand(spaced, ["x.test.ts"]), "cd 'my app' && npx vitest run 'x.test.ts'");
  });

  it("find the unit and the files of each test and package", () => {
    const tree = new Tree("/t");
    const web = { dir: "/t/web", rel: "web", runner: "jest", flags: [], testFiles: ["/t/web/src/a.test.js", "/t/web/src/b.test.js", "/t/web/test/c.test.js"] };
    const commands = unitCommands([web], tree, "root");
    assert.equal(commands.test("/t/web/test/c.test.js", ["c"], true), "cd web && npx jest 'test/c.test.js' -t '^(?:c)$'");
    assert.equal(commands.package("web/src"), "cd web && npx jest 'src/a.test.js' 'src/b.test.js'");
    assert.equal(commands.package("lib"), null);
    assert.equal(commands.test("/t/other.test.js", ["x"], true), null);
  });

  it("tell a suite that does not compile from one that fails", () => {
    assert.ok(isBuildFailure("SyntaxError: Unexpected token ';'"));
    assert.ok(isBuildFailure("Error: Cannot find module './x'"));
    assert.ok(!isBuildFailure("Error: the database is down"));
  });
});

describe("results", () => {
  it("list every level of a test with the outcome of what it holds", () => {
    const tree = new Tree(fixture("vitest-proj"));
    const progress = [];
    const session = new Session({ tree, rootName: "vitest-proj", progress: (p) => progress.push(p) });
    const file = join(tree.root, "src/math.test.ts");
    session.expect([file]);
    session.handle({ e: "start", file }, null);
    session.handle({ e: "test", file, path: ["math", "adds"], status: "pass", ms: 2, output: null }, null);
    session.handle({ e: "test", file, path: ["math", "divide", "divides"], status: "fail", ms: 3, output: "boom" }, null);
    session.handle({ e: "end", file, error: null, ms: 9 }, null);
    const { tests, packages } = session.results(["src", "src/strings"]);
    const byName = Object.fromEntries(tests.map((t) => [t.name, t]));
    assert.deepEqual(Object.keys(byName), ["math/adds", "math/divide/divides", "math/divide", "math"]);
    assert.equal(byName.math.outcome, "fail");
    assert.equal(byName.math.durationMs, 5);
    assert.match(byName.math.output, /math\/divide\/divides:\nboom/);
    assert.equal(byName["math/adds"].output, null);
    assert.equal(byName.math.function, 'src/math.test.describe("math")');
    assert.deepEqual([byName.math.file, byName.math.line], ["src/math.test.ts", 5]);
    assert.deepEqual(byName["math/divide/divides"].hierarchy, ["math", "divide", "divides"]);
    assert.deepEqual(byName.math.hierarchy, ["math"]);
    assert.ok(tests.every((t) => t.command === null), "no commands without a unit");
    assert.deepEqual(packages, [
      { container: "src", outcome: "fail", durationMs: 9, output: null, command: null },
      { container: "src/strings", outcome: "no_tests", durationMs: 0, output: null, command: null },
    ]);
    assert.deepEqual(progress.at(-1), { packagesDone: 1, packagesTotal: 1, tests: 1, failed: 1, current: null });
  });

  it("write the hierarchy as the name writes each level, a slash in a title included", () => {
    const tree = new Tree(fixture("vitest-proj"));
    const file = join(tree.root, "src/math.test.ts");
    const unit = { dir: tree.root, rel: ".", runner: "vitest", flags: [], testFiles: [file] };
    const session = new Session({ tree, rootName: "vitest-proj", progress: null, commands: unitCommands([unit], tree, "vitest-proj") });
    session.handle({ e: "test", file, path: ["a/b", "c", "d/e/f"], status: "pass", ms: 1, output: null }, null);
    session.handle({ e: "end", file, error: null, ms: 1 }, null);
    const { tests, packages } = session.results(["src"]);
    const leaf = tests.find((t) => t.name === "a∕b/c/d∕e∕f");
    assert.deepEqual(leaf.hierarchy, ["a∕b", "c", "d∕e∕f"]);
    assert.ok(tests.every((t) => t.hierarchy.join("/") === t.name));
    assert.deepEqual(tests.map((t) => t.hierarchy.length), [3, 2, 1]);
    // The real "/" in the pattern, anchored at the end for the test, open for the levels above it.
    assert.equal(leaf.command, "npx vitest run 'src/math.test.ts' -t '^(?:a\\/b c d\\/e\\/f)$'");
    assert.equal(tests.find((t) => t.name === "a∕b").command, "npx vitest run 'src/math.test.ts' -t '^(?:a\\/b)(?: |$)'");
    assert.equal(packages[0].command, "npx vitest run 'src/math.test.ts'");
  });

  it("give the function ids analyze gives the test callbacks", () => {
    const analysis = analysisOf("vitest-proj");
    const ids = new Set(analysis.functions.map((f) => f.id));
    assert.ok(ids.has('src/math.test.describe("math")') && ids.has('src/strings/upper.test.test("upper")'));
  });
});

describe("coverage", () => {
  const tree = new Tree(fixture("nodetest-proj"));

  it("turns Istanbul statements into blocks of the project's own code", () => {
    const cov = new Coverage(tree);
    cov.addIstanbul({
      [join(tree.root, "src/str.js")]: {
        path: join(tree.root, "src/str.js"),
        statementMap: { 0: { start: { line: 2 }, end: { line: 2 } }, 1: { start: { line: 2 }, end: { line: 2 } }, 2: { start: { line: 11 }, end: { line: 11 } } },
        s: { 0: 3, 1: 0, 2: 0 },
      },
      [join(tree.root, "test/str.test.js")]: { path: join(tree.root, "test/str.test.js"), statementMap: { 0: { start: { line: 1 }, end: { line: 1 } } }, s: { 0: 1 } },
      "/elsewhere/x.js": { path: "/elsewhere/x.js", statementMap: { 0: { start: { line: 1 }, end: { line: 1 } } }, s: { 0: 1 } },
    });
    assert.deepEqual(cov.result().blocks, [
      { file: "src/str.js", startLine: 2, endLine: 2, statements: 2, count: 3 },
      { file: "src/str.js", startLine: 11, endLine: 11, statements: 1, count: 0 },
    ]);
  });

  it("keeps node's line counts only for lines holding code", () => {
    const cov = new Coverage(tree);
    // Line 3 is a lone brace and line 4 is blank.
    cov.addLines([{ path: join(tree.root, "src/str.js"), lines: [[1, 1], [3, 1], [4, 1], [6, 0]] }]);
    assert.deepEqual(cov.result().blocks.map((b) => [b.startLine, b.count]), [[1, 1], [6, 0]]);
  });
});

describe("test: the command", () => {
  const run = (name, args = []) => {
    const c = copy(name);
    try {
      const progressFile = join(c.run, "progress.json");
      const result = cli(["test", "--dir", c.tree, "--timeout", "120s", "--progress", progressFile, ...args]);
      let progress = null;
      try {
        progress = JSON.parse(readFileSync(progressFile, "utf8"));
      } catch {
        progress = null;
      }
      return { ...result, progress, c };
    } finally {
      c.done();
    }
  };
  const outcomes = (json) => Object.fromEntries(json.tests.map((t) => [t.name, t.outcome]));
  const packages = (json) => Object.fromEntries(json.packages.map((p) => [p.container, p.outcome]));
  const command = (json, name) => {
    for (const t of json.tests) assert.equal(t.hierarchy.join("/"), t.name);
    return json.tests.find((t) => t.name === name).command;
  };
  const packageCommandOf = (json, container) => json.packages.find((p) => p.container === container).command;

  it("runs node --test with coverage", () => {
    const { code, json, progress } = run("nodetest-proj");
    assert.equal(code, 0);
    assert.equal(json.race, false);
    assert.deepEqual(outcomes(json), {
      "reverse/reverses": "pass",
      "reverse/keeps empty": "pass",
      reverse: "pass",
      "palindromes/racecar": "pass",
      "palindromes/not hello": "pass",
      palindromes: "pass",
      "skipped one": "skip",
    });
    assert.deepEqual(packages(json), { src: "no_tests", test: "pass" });
    assert.equal(command(json, "palindromes/racecar"), "node --test --test-name-pattern='^(?:palindromes)$' 'test/str.test.js'");
    assert.equal(packageCommandOf(json, "test"), "node --test 'test/str.test.js'");
    assert.equal(packageCommandOf(json, "src"), null);
    const blocks = json.coverage.blocks;
    assert.ok(blocks.every((b) => b.file === "src/str.js"));
    assert.ok(blocks.some((b) => b.startLine === 11 && b.count === 0), "never() did not run");
    assert.deepEqual(progress, { protocol: 1, packagesDone: 2, packagesTotal: 2, tests: 3, failed: 0, current: null });
  });

  it("reports failing tests with their output, and files that do not load as build failures", () => {
    const { code, json } = run("failing", ["--no-race", "--no-cover"]);
    assert.equal(code, 0);
    assert.equal(outcomes(json)["discount/takes ten percent off"], "fail");
    const failed = json.tests.find((t) => t.name === "discount/takes ten percent off");
    assert.match(failed.output, /110 !== 90/);
    assert.equal(failed.function, 'test/price.test.describe("discount")');
    assert.deepEqual(packages(json), { lib: "no_tests", test: "fail", "test/broken": "build_failed" });
    assert.match(json.packages.find((p) => p.container === "test/broken").output, /SyntaxError/);
    assert.equal(json.coverage, null);
    assert.ok(json.diagnostics.includes("coverage was not measured: --no-cover was given"));
  });

  it("runs only a selection", () => {
    const c = copy("nodetest-proj");
    try {
      const selection = join(c.run, "selection.json");
      writeFileSync(selection, JSON.stringify({ packages: [{ package: "test", tests: [{ name: "reverse", subtests: ["keeps empty"] }] }, { package: "nope", tests: null }] }));
      const { code, json } = cli(["test", "--dir", c.tree, "--selection", selection, "--no-cover"]);
      assert.equal(code, 0);
      assert.deepEqual(Object.keys(outcomes(json)).sort(), ["reverse", "reverse/keeps empty", "reverse/reverses"]);
      assert.deepEqual(packages(json), { test: "pass" });
      assert.ok(json.diagnostics.some((d) => d.includes("nope was not found")));
    } finally {
      c.done();
    }
  });

  it("says why tests without a runner did not run", () => {
    const { code, json } = run("tslib");
    assert.equal(code, 0);
    assert.deepEqual(json.tests, []);
    assert.ok(json.diagnostics.some((d) => d.includes("src/shapes.test.ts")));
  });

  it("runs Vitest with v8 coverage", { skip: !installed("vitest-proj", "vitest") && "npm install in testdata/vitest-proj" }, () => {
    const { code, json, progress } = run("vitest-proj");
    assert.equal(code, 0);
    assert.equal(outcomes(json)["math/divide/refuses zero"], "pass");
    assert.equal(outcomes(json)["math/is skipped"], "skip");
    assert.equal(outcomes(json)["adds negatives ∕ with a slash"], "pass");
    assert.equal(outcomes(json)["fails on purpose"], "fail");
    assert.match(json.tests.find((t) => t.name === "fails on purpose").output, /expected 'A' to be 'B'/);
    assert.deepEqual(packages(json), { src: "pass", "src/strings": "fail" });
    assert.equal(command(json, "adds negatives ∕ with a slash"), "npx vitest run 'src/math.test.ts' -t '^(?:adds negatives \\/ with a slash)$'");
    assert.equal(command(json, "math/divide"), "npx vitest run 'src/math.test.ts' -t '^(?:math divide)(?: |$)'");
    assert.equal(packageCommandOf(json, "src/strings"), "npx vitest run 'src/strings/upper.test.ts'");
    const files = new Set(json.coverage.blocks.map((b) => b.file));
    assert.deepEqual([...files].sort(), ["src/math.ts", "src/strings/upper.ts"]);
    assert.ok(json.coverage.blocks.some((b) => b.file === "src/math.ts" && b.startLine === 13 && b.count === 0));
    assert.equal(progress.packagesDone, 2);
  });

  it("runs Vitest on a selection by name pattern", { skip: !installed("vitest-proj", "vitest") && "npm install in testdata/vitest-proj" }, () => {
    const c = copy("vitest-proj");
    try {
      const selection = join(c.run, "selection.json");
      writeFileSync(selection, JSON.stringify({ packages: [{ package: "src", tests: [{ name: "math", subtests: ["adds"] }] }] }));
      const { code, json } = cli(["test", "--dir", c.tree, "--selection", selection, "--no-cover"]);
      assert.equal(code, 0);
      assert.deepEqual(Object.keys(outcomes(json)).sort(), ["math", "math/adds"]);
    } finally {
      c.done();
    }
  });

  it("runs Jest with coverage and reports a suite that does not compile", { skip: !installed("jest-proj", "jest") && "npm install in testdata/jest-proj" }, () => {
    const { code, json } = run("jest-proj");
    assert.equal(code, 0);
    assert.deepEqual(outcomes(json), { "sum/adds": "pass", "sum/is zero for nothing": "pass", sum: "pass", averages: "pass" });
    assert.deepEqual(packages(json), { src: "build_failed", "src/__tests__": "pass" });
    assert.equal(command(json, "sum/adds"), "npx jest 'src/__tests__/sum.test.js' -t '^(?:sum adds)$'");
    assert.equal(packageCommandOf(json, "src"), "npx jest 'src/broken.test.js'");
    assert.ok(json.coverage.blocks.some((b) => b.file === "src/sum.js" && b.startLine === 7 && b.count === 0));
  });

  it("runs Mocha with c8 coverage", { skip: !installed("mocha-proj", "mocha") && "npm install in testdata/mocha-proj" }, () => {
    const { code, json } = run("mocha-proj");
    assert.equal(code, 0);
    assert.equal(outcomes(json)["Stack/when empty/fails on purpose"], "fail");
    assert.equal(outcomes(json).Stack, "fail");
    assert.equal(json.tests.find((t) => t.name === "Stack").function, 'test/stack.spec.describe("Stack")');
    assert.equal(command(json, "Stack/when empty/throws"), "npx mocha 'test/stack.spec.js' --grep '^(?:Stack when empty throws)$'");
    assert.equal(packageCommandOf(json, "test"), "npx mocha 'test/stack.spec.js'");
    assert.ok(json.coverage.blocks.every((b) => b.file === "lib/stack.js"));
  });
});
