import assert from "node:assert/strict";
import { cpSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "node:test";

import { dedupeCalls } from "../src/analyze/normalize.js";
import { analysisOf, call, callsFrom, cli, fixture, fn } from "./helpers.js";

const entry = (f, kind) => (f?.entryPoints ?? []).filter((e) => e.kind === kind).map((e) => e.label);

describe("analyze: a TypeScript library", () => {
  const a = analysisOf("tslib");

  it("reports a document for TypeScript", () => {
    assert.equal(a.protocol, 1);
    assert.equal(a.language, "typescript");
    assert.equal(a.analyzer.name, "difflens-analyzer-jsts");
  });

  it("lists functions, methods and accessors with containers, files and lines", () => {
    const area = fn(a, "src/shapes.Circle.area");
    assert.deepEqual(
      { name: area.name, qualifiedName: area.qualifiedName, container: area.container, file: area.file, startLine: area.startLine, endLine: area.endLine },
      { name: "Circle.area", qualifiedName: "shapes.Circle.area", container: "src", file: "src/shapes.ts", startLine: 22, endLine: 24 },
    );
    assert.ok(fn(a, "src/shapes.Square.set side"));
    assert.equal(fn(a, "src/shapes.Square.set side").complexity, 3);
    assert.ok(fn(a, "src/util.helpers.double"));
    assert.ok(!a.functions.some((f) => f.id.includes("Base.area")), "an abstract method has no body");
  });

  it("flags generated and test code", () => {
    assert.equal(fn(a, "src/generated/schema.decodeUser").generated, true);
    assert.equal(fn(a, 'src/shapes.test.describe("totalArea")').test, true);
    assert.equal(fn(a, "src/shapes.totalArea").test, false);
  });

  it("resolves static calls through path aliases and imports", () => {
    assert.equal(call(a, "src/shapes.Base.describe", "src/util.round")[0].resolution, "static");
    assert.equal(call(a, "src/repository.printAll", "src/repository.each")[0].kind, "always");
  });

  it("resolves interface and abstract calls to the classes implementing them", () => {
    const viaInterface = call(a, "src/repository.Repository.find", "src/repository.MemoryStore.get");
    assert.equal(viaInterface[0].resolution, "dynamic");
    const viaAbstract = callsFrom(a, "src/shapes.Base.describe").filter((c) => c.resolution === "dynamic").map((c) => c.callee);
    assert.deepEqual(viaAbstract.sort(), ["src/shapes.Circle.area", "src/shapes.Circle.get name", "src/shapes.Square.area", "src/shapes.Square.get name"]);
    assert.equal(call(a, "src/kinds.union", "src/shapes.Square.area")[0].resolution, "dynamic");
  });

  it("resolves calls of function parameters to the functions passed for them", () => {
    const c = call(a, "src/repository.each", "src/repository.print")[0];
    assert.equal(c.resolution, "dynamic");
    assert.equal(c.kind, "conditional");
    assert.equal(call(a, "src/repository.printAll", "src/repository.print", "reference").length, 1);
  });

  it("tells always, conditional and async calls apart", () => {
    const kind = (caller, callee, line) => call(a, caller, callee).find((c) => c.line === line)?.kind;
    assert.equal(kind("src/kinds.early", "src/kinds.first", 5), "always");
    assert.equal(kind("src/kinds.early", "src/kinds.second", 9), "conditional");
    assert.equal(kind("src/kinds.loops", "src/kinds.first", 15), "conditional");
    assert.equal(kind("src/kinds.loops", "src/kinds.second", 19), "always");
    assert.equal(kind("src/kinds.loops", "src/kinds.third", 21), "conditional");
    assert.equal(kind("src/kinds.loops", "src/kinds.first", 24), "always");
    assert.equal(kind("src/kinds.branches", "src/kinds.fallback", 29), "conditional");
    assert.equal(kind("src/kinds.branches", "src/kinds.second", 34), "conditional");
    assert.equal(kind("src/kinds.branches", "src/kinds.third", 36), "always");
    assert.equal(kind("src/kinds.waits", "src/kinds.later", 46), "always");
    assert.equal(kind("src/kinds.waits", "src/kinds.later", 47), "async");
    assert.equal(kind("src/kinds.waits", "src/kinds.first", 48), "async");
    assert.equal(kind("src/repository.Repository.saveLater", "src/repository.Repository.saveLater$1", 36), "async");
    assert.equal(kind("src/repository.Repository.audit", "src/repository.notify", 43), "async");
  });

  it("summarises an interface member without implementations as an external", () => {
    const run = callsFrom(a, "src/kinds.branches").find((c) => c.line === 30);
    assert.equal(run.callee, "src/kinds.{}.run");
    assert.ok(a.externals.some((e) => e.id === "src/kinds.{}.run"));
  });

  it("lists callees outside the tree as externals", () => {
    const ext = a.externals.find((e) => e.id === "lib.Math.round");
    assert.deepEqual(ext, { id: "lib.Math.round", name: "Math.round", container: "lib" });
    assert.ok(!a.externals.some((e) => a.functions.some((f) => f.id === e.id)));
  });

  it("marks the exports of the package's entry points as its API", () => {
    assert.deepEqual(entry(fn(a, "src/util.round"), "exported_api"), ["@demo/tslib.roundTo", "@demo/tslib/util.round"]);
    assert.deepEqual(entry(fn(a, "src/repository.MemoryStore.get"), "exported_api"), ["@demo/tslib.MemoryStore.get"]);
    assert.deepEqual(entry(fn(a, "src/index.version"), "exported_api"), ["@demo/tslib.default"]);
    assert.deepEqual(entry(fn(a, "src/repository.print"), "exported_api"), []);
    assert.deepEqual(entry(fn(a, "src/repository.Repository.audit"), "exported_api"), [], "private");
  });

  it("marks top-level tests as entry points registered by their module", () => {
    assert.deepEqual(entry(fn(a, 'src/shapes.test.describe("totalArea")'), "test"), ["totalArea"]);
    assert.deepEqual(entry(fn(a, 'src/shapes.test.describe("totalArea").it("adds the areas")'), "test"), []);
    const reg = call(a, 'src/shapes.test.describe("totalArea")', 'src/shapes.test.describe("totalArea").it("adds the areas")', "registration")[0];
    assert.equal(reg.evidence, "it adds the areas");
    assert.equal(reg.kind, "async");
  });

  it("reports files that do not parse as diagnostics and keeps going", () => {
    const parse = a.diagnostics.filter((d) => d.file === "src/broken.ts");
    assert.ok(parse.length > 0 && parse.every((d) => d.level === "error" && d.line >= 1));
    assert.ok(fn(a, "src/broken.fine"));
  });

  it("gives the same bytes for the same tree", () => {
    const again = cli(["analyze", "--dir", fixture("tslib")]).json;
    assert.deepEqual(again, a);
  });
});

describe("analyze: an HTTP server", () => {
  const a = analysisOf("express-app");

  it("finds Express routes with the prefixes their routers are mounted at", () => {
    assert.deepEqual(entry(fn(a, "src/routes/users.getUser"), "http_route"), ["GET /api/users/:id"]);
    assert.deepEqual(entry(fn(a, "src/routes/users.createUser"), "http_route"), ["POST /api/users"]);
    assert.deepEqual(entry(fn(a, "src/routes/users.getAvatar"), "http_route"), ["GET /api/users/:id/avatar"]);
    assert.deepEqual(entry(fn(a, "src/server.health"), "http_route"), ["GET /health"]);
    assert.deepEqual(entry(fn(a, "src/server.(module)$1"), "http_route"), ["POST /echo"]);
    const reg = call(a, "src/routes/users.(module)", "src/routes/users.getUser", "registration")[0];
    assert.equal(reg.evidence, "route GET /api/users/:id");
    assert.equal(reg.line, 9);
  });

  it("registers middleware without making it an entry point", () => {
    const reg = call(a, "src/server.(module)", "src/middleware.requireUser", "registration")[0];
    assert.equal(reg.evidence, "middleware");
    assert.equal(fn(a, "src/middleware.requireUser").entryPoints, undefined);
  });

  it("finds Fastify routes, route objects and plugin prefixes", () => {
    assert.deepEqual(entry(fn(a, "src/fastify.(module)$1"), "http_route"), ["GET /ping"]);
    assert.deepEqual(entry(fn(a, "src/fastify.createItem"), "http_route"), ["POST /items"]);
    assert.deepEqual(entry(fn(a, "src/fastify.listItem"), "http_route"), ["GET /v2/items/:id"]);
    assert.equal(call(a, "src/fastify.(module)", "src/fastify.itemsPlugin", "registration")[0].evidence, "plugin");
  });

  it("finds NestJS handlers from their decorators", () => {
    assert.deepEqual(entry(fn(a, "src/orders.controller.OrdersController.findOne"), "http_route"), ["GET /orders/:id"]);
    assert.deepEqual(entry(fn(a, "src/orders.controller.OrdersController.create"), "http_route"), ["POST /orders"]);
    assert.deepEqual(entry(fn(a, "src/orders.controller.OrdersController.onPaid"), "message_handler"), ["order.paid"]);
    assert.deepEqual(entry(fn(a, "src/orders.controller.OrdersController.sweep"), "scheduled_job"), ["0 * * * *"]);
  });

  it("makes the main module of an application its main entry point", () => {
    assert.deepEqual(entry(fn(a, "src/server.(module)"), "main"), ["express-app"]);
  });

  it("names externals after their imports, so they do not depend on @types", () => {
    assert.ok(a.externals.some((e) => e.id === "express.Router"));
    assert.ok(a.externals.some((e) => e.id === "express"));
    assert.ok(a.externals.some((e) => e.id === "express-serve-static-core.Response.json"), "typed through @types/express");
  });
});

describe("analyze: a JavaScript project without tsconfig", () => {
  const a = analysisOf("jsproj");

  it("reports JavaScript and skips build output", () => {
    assert.equal(a.language, "javascript");
    assert.ok(!a.functions.some((f) => f.file.startsWith("dist/")));
    assert.equal(fn(a, "lib/vendor.min.v").generated, true);
  });

  it("makes a bin its main entry point and finds commander commands", () => {
    assert.deepEqual(entry(fn(a, "bin/cli.(module)"), "main"), ["democli"]);
    assert.deepEqual(entry(fn(a, "bin/cli.shoutCommand"), "cli_command"), ["shout"]);
    assert.deepEqual(entry(fn(a, "bin/cli.(module)$1"), "cli_command"), ["greet"]);
    assert.equal(call(a, "bin/cli.(module)", "bin/cli.shoutCommand", "registration")[0].evidence, "command shout");
  });

  it("follows CommonJS requires and exports", () => {
    assert.equal(call(a, "bin/cli.shoutCommand", "lib/greet.shout").length, 1);
    assert.equal(call(a, "lib/greet.shout", "lib/greet.greet").length, 1);
    assert.deepEqual(entry(fn(a, "lib/index.exports.startTicker"), "exported_api"), ["democli.startTicker"]);
    assert.equal(call(a, "lib/legacy.Counter.increment", "lib/legacy.Counter").length, 0);
  });

  it("registers event listeners and calls timer and promise callbacks asynchronously", () => {
    assert.equal(call(a, "lib/index.exports.startTicker", "lib/index.onTick", "registration")[0].evidence, "event tick");
    assert.equal(call(a, "lib/events.Ticker.start", "lib/events.Ticker.timer$1")[0].kind, "async");
    assert.equal(call(a, "lib/events.load", "lib/events.load$1")[0].kind, "async");
  });

  it("calls JSX components and references handlers passed as props", () => {
    assert.equal(call(a, "src/App.App", "src/App.Button")[0].kind, "always");
    assert.equal(call(a, "src/App.App", "src/App.App.handleClick", "reference").length, 1);
  });
});

describe("analyze: edge cases", () => {
  it("analyses an empty tree with a warning", () => {
    const dir = mkdtempSync(join(tmpdir(), "jsts-empty-"));
    try {
      const { code, json } = cli(["analyze", "--dir", dir]);
      assert.equal(code, 0);
      assert.equal(json.language, "javascript");
      assert.deepEqual(json.functions, []);
      assert.equal(json.diagnostics[0].level, "warning");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("does not write inside the tree", () => {
    const dir = mkdtempSync(join(tmpdir(), "jsts-ro-"));
    try {
      cpSync(fixture("tslib"), dir, { recursive: true });
      writeFileSync(join(dir, "tsconfig.json"), '{ "compilerOptions": { "incremental": true, "composite": true, "outDir": "out" } }');
      const { code } = cli(["analyze", "--dir", dir]);
      assert.equal(code, 0);
      assert.ok(!readdirSync(dir).some((name) => name.endsWith(".tsbuildinfo") || name === "out"));
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("joins the evidence of registrations on one line", () => {
    const calls = dedupeCalls([
      { caller: "a", callee: "b", line: 1, kind: "async", resolution: "static", relation: "registration", evidence: "route GET /x" },
      { caller: "a", callee: "b", line: 1, kind: "async", resolution: "static", relation: "registration", evidence: "route POST /x" },
      { caller: "a", callee: "b", line: 1, kind: "always", resolution: "static", relation: "call" },
    ]);
    assert.equal(calls.length, 2);
    assert.equal(calls.find((c) => c.relation === "registration").evidence, "route GET /x; route POST /x");
  });
});
