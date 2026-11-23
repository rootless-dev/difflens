import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";

import ts from "typescript";

import { readChanged } from "../src/cli.js";
import { buildGroups, isNewSide, mergeRanges } from "../src/dupes/groups.js";
import { addedLineFlags, findDuplicates, isExcluded, keepsPair, scopeOf } from "../src/dupes/index.js";
import { candidateGroups, maximalPairs, touchedPrefix, windowKeys } from "../src/dupes/search.js";
import { ID, NUM, STR, tokenize } from "../src/dupes/tokens.js";
import { cli, copy, fixture } from "./helpers.js";
import { protocolSchema, validate } from "./schema.js";

const DUPES = fixture("dupes");
const CHANGED = join(DUPES, "changed.json");

/** The tokens of `text` as words: ID, STR, NUM, or the token's own text. */
function words(text, rel = "a.ts") {
  const { ids } = tokenize(text, rel);
  return [...ids].map((id) => (id === ID ? "ID" : id === STR ? "STR" : id === NUM ? "NUM" : ts.tokenToString(id))).join(" ");
}

describe("dupes: tokens", () => {
  it("reads template literals as their pieces, nested templates included", () => {
    assert.equal(words("const s = `a${b + `c${d}`}e${f}g`;\nconst t = `plain`;"), "const ID = STR ID + STR ID STR STR ID STR ; const ID = STR ;");
  });

  it("tells a regular expression from a division", () => {
    assert.equal(words("const r = /x\\/y}/g.test(s);\nconst q = a / b / c;"), "const ID = STR . ID ( ID ) ; const ID = ID / ID / ID ;");
  });

  it("reads JSX tags as identifiers and the text between them as one string", () => {
    const text = 'const e = <div className="x" {...p}>\n  Hello {name} world\n  <br/>\n</div>;';
    const expected = "const ID = < ID ID = STR { ... ID } > STR { ID } STR < ID / > </ ID > ;";
    assert.equal(words(text, "a.tsx"), expected);
    assert.equal(words(text, "a.jsx"), expected);
    const { ids, startLines } = tokenize(text, "a.tsx");
    assert.deepEqual([...startLines].filter((_, i) => ids[i] === STR), [1, 2, 2], "JSX text starts where its words do");
  });

  it("drops comments, and makes numbers, bigints and private names placeholders", () => {
    assert.equal(
      words("class A {\n  // a comment\n  #p = 1.5; /* another */\n  m() { return this.#p ?? 2n; }\n}"),
      "class ID { ID = NUM ; ID ( ) { return this . ID ?? NUM ; } }",
    );
  });

  it("gives code that differs only in names and literals the same tokens", () => {
    assert.equal(words("f(a, 'x', 1, `t${u}`)"), words('renamed(other, "why", 2.5e3, `v${w}`)'));
    assert.notEqual(words("f(a, b)"), words("f(a, b, c)"));
  });

  it("leaves imports and top-level requires out, and they split the file", () => {
    const text = [
      "import a from 'a';",
      "const x = 1;",
      "import b = require('b');",
      "const y = 2;",
      "export * from 'c';",
      "const z = 3;",
      "const { r } = require('r');",
      "require('side');",
      "const w = 4;",
      "function f() { const k = require('k'); return k; }",
      "export { x };",
    ].join("\n");
    const { startLines, splits } = tokenize(text, "a.ts");
    assert.deepEqual([...new Set(startLines)], [2, 4, 6, 9, 10, 11]);
    const splitOf = (line) => splits[startLines.indexOf(line)];
    assert.equal(new Set([2, 4, 6, 9].map(splitOf)).size, 4);
    assert.equal(splitOf(9), splitOf(10));
    assert.equal(splitOf(10), splitOf(11));
    assert.match(words("function f() { const k = require('k'); }"), /require|ID \( STR \)/);
  });

  it("keeps the line each token starts and ends on", () => {
    const { startLines, endLines } = tokenize("const s = `one\ntwo`;\n", "a.ts");
    assert.deepEqual([...startLines], [1, 1, 1, 1, 2]);
    assert.deepEqual([...endLines], [1, 1, 1, 2, 2]);
  });
});

