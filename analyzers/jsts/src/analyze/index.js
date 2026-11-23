// The analyze command: the functions, calls, externals and diagnostics of a
// JavaScript / TypeScript tree (analysis.schema.json). Reads the tree only.

import ts from "typescript";

import { Tree, containerOf, isGenerated, isTestFile, rootContainerName, sourceFiles } from "../files.js";
import { CallCollector, nodeKey } from "./calls.js";
import { detect } from "./entrypoints.js";
import { importsTestFramework, indexFile, isFunctionValue, lineOf, moduleSpecifiers } from "./functions.js";
import { normalize } from "./normalize.js";
import { buildPrograms } from "./programs.js";
import { Resolver, safe } from "./resolve.js";

/** Syntax errors reported per file, at most. */
const MAX_FILE_DIAGNOSTICS = 3;

/**
 * Analyses the tree at `dir` (absolute) and returns { functions, calls, externals, diagnostics }. Stops adding
 * calls once `deadline` (a Date.now() value) is near and says so in a diagnostic.
 */
export function analyzeTree(dir, { log, deadline = Infinity } = {}) {
  const tree = new Tree(dir);
  const rootName = rootContainerName(tree.root);
  const diagnostics = [];
  const rels = sourceFiles(tree.root, { deadline });
  if (rels.length === 0) {
    diagnostics.push({ level: "warning", message: "no JavaScript or TypeScript source files were found" });
    return { functions: [], calls: [], externals: [], diagnostics };
  }
  const started = Date.now();
  const built = buildPrograms(tree.root, rels.map((rel) => tree.abs(rel)), { log, deadline });
  for (const warning of built.warnings) {
    const rel = tree.rel(warning.file);
    diagnostics.push({ level: "warning", message: warning.message, ...(rel ? { file: rel } : {}) });
  }
  log?.info("programs built", { programs: built.programs.length, files: rels.length, elapsed: `${Date.now() - started}ms` });

  // Index every file's functions from its syntax.
  const index = { byKey: new Map(), byId: new Map(), files: new Map(), checkerOf: null };
  const checkers = new Map();
  const checkerOfProgram = (program) => {
    if (!checkers.has(program)) checkers.set(program, program.getTypeChecker());
    return checkers.get(program);
  };
  const ownerOfFile = new Map();
  for (const rel of rels) {
    const abs = tree.abs(rel);
    const program = built.owner.get(abs);
    const sf = program?.getSourceFile(abs);
    if (!sf) {
      diagnostics.push({ level: "warning", message: "the file could not be read", file: rel });
      continue;
    }
    ownerOfFile.set(sf.fileName, program);
    const testFile = isTestFile(rel);
    const result = indexFile(sf, rel, {
      container: containerOf(rel, rootName),
      generated: isGenerated(rel, sf.text.slice(0, 4096)),
      testFile,
      testFramework: testFile || importsTestFramework(sf),
    });
    for (const record of result.functions) {
      if (index.byId.has(record.id)) {
        // Two files with one module path (a.js and a.ts): the later one keeps its extension in its ids.
        record.id = `${rel}.${record.name}`;
      }
      index.byId.set(record.id, record);
      index.byKey.set(record.key, record);
    }
    let syntax = [];
    try {
      syntax = program.getSyntacticDiagnostics(sf);
    } catch {
      syntax = [];
    }
    for (const d of syntax.slice(0, MAX_FILE_DIAGNOSTICS)) {
      diagnostics.push({
        level: "error",
        message: ts.flattenDiagnosticMessageText(d.messageText, "\n"),
        file: rel,
        ...(d.start !== undefined ? { line: lineOf(sf, d.start) } : {}),
      });
    }
    index.files.set(sf.fileName, {
      rel,
      sf,
      records: result.functions,
      module: result.module,
      testCalls: result.testCalls,
      imports: moduleSpecifiers(sf),
      checker: null,
      program,
    });
  }
  index.checkerOf = (node) => {
    const program = ownerOfFile.get(node.getSourceFile().fileName);
    return program ? checkerOfProgram(program) : null;
  };
  for (const file of index.files.values()) file.checker = checkerOfProgram(file.program);

  const resolver = new Resolver(index, tree, rootName);
  const out = { calls: [], externals: new Map() };
  const collector = new CallCollector(resolver, out);
  let stopped = 0;
  // Time kept for entry points, references and printing.
  const callDeadline = deadline === Infinity ? Infinity : deadline - Math.min(5000, (deadline - started) / 10);
  for (const file of index.files.values()) {
    if (Date.now() > callDeadline) {
      stopped++;
      continue;
    }
    for (const [n, record] of file.records.entries()) {
      if (n % 256 === 255 && Date.now() > callDeadline) {
        stopped++;
        break;
      }
      try {
        collector.collect(record, file.checker);
      } catch (error) {
        log?.warn("calls not analysed", { function: record.id, error: String(error?.message ?? error) });
      }
    }
  }
  collector.finish();
  if (stopped > 0) {
    diagnostics.push({
      level: "warning",
      message: `the analysis ran out of time: the calls of ${stopped} of ${index.files.size} files were not all analysed`,
    });
  }

  // Entry points and registrations.
  const entries = new Map();
  const registered = new Set();
  let found = [];
  try {
    found = detect({ resolver, index, root: tree.root, packages: built.packages });
  } catch (error) {
    log?.warn("entry points not analysed", { error: String(error?.stack ?? error) });
    diagnostics.push({ level: "warning", message: `entry points could not be analysed: ${error?.message ?? error}` });
  }
  for (const item of found) {
    if (item.entryPoint) {
      const list = entries.get(item.funcId) ?? [];
      list.push(item.entryPoint);
      entries.set(item.funcId, list);
    }
    if (item.registration) {
      const reg = item.registration;
      out.calls.push({
        caller: reg.caller,
        callee: item.funcId,
        line: reg.line,
        kind: "async",
        resolution: "static",
        relation: "registration",
        evidence: reg.evidence,
      });
      if (reg.value) registered.add(`${reg.value.getSourceFile().fileName}:${reg.value.pos}`);
    }
  }

  if (Date.now() < callDeadline) references(index, resolver, out, registered, collector.handled);

  const functions = [];
  for (const record of index.byId.values()) {
    const fn = {
      id: record.id,
      name: record.name,
      qualifiedName: record.qualifiedName,
      container: record.container,
      file: record.file,
      startLine: record.startLine,
      endLine: Math.max(record.endLine, record.startLine),
      generated: record.generated,
      test: record.test,
      complexity: record.complexity,
    };
    const eps = entries.get(record.id);
    if (eps && eps.length > 0) fn.entryPoints = eps;
    functions.push(fn);
  }
  return normalize({ functions, calls: out.calls, externals: [...out.externals.values()], diagnostics });
}

