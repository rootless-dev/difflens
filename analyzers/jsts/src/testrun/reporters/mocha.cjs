// A Mocha reporter (--reporter <this file>) that writes the analyzer's event
// file (events.cjs) and prints nothing. Mocha loads every file before
// running any test, so a file that does not load stops the whole run; that
// is reported by the analyzer from Mocha's output.

"use strict";

const { emit, errorText } = require("./events.cjs");

function DiffLensReporter(runner) {
  const started = new Map();
  const files = new Set();
  const titles = (test) => (typeof test.titlePath === "function" ? test.titlePath() : [test.title]);
  const begin = (file) => {
    if (file && !files.has(file)) {
      files.add(file);
      started.set(file, Date.now());
      emit({ e: "start", file });
    }
  };
  runner.on("suite", (suite) => begin(suite.file));
  runner.on("test", (test) => begin(test.file));
  runner.on("pass", (test) => {
    emit({ e: "test", file: test.file, path: titles(test), status: "pass", ms: Math.round(test.duration ?? 0), output: null });
  });
  runner.on("pending", (test) => {
    begin(test.file);
    emit({ e: "test", file: test.file, path: titles(test), status: "skip", ms: 0, output: null });
  });
  runner.on("fail", (test, error) => {
    const file = test.file ?? test.parent?.file;
    begin(file);
    if (test.type === "hook") {
      // A failed hook fails its suite (or the file, at the root).
      const path = test.parent && !test.parent.root ? titles(test.parent) : [];
      if (path.length === 0) emit({ e: "end", file, error: `${test.title}\n${errorText(error)}`, ms: 0 });
      else emit({ e: "suite", file, path, status: "fail", ms: 0, output: `${test.title}\n${errorText(error)}` });
      return;
    }
    emit({ e: "test", file, path: titles(test), status: "fail", ms: Math.round(test.duration ?? 0), output: errorText(error) });
  });
  runner.on("suite end", (suite) => {
    // The end of a file's root-level suite (Mocha has no event for the end of a file).
    if (suite.parent && suite.parent.root && suite.file) {
      const siblings = suite.parent.suites.filter((s) => s.file === suite.file);
      if (siblings.at(-1) === suite) emit({ e: "end", file: suite.file, error: null, ms: Date.now() - (started.get(suite.file) ?? Date.now()) });
    }
  });
}

module.exports = DiffLensReporter;
