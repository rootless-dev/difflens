// A Vitest reporter (--reporter=<this file>) that writes the analyzer's event
// file (events.cjs) and prints nothing. It speaks both reporter APIs: the
// test module API of Vitest 3 and later (onTestModuleStart/End,
// onTestRunEnd), which reports each file as it ends, and the task API of
// Vitest 1 and 2 (onFinished), which reports every file at the end.

import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { emit, errorText } = require("./events.cjs");

function vitestError(error) {
  let text = errorText(error);
  if (error && typeof error.diff === "string" && error.diff !== "" && !text.includes(error.diff)) text += `\n${error.diff}`;
  return text;
}

const STATUS = { passed: "pass", pass: "pass", failed: "fail", fail: "fail" };

export default class DiffLensReporter {
  constructor() {
    this.done = new Set();
    this.started = new Set();
  }

  start(file) {
    if (file && !this.started.has(file)) {
      this.started.add(file);
      emit({ e: "start", file });
    }
  }

  // Vitest 3 and later.
  onTestModuleStart(module) {
    this.start(module.moduleId);
  }

  onTestModuleEnd(module) {
    this.module(module);
  }

  onTestRunEnd(modules, errors) {
    for (const module of modules ?? []) this.module(module);
    for (const error of errors ?? []) emit({ e: "fatal", error: vitestError(error) });
  }

  module(module) {
    const file = module.moduleId;
    if (!file || this.done.has(file)) return;
    this.done.add(file);
    this.start(file);
    for (const test of module.children.allTests()) {
      const path = [];
      for (let cur = test; cur && cur.type !== "module"; cur = cur.parent) path.unshift(cur.name);
      const result = test.result();
      const status = STATUS[result?.state] ?? "skip";
      const output = status === "fail" ? (result.errors ?? []).map(vitestError).join("\n\n") : null;
      const ms = Math.round(test.diagnostic?.()?.duration ?? 0);
      emit({ e: "test", file, path, status, ms, output, line: test.location?.line ?? null });
    }
    for (const suite of module.children.allSuites?.() ?? []) {
      const errors = suite.errors?.() ?? [];
      if (errors.length === 0) continue;
      const path = [];
      for (let cur = suite; cur && cur.type !== "module"; cur = cur.parent) path.unshift(cur.name);
      emit({ e: "suite", file, path, status: "fail", ms: 0, output: errors.map(vitestError).join("\n\n") });
    }
    const errors = module.errors?.() ?? [];
    const ms = Math.round(module.diagnostic?.()?.duration ?? 0);
    emit({ e: "end", file, error: errors.length > 0 ? errors.map(vitestError).join("\n\n") : null, ms });
  }

  // Vitest 1 and 2 (and 3, which also calls the methods above first).
  onFinished(files, errors) {
    for (const task of files ?? []) {
      const file = task.filepath;
      if (!file || this.done.has(file)) continue;
      this.done.add(file);
      this.start(file);
      const walk = (t, path) => {
        for (const child of t.tasks ?? []) {
          const childPath = [...path, child.name];
          if (child.type === "suite") {
            const errors = child.result?.errors ?? [];
            if (child.result?.state === "fail" && errors.length > 0) {
              emit({ e: "suite", file, path: childPath, status: "fail", ms: 0, output: errors.map(vitestError).join("\n\n") });
            }
            walk(child, childPath);
          } else {
            const status = STATUS[child.result?.state] ?? "skip";
            const output = status === "fail" ? (child.result?.errors ?? []).map(vitestError).join("\n\n") : null;
            emit({ e: "test", file, path: childPath, status, ms: Math.round(child.result?.duration ?? 0), output, line: child.location?.line ?? null });
          }
        }
      };
      walk(task, []);
      const errors = task.result?.errors ?? [];
      const failedToRun = task.result?.state === "fail" && errors.length > 0;
      emit({ e: "end", file, error: failedToRun ? errors.map(vitestError).join("\n\n") : null, ms: Math.round(task.result?.duration ?? 0) });
    }
    for (const error of errors ?? []) emit({ e: "fatal", error: vitestError(error) });
  }
}
