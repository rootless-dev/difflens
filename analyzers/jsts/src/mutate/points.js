// The mutants of one file, from the list StrykerJS's instrumenter makes,
// placed with the TypeScript parser the analyze command uses: the candidate
// lines only (a mutant whose first character is on one), without those
// rule 2 leaves out (test code, constant declarations at the top of a module,
// logging and assertion calls), each with its function (the innermost one
// holding it, by analyze's ids), its position in Unicode characters, its
// original text, its operator family, and its stable id.

import ts from "typescript";

import { importsTestFramework, indexFile, nameChain, parseFile } from "../analyze/functions.js";
import { codePointColumn, excerpt, lineStarts, mutantId } from "./plan.js";
import { operatorOf } from "./operators.js";

/** Objects whose method calls log or assert: a mutant inside one survives for no good reason. */
const LOGGERS = new Set(["console", "log", "logger", "assert", "expect", "invariant", "debug"]);

/** Whether `call` logs or asserts: console.log(...), logger.info(...), this.log.warn(...), assert(...), expect(...). */
export function isLoggingOrAssertion(call) {
  const chain = nameChain(call.expression);
  if (!chain) return false;
  const head = chain[0] === "this" ? chain[1] : chain[0];
  if (head === undefined) return false;
  return LOGGERS.has(head);
}

/** The deepest nodes of `sf` holding the span [start, end): outermost first. */
function nodesHolding(sf, start, end) {
  const path = [];
  const visit = (node) => {
    if (node.getStart(sf) <= start && end <= node.end) {
      path.push(node);
      ts.forEachChild(node, visit);
      return true;
    }
    return false;
  };
  ts.forEachChild(sf, visit);
  return path;
}

/** What the expression spanning [start, end) is to its parent: "condition", "return" or null. */
function roleOf(path, sf, start, end) {
  let at = path.findIndex((n) => n.getStart(sf) === start && n.end === end);
  if (at < 0) return null;
  let node = path[at];
  let parent = path[at - 1];
  while (parent && ts.isParenthesizedExpression(parent)) {
    node = parent;
    at--;
    parent = path[at - 1];
  }
  if (!parent) return null;
  if ((ts.isIfStatement(parent) || ts.isWhileStatement(parent) || ts.isDoStatement(parent)) && parent.expression === node) return "condition";
  if (ts.isForStatement(parent) && parent.condition === node) return "condition";
  if (ts.isConditionalExpression(parent) && parent.condition === node) return "condition";
  if (ts.isReturnStatement(parent) && parent.expression === node) return "return";
  if (ts.isArrowFunction(parent) && parent.body === node) return "return";
  return null;
}

/** Why a mutant is left out (rule 2), or null: inside a logging or assertion call, or a top-level constant. */
function leftOut(path) {
  let inFunction = false;
  for (const node of path) if (ts.isFunctionLike(node) || ts.isClassStaticBlockDeclaration(node)) inFunction = true;
  for (const node of path) {
    if (ts.isCallExpression(node) && isLoggingOrAssertion(node)) return "logging";
    if (!inFunction && ts.isVariableDeclarationList(node) && node.flags & ts.NodeFlags.Const) return "constant";
    if (ts.isEnumDeclaration(node)) return "constant";
  }
  return null;
}

/**
 * The mutants of `rel` (text `text`) on the `lines` given, from `raw` (the instrumenter's mutants: mutatorName,
 * replacement, location with 0-based lines and UTF-16 columns). Resolves to { mutants, leftOut } where leftOut
 * counts those rule 2 drops.
 */
export function filePoints(rel, text, raw, lines) {
  const sf = parseFile(rel, text, rel);
  const { functions } = indexFile(sf, rel, { container: "", testFramework: importsTestFramework(sf) });
  const spans = functions
    .filter((f) => f.kind !== "module")
    .map((f) => ({ f, start: f.node.getStart(sf), end: f.node.end }));
  const starts = lineStarts(text);
  const mutants = [];
  let dropped = 0;
  for (const m of raw) {
    const line = m.location.start.line + 1;
    if (!lines.has(line)) continue;
    const endLine = m.location.end.line + 1;
    const startOffset = starts[m.location.start.line] + m.location.start.column;
    const endOffset = starts[m.location.end.line] + m.location.end.column;
    // The innermost function holding the mutant; a mutant of a whole function (ArrowFunction) is its parent's.
    let holder = null;
    for (const span of spans) {
      const holds = span.start <= startOffset && endOffset <= span.end && !(span.start === startOffset && span.end === endOffset);
      if (holds && (holder === null || span.end - span.start < holder.end - holder.start)) holder = span;
    }
    if (holder?.f.test) {
      dropped++;
      continue;
    }
    const path = nodesHolding(sf, startOffset, endOffset);
    if (leftOut(path) !== null) {
      dropped++;
      continue;
    }
    const original = text.slice(startOffset, endOffset);
    const mutated = m.replacement ?? "";
    const column = codePointColumn(text, starts[m.location.start.line], m.location.start.column);
    const endColumn = codePointColumn(text, starts[m.location.end.line], m.location.end.column);
    const fn = holder?.f ?? null;
    const operator = operatorOf(m.mutatorName, original, mutated, roleOf(path, sf, startOffset, endOffset));
    const where = fn ? ` in ${fn.name}` : "";
    const point = {
      file: rel,
      line,
      column,
      endLine,
      endColumn,
      original,
      mutated,
      operator,
      mutator: m.mutatorName,
      description: `${m.mutatorName}: replace ${excerpt(original)} with ${excerpt(mutated) || "nothing"}${where}`,
      functionId: fn?.id ?? null,
      functionLine: fn?.startLine ?? null,
      // StrykerJS's own positions: line 1-based, column 0-based in UTF-16 (its `mutate` ranges and results).
      start: { line, column: m.location.start.column },
      end: { line: endLine, column: m.location.end.column },
    };
    point.id = mutantId(point);
    mutants.push(point);
  }
  return { mutants, leftOut: dropped };
}
