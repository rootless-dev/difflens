import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { complexity, indexFile, parseFile, testCall } from "../src/analyze/functions.js";

function index(text, rel = "src/a.ts", options = {}) {
  const sf = parseFile(`/t/${rel}`, text, rel);
  return indexFile(sf, rel, { container: "src", ...options });
}

const ids = (text, rel, options) => index(text, rel, options).functions.map((f) => f.id);

describe("function ids", () => {
  it("names declarations, members and values by their path in the module", () => {
    const text = `
      export function plain() {}
      export const arrow = () => 1;
      export default function () {}
      export class Service {
        constructor() {}
        get size() { return 1; }
        set size(v) {}
        static create() {}
        #hidden() {}
        handler = () => {};
      }
      export const api = { list() {}, nested: { get: async () => {} } };
    `;
    assert.deepEqual(ids(text).sort(), [
      "src/a.Service.#hidden",
      "src/a.Service.constructor",
      "src/a.Service.create",
      "src/a.Service.get size",
      "src/a.Service.handler",
      "src/a.Service.set size",
      "src/a.api.list",
      "src/a.api.nested.get",
      "src/a.arrow",
      "src/a.default",
      "src/a.plain",
    ]);
  });

  it("numbers closures inside their parent, so other edits do not rename them", () => {
    const before = ids("function outer() { a(() => 1); b(function () {}); }\nfunction other() {}");
    const after = ids("function other() { x(() => 0); }\nfunction outer() { a(() => 1); b(function () {}); }");
    for (const id of ["src/a.outer$1", "src/a.outer$2"]) {
      assert.ok(before.includes(id) && after.includes(id), id);
    }
    assert.ok(ids("const handler = wrap(() => {});").includes("src/a.handler$1"));
    assert.ok(ids("function f() { return () => () => 1; }").includes("src/a.f$1$1"));
  });

  it("names CommonJS assignments and nested functions", () => {
    const text = `
      function Counter() {}
      Counter.prototype.inc = function () {};
      module.exports.start = function start() {};
      exports.stop = () => {};
      function outer() { function inner() {} const helper = () => {}; }
    `;
    assert.deepEqual(ids(text, "lib/c.js").sort(), [
      "lib/c.Counter",
      "lib/c.Counter.inc",
      "lib/c.exports.start",
      "lib/c.exports.stop",
      "lib/c.outer",
      "lib/c.outer.helper",
      "lib/c.outer.inner",
    ]);
  });

  it("names test callbacks after their titles, hooks after their kind", () => {
    const text = `
      describe("math", () => {
        beforeEach(() => {});
        it("adds", () => {});
        it.each([1, 2])("case %i", (n) => {});
        describe.skip("nested", function () { test("deep", () => {}); });
      });
      test("same", () => {});
      test("same", () => {});
    `;
    const result = index(text, "src/a.test.ts", { testFile: true });
    assert.deepEqual(result.functions.map((f) => f.id).sort(), [
      'src/a.test.(module)',
      'src/a.test.describe("math")',
      'src/a.test.describe("math").beforeEach',
      'src/a.test.describe("math").describe("nested")',
      'src/a.test.describe("math").describe("nested").test("deep")',
      'src/a.test.describe("math").it("adds")',
      'src/a.test.describe("math").it("case %i")',
      'src/a.test.test("same")',
      'src/a.test.test("same")#2',
    ]);
    assert.ok(result.functions.every((f) => f.test));
    const tops = result.testCalls.filter((t) => t.top).map((t) => t.call.title);
    assert.deepEqual(tops, ["math", "same", "same"]);
  });

  it("does not take calls named like tests for tests outside test code", () => {
    const result = index('describe("x", () => {});', "src/a.ts");
    assert.equal(result.testCalls.length, 0);
    assert.ok(result.functions.some((f) => f.id === "src/a.(module)$1"));
  });

  it("lists a module body only when the module runs code when it loads", () => {
    assert.ok(!ids("export function f() { g(); }\nclass A { x = new Map(); }").some((id) => id.endsWith("(module)")));
    assert.ok(ids("const app = express();").includes("src/a.(module)"));
  });

  it("recognises test calls with modifiers, tables and node:test subtests", () => {
    const call = (text) => testCall(parseFile("/t/x.ts", text).statements[0].expression);
    assert.equal(call('it.only("a", () => {})').title, "a");
    assert.equal(call('describe.each([1])("t %s", () => {})').table, true);
    assert.equal(call('t.test("sub", () => {})').api, "test");
    assert.equal(call('it.todo("later")'), null);
    assert.equal(call('app.get("/x", () => {})'), null);
    assert.equal(call('it.weird("a", () => {})'), null);
  });
});

describe("complexity", () => {
  const of = (body) => complexity(parseFile("/t/x.ts", `function f(a, b) { ${body} }`).statements[0]);

  it("is 1 without branches", () => assert.equal(of("return a + b;"), 1));

  it("counts if, loops, cases, catch, ternaries and short circuits", () => {
    assert.equal(of("if (a) {} else if (b) {} else {}"), 3);
    assert.equal(of("for (;;) {} for (const x of a) {} for (const k in a) {} while (a) {} do {} while (b);"), 6);
    assert.equal(of("switch (a) { case 1: case 2: break; default: }"), 3);
    assert.equal(of("try {} catch {} finally {}"), 2);
    assert.equal(of("return a ? b : a && b || a ?? b;"), 5);
    assert.equal(of("a ||= b; a &&= b; a ??= b;"), 4);
  });

  it("does not count optional chaining, defaults or nested functions", () => {
    assert.equal(of("return a?.b?.(c);"), 1);
    assert.equal(of("const g = (x = 1) => (x ? 1 : 2); return g;"), 1);
  });
});
