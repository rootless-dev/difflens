import { describe, expect, it } from "vitest";

import {
  countSelection,
  indexSelection,
  packageCheck,
  packageKey,
  packageLeaves,
  selectionPayload,
  subtestKey,
  testCheck,
  testKey,
  toggled,
} from "@/features/review/tests/selection";
import { buildSuite, failureCount, selectionRuns, type SuiteTest, testCommands } from "@/features/review/tests/suite";
import type { TestRun } from "@/lib/api";
import { makeFailingRun, makeTestCase, makeTestRun } from "@/test/tests-fixtures";

describe("the suite of a run", () => {
  it("groups subtests under their test, a test known only by its subtests included, failures first", () => {
    const run = makeTestRun({
      tests: [
        makeTestCase("TestB"),
        makeTestCase("TestA/x/deep", { outcome: "fail" }),
        makeTestCase("TestA/y"),
        makeTestCase("TestC/skipped", { outcome: "skip" }),
      ],
      packages: [],
    });

    const suite = buildSuite(run);

    const [pkg] = suite.packages;
    expect(pkg.tests.map((test) => [test.name, test.outcome, test.subtests.map((sub) => sub.name)])).toEqual([
      ["TestA", "fail", ["x/deep", "y"]],
      ["TestB", "pass", []],
      ["TestC", "skip", ["skipped"]],
    ]);
    expect(pkg.tests[0].subtests[0].title).toBe("x/deep");
    expect([suite.total, suite.failed, suite.skipped]).toEqual([3, 1, 1]);
    expect(buildSuite(run)).toBe(suite);
    expect(failureCount(makeFailingRun())).toBe(3);
  });

  it("reads a Go subtest's title with spaces, any other language's as it is", () => {
    const go = buildSuite(makeTestRun({ tests: [makeTestCase("TestA/gateway_declines")], packages: [] }));
    expect(go.packages[0].tests[0].subtests[0].title).toBe("gateway declines");

    const rust = buildSuite(
      makeTestRun({
        language: "rust",
        tests: [makeTestCase("tests/snake_case", { hierarchy: ["tests", "snake_case"] })],
        packages: [],
      }),
    );
    expect(rust.packages[0].tests[0].subtests[0].title).toBe("snake_case");
  });

  it("groups by the hierarchy: a top-level name holding a slash stays one test", () => {
    const run = makeTestRun({
      language: "rust",
      tests: [
        makeTestCase("src/lib.rs - add (line 5)", { container: "shapes", hierarchy: ["src/lib.rs - add (line 5)"] }),
        makeTestCase("src/lib.rs - sub (line 9)", { container: "shapes", hierarchy: ["src/lib.rs - sub (line 9)"] }),
        makeTestCase("test_url[a/b]", { container: "tests/test_urls.py", hierarchy: ["test_url[a/b]"] }),
        makeTestCase("outer/inner title/does it", {
          container: "src/app.test.ts",
          hierarchy: ["outer", "inner title", "does it"],
        }),
      ],
      packages: [],
    });

    const suite = buildSuite(run);

    const tests = (pkg: string) =>
      suite.packages
        .find((entry) => entry.name === pkg)!
        .tests.map((test) => [test.name, test.case?.name ?? null, test.subtests.map((sub) => sub.name)]);
    expect(tests("shapes")).toEqual([
      ["src/lib.rs - add (line 5)", "src/lib.rs - add (line 5)", []],
      ["src/lib.rs - sub (line 9)", "src/lib.rs - sub (line 9)", []],
    ]);
    expect(tests("tests/test_urls.py")).toEqual([["test_url[a/b]", "test_url[a/b]", []]]);
    expect(tests("src/app.test.ts")).toEqual([["outer", null, ["inner title/does it"]]]);
    expect(suite.total).toBe(4);
  });

  it("says which tests a selection runs, and the go test lines it takes", () => {
    const run = makeTestRun({
      tests: [
        makeTestCase("TestAnything/sub", { container: "a" }),
        makeTestCase("TestX", { container: "b" }),
        makeTestCase("TestY", { container: "b" }),
        makeTestCase("TestY/one", { container: "b" }),
        makeTestCase("TestY/two", { container: "b" }),
        makeTestCase("TestY/two/deep", { container: "b" }),
        makeTestCase("TestY/two/deep/er", { container: "b" }),
        makeTestCase("TestY/three", { container: "b" }),
        makeTestCase("TestX", { container: "d" }),
      ],
      packages: [],
    });
    const suite = buildSuite(run);
    const test = (pkg: string, name: string) =>
      suite.packages.find((entry) => entry.name === pkg)!.tests.find((entry) => entry.name === name)!;
    const sub = (owner: SuiteTest, name: string) => owner.subtests.find((entry) => entry.name === name)!;
    const testY = test("b", "TestY");
    const selection = {
      packages: [
        { package: "a", tests: null },
        { package: "b", tests: [{ name: "TestX", subtests: null }, { name: "TestY", subtests: ["one", "two/deep"] }] },
        { package: "c", tests: [{ name: "Test(Odd)", subtests: ["v1,_the_edge"] }] },
      ],
    };
    const anything = test("a", "TestAnything");
    expect(selectionRuns(selection, anything, sub(anything, "sub"))).toBe(true);
    expect(selectionRuns(selection, testY)).toBe(true);
    expect(selectionRuns(selection, testY, sub(testY, "two/deep/er"))).toBe(true);
    expect(selectionRuns(selection, testY, sub(testY, "three"))).toBe(false);
    expect(selectionRuns(selection, test("d", "TestX"))).toBe(false);
    expect(testCommands(run, selection, false)).toEqual([
      "go test ./a",
      "go test -run '^TestX$' ./b",
      "go test -run '^TestY$/^(one|two)$' ./b",
      "go test -run '^Test\\(Odd\\)$/^v1,_the_edge$' ./c",
    ]);
    expect(testCommands(run, { packages: [{ package: "b", tests: [{ name: "TestX", subtests: null }] }] }, true)).toEqual([
      "go test -race -run '^TestX$' ./b",
    ]);
  });

  it("tells a running subtest by its first level, whatever its name holds", () => {
    const run = makeTestRun({
      language: "javascript",
      tests: [
        makeTestCase("outer/a/b/leaf", { container: "app.test.js", hierarchy: ["outer", "a/b", "leaf"] }),
        makeTestCase("outer/a/other", { container: "app.test.js", hierarchy: ["outer", "a", "other"] }),
      ],
      packages: [],
    });
    const [outer] = buildSuite(run).packages[0].tests;
    const leaf = outer.subtests.find((sub) => sub.name === "a/b/leaf")!;
    const other = outer.subtests.find((sub) => sub.name === "a/other")!;
    const selection = { packages: [{ package: "app.test.js", tests: [{ name: "outer", subtests: ["a/b/leaf"] }] }] };

    expect(selectionRuns(selection, outer, leaf)).toBe(true);
    expect(selectionRuns(selection, outer, other)).toBe(false);
  });
});

