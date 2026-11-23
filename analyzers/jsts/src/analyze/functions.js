// Lists the functions of one source file and gives each a stable id, from
// syntax only (no type checking), so that analyze and the test runner name
// them alike and an edit elsewhere never renames them.
//
// An id is the module path (the file's path without its extension) and the
// function's path inside the module:
//
//   function / const f = () => {}      src/util.round, src/util.clamp
//   class member                        src/shapes.Circle.area, ....Circle.constructor,
//                                       ....Square.get name, ....Square.set side, ....Square.#tick
//   object literal member               src/util.helpers.double
//   export default function / object    src/index.default, src/app.default.fetch
//   CommonJS assignment                 lib/index.exports.startTicker, lib/legacy.Counter.increment
//   nested named function               src/a.outer.inner
//   test callback                       src/a.test.describe("math").it("adds")
//   test hook                           src/a.test.describe("math").beforeEach
//   closure (anonymous function)        <parent path>$<k>: src/a.outer$1, ...outer$1$2
//   closure in a named value            src/a.handler$1 (const handler = wrap(() => {}))
//   module body                         src/server.(module), when the module runs code at load time
//
// A closure's k is its position among the closures written directly inside
// its parent, in source order (as for Go closures). Names that repeat in a
// file get "#2", "#3", in source order.

import ts from "typescript";

import { modulePath } from "../files.js";

export const MODULE_FUNCTION = "(module)";

const TEST_CASES = new Set(["it", "test", "specify", "bench", "fit", "xit", "xtest", "xspecify"]);
const TEST_SUITES = new Set(["describe", "context", "suite", "fdescribe", "xdescribe", "xcontext"]);
const TEST_HOOKS = new Set(["beforeAll", "afterAll", "beforeEach", "afterEach", "before", "after"]);
const TEST_MODIFIERS = new Set([
  "skip",
  "only",
  "todo",
  "concurrent",
  "sequential",
  "fails",
  "failing",
  "each",
  "for",
  "skipIf",
  "runIf",
  "serial",
  "shuffle",
]);

/** Modules whose functions make a file test code even when its name does not say so (in-source tests). */
export const TEST_MODULES = new Set([
  "vitest",
  "@jest/globals",
  "node:test",
  "test",
  "mocha",
  "bun:test",
  "ava",
  "@playwright/test",
  "uvu",
  "tap",
]);

/** Strips parentheses, type assertions and `satisfies` around an expression. */
export function unwrap(expr) {
  let e = expr;
  for (;;) {
    if (!e) return e;
    if (ts.isParenthesizedExpression(e) || ts.isAsExpression(e) || ts.isNonNullExpression(e)) e = e.expression;
    else if (ts.isSatisfiesExpression?.(e) || ts.isTypeAssertionExpression(e)) e = e.expression;
    else return e;
  }
}

/** Whether `node` is a function expression or an arrow function. */
export function isFunctionValue(node) {
  return node !== undefined && (ts.isArrowFunction(node) || ts.isFunctionExpression(node));
}

/** ["describe", "skip"] for describe.skip, ["t", "test"] for t.test; null for anything else. */
export function nameChain(expr) {
  if (ts.isIdentifier(expr)) return [expr.text];
  if (ts.isPropertyAccessExpression(expr) && ts.isIdentifier(expr.name)) {
    const head = nameChain(expr.expression);
    return head && [...head, expr.name.text];
  }
  if (expr.kind === ts.SyntaxKind.ThisKeyword) return ["this"];
  return null;
}

/** The text of a constant string expression (a literal or a template without substitutions), or null. */
export function stringValue(expr) {
  const e = expr && unwrap(expr);
  if (!e) return null;
  if (ts.isStringLiteral(e) || ts.isNoSubstitutionTemplateLiteral(e)) return e.text;
  return null;
}

/**
 * The test framework call `node` is, if any: describe/it/test/suite/context/specify and their modifiers
 * (describe.skip, it.each(table), test.concurrent), node:test subtests (t.test), and hooks (beforeEach...).
 * `api` is the function's name, `title` the test's name when it is a constant (null otherwise), `callback`
 * the function it registers.
 */