/** A file as search.js reads it, from token ids; every token on line 1 + its index, all added when `added`. */
function file(ids, { added = false, splits = null } = {}) {
  const startLines = Int32Array.from(ids, (_, i) => i + 1);
  const addedLines = new Uint8Array(ids.length + 2).fill(added ? 1 : 0);
  return {
    ids: Int32Array.from(ids),
    splits: splits === null ? new Int32Array(ids.length) : Int32Array.from(splits),
    startLines,
    endLines: startLines,
    touched: touchedPrefix(startLines, addedLines),
  };
}

/** Token ids no other call returns. */
let nextId = 10000;
const fresh = (count) => Array.from({ length: count }, () => nextId++);

function pairsOf(files, size) {
  const keys = files.map((f) => windowKeys(f.ids, f.splits, size));
  return maximalPairs(files, candidateGroups(files, keys, size), size).pairs;
}

describe("dupes: windows and pairs", () => {
  it("rolls the hash of each window as if hashed on its own, and leaves out windows across splits", () => {
    const ids = Array.from({ length: 200 }, (_, i) => (i * 7919) % 97);
    const splits = ids.map((_, i) => (i < 120 ? 0 : 1));
    const keys = windowKeys(Int32Array.from(ids), Int32Array.from(splits), 10);
    assert.equal(keys.length, 191);
    for (let i = 0; i < keys.length; i++) {
      if (i > 110 && i < 120) {
        assert.equal(keys[i], -1, `window ${i} crosses the split`);
      } else {
        assert.equal(keys[i], windowKeys(Int32Array.from(ids.slice(i, i + 10)), new Int32Array(10), 10)[0], `window ${i}`);
      }
    }
    assert.equal(windowKeys(new Int32Array(5), new Int32Array(5), 10).length, 0);
  });

  it("extends a long clone once into one maximal pair", () => {
    const clone = fresh(5000);
    const existing = file([...fresh(3), ...clone, ...fresh(3)]);
    const added = file([...fresh(2), ...clone, ...fresh(2)], { added: true });
    const started = Date.now();
    assert.deepEqual(pairsOf([existing, added], 50), [{ a: 0, p: 3, b: 1, q: 2, length: 5000 }]);
    assert.ok(Date.now() - started < 2000, "linear in the clone, not quadratic");
  });

  it("never lets the two sides of a pair in one file overlap", () => {
    const block = fresh(60);
    const one = file([...fresh(1), ...block, ...block, ...fresh(1)], { added: true });
    assert.deepEqual(pairsOf([one], 50), [{ a: 0, p: 1, b: 0, q: 61, length: 60 }]);
  });

  it("pairs only windows touching the change, and drops groups of more than 64 windows", () => {
    const block = fresh(50);
    assert.deepEqual(pairsOf([file(block), file(block)], 50), []);
    const many = Array.from({ length: 65 }, (_, i) => file(block, { added: i === 0 }));
    assert.deepEqual(pairsOf(many, 50), []);
    const enough = Array.from({ length: 64 }, (_, i) => file(block, { added: i === 0 }));
    assert.equal(pairsOf(enough, 50).length, 63);
  });

  it("stops at a split", () => {
    const block = fresh(80);
    const a = file([...block], { splits: block.map((_, i) => (i < 60 ? 0 : 1)) });
    const b = file([...block], { added: true });
    assert.deepEqual(pairsOf([a, b], 50), [{ a: 0, p: 0, b: 1, q: 0, length: 60 }]);
  });
});

const side = (f, startLine, endLine) => ({ file: f, startLine, endLine });