describe("the commands a selection takes", () => {
  it("takes Go's reported commands when every selected item has one, and builds them when one has none", () => {
    const run = makeTestRun({
      tests: [
        makeTestCase("TestAdd", { container: "calc", command: "go test -C mod -race -run '^TestAdd$' ./calc" }),
        makeTestCase("TestDiv", { container: "calc", command: "go test -C mod -race -run '^TestDiv$' ./calc" }),
        makeTestCase("TestDiv/by_zero", {
          container: "calc",
          command: "go test -C mod -race -run '^TestDiv$/^by_zero$' ./calc",
        }),
        makeTestCase("TestRoot", { container: "root" }),
      ],
      packages: [
        { container: "calc", outcome: "pass", durationMs: 1, output: null, failure: null, command: "go test -C mod -race ./calc" },
      ],
    });
    const reported = {
      packages: [
        { package: "calc", tests: [{ name: "TestDiv", subtests: ["by_zero"] }, { name: "TestAdd", subtests: null }] },
        { package: "calc", tests: null },
      ],
    };

    expect(testCommands(run, reported, false)).toEqual([
      "go test -C mod -race -run '^TestDiv$/^by_zero$' ./calc",
      "go test -C mod -race -run '^TestAdd$' ./calc",
      "go test -C mod -race ./calc",
    ]);
    expect(
      testCommands(
        run,
        { packages: [{ package: "calc", tests: null }, { package: "root", tests: [{ name: "TestRoot", subtests: null }] }] },
        false,
      ),
    ).toEqual(["go test ./calc", "go test -run '^TestRoot$' ./root"]);
  });

  it("uses the commands the run reports, de-duplicated, in selection order", () => {
    const run = makeTestRun({
      language: "python",
      tests: [
        makeTestCase("test_url[a/b]", {
          container: "tests/test_urls.py",
          hierarchy: ["test_url[a/b]"],
          file: "tests/test_urls.py",
          command: "python -m pytest 'tests/test_urls.py::test_url[a/b]'",
        }),
        makeTestCase("TestCart/test_add", {
          container: "tests/test_cart.py",
          hierarchy: ["TestCart", "test_add"],
          command: "python -m pytest 'tests/test_cart.py::TestCart::test_add'",
        }),
        makeTestCase("TestCart/test_remove", {
          container: "tests/test_cart.py",
          hierarchy: ["TestCart", "test_remove"],
          command: "python -m pytest 'tests/test_cart.py::TestCart::test_remove'",
        }),
      ],
      packages: [
        {
          container: "tests/test_urls.py",
          outcome: "pass",
          durationMs: 1,
          output: null,
          failure: null,
          command: "python -m pytest tests/test_urls.py",
        },
      ],
    });

    expect(
      testCommands(
        run,
        {
          packages: [
            { package: "tests/test_cart.py", tests: [{ name: "TestCart", subtests: ["test_remove", "test_add"] }] },
            { package: "tests/test_urls.py", tests: [{ name: "test_url[a/b]", subtests: null }] },
            { package: "tests/test_urls.py", tests: null },
            { package: "tests/test_urls.py", tests: [{ name: "test_url[a/b]", subtests: null }] },
          ],
        },
        true,
      ),
    ).toEqual([
      "python -m pytest 'tests/test_cart.py::TestCart::test_remove'",
      "python -m pytest 'tests/test_cart.py::TestCart::test_add'",
      "python -m pytest 'tests/test_urls.py::test_url[a/b]'",
      "python -m pytest tests/test_urls.py",
    ]);
  });

  it("runs some subtests by the command of the first level they are under", () => {
    const js = (name: string, hierarchy: string[], command: string | null) =>
      makeTestCase(name, { container: "src/cart.test.ts", hierarchy, command });
    const run = makeTestRun({
      language: "typescript",
      tests: [
        js("cart/adds", ["cart", "adds"], "npx vitest run src/cart.test.ts -t 'cart adds'"),
        js("cart/adds/twice", ["cart", "adds", "twice"], "npx vitest run src/cart.test.ts -t 'cart adds twice'"),
        js("cart/adds/once", ["cart", "adds", "once"], "npx vitest run src/cart.test.ts -t 'cart adds once'"),
        js("cart/removes/all", ["cart", "removes", "all"], "npx vitest run src/cart.test.ts -t 'cart removes all'"),
      ],
      packages: [],
    });

    expect(
      testCommands(
        run,
        { packages: [{ package: "src/cart.test.ts", tests: [{ name: "cart", subtests: ["adds/twice", "adds/once", "removes/all"] }] }] },
        false,
      ),
    ).toEqual(["npx vitest run src/cart.test.ts -t 'cart adds'", "npx vitest run src/cart.test.ts -t 'cart removes all'"]);
  });

  it("writes the language's usual command where the run reported none, quoted for the shell", () => {
    const rust = makeTestRun({
      language: "rust",
      tests: [
        makeTestCase("tests::it's_fine", { container: "shapes", hierarchy: ["tests::it's_fine"] }),
        makeTestCase("area_of_a_square", { container: "shapes/tests/area", hierarchy: ["area_of_a_square"] }),
        makeTestCase("parses", { container: "cli/bin/shapes-cli", hierarchy: ["parses"] }),
      ],
      packages: [],
    });
    const one = (run: TestRun, pkg: string, name: string | null) =>
      testCommands(run, { packages: [{ package: pkg, tests: name === null ? null : [{ name, subtests: null }] }] }, false);

    expect(one(rust, "shapes", "tests::it's_fine")).toEqual(["cargo test -p shapes 'tests::it'\\''s_fine' -- --exact"]);
    expect(one(rust, "shapes/tests/area", "area_of_a_square")).toEqual([
      "cargo test -p shapes --test area 'area_of_a_square' -- --exact",
    ]);
    expect(one(rust, "cli/bin/shapes-cli", "parses")).toEqual(["cargo test -p cli --bin shapes-cli 'parses' -- --exact"]);
    expect(one(rust, "shapes/tests/area", null)).toEqual(["cargo test -p shapes --test area"]);

    const python = makeTestRun({
      language: "python",
      tests: [makeTestCase("test_total", { container: "tests/test_cart.py", hierarchy: ["test_total"], file: "tests/test_cart.py" })],
      packages: [],
    });
    expect(one(python, "tests/test_cart.py", "test_total")).toEqual(["python -m pytest 'tests/test_cart.py::test_total'"]);
    expect(one(python, "tests/test_cart.py", null)).toEqual(["python -m pytest 'tests/test_cart.py'"]);

    const javascript = makeTestRun({
      language: "javascript",
      tests: [makeTestCase("cart/adds/twice", { container: "cart.test.js", hierarchy: ["cart", "adds", "twice"] })],
      packages: [],
    });
    expect(
      testCommands(javascript, { packages: [{ package: "cart.test.js", tests: [{ name: "cart", subtests: ["adds/twice"] }] }] }, false),
    ).toEqual(["npm test -- -t 'cart adds twice'"]);
    expect(one(javascript, "cart.test.js", null)).toEqual(["npm test"]);
  });
});