export function testCall(node) {
  if (!ts.isCallExpression(node)) return null;
  let callee = node.expression;
  let table = false;
  if (ts.isCallExpression(callee)) {
    table = true;
    callee = callee.expression;
  } else if (ts.isTaggedTemplateExpression(callee)) {
    table = true;
    callee = callee.tag;
  }
  const chain = nameChain(callee);
  if (!chain || chain[0] === "this") return null;
  const known = (name) => TEST_CASES.has(name) || TEST_SUITES.has(name) || TEST_HOOKS.has(name);
  let at = -1;
  if (known(chain[0])) at = 0;
  else if (chain.length >= 2 && known(chain[1])) at = 1;
  if (at < 0) return null;
  if (!chain.slice(at + 1).every((part) => TEST_MODIFIERS.has(part))) return null;
  if (table && !chain.slice(at + 1).some((part) => ["each", "for", "skipIf", "runIf"].includes(part))) return null;
  const api = chain[at];
  const callback = [...node.arguments].reverse().find((arg) => isFunctionValue(unwrap(arg)));
  if (callback === undefined) return null;
  const hook = TEST_HOOKS.has(api);
  let title = null;
  if (!hook && node.arguments.length > 0 && node.arguments[0] !== callback) {
    const first = unwrap(node.arguments[0]);
    title = stringValue(first);
    // describe(MyClass, ...): the runners print the class's name.
    if (title === null && (ts.isIdentifier(first) || ts.isPropertyAccessExpression(first))) title = first.getText();
  }
  return { api, title, callback: unwrap(callback), hook, suite: TEST_SUITES.has(api), table, node };
}

/** The segment naming a test callback in its function's path: describe("math"), it("adds"), beforeEach. */
export function testSegment(call) {
  if (call.hook || call.title === null) return call.api;
  return `${call.api}(${JSON.stringify(call.title)})`;
}

/** The name of a class member or object property, or null when it is computed from something not constant. */
function memberName(name, sf) {
  if (!name) return null;
  if (ts.isIdentifier(name) || ts.isPrivateIdentifier(name)) return name.text;
  if (ts.isStringLiteral(name) || ts.isNumericLiteral(name) || ts.isNoSubstitutionTemplateLiteral(name)) return name.text;
  if (ts.isComputedPropertyName(name)) return `[${name.expression.getText(sf)}]`;
  return null;
}

/** Line (1-based) of position `pos` in `sf`. */
export function lineOf(sf, pos) {
  return sf.getLineAndCharacterOfPosition(pos).line + 1;
}

/** The key that identifies a function node across programs: its file and span. */
export function keyOf(node) {
  const sf = node.getSourceFile();
  return `${sf.fileName}:${node.pos}:${node.end}`;
}

/**
 * Cyclomatic complexity of `node`: 1 plus the decision points of its body. Counted: if (an else-if is an if),
 * for, for-in, for-of, while, do-while, each case clause (not default), catch, the conditional operator, and
 * the short-circuit operators && || ?? and their assignments &&= ||= ??=. Not counted: optional chaining (?.)
 * and default values, which handle a missing value rather than choose a path through the logic (as ESLint's
 * classic complexity rule and Go's gocyclo do). Nested functions and classes are separate functions and are not
 * counted. For a module body, only the code that runs at load time.
 */
export function complexity(node) {
  let n = 1;
  const count = (child) => {
    switch (child.kind) {
      case ts.SyntaxKind.IfStatement:
      case ts.SyntaxKind.ForStatement:
      case ts.SyntaxKind.ForInStatement:
      case ts.SyntaxKind.ForOfStatement:
      case ts.SyntaxKind.WhileStatement:
      case ts.SyntaxKind.DoStatement:
      case ts.SyntaxKind.CaseClause:
      case ts.SyntaxKind.CatchClause:
      case ts.SyntaxKind.ConditionalExpression:
        n++;
        break;
      case ts.SyntaxKind.BinaryExpression:
        switch (child.operatorToken.kind) {
          case ts.SyntaxKind.AmpersandAmpersandToken:
          case ts.SyntaxKind.BarBarToken:
          case ts.SyntaxKind.QuestionQuestionToken:
          case ts.SyntaxKind.AmpersandAmpersandEqualsToken:
          case ts.SyntaxKind.BarBarEqualsToken:
          case ts.SyntaxKind.QuestionQuestionEqualsToken:
            n++;
        }
        break;
    }
    if (ts.isFunctionLike(child) || ts.isClassLike(child)) return;
    ts.forEachChild(child, count);
  };
  if (ts.isSourceFile(node)) {
    for (const statement of node.statements) count(statement);
  } else if (node.body !== undefined) {
    count(node.body);
  }
  return n;
}

