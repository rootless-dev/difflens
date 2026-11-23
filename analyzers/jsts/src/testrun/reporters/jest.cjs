// A Jest reporter (--reporters=<this file>) that writes the analyzer's event
// file (events.cjs) and prints nothing: each test file as it ends, with a
// suite that failed to run (it does not compile, an import fails) as the
// file's error.

"use strict";

const { emit } = require("./events.cjs");

const STATUS = { passed: "pass", failed: "fail" };

class DiffLensReporter {
  onTestFileStart(test) {
    emit({ e: "start", file: test.path });
  }

  onTestResult(test, result) {
    const file = result.testFilePath ?? test.path;
    for (const r of result.testResults ?? []) {
      const status = STATUS[r.status] ?? "skip";
      emit({
        e: "test",
        file,
        path: [...(r.ancestorTitles ?? []), r.title],
        status,
        ms: Math.round(r.duration ?? 0),
        output: status === "fail" ? (r.failureMessages ?? []).join("\n\n") : null,
        line: r.location?.line ?? null,
      });
    }
    const failedToRun = result.testExecError !== undefined && result.testExecError !== null;
    const error = failedToRun ? result.failureMessage || result.testExecError.message || String(result.testExecError) : null;
    const ms = result.perfStats ? Math.max(0, (result.perfStats.end ?? 0) - (result.perfStats.start ?? 0)) : 0;
    emit({ e: "end", file, error, ms: Math.round(ms) });
  }

  onRunComplete() {}

  getLastError() {
    return undefined;
  }
}

module.exports = DiffLensReporter;