describe("a selection", () => {
  const suite = buildSuite(makeFailingRun());
  const auth = suite.packages.find((pkg) => pkg.name === "app/services/auth_service")!;
  const adTest = auth.tests.find((test) => test.name.startsWith("TestAuthenticateAD_Every"))!;

  it("mixes parents, counts by the rule and asks the runner the least", () => {
    let selected = toggled(new Set(), [subtestKey(auth.name, adTest.name, "AD_accepts")], true);
    let index = indexSelection(selected);
    expect([testCheck(adTest, index), packageCheck(auth, index)]).toEqual(["some", "some"]);
    expect(countSelection(suite, index)).toEqual({ tests: 0, subtests: 1, packages: 0 });
    expect(selectionPayload(suite, selected).packages).toEqual([
      { package: auth.name, tests: [{ name: adTest.name, subtests: ["AD_accepts"] }] },
    ]);

    selected = toggled(selected, packageLeaves(auth), true);
    expect(packageLeaves(auth)).toContain(testKey(auth.name, "TestRefreshToken_RotatesOnUse"));
    selected = toggled(selected, [packageKey("app/containers")], true);
    index = indexSelection(selected);
    expect(packageCheck(auth, index)).toBe("all");
    expect(countSelection(suite, index)).toEqual({ tests: 3, subtests: 0, packages: 1 });
    expect(selectionPayload(suite, selected).packages).toEqual([
      { package: "app/containers", tests: null },
      { package: auth.name, tests: null },
    ]);
  });
});
