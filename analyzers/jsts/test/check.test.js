import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "node:test";

import { eslintFolders, eslintSeverity, parseEslintOutput, parseTscOutput, typeScriptProjects } from "../src/check.js";
import { Tree } from "../src/files.js";
import { cli, copy, fixture, installed } from "./helpers.js";

describe("check: parsing the checkers' output", () => {
  const tree = new Tree("/p");

  it("reads tsc's findings with their continuation lines and severities, in the tree only", () => {
    const out = [
      "src/a.ts(3,7): error TS2322: Type 'string' is not assignable to type 'number'.",
      "  The expected type comes from here.",
      "node_modules/x/index.d.ts(1,1): error TS1005: ';' expected.",
      "src/b.ts(1,1): error TS6305: Output file '/p/dist/b.d.ts' has not been built from source file '/p/b.ts'.",
      "src/c.ts(2,1): warning TS6133: 'x' is declared but its value is never read.",
      "src/c.ts(4,1): message TS6385: 'f' is deprecated.",
      "src/c.ts(5,1): suggestion TS80001: File is a CommonJS module.",
      "error TS5083: Cannot read file '/p/base.json'.",
    ].join("\n");
    const { findings, globals } = parseTscOutput(out, "/p", tree);
    assert.deepEqual(findings, [
      { file: "src/a.ts", line: 3, message: "Type 'string' is not assignable to type 'number'.\nThe expected type comes from here.", rule: "TS2322", severity: "error" },
      { file: "src/c.ts", line: 2, message: "'x' is declared but its value is never read.", rule: "TS6133", severity: "warning" },
      { file: "src/c.ts", line: 4, message: "'f' is deprecated.", rule: "TS6385", severity: "info" },
      { file: "src/c.ts", line: 5, message: "File is a CommonJS module.", rule: "TS80001", severity: "info" },
    ]);
    assert.deepEqual(globals, ["TS5083: Cannot read file '/p/base.json'."]);
  });

  it("reads ESLint's JSON report with severities and the messages as ESLint words them", () => {
    const report = JSON.stringify([
      {
        filePath: "/p/src/a.js",
        messages: [
          { ruleId: "no-undef", severity: 2, message: "'x' is not defined.", line: 4 },
          { ruleId: null, severity: 1, message: "Unused directive", line: 1 },
          { ruleId: null, fatal: true, severity: 2, message: "Parsing error: Unexpected token", line: 9 },
        ],
      },
      { filePath: "/elsewhere/b.js", messages: [{ ruleId: "semi", severity: 2, message: "Missing semicolon.", line: 1 }] },
    ]);
    assert.deepEqual(parseEslintOutput(report, tree), [
      { file: "src/a.js", line: 4, message: "'x' is not defined.", rule: "no-undef", severity: "error" },
      { file: "src/a.js", line: 1, message: "Unused directive", rule: null, severity: "warning" },
      { file: "src/a.js", line: 9, message: "Parsing error: Unexpected token", rule: null, severity: "error" },
    ]);
    assert.equal(parseEslintOutput("Oops! Something went wrong!", tree), null);
  });

  it("maps ESLint's levels: 2 and fatal are errors, 1 a warning", () => {
    assert.equal(eslintSeverity({ severity: 2 }), "error");
    assert.equal(eslintSeverity({ severity: 1 }), "warning");
    assert.equal(eslintSeverity({ severity: 1, fatal: true }), "error");
  });

  it("finds the TypeScript projects and the ESLint configurations", () => {
    assert.deepEqual(typeScriptProjects(fixture("tslib")).map((p) => p.solution), [false]);
    assert.deepEqual(eslintFolders(fixture("eslint-proj")).map((f) => f.flat), [true]);
    assert.deepEqual(eslintFolders(fixture("tslib")), []);
  });

  it("finds ESLint configurations in the packages of a workspace and in the folders one down", () => {
    assert.deepEqual(
      eslintFolders(fixture("pnpm-workspace")).map((f) => [f.dir, f.flat]),
      [[join(fixture("pnpm-workspace"), "packages/app"), true]],
    );
    const root = mkdtempSync(join(tmpdir(), "jsts-eslint-"));
    try {
      writeFileSync(join(root, "package.json"), JSON.stringify({ private: true, workspaces: ["libs/*"] }));
      for (const dir of ["libs/a", "libs/a/nested", "web", "libs/b"]) mkdirSync(join(root, dir), { recursive: true });
      writeFileSync(join(root, "libs/a/package.json"), "{}");
      writeFileSync(join(root, "libs/a/eslint.config.js"), "export default [];");
      writeFileSync(join(root, "libs/b/package.json"), "{}");
      writeFileSync(join(root, "web/.eslintrc.json"), "{}");
      assert.deepEqual(
        eslintFolders(root).map((f) => [f.dir.slice(root.length + 1), f.flat]),
        [["libs/a", true], ["web", false]],
      );
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

describe("check: the command", () => {
  it("lists tsc and eslint, in order, unavailable when not installed", () => {
    const { code, json } = cli(["check", "--dir", fixture("tslib")]);
    assert.equal(code, 0);
    assert.deepEqual(json.checks.map((c) => [c.tool, c.status]), [["tsc", "unavailable"], ["eslint", "unavailable"]]);
    assert.ok(json.checks.every((c) => typeof c.message === "string" && c.message.length > 0));
  });

  it("reports the project's type errors with tsc", { skip: !installed("typeerror", "typescript") && "npm install in testdata/typeerror" }, () => {
    const { code, json } = cli(["check", "--dir", fixture("typeerror")]);
    assert.equal(code, 0);
    const tsc = json.checks[0];
    assert.equal(tsc.status, "findings");
    assert.deepEqual(tsc.findings, [{ file: "src/total.ts", line: 11, message: "Type 'string' is not assignable to type 'number'.", rule: "TS2322", severity: "error" }]);
  });

  it("reports the project's lint findings with eslint", { skip: !installed("eslint-proj", "eslint") && "npm install in testdata/eslint-proj" }, () => {
    const { code, json } = cli(["check", "--dir", fixture("eslint-proj")]);
    assert.equal(code, 0);
    const eslint = json.checks[1];
    assert.equal(eslint.status, "findings");
    assert.deepEqual(
      eslint.findings.map((f) => [f.line, f.rule, f.severity]),
      [
        [2, "no-unused-vars", "error"],
        [4, "no-undef", "error"],
        [6, "no-console", "warning"],
      ],
    );
    assert.equal(eslint.findings[2].message, "Unexpected console statement.");
  });

  it(
    "uses the tsc, eslint and plugins pnpm installed in a workspace package, under the sandbox",
    { skip: !existsSync(join(fixture("pnpm-workspace"), "packages/app/node_modules/eslint/package.json")) && "pnpm install in testdata/pnpm-workspace" },
    () => {
      // As the core does it: a copy of the prepared tree (pnpm's symbolic links kept), and a cache folder.
      const c = copy("pnpm-workspace");
      try {
        const cacheDir = join(c.run, "cache");
        mkdirSync(cacheDir);
        const { code, json, stderr } = cli(["check", "--dir", c.tree, "--timeout", "120s", "--cache-dir", cacheDir], { env: { DIFFLENS_LOG_LEVEL: "info" } });
        assert.equal(code, 0);
        assert.deepEqual(json.checks.map((ch) => [ch.tool, ch.status]), [["tsc", "findings"], ["eslint", "findings"]]);
        assert.deepEqual(json.checks[0].findings, [
          { file: "packages/app/src/index.ts", line: 6, message: "Type 'string' is not assignable to type 'number'.", rule: "TS2322", severity: "error" },
        ]);
        // eslint.config.ts (loaded through jiti) and typescript-eslint's rules, reached through pnpm's links.
        assert.deepEqual(
          json.checks[1].findings.map((f) => [f.line, f.rule, f.severity]),
          [
            [2, "@typescript-eslint/no-unused-vars", "error"],
            [5, "no-console", "warning"],
          ],
        );
        assert.deepEqual(json.diagnostics, []);
        const logs = stderr.trim().split("\n").map((l) => JSON.parse(l));
        const done = logs.filter((l) => l.msg === "tsc done" || l.msg === "eslint done");
        assert.ok(done.length === 2 && done.every((l) => l.sandboxed === (process.platform === "darwin")));
        assert.equal(done.find((l) => l.msg === "tsc done").typescript, "5.9.3", "the project's typescript, not the analyzer's");
      } finally {
        c.done();
      }
    },
  );
});
