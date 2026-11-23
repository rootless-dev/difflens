import assert from "node:assert/strict";
import { mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "node:test";

import { parseDuration, parseOptions } from "../src/cli.js";
import { cli, fixture } from "./helpers.js";

describe("command line", () => {
  it("prints its version", () => {
    for (const flag of ["--version", "-version", "version"]) {
      const { code, stdout } = cli([flag]);
      assert.equal(code, 0);
      assert.equal(stdout, "difflens-analyzer-jsts 0.0.0\n");
    }
  });

  it("reads Go durations", () => {
    assert.equal(parseDuration("120s"), 120);
    assert.equal(parseDuration("1m30s"), 90);
    assert.equal(parseDuration("1.5h"), 5400);
    assert.throws(() => parseDuration("10"));
    assert.throws(() => parseDuration(""));
  });

  it("exits 2 on a bad command line", () => {
    assert.equal(cli([]).code, 2);
    assert.equal(cli(["frobnicate"]).code, 2);
    assert.equal(cli(["analyze"]).code, 2);
    assert.equal(cli(["analyze", "--dir", fixture("tslib"), "--bogus"]).code, 2);
    assert.equal(cli(["analyze", "--dir", fixture("tslib"), "extra"]).code, 2);
    assert.equal(cli(["analyze", "--dir", fixture("tslib"), "--timeout", "0s"]).code, 2);
    assert.equal(cli(["check", "--dir", fixture("tslib"), "--no-cover"]).code, 2);
  });

  it("exits 2 for a selection it cannot read, 1 for a tree that is not a folder", () => {
    const dir = mkdtempSync(join(tmpdir(), "jsts-cli-"));
    try {
      writeFileSync(join(dir, "selection.json"), '{"nope": true}');
      assert.equal(cli(["test", "--dir", dir, "--selection", join(dir, "selection.json")]).code, 2);
      assert.equal(cli(["test", "--dir", dir, "--selection", join(dir, "missing.json")]).code, 2);
      const missing = cli(["analyze", "--dir", join(dir, "missing")]);
      assert.equal(missing.code, 1);
      assert.match(missing.stderr, /is not a directory/);
      assert.equal(missing.stdout, "");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("takes --cache-dir on prepare, test and check, as --flag value and --flag=value", () => {
    for (const command of ["prepare", "test", "check"]) {
      assert.equal(parseOptions(command, ["--dir", "/t", "--cache-dir", "/c"])["cache-dir"], "/c");
      assert.equal(parseOptions(command, ["--dir=/t", "--cache-dir=/c d"])["cache-dir"], "/c d");
      assert.equal(parseOptions(command, ["--dir", "/t"])["cache-dir"], "", "optional");
    }
    assert.throws(() => parseOptions("analyze", ["--dir", "/t", "--cache-dir", "/c"]), /cache-dir/);
    assert.throws(() => parseOptions("check", ["--dir", "/t", "--cache-dir"]), /cache-dir/);
  });

  it("runs check with a cache folder it leaves as it was", () => {
    const dir = mkdtempSync(join(tmpdir(), "jsts-cache-"));
    try {
      const { code, json } = cli(["check", "--dir", fixture("tslib"), "--timeout", "30s", "--cache-dir", dir]);
      assert.equal(code, 0);
      assert.equal(json.checks.length, 2);
      assert.deepEqual(readdirSync(dir), []);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("accepts --flag=value and logs JSON lines on stderr", () => {
    const { code, stderr, json } = cli([`analyze`, `--dir=${fixture("jsproj")}`, "--timeout=30s"], { env: { DIFFLENS_LOG_LEVEL: "nonsense" } });
    assert.equal(code, 0);
    assert.equal(json.language, "javascript");
    const lines = stderr.trim().split("\n").map((l) => JSON.parse(l));
    assert.ok(lines.every((l) => l.service === "difflens-analyzer-jsts" && l.time && l.level && l.msg));
    assert.ok(lines.some((l) => l.level === "info"), "an unknown level is info");
  });
});