/** Whether a call, construction or element at the top level of a module makes it run code at load time. */
function runsCode(node) {
  return (
    ts.isCallExpression(node) ||
    ts.isNewExpression(node) ||
    ts.isTaggedTemplateExpression(node) ||
    ts.isJsxElement(node) ||
    ts.isJsxSelfClosingElement(node) ||
    ts.isAwaitExpression(node)
  );
}

/** The qualified name's first part for a file: its base name, or for an index file its folder's name. */
function moduleLabel(rel) {
  const parts = modulePath(rel).split("/");
  const base = parts.at(-1);
  if (base === "index" && parts.length > 1) return parts.at(-2);
  return base;
}

/**
 * The functions of `sf` (parsed with parent pointers), as protocol functions plus the syntax they came from.
 * `rel` is the file's path in the tree, `container` its container; `generated` and `testFile` flag every function
 * of the file; `testFramework` says whether test calls in a file that is not a test file are test code (it
 * imports a test framework).
 */
export function indexFile(sf, rel, { container, generated = false, testFile = false, testFramework = testFile } = {}) {
  const mod = modulePath(rel);
  const label = moduleLabel(rel);
  const functions = [];
  const byKey = new Map();
  const used = new Map();
  const counters = new Map();
  const testCalls = [];
  let moduleRunsCode = false;

  const unique = (path) => {
    const seen = used.get(path) ?? 0;
    used.set(path, seen + 1);
    return seen === 0 ? path : `${path}#${seen + 1}`;
  };
  const join = (scope, name) => (scope === "" ? name : `${scope}.${name}`);
  const closurePath = (ctx) => {
    const base = ctx.closureBase;
    const k = (counters.get(base) ?? 0) + 1;
    counters.set(base, k);
    return `${base}$${k}`;
  };

  const define = (node, path, kind, ctx, extra = {}) => {
    path = unique(path);
    const record = {
      id: `${mod}.${path}`,
      name: path,
      qualifiedName: `${label}.${path}`,
      container,
      file: rel,
      startLine: lineOf(sf, node.getStart(sf)),
      endLine: lineOf(sf, node.end),
      generated,
      test: testFile || ctx.inTest || kind === "test",
      complexity: complexity(node),
      node,
      kind,
      key: keyOf(node),
      ...extra,
    };
    functions.push(record);
    byKey.set(record.key, record);
    const inner = {
      scope: path,
      closureBase: path,
      classPath: ctx.classPath,
      inTest: record.test,
      inFunction: true,
    };
    for (const parameter of node.parameters ?? []) visit(parameter, inner);
    if (node.body !== undefined) visit(node.body, inner);
    return record;
  };

  const visitClass = (node, path, ctx) => {
    // Field initializers run when an instance is made, not when the module loads.
    const classCtx = { ...ctx, classPath: path, closureBase: path, inFunction: true };
    for (const decorator of ts.canHaveDecorators(node) ? (ts.getDecorators(node) ?? []) : []) visit(decorator, ctx);
    for (const clause of node.heritageClauses ?? []) visit(clause, ctx);
    for (const member of node.members) {
      const name = memberName(member.name, sf);
      for (const decorator of ts.canHaveDecorators(member) ? (ts.getDecorators(member) ?? []) : []) visit(decorator, ctx);
      if (ts.isConstructorDeclaration(member)) {
        if (member.body) define(member, `${path}.constructor`, "method", classCtx, { className: path });
      } else if (ts.isMethodDeclaration(member)) {
        if (member.body && name !== null) define(member, `${path}.${name}`, "method", classCtx, { className: path });
      } else if (ts.isGetAccessorDeclaration(member) || ts.isSetAccessorDeclaration(member)) {
        const prefix = ts.isGetAccessorDeclaration(member) ? "get" : "set";
        if (member.body && name !== null) define(member, `${path}.${prefix} ${name}`, "method", classCtx, { className: path });
      } else if (ts.isPropertyDeclaration(member)) {
        if (member.initializer !== undefined) {
          if (name !== null) value(member.initializer, `${path}.${name}`, classCtx, { className: path });
          else visit(member.initializer, classCtx);
        }
      } else if (ts.isClassStaticBlockDeclaration(member)) {
        define(member, `${path}.static`, "method", classCtx, { className: path });
      }
    }
  };

  // A value written under a name: a function gets that name, an object or a class names its members after it,
  // anything else numbers the closures inside it after it.
  const value = (expr, path, ctx, extra = {}) => {
    const e = unwrap(expr);
    if (isFunctionValue(e)) {
      define(e, path, "function", ctx, extra);
    } else if (ts.isClassExpression(e)) {
      visitClass(e, e.name ? join(ctx.scope, e.name.text) : path, ctx);
    } else if (ts.isObjectLiteralExpression(e)) {
      for (const prop of e.properties) {
        const name = memberName(prop.name, sf);
        if (ts.isPropertyAssignment(prop)) {
          if (name === null) visit(prop.initializer, { ...ctx, closureBase: path });
          else value(prop.initializer, `${path}.${name}`, ctx);
        } else if (ts.isMethodDeclaration(prop)) {
          if (prop.body && name !== null) define(prop, `${path}.${name}`, "method", ctx);
        } else if (ts.isGetAccessorDeclaration(prop) || ts.isSetAccessorDeclaration(prop)) {
          const prefix = ts.isGetAccessorDeclaration(prop) ? "get" : "set";
          if (prop.body && name !== null) define(prop, `${path}.${prefix} ${name}`, "method", ctx);
        } else {
          visit(prop, { ...ctx, closureBase: path });
        }
      }
    } else {
      visit(e, { ...ctx, closureBase: path });
    }
  };

  // The path an assignment target names: x, a.b, exports.x (module.exports.x), C.m (C.prototype.m), Class.x
  // (this.x inside a class); null for anything else.
  const assignedPath = (left, ctx) => {
    const chain = nameChain(unwrap(left));
    if (!chain) return null;
    let names = chain;
    if (names[0] === "module" && names[1] === "exports") names = ["exports", ...names.slice(2)];
    else if (names.length >= 3 && names[1] === "prototype") names = [names[0], ...names.slice(2)];
    if (names[0] === "this") {
      if (!ctx.classPath || names.length < 2) return null;
      return `${ctx.classPath}.${names.slice(1).join(".")}`;
    }
    return join(ctx.scope, names.join("."));
  };

  const visit = (node, ctx) => {
    if (!ctx.inFunction && runsCode(node)) moduleRunsCode = true;
    switch (node.kind) {
      case ts.SyntaxKind.FunctionDeclaration: {
        if (node.body) {
          const exportedDefault = ts.getCombinedModifierFlags(node) & ts.ModifierFlags.Default;
          const name = node.name?.text ?? (exportedDefault ? "default" : null);
          define(node, name === null ? closurePath(ctx) : join(ctx.scope, name), name === null ? "closure" : "function", ctx);
        }
        return;
      }
      case ts.SyntaxKind.ClassDeclaration: {
        const exportedDefault = ts.getCombinedModifierFlags(node) & ts.ModifierFlags.Default;
        const name = node.name?.text ?? (exportedDefault ? "default" : null);
        visitClass(node, name === null ? closurePath(ctx) : join(ctx.scope, name), ctx);
        return;
      }
      case ts.SyntaxKind.ClassExpression:
        visitClass(node, node.name ? join(ctx.scope, node.name.text) : closurePath(ctx), ctx);
        return;
      case ts.SyntaxKind.VariableDeclaration:
        if (node.initializer !== undefined) {
          if (ts.isIdentifier(node.name)) value(node.initializer, join(ctx.scope, node.name.text), ctx);
          else {
            visit(node.name, ctx);
            visit(node.initializer, ctx);
          }
        }
        return;
      case ts.SyntaxKind.ExportAssignment:
        value(node.expression, join(ctx.scope, "default"), ctx);
        return;
      case ts.SyntaxKind.BinaryExpression:
        if (node.operatorToken.kind === ts.SyntaxKind.EqualsToken) {
          const path = assignedPath(node.left, ctx);
          if (path !== null) {
            visit(node.left, ctx);
            value(node.right, path, ctx);
            return;
          }
        }
        break;
      case ts.SyntaxKind.CallExpression: {
        const call = (ctx.inTest || testFramework) && testCall(node);
        if (call) {
          visit(node.expression, ctx);
          for (const arg of node.arguments) {
            if (unwrap(arg) === call.callback) continue;
            visit(arg, ctx);
          }
          const record = define(call.callback, join(ctx.scope, testSegment(call)), call.hook ? "hook" : "test", { ...ctx, inTest: true }, { testCall: call });
          testCalls.push({ call, record, top: !ctx.inFunction });
          return;
        }
        break;
      }
      case ts.SyntaxKind.ArrowFunction:
      case ts.SyntaxKind.FunctionExpression:
        define(node, closurePath(ctx), "closure", ctx);
        return;
      case ts.SyntaxKind.ObjectLiteralExpression:
        // An object without a name: its function members are closures of the enclosing function.
        for (const prop of node.properties) {
          const method = ts.isMethodDeclaration(prop) || ts.isGetAccessorDeclaration(prop) || ts.isSetAccessorDeclaration(prop);
          if (method && prop.body) {
            define(prop, closurePath(ctx), "closure", ctx);
          } else {
            visit(prop, ctx);
          }
        }
        return;
    }
    ts.forEachChild(node, (child) => visit(child, ctx));
  };

  const top = { scope: "", closureBase: MODULE_FUNCTION, classPath: null, inTest: false, inFunction: false };
  for (const statement of sf.statements) visit(statement, top);

  let moduleRecord = null;
  if (moduleRunsCode) {
    const path = unique(MODULE_FUNCTION);
    moduleRecord = {
      id: `${mod}.${path}`,
      name: path,
      qualifiedName: `${label}.${path}`,
      container,
      file: rel,
      startLine: 1,
      endLine: Math.max(1, lineOf(sf, sf.end)),
      generated,
      test: testFile,
      complexity: complexity(sf),
      node: sf,
      kind: "module",
      key: `${sf.fileName}:module`,
    };
    functions.push(moduleRecord);
    byKey.set(moduleRecord.key, moduleRecord);
  }
  return { functions, byKey, module: moduleRecord, testCalls };
}

