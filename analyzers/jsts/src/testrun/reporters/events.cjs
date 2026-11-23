// The event file every reporter of this folder writes: one JSON object per
// line, appended as tests end, read by the analyzer while the run goes on
// (testrun/collect.js). The file is named by DIFFLENS_TEST_EVENTS.
//
//   {"e":"start","file":"/abs/a.test.ts"}                       a test file starts
//   {"e":"test","file":...,"path":["suite","test"],"status":"pass"|"fail"|"skip","ms":3,"output":null}
//   {"e":"suite","file":...,"path":["suite"],"status":"fail","ms":5,"output":"..."}  a suite's own failure (a hook)
//   {"e":"end","file":...,"error":null|"...","ms":12}            a test file ended; error: it could not run
//   {"e":"coverage","files":[{"path":"/abs/a.js","lines":[[1,3],[2,0]]}]}
//   {"e":"fatal","error":"..."}                                   the run failed outside any file
//
// CommonJS, so Jest and Mocha can require it; the ESM reporters import it.

"use strict";

const { appendFileSync } = require("node:fs");
const { inspect } = require("node:util");

const target = process.env.DIFFLENS_TEST_EVENTS;

function emit(event) {
  if (!target) return;
  try {
    appendFileSync(target, `${JSON.stringify(event)}\n`);
  } catch {
    // The analyzer reads what it can.
  }
}

/** An error as a runner prints it: its message and stack, and what was expected against what came. */
function errorText(error) {
  if (error === undefined || error === null) return "";
  if (typeof error === "string") return error;
  let text = "";
  const stack = typeof error.stack === "string" ? error.stack : "";
  const message = typeof error.message === "string" ? error.message : "";
  if (stack !== "" && message !== "" && stack.includes(message)) text = stack;
  else if (stack !== "") text = message !== "" ? `${message}\n${stack}` : stack;
  else if (message !== "") text = message;
  else text = inspect(error, { depth: 4 });
  if (error.expected !== undefined || error.actual !== undefined) {
    if (!/\bexpected\b/i.test(text)) {
      text += `\nexpected: ${inspect(error.expected, { depth: 4 })}\nactual: ${inspect(error.actual, { depth: 4 })}`;
    }
  }
  if (error.cause && typeof error.cause === "object" && error.cause !== error) {
    text += `\ncause: ${errorText(error.cause)}`;
  }
  return text;
}

module.exports = { emit, errorText };