describe("dupes: sides, blocks and groups", () => {
  it("calls a side new when at least half of its lines are added", () => {
    const flags = addedLineFlags([[1, 4]], 20);
    assert.ok(isNewSide(1, 8, flags));
    assert.ok(!isNewSide(1, 9, flags));
    assert.ok(!isNewSide(5, 12, flags));
    assert.deepEqual([...addedLineFlags([[2, 3], [9, 40]], 10)], [0, 0, 1, 1, 0, 0, 0, 0, 0, 1, 1, 0]);
  });

  it("keeps a pair only when both sides reach --min-lines and one of them is new", () => {
    const pair = (aLines, bLines, fresh = [true, false]) => ({
      sides: [side("a.ts", 1, aLines), side("b.ts", 1, bLines)],
      fresh,
      tokens: 60,
    });
    assert.ok(keepsPair(pair(6, 6), 6));
    assert.ok(keepsPair(pair(6, 9), 6));
    assert.ok(!keepsPair(pair(5, 9), 6), "a side below the minimum drops the pair");
    assert.ok(!keepsPair(pair(9, 5), 6), "whichever side it is");
    assert.ok(!keepsPair(pair(9, 9, [false, false]), 6), "a pair without a new side is dropped");
  });

  it("merges ranges of a file whose lines overlap, not those that only touch", () => {
    assert.deepEqual(mergeRanges([side("b", 1, 5), side("a", 10, 20), side("a", 15, 30), side("a", 31, 40), side("a", 1, 3)]), [
      side("a", 1, 3),
      side("a", 10, 30),
      side("a", 31, 40),
      side("b", 1, 5),
    ]);
  });

  it("merges overlapping new sides into one block and its existing occurrences per file", () => {
    const pairs = [
      { sides: [side("a.ts", 10, 20), side("old.ts", 1, 11)], fresh: [true, false], tokens: 80 },
      { sides: [side("old.ts", 40, 55), side("a.ts", 15, 30)], fresh: [false, true], tokens: 90 },
      { sides: [side("a.ts", 18, 25), side("old.ts", 5, 14)], fresh: [true, false], tokens: 60 },
    ];
    assert.deepEqual(buildGroups(pairs).duplicates, [
      { kind: "copies_existing", block: side("a.ts", 10, 30), others: [side("old.ts", 1, 14), side("old.ts", 40, 55)], tokens: 90, lines: 21 },
    ]);
  });

  it("groups blocks repeated in the change transitively, the first one as the block", () => {
    const pairs = [
      { sides: [side("c.ts", 1, 10), side("b.ts", 1, 10)], fresh: [true, true], tokens: 70 },
      { sides: [side("c.ts", 5, 12), side("d.ts", 3, 10)], fresh: [true, true], tokens: 75 },
      // A block whose only occurrence copies existing code has no group of its own.
      { sides: [side("e.ts", 1, 10), side("a.ts", 12, 20)], fresh: [true, true], tokens: 66 },
      { sides: [side("a.ts", 12, 20), side("old.ts", 1, 9)], fresh: [true, false], tokens: 66 },
    ];
    const { duplicates, dropped } = buildGroups(pairs);
    assert.equal(dropped, 0);
    assert.deepEqual(duplicates, [
      { kind: "copies_existing", block: side("a.ts", 12, 20), others: [side("old.ts", 1, 9)], tokens: 66, lines: 9 },
      { kind: "repeated_in_change", block: side("b.ts", 1, 10), others: [side("c.ts", 1, 12), side("d.ts", 3, 10)], tokens: 75, lines: 10 },
    ]);
    const limited = buildGroups(pairs, 1);
    assert.equal(limited.duplicates.length, 1);
    assert.equal(limited.dropped, 1);
  });
});