/**
 * References: a function of the tree used as a value without being called there (passed as an argument,
 * stored, returned, exported in an object), from the function that uses it. A value that a registration or an
 * async callback already accounts for, or that is immediately called, is not one; a function referring to
 * itself is skipped. Function literals count too, from the function they are written in, unless they are the
 * value of a name (a declaration, not a use).
 */
function references(index, resolver, out, registered, handled) {
  const covered = new Set(out.calls.map((c) => `${c.caller}|${c.callee}|${c.line}`));
  const add = (caller, callee, line) => {
    if (!caller || caller.id === callee.id) return;
    const key = `${caller.id}|${callee.id}|${line}`;
    if (covered.has(key)) return;
    covered.add(key);
    out.calls.push({ caller: caller.id, callee: callee.id, line, kind: "async", resolution: "static", relation: "reference" });
  };
  for (const file of index.files.values()) {
    const checker = file.checker;
    const sf = file.sf;
    const visit = (node) => {
      if (isFunctionValue(node)) {
        const record = resolver.recordOfNode(node);
        if (record && record.kind === "closure" && !immediatelyCalled(node) && !registered.has(`${sf.fileName}:${node.pos}`)) {
          const caller = resolver.enclosing(node);
          if (caller && !handled.has(nodeKey(node, caller))) add(caller, record, lineOf(sf, node.getStart()));
        }
      } else if ((ts.isIdentifier(node) || ts.isPropertyAccessExpression(node)) && valuePosition(node)) {
        if (!registered.has(`${sf.fileName}:${node.pos}`)) {
          const symbol = safe(() => checker.getSymbolAtLocation(ts.isPropertyAccessExpression(node) ? node.name : node));
          const target = symbol && resolver.recordOfSymbol(symbol, checker);
          // A getter or setter read or assigned is a call (calls.js), not a value.
          const accessor = target && (ts.isGetAccessorDeclaration(target.node) || ts.isSetAccessorDeclaration(target.node));
          if (target && target.kind !== "module" && !accessor) {
            const caller = resolver.enclosing(node);
            if (caller && !handled.has(nodeKey(node, caller))) add(caller, target, lineOf(sf, node.getStart()));
          }
        }
        if (ts.isPropertyAccessExpression(node)) return;
      }
      ts.forEachChild(node, visit);
    };
    visit(sf);
  }
}

/** Whether a function literal is called where it is written: (() => {})() or (function () {}).call(x). */
function immediatelyCalled(node) {
  let cur = node;
  while (cur.parent && (ts.isParenthesizedExpression(cur.parent) || ts.isAsExpression(cur.parent))) cur = cur.parent;
  const parent = cur.parent;
  if (ts.isCallExpression(parent) && parent.expression === cur) return true;
  if (ts.isPropertyAccessExpression(parent) && parent.expression === cur && ["call", "apply"].includes(parent.name.text)) return true;
  return false;
}

/**
 * Whether an identifier or property access is a function value being used: an argument, an array element, a
 * property value (shorthand included), an assignment's value, a variable's initializer, a return value, a JSX
 * attribute value. Not a callee, not a declaration's name, not the object of a property access.
 */
function valuePosition(node) {
  let cur = node;
  while (cur.parent && (ts.isParenthesizedExpression(cur.parent) || ts.isAsExpression(cur.parent) || ts.isNonNullExpression(cur.parent))) {
    cur = cur.parent;
  }
  const parent = cur.parent;
  if (!parent) return false;
  if (ts.isPropertyAccessExpression(node.parent) && node.parent.name === node) return false;
  if (ts.isCallExpression(parent) || ts.isNewExpression(parent)) return parent.expression !== cur;
  if (ts.isArrayLiteralExpression(parent) || ts.isSpreadElement(parent)) return true;
  if (ts.isPropertyAssignment(parent)) return parent.initializer === cur;
  if (ts.isShorthandPropertyAssignment(parent)) return true;
  if (ts.isBinaryExpression(parent)) return parent.right === cur && parent.operatorToken.kind === ts.SyntaxKind.EqualsToken;
  if (ts.isVariableDeclaration(parent)) return parent.initializer === cur;
  if (ts.isReturnStatement(parent) || ts.isArrowFunction(parent)) return parent.body === cur || ts.isReturnStatement(parent);
  if (ts.isJsxExpression(parent)) return ts.isJsxAttribute(parent.parent);
  if (ts.isConditionalExpression(parent)) return parent.condition !== cur;
  return false;
}
