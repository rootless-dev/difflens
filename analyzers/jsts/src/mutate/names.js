// The names `killedBy` gives: a test StrykerJS reports (its runner's id, its
// full name with the titles joined by spaces, its file and the position of
// its call) under the name the `test` command writes for it: the titles of
// the test and the describe blocks around it joined by "/", a "/" inside a
// title written "∕". The titles come from the test file's syntax: the test
// call at the position StrykerJS gives (Vitest, Jest) when its titles joined
// by spaces are the full name StrykerJS gives, else the one test of the file
// whose titles are (Mocha). Null when neither finds it (a title that is not a
// constant, a test made in a loop, node --test through the command runner,
// which names no test).

import { readFileSync } from "node:fs";

import ts from "typescript";

import { parseFile, testCall } from "../analyze/functions.js";
import { escapeTitle } from "../testrun/collect.js";

/** The tests of a test file's source: { line, column (0-based, of the call), titles (or null) } . */
export function testTitles(rel, text) {
  const sf = parseFile(rel, text, rel);
  const out = [];
  const visit = (node, titles) => {
    const call = ts.isCallExpression(node) ? testCall(node) : null;
    if (call && !call.hook) {
      const own = titles === null || call.title === null ? null : [...titles, call.title];
      const { line, character } = sf.getLineAndCharacterOfPosition(node.getStart(sf));
      out.push({ line, column: character, titles: own, suite: call.suite });
      ts.forEachChild(node, (child) => visit(child, own));
      return;
    }
    ts.forEachChild(node, (child) => visit(child, titles));
  };
  visit(sf, []);
  return out;
}

export class TestNames {
  /** `tree` turns the absolute file names StrykerJS reports into paths of the tree. */
  constructor(tree) {
    this.tree = tree;
    this.files = new Map();
    this.byId = new Map();
  }

  /** Remembers the tests of a dry run ({ id, name, file, line, column }). */
  add(tests) {
    for (const test of tests) this.byId.set(test.id, test);
  }

  /** The tests of a file, parsed once; [] when it cannot be read. */
  titlesOf(file) {
    if (!this.files.has(file)) {
      let tests;
      try {
        tests = testTitles(this.tree.rel(file) ?? file, readFileSync(file, "utf8"));
      } catch {
        tests = [];
      }
      this.files.set(file, tests);
    }
    return this.files.get(file);
  }

  /** The `test` command's name of the test StrykerJS calls `id`, or null. */
  nameOf(id) {
    const test = this.byId.get(id);
    if (!test || !test.file) return null;
    const tests = this.titlesOf(test.file).filter((t) => !t.suite && t.titles !== null);
    let found = null;
    if (Number.isInteger(test.line)) found = tests.find((t) => t.line === test.line && t.column === test.column) ?? tests.find((t) => t.line === test.line) ?? null;
    // The titles read from the syntax must be the ones the runner printed (a title held in a variable is not).
    if (found !== null && typeof test.name === "string" && found.titles.join(" ") !== test.name) found = null;
    if (found === null && typeof test.name === "string") {
      const same = tests.filter((t) => t.titles.join(" ") === test.name);
      if (same.length === 1) found = same[0];
    }
    return found ? found.titles.map(escapeTitle).join("/") : null;
  }
}