/** Whether a file imports or requires one of the test framework modules (in-source tests), from syntax. */
export function importsTestFramework(sf) {
  for (const specifier of moduleSpecifiers(sf)) if (TEST_MODULES.has(specifier)) return true;
  return /\bimport\.meta\.vitest\b/.test(sf.text);
}

/** The module specifiers a file imports, re-exports or requires with a constant string. */
export function moduleSpecifiers(sf) {
  const out = [];
  const visit = (node) => {
    if ((ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) && node.moduleSpecifier && ts.isStringLiteral(node.moduleSpecifier)) {
      out.push(node.moduleSpecifier.text);
    } else if (
      ts.isCallExpression(node) &&
      node.arguments.length === 1 &&
      ts.isStringLiteralLike(node.arguments[0]) &&
      ((ts.isIdentifier(node.expression) && node.expression.text === "require") || node.expression.kind === ts.SyntaxKind.ImportKeyword)
    ) {
      out.push(node.arguments[0].text);
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
  return out;
}

/** The script kind TypeScript parses a file of this name with. */
export function scriptKind(rel) {
  if (rel.endsWith(".tsx")) return ts.ScriptKind.TSX;
  if (rel.endsWith(".jsx")) return ts.ScriptKind.JSX;
  if (/\.[cm]?ts$/.test(rel)) return ts.ScriptKind.TS;
  return ts.ScriptKind.JS;
}

/** Parses one file for its syntax only (with parent pointers). */
export function parseFile(fileName, text, rel = fileName) {
  return ts.createSourceFile(fileName, text, ts.ScriptTarget.Latest, true, scriptKind(rel));
}