describe("dupes: the command", () => {
  it("reports the copies the change adds, and nothing else, in the fixture", () => {
    const { code, json, stderr } = cli(["dupes", "--dir", DUPES, "--changed", CHANGED]);
    assert.equal(code, 0, stderr);
    assert.deepEqual(json, {
      protocol: 1,
      language: "typescript",
      runner: { name: "difflens-analyzer-jsts", version: "0.0.0" },
      minTokens: 50,
      minLines: 6,
      duplicates: [
        {
          kind: "repeated_in_change",
          scope: "production",
          block: { file: "src/added/handlers.ts", startLine: 2, endLine: 14 },
          others: [
            { file: "src/added/handlers.ts", startLine: 18, endLine: 30 },
            { file: "src/added/handlers.ts", startLine: 36, endLine: 46 },
          ],
          tokens: 113,
          lines: 13,
        },
        {
          kind: "copies_existing",
          scope: "production",
          block: { file: "src/billing/summary.ts", startLine: 7, endLine: 17 },
          others: [{ file: "src/existing/invoice.ts", startLine: 11, endLine: 21 }],
          tokens: 100,
          lines: 11,
        },
      ],
      diagnostics: [],
    });
  });

  it("prints a document the schema accepts, and the fixture's changed file matches the schema too", () => {
    const schema = protocolSchema("dupes.schema.json");
    const { json } = cli(["dupes", "--dir", DUPES, "--changed", CHANGED, "--min-tokens=30", "--min-lines=5"]);
    assert.deepEqual(validate(json, schema), []);
    assert.deepEqual(validate(JSON.parse(readFileSync(CHANGED, "utf8")), schema.$defs.changed, schema), []);
    assert.ok(validate({ ...json, extra: 1 }, schema).length > 0, "the checker itself rejects an unknown field");
  });

  it("finds the near miss once --min-tokens and --min-lines are lowered", () => {
    const strict = cli(["dupes", "--dir", DUPES, "--changed", CHANGED, "--min-tokens", "30"]).json;
    assert.ok(!strict.duplicates.some((d) => d.block.file === "src/added/limits.ts"), "5 lines are below --min-lines 6");
    const loose = cli(["dupes", "--dir", DUPES, "--changed", CHANGED, "--min-tokens", "30", "--min-lines", "5"]).json;
    assert.equal(loose.minTokens, 30);
    assert.equal(loose.minLines, 5);
    assert.deepEqual(
      loose.duplicates.find((d) => d.block.file === "src/added/limits.ts"),
      {
        kind: "copies_existing",
        scope: "production",
        block: { file: "src/added/limits.ts", startLine: 1, endLine: 5 },
        others: [{ file: "src/existing/math.ts", startLine: 6, endLine: 10 }],
        tokens: 41,
        lines: 5,
      },
    );
    const fewer = cli(["dupes", "--dir", DUPES, "--changed", CHANGED, "--min-lines", "12"]).json;
    assert.deepEqual(fewer.duplicates.map((d) => d.kind), ["repeated_in_change"]);
  });

  it("would report the generated copies under plain names", () => {
    const tree = copy("dupes");
    try {
      const added = join(tree.tree, "src", "added");
      renameSync(join(added, "schema.generated.ts"), join(added, "schema.ts"));
      renameSync(join(added, "vendor.min.js"), join(added, "vendor.js"));
      const client = readFileSync(join(added, "client.ts"), "utf8");
      writeFileSync(join(added, "client.ts"), client.slice(client.indexOf("\n") + 1));
      const changed = JSON.parse(readFileSync(CHANGED, "utf8"));
      for (const entry of changed.files) entry.path = entry.path.replace("schema.generated.ts", "schema.ts").replace("vendor.min.js", "vendor.js");
      writeFileSync(join(tree.run, "changed.json"), JSON.stringify(changed));
      const { json } = cli(["dupes", "--dir", tree.tree, "--changed", join(tree.run, "changed.json")]);
      const copies = json.duplicates.filter((d) => d.kind === "copies_existing");
      assert.deepEqual(
        copies.map((d) => [d.block.file, d.others.map((o) => o.file)]),
        [
          ["src/added/client.ts", ["src/existing/invoice.ts"]],
          ["src/added/schema.ts", ["src/existing/invoice.ts"]],
          ["src/added/vendor.js", ["src/existing/legacy.cjs"]],
          ["src/billing/summary.ts", ["src/existing/invoice.ts"]],
        ],
      );
    } finally {
      tree.done();
    }
  });

  it("reports what it found and a diagnostic when it runs out of time", () => {
    const { added } = readChanged(CHANGED);
    const late = findDuplicates(DUPES, added, { deadline: Date.now() - 1 });
    assert.deepEqual(late.duplicates, []);
    assert.equal(late.diagnostics.length, 1);
    assert.match(late.diagnostics[0], /ran out of time/);
  });

  it("exits 2 without a readable --changed file or with a minimum below 1, 1 for a tree that is not a folder", () => {
    const dir = mkdtempSync(join(tmpdir(), "jsts-dupes-"));
    try {
      const write = (name, value) => {
        writeFileSync(join(dir, name), typeof value === "string" ? value : JSON.stringify(value));
        return join(dir, name);
      };
      const usage = (args) => {
        const result = cli(["dupes", "--dir", DUPES, ...args]);
        assert.equal(result.code, 2, `${args.join(" ")}: ${result.stderr}`);
        assert.equal(result.stdout, "");
        assert.ok(result.stderr.length > 0);
      };
      usage([]);
      usage(["--changed", join(dir, "missing.json")]);
      usage(["--changed", write("bad.json", "{")]);
      usage(["--changed", write("protocol.json", { protocol: 2, files: [] })]);
      usage(["--changed", write("unknown.json", { protocol: 1, files: [], extra: true })]);
      usage(["--changed", write("entry.json", { protocol: 1, files: [{ path: "a.ts", added: [[1, 2]], why: "x" }] })]);
      usage(["--changed", write("range.json", { protocol: 1, files: [{ path: "a.ts", added: [[0, 2]] }] })]);
      usage(["--changed", write("triple.json", { protocol: 1, files: [{ path: "a.ts", added: [[1, 2, 3]] }] })]);
      usage(["--changed", write("nofiles.json", { protocol: 1 })]);
      usage(["--changed", write("exclude-half.json", { protocol: 1, files: [], exclude: { dirs: [] } })]);
      usage(["--changed", write("exclude-type.json", { protocol: 1, files: [], exclude: { dirs: [1], extensions: [] } })]);
      usage(["--changed", write("exclude-extra.json", { protocol: 1, files: [], exclude: { dirs: [], extensions: [], names: [] } })]);
      usage(["--changed", write("exclude-list.json", { protocol: 1, files: [], exclude: [] })]);
      for (const bad of ["0", "-1", "abc", "1.5", ""]) {
        usage(["--changed", CHANGED, "--min-tokens", bad]);
        usage(["--changed", CHANGED, "--min-lines", bad]);
      }
      usage(["--changed", CHANGED, "--timeout", "0s"]);
      usage(["--changed", CHANGED, "--bogus"]);
      const missing = cli(["dupes", "--dir", join(dir, "missing"), "--changed", CHANGED]);
      assert.equal(missing.code, 1);
      assert.match(missing.stderr, /is not a directory/);
      const empty = cli(["dupes", "--dir", dir, "--changed", write("none.json", { protocol: 1, files: [] })]);
      assert.equal(empty.code, 0);
      assert.deepEqual(empty.json.duplicates, []);
      assert.equal(empty.json.language, "javascript");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

/** A function of about a hundred tokens over eight lines, named `name`: every copy is a clone of the others. */
function shipping(name) {
  return [
    `export function ${name}(order) {`,
    "  const weight = order.items.reduce((sum, item) => sum + item.weight * item.quantity, 0);",
    "  const base = weight > 10 ? 12.5 : 7.25;",
    '  const zone = order.destination.zone === "remote" ? 2 : 1;',
    "  const insured = order.total > 100 ? order.total * 0.01 : 0;",
    "  const handling = order.fragile ? base * 0.2 : 0;",
    "  return Math.round((base * zone + insured + handling) * 100) / 100;",
    "}",
    "",
  ].join("\n");
}

/**
 * A throwaway tree of `files` (relative path -> text) and a --changed file adding the whole of each file of
 * `added`, with `extra` merged into it. Returns `{ tree, changed, done }`.
 */
function scratchTree(files, added, extra = {}) {
  const run = mkdtempSync(join(tmpdir(), "jsts-dupes-scope-"));
  const tree = join(run, "tree");
  for (const [rel, text] of Object.entries(files)) {
    mkdirSync(dirname(join(tree, rel)), { recursive: true });
    writeFileSync(join(tree, rel), text);
  }
  const changed = join(run, "changed.json");
  const entries = added.map((rel) => ({ path: rel, added: [[1, files[rel].split("\n").length]] }));
  writeFileSync(changed, JSON.stringify({ protocol: 1, files: entries, ...extra }));
  return { tree, changed, done: () => rmSync(run, { recursive: true, force: true }) };
}

/** The duplicates dupes prints for `scratchTree(files, added, extra)`, each as [scope, kind, block file, other files]. */
function dupesOf(files, added, extra = {}) {
  const { tree, changed, done } = scratchTree(files, added, extra);
  try {
    const { code, json, stderr } = cli(["dupes", "--dir", tree, "--changed", changed]);
    assert.equal(code, 0, stderr);
    assert.deepEqual(validate(json, protocolSchema("dupes.schema.json")), []);
    return json.duplicates.map((d) => [d.scope, d.kind, d.block.file, d.others.map((o) => o.file)]);
  } finally {
    done();
  }
}

describe("dupes: scopes and exclude", () => {
  it("classifies files as analyze does", () => {
    for (const rel of ["src/a.test.ts", "src/a.spec.tsx", "src/__tests__/a.ts", "test/a.js", "src/a-test.js"]) {
      assert.equal(scopeOf(rel), "test", rel);
    }
    for (const rel of ["src/a.ts", "src/testing.ts", "src/latest/a.ts"]) assert.equal(scopeOf(rel), "production", rel);
  });

  it("never pairs a test file with the production code it copies, either way", () => {
    const testCopiesProduction = {
      "src/shipping.ts": shipping("calculateShipping"),
      "src/shipping.test.ts": `import { calculateShipping } from "./shipping";\n${shipping("expectedShipping")}`,
    };
    assert.deepEqual(dupesOf(testCopiesProduction, ["src/shipping.test.ts"]), []);
    const productionCopiesTest = {
      "src/__tests__/helpers.ts": shipping("expectedShipping"),
      "src/shipping.ts": shipping("calculateShipping"),
    };
    assert.deepEqual(dupesOf(productionCopiesTest, ["src/shipping.ts"]), []);
  });

  it("reports test files copying each other as a test group, and nothing in production", () => {
    const files = {
      "src/shipping.ts": shipping("calculateShipping"),
      "src/__tests__/shipping.ts": shipping("expectedShipping"),
      "src/shipping.spec.ts": shipping("expectedQuote"),
    };
    assert.deepEqual(dupesOf(files, ["src/shipping.spec.ts"]), [
      ["test", "copies_existing", "src/shipping.spec.ts", ["src/__tests__/shipping.ts"]],
    ]);
  });

  it("reports production copies as production groups, before the test groups", () => {
    // By file name alone src/__tests__ comes before src/zone: the scope orders first.
    const files = {
      "src/zone/shipping.ts": shipping("calculateShipping"),
      "src/zone/quote.ts": shipping("calculateQuote"),
      "src/__tests__/shipping.ts": shipping("expectedShipping"),
      "src/__tests__/quote.ts": shipping("expectedQuote"),
    };
    assert.deepEqual(dupesOf(files, ["src/zone/quote.ts", "src/__tests__/quote.ts"]), [
      ["production", "copies_existing", "src/zone/quote.ts", ["src/zone/shipping.ts"]],
      ["test", "copies_existing", "src/__tests__/quote.ts", ["src/__tests__/shipping.ts"]],
    ]);
  });

  it("caps the groups per scope, with a diagnostic for each scope past the cap", () => {
    const files = {
      "src/shipping.ts": shipping("calculateShipping"),
      "src/quote.ts": shipping("calculateQuote"),
      "src/rates.ts": shipping("calculateRates"),
      "src/__tests__/shipping.ts": shipping("expectedShipping"),
      "src/__tests__/quote.ts": shipping("expectedQuote"),
      "src/__tests__/rates.ts": shipping("expectedRates"),
    };
    const added = ["src/quote.ts", "src/rates.ts", "src/__tests__/quote.ts", "src/__tests__/rates.ts"];
    const { tree, changed, done } = scratchTree(files, added);
    try {
      const all = findDuplicates(tree, readChanged(changed).added);
      assert.deepEqual(all.duplicates.map((d) => d.scope), ["production", "production", "test", "test"]);
      assert.deepEqual(all.diagnostics, []);
      const capped = findDuplicates(tree, readChanged(changed).added, { maxGroups: 1 });
      assert.deepEqual(capped.duplicates.map((d) => [d.scope, d.block.file]), [
        ["production", "src/quote.ts"],
        ["test", "src/__tests__/quote.ts"],
      ]);
      assert.equal(capped.diagnostics.length, 2);
      assert.match(capped.diagnostics[0], /1 more production duplicate groups .* at most 1 per scope/);
      assert.match(capped.diagnostics[1], /1 more test duplicate groups .* at most 1 per scope/);
    } finally {
      done();
    }
  });

  it("leaves out the files of excluded folders and extensions, ignoring case, and only those", () => {
    // Different parents for locales/ and Locales/: on a case-insensitive file system they would be one folder.
    const files = {
      "src/shipping.ts": shipping("calculateShipping"),
      "src/i18n/locales/en.ts": shipping("englishShipping"),
      "src/web/Locales/pt.ts": shipping("portugueseShipping"),
      "src/strings.I18N.ts": shipping("stringShipping"),
      "src/locales.ts": shipping("localeShipping"),
    };
    const added = ["src/i18n/locales/en.ts", "src/web/Locales/pt.ts", "src/strings.I18N.ts", "src/locales.ts"];
    const exclude = { dirs: ["LOCALES"], extensions: [".i18n.ts"] };
    assert.deepEqual(dupesOf(files, added, { exclude }), [
      ["production", "copies_existing", "src/locales.ts", ["src/shipping.ts"]],
    ]);
    assert.deepEqual(
      dupesOf(files, added).map((d) => d[2]),
      ["src/i18n/locales/en.ts", "src/locales.ts", "src/strings.I18N.ts", "src/web/Locales/pt.ts"],
      "without exclude every copy is reported",
    );
    assert.deepEqual(dupesOf(files, added, { exclude: { dirs: [], extensions: [] } }).length, 4, "an empty exclude leaves nothing out");
  });

  it("matches exclude folders against folder segments only, extensions against the file name", () => {
    const exclude = { dirs: ["locales"], extensions: [".i18n.ts"] };
    assert.ok(isExcluded("locales/en.ts", exclude));
    assert.ok(isExcluded("src/i18n/LoCaLeS/en.ts", exclude));
    assert.ok(isExcluded("src/messages.I18n.TS", exclude));
    assert.ok(!isExcluded("src/locales.ts", exclude), "a file named like the folder is kept");
    assert.ok(!isExcluded("src/locales-old/en.ts", exclude), "a folder only containing the name is kept");
    assert.ok(!isExcluded("src/pack.i18n.ts/en.ts", exclude), "an extension is matched on the file name only");
    assert.ok(!isExcluded("src/locales/en.ts", null));
  });

  it("reads exclude from the changed file, for dupes and for mutate, which ignores it", () => {
    const dir = mkdtempSync(join(tmpdir(), "jsts-dupes-changed-"));
    try {
      const file = join(dir, "changed.json");
      const exclude = { dirs: ["locales"], extensions: [".po"] };
      const document = { protocol: 1, exclude, files: [{ path: "src/a.ts", added: [[1, 3]] }] };
      writeFileSync(file, JSON.stringify(document));
      const schema = protocolSchema("dupes.schema.json");
      assert.deepEqual(validate(document, schema.$defs.changed, schema), [], "the document is one the core may write");
      assert.deepEqual(readChanged(file), { added: new Map([["src/a.ts", [[1, 3]]]]), exclude });
      assert.deepEqual(readChanged(file, "mutate").added, new Map([["src/a.ts", [[1, 3]]]]));
      writeFileSync(file, JSON.stringify({ protocol: 1, files: [] }));
      assert.equal(readChanged(file, "mutate").exclude, null);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
