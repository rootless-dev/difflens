// A reporter for `node --test` (--test-reporter=<this file>) that writes the
// analyzer's event file (events.cjs) and prints nothing.
//
// Node reports a test after its subtests, and each event says its nesting
// level, so the titles of a test's ancestors are those of the tests started
// at the lower levels of the same file, most recently. A file that cannot be
// loaded is reported as a failed test named after the file.

import { createRequire } from "node:module";
import { isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const { emit, errorText } = require("./events.cjs");

function absolute(file) {
  if (!file) return null;
  if (file.startsWith("file://")) return fileURLToPath(file);
  return isAbsolute(file) ? file : resolve(file);
}

export default async function* reporter(source) {
  const stacks = new Map();
  const started = new Map();
  const failedToLoad = new Set();
  const ended = new Set();
  const seen = new Set();
  const stderr = new Map();
  const end = (file, error, ms) => {
    if (ended.has(file)) return;
    ended.add(file);
    emit({ e: "end", file, error, ms });
  };
  const begin = (file) => {
    if (file && !seen.has(file)) {
      seen.add(file);
      started.set(file, Date.now());
      emit({ e: "start", file });
    }
  };
  for await (const event of source) {
    const data = event.data ?? {};
    const file = absolute(data.file);
    switch (event.type) {
      case "test:enqueue":
      case "test:dequeue":
      case "test:start": {
        begin(file);
        if (event.type === "test:start" && file) {
          const stack = stacks.get(file) ?? [];
          stack.length = data.nesting;
          stack[data.nesting] = data.name;
          stacks.set(file, stack);
        }
        break;
      }
      case "test:pass":
      case "test:fail": {
        if (!file) break;
        begin(file);
        const stack = stacks.get(file) ?? [];
        const ancestors = stack.slice(0, data.nesting);
        const details = data.details ?? {};
        const ms = Math.round(details.duration_ms ?? 0);
        const failed = event.type === "test:fail";
        const error = details.error;
        // The file itself, as a top-level "test" named after it: it did not load when it failed, and ran
        // none of its tests (a name pattern matched none) when it passed.
        if (data.nesting === 0 && absolute(data.name) === file) {
          if (failed) {
            failedToLoad.add(file);
            const printed = stderr.get(file) ?? "";
            const cause = errorText(error?.cause ?? error);
            end(file, printed.trim() !== "" ? printed : cause, ms);
          }
          break;
        }
        const status = data.skip !== undefined && data.skip !== false ? "skip" : data.todo !== undefined && data.todo !== false ? "skip" : failed ? "fail" : "pass";
        const kind = details.type === "suite" ? "suite" : "test";
        const output = failed ? errorText(error?.cause ?? error) : null;
        if (kind === "suite") {
          // A suite fails by itself only when a hook or its own body failed.
          if (failed && error?.failureType !== "subtestsFailed") {
            emit({ e: "suite", file, path: [...ancestors, data.name], status: "fail", ms, output });
          }
          break;
        }
        if (failed && error?.failureType === "subtestsFailed") {
          // A test whose subtests failed: listed through them.
          emit({ e: "test", file, path: [...ancestors, data.name], status: "fail", ms, output: null, parent: true });
          break;
        }
        emit({ e: "test", file, path: [...ancestors, data.name], status, ms, output });
        break;
      }
      case "test:stderr":
      case "test:stdout": {
        if (file && typeof data.message === "string") {
          const text = (stderr.get(file) ?? "") + data.message;
          stderr.set(file, text.length > 65536 ? text.slice(-65536) : text);
        }
        break;
      }
      case "test:summary": {
        if (file) end(file, null, Math.round(data.duration_ms ?? Date.now() - (started.get(file) ?? Date.now())));
        break;
      }
      case "test:coverage": {
        const files = (data.summary?.files ?? []).map((f) => ({
          path: absolute(f.path),
          lines: (f.lines ?? []).map((l) => [l.line, l.count]),
        }));
        emit({ e: "coverage", files });
        break;
      }
      default:
        break;
    }
  }
  for (const file of seen) end(file, null, Date.now() - (started.get(file) ?? Date.now()));
}
