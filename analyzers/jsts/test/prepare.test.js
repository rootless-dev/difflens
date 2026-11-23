import assert from "node:assert/strict";
import { cpSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "node:test";

import { installCommand, installRoots } from "../src/prepare.js";
import { which } from "../src/process.js";
import { cli, fixture } from "./helpers.js";

function tree(files) {
  const dir = mkdtempSync(join(tmpdir(), "jsts-prepare-"));
  for (const [rel, text] of Object.entries(files)) {
    mkdirSync(join(dir, rel, ".."), { recursive: true });
    writeFileSync(join(dir, rel), text);
  }
  return dir;
}

describe("prepare", () => {
  it("picks the package manager by lockfile", () => {
    const cases = [
      [{ "pnpm-lock.yaml": "" }, "pnpm", "--frozen-lockfile"],
      [{ "yarn.lock": "# yarn lockfile v1\n" }, "yarn", "--frozen-lockfile"],
      [{ "yarn.lock": "__metadata:\n  version: 8\n" }, "yarn", "--immutable"],
      [{ "bun.lock": "" }, "bun", "--frozen-lockfile"],
      [{ "package-lock.json": "{}" }, "npm", "ci"],
      [{}, "npm", "--no-package-lock"],
    ];
    for (const [files, tool, flag] of cases) {
      const dir = tree(files);
      try {
        const command = installCommand(dir, {});
        assert.equal(command.tool, tool);
        assert.ok(command.args.includes(flag), `${tool} ${command.args}`);
        assert.ok(command.args.includes("--ignore-scripts") || command.args.includes("--mode=skip-build"));
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    }
    assert.equal(installCommand("/nowhere", { packageManager: "pnpm@9.0.0" }).tool, "pnpm");
  });

  it("installs the root package, else the packages one folder down", () => {
    const dir = tree({ "a/package.json": "{}", "b/package.json": "{}", "node_modules/x/package.json": "{}", "c/x.js": "" });
    try {
      assert.deepEqual(installRoots(dir).map((d) => d.slice(dir.length + 1)), ["a", "b"]);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("installs nothing for a package without dependencies and needs no writable folder", () => {
    const dir = tree({ "package.json": '{"name":"x","scripts":{"test":"node --test"}}', "index.js": "" });
    try {
      const { code, json } = cli(["prepare", "--dir", dir]);
      assert.equal(code, 0);
      assert.deepEqual(json, { protocol: 1, writable: [] });
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it(
    "prepares a copy of a pnpm workspace whose root depends on nothing, for check to use its tools (as the core does)",
    { skip: !which("pnpm") && "pnpm is not on the PATH" },
    () => {
      const run = mkdtempSync(join(tmpdir(), "jsts-prepare-"));
      const copy = join(run, "tree");
      const cacheDir = join(run, "cache");
      mkdirSync(cacheDir);
      try {
        cpSync(fixture("pnpm-workspace"), copy, { recursive: true, filter: (src) => !src.split("/").includes("node_modules") });
        const prepared = cli(["prepare", "--dir", copy, "--cache-dir", cacheDir]);
        assert.equal(prepared.code, 0, prepared.stderr);
        assert.deepEqual(prepared.json, { protocol: 1, writable: [] });
        assert.ok(existsSync(join(copy, "packages/app/node_modules/eslint/package.json")));
        const checked = cli(["check", "--dir", copy, "--timeout", "120s", "--cache-dir", cacheDir]);
        assert.equal(checked.code, 0);
        assert.deepEqual(checked.json.checks.map((c) => [c.tool, c.status]), [["tsc", "findings"], ["eslint", "findings"]]);
      } finally {
        rmSync(run, { recursive: true, force: true });
      }
    },
  );

  it("fails with what the package manager said", () => {
    const dir = tree({ "package.json": '{"dependencies":{"x":"1"}}', "package-lock.json": "not json" });
    try {
      const { code, stderr } = cli(["prepare", "--dir", dir], { env: { npm_config_offline: "true" } });
      assert.equal(code, 1);
      assert.match(stderr, /npm ci/);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
