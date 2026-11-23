// The call sites of the indexed functions, resolved with the type checker.
//
// Resolution: a call whose callee the checker resolves to one declaration in
// the tree is static; one through an interface member or an abstract method
// is dynamic, with one entry per method of the tree's classes that implement
// it (or, without any, the interface member itself as an external), and so
// is a method called on a union of class types. A call of a function
// parameter is dynamic, resolved to the functions of the tree passed for
// that parameter at the call sites of its function. A callee declared
// outside the tree (the JavaScript library, @types, dependencies) is an
// external; without type information (dependencies not installed) a callee
// rooted at an import or require is the external that import names.
// Calls of values whose type is unknown otherwise are left out.
//
// Kind: a call is "always" when every path through the function that ends
// in a return reaches it, like Go's post-dominance rule with exceptions
// (throw) in the role of panics: not inside a branch (if/else, case, the
// conditional operator, the right side of && || ??, an optional chain ?.,
// a catch, a default value, a loop body other than a do-while or a
// while (true)), and not after a statement that may return early (or break
// out of such a loop). "async" is a call started without waiting for it: a
// function handed to setTimeout, setInterval, setImmediate, queueMicrotask,
// process.nextTick, requestAnimationFrame or requestIdleCallback, a promise
// callback (then, catch, finally), and a call returning a promise whose
// result is dropped (an expression statement, `void`, or a then/catch chain
// that is dropped). Awaited calls are not async: the caller waits.

import ts from "typescript";

import { isFunctionValue, lineOf, unwrap } from "./functions.js";
import { safe } from "./resolve.js";

const ALWAYS = "always";
const CONDITIONAL = "conditional";
const ASYNC = "async";

const TIMERS = new Set([
  "setTimeout",
  "setInterval",
  "setImmediate",
  "queueMicrotask",
  "requestAnimationFrame",
  "requestIdleCallback",
]);
const TIMER_HOSTS = new Set(["window", "globalThis", "global", "self", "timers"]);
const PROMISE_CALLBACKS = new Set(["then", "catch", "finally"]);

/** Collects the calls of every function of a run into `out` (calls, externals and value flows). */
export class CallCollector {
  constructor(resolver, out) {
    this.resolver = resolver;
    this.out = out;
    // Functions of the tree passed for parameters: "<function id>#<index>" -> records.
    this.flows = new Map();
    // Calls of parameters, resolved once every flow is known.
    this.parameterCalls = [];
    // Function values already accounted for (async callbacks), so they are not references too.
    this.handled = new Set();
  }

  /** Collects the calls of `record`, whose syntax is checked with `checker`. */
  collect(record, checker) {
    const fn = record.node;
    const state = { record, checker };
    if (record.kind === "module") {
      this.statements(fn.statements, ALWAYS, state);
      return;
    }
    if (ts.isConstructorDeclaration(fn)) {
      for (const member of fn.parent.members) {
        if (ts.isPropertyDeclaration(member) && member.initializer && !isStatic(member) && !isFunctionValue(unwrap(member.initializer))) {
          this.walk(member.initializer, ALWAYS, state);
        }
      }
    }
    for (const parameter of fn.parameters ?? []) this.walk(parameter, ALWAYS, state);
    if (fn.body === undefined) return;
    if (ts.isBlock(fn.body)) this.statements(fn.body.statements, ALWAYS, state);
    else this.walk(fn.body, ALWAYS, state);
  }

  statements(list, kind, state) {
    let current = kind;
    for (const statement of list) {
      this.walk(statement, current, state);
      if (current === ALWAYS && mayExit(statement)) current = CONDITIONAL;
    }
  }

  walk(node, kind, state) {
    if (!node) return;
    if (ts.isFunctionLike(node) || ts.isClassLike(node) || ts.isClassStaticBlockDeclaration(node)) return;
    if (ts.isDecorator(node) || ts.isTypeNode(node) || ts.isInterfaceDeclaration(node) || ts.isTypeAliasDeclaration(node)) return;
    switch (node.kind) {
      case ts.SyntaxKind.Block:
        this.statements(node.statements, kind, state);
        return;
      case ts.SyntaxKind.IfStatement:
        this.walk(node.expression, kind, state);
        this.walk(node.thenStatement, CONDITIONAL, state);
        this.walk(node.elseStatement, CONDITIONAL, state);
        return;
      case ts.SyntaxKind.ConditionalExpression:
        this.walk(node.condition, kind, state);
        this.walk(node.whenTrue, CONDITIONAL, state);
        this.walk(node.whenFalse, CONDITIONAL, state);
        return;
      case ts.SyntaxKind.BinaryExpression:
        if (isShortCircuit(node.operatorToken.kind)) {
          this.walk(node.left, kind, state);
          this.walk(node.right, CONDITIONAL, state);
          return;
        }
        break;
      case ts.SyntaxKind.ForStatement:
        this.walk(node.initializer, kind, state);
        this.walk(node.condition, kind, state);
        this.walk(node.statement, node.condition === undefined || isTrue(node.condition) ? kind : CONDITIONAL, state);
        this.walk(node.incrementor, CONDITIONAL, state);
        return;
      case ts.SyntaxKind.ForInStatement:
      case ts.SyntaxKind.ForOfStatement:
        this.walk(node.expression, kind, state);
        this.walk(node.initializer, CONDITIONAL, state);
        this.walk(node.statement, CONDITIONAL, state);
        return;
      case ts.SyntaxKind.WhileStatement:
        this.walk(node.expression, kind, state);
        this.walk(node.statement, isTrue(node.expression) ? kind : CONDITIONAL, state);
        return;
      case ts.SyntaxKind.DoStatement:
        this.walk(node.statement, kind, state);
        this.walk(node.expression, kind, state);
        return;
      case ts.SyntaxKind.SwitchStatement:
        this.walk(node.expression, kind, state);
        for (const clause of node.caseBlock.clauses) {
          if (ts.isCaseClause(clause)) this.walk(clause.expression, CONDITIONAL, state);
          this.statements(clause.statements, CONDITIONAL, state);
        }
        return;
      case ts.SyntaxKind.TryStatement:
        this.walk(node.tryBlock, kind, state);
        if (node.catchClause) this.walk(node.catchClause.block, CONDITIONAL, state);
        this.walk(node.finallyBlock, kind, state);
        return;
      case ts.SyntaxKind.Parameter:
      case ts.SyntaxKind.BindingElement:
        this.walk(node.initializer, CONDITIONAL, state);
        if (node.name && !ts.isIdentifier(node.name)) this.walk(node.name, kind, state);
        return;
      case ts.SyntaxKind.CallExpression: {
        const own = optional(node) ? CONDITIONAL : kind;
        this.call(node, own, state);
        this.walk(node.expression, kind, state);
        for (const arg of node.arguments) this.walk(arg, own, state);
        return;
      }
      case ts.SyntaxKind.NewExpression:
        this.construct(node, kind, state);
        this.walk(node.expression, kind, state);
        for (const arg of node.arguments ?? []) this.walk(arg, kind, state);
        return;
      case ts.SyntaxKind.TaggedTemplateExpression:
        this.callee(node, node.tag, kind, state);
        this.walk(node.tag, kind, state);
        this.walk(node.template, kind, state);
        return;
      case ts.SyntaxKind.JsxSelfClosingElement:
      case ts.SyntaxKind.JsxOpeningElement:
        this.element(node, kind, state);
        this.walk(node.attributes, kind, state);
        return;
      case ts.SyntaxKind.PropertyAccessExpression:
        this.accessor(node, optional(node) ? CONDITIONAL : kind, state);
        break;
    }
    ts.forEachChild(node, (child) => this.walk(child, kind, state));
  }

  /** A call expression: its callee, and the async callbacks and value flows of its arguments. */
  call(node, kind, state) {
    const callee = unwrap(node.expression);
    if (ts.isIdentifier(callee) && callee.text === "require") return;
    if (node.expression.kind === ts.SyntaxKind.ImportKeyword) return;
    const { checker, record } = state;
    const line = lineOf(node.getSourceFile(), calleeStart(node.expression));
    let callKind = kind;
    if (discardedPromise(node, checker, this.resolver)) callKind = ASYNC;

    // f.call(this, ...) and f.apply(this, args) call f.
    if (ts.isPropertyAccessExpression(callee) && (callee.name.text === "call" || callee.name.text === "apply")) {
      const targets = this.resolver.valueTargets(callee.expression, checker);
      if (targets.length > 0) {
        for (const t of targets) this.emit(record, { record: t.record }, line, callKind, "static");
        return;
      }
    }
    const targets = this.callee(node, node.expression, callKind, state, line);

    // Functions handed to a timer or to a promise run later.
    if (isTimer(callee) || isPromiseCallback(callee)) {
      for (const arg of node.arguments) {
        for (const t of this.resolver.valueTargets(arg, checker)) {
          if (!isFunctionValue(unwrap(arg)) && !ts.isIdentifier(unwrap(arg)) && !ts.isPropertyAccessExpression(unwrap(arg))) continue;
          this.emit(record, { record: t.record }, line, ASYNC, "static");
          this.handled.add(nodeKey(t.node, record));
        }
      }
      return;
    }
    // new Promise(executor) runs the executor at once; that is handled in construct().
    for (const target of targets) {
      if (!target.record) continue;
      node.arguments.forEach((arg, index) => {
        for (const t of this.resolver.valueTargets(arg, checker)) {
          if (!isFunctionValue(unwrap(arg)) && !ts.isIdentifier(unwrap(arg)) && !ts.isPropertyAccessExpression(unwrap(arg))) continue;
          this.flow(target.record, index, t.record);
        }
      });
    }
  }

  /** `new C(...)`: the constructor of C; `new Promise(executor)` also calls the executor. */
  construct(node, kind, state) {
    const { checker, record } = state;
    const line = lineOf(node.getSourceFile(), calleeStart(node.expression));
    const callee = unwrap(node.expression);
    if (ts.isIdentifier(callee) && callee.text === "Promise" && node.arguments?.length > 0) {
      for (const t of this.resolver.valueTargets(node.arguments[0], checker)) {
        this.emit(record, { record: t.record }, line, kind, "static");
        this.handled.add(nodeKey(t.node, record));
      }
    }
    const sig = safe(() => checker.getResolvedSignature(node));
    const decl = sig?.declaration;
    if (decl && !ts.isJSDocSignature(decl)) {
      const found = this.resolver.recordOfDeclaration(decl, checker);
      if (found) {
        this.emit(record, { record: found }, line, kind, "static");
        return;
      }
      if (!this.resolver.inSource(decl)) {
        const ext = this.resolver.externalOfOrigin(this.resolver.expressionOrigin(callee, checker)) ?? this.resolver.externalOfDeclaration(decl);
        if (ext) this.emit(record, { external: ext }, line, kind, "static");
        return;
      }
    }
    // A class of the tree without a constructor: nothing runs that is listed.
    const symbol = safe(() => checker.getSymbolAtLocation(callee));
    const viaSymbol = this.resolver.recordOfSymbol(symbol, checker);
    if (viaSymbol) {
      this.emit(record, { record: viaSymbol }, line, kind, "static");
      return;
    }
    if (symbol?.declarations?.some((d) => this.resolver.inSource(d))) return;
    const origin = this.resolver.expressionOrigin(callee, checker);
    const ext = this.resolver.externalOfOrigin(origin);
    if (ext) this.emit(record, { external: { ...ext, id: `${ext.id}.constructor`, name: `${ext.name}.constructor` } }, line, kind, "static");
  }

  /** Reading (or assigning) a property that is a getter (setter) of the tree calls it. */
  accessor(node, kind, state) {
    const { checker, record } = state;
    const symbol = safe(() => checker.getSymbolAtLocation(node.name));
    if (!symbol || !(symbol.flags & ts.SymbolFlags.Accessor)) return;
    const parent = node.parent;
    const writing = ts.isBinaryExpression(parent) && parent.left === node && parent.operatorToken.kind === ts.SyntaxKind.EqualsToken;
    const wanted = writing ? ts.isSetAccessorDeclaration : ts.isGetAccessorDeclaration;
    const line = lineOf(node.getSourceFile(), node.name.getStart());
    for (const decl of symbol.declarations ?? []) {
      if (!wanted(decl)) continue;
      const found = this.resolver.recordOfNode(decl);
      if (found) this.emit(record, { record: found }, line, kind, "static");
      else if (abstractMember(decl) && this.resolver.inSource(decl)) {
        for (const t of this.interfaceTargets(decl, record)) {
          if (t.record && !wanted(t.record.node)) continue;
          this.emit(record, t, line, kind, "dynamic");
        }
      }
    }
  }

  /** A JSX element whose tag is a component: React calls it to render. */
  element(node, kind, state) {
    const tag = node.tagName;
    if (ts.isIdentifier(tag) && !/^[A-Z]/.test(tag.text)) return;
    const line = lineOf(node.getSourceFile(), tag.getStart());
    for (const t of this.resolver.valueTargets(tag, state.checker)) this.emit(state.record, { record: t.record }, line, kind, "static");
  }

  /** Resolves the callee of `node` and records the calls; returns the targets. */
  callee(node, calleeExpr, kind, state, line = lineOf(node.getSourceFile(), calleeStart(calleeExpr))) {
    const { checker, record } = state;
    const callee = unwrap(calleeExpr);
    const resolver = this.resolver;
    const emitAll = (targets, resolution) => {
      for (const t of targets) this.emit(record, t, line, kind, resolution);
      return targets;
    };
    if (isFunctionValue(callee)) {
      const found = resolver.recordOfNode(callee);
      return found ? emitAll([{ record: found }], "static") : [];
    }
    // A method of a union of types: each member's method.
    if (ts.isPropertyAccessExpression(callee) || ts.isElementAccessExpression(callee)) {
      const union = this.unionTargets(callee, checker);
      if (union !== null) return emitAll(union, "dynamic");
    }
    const sig = ts.isCallExpression(node) || ts.isNewExpression(node) || ts.isTaggedTemplateExpression(node) ? safe(() => checker.getResolvedSignature(node)) : undefined;
    let decl = sig?.declaration;
    if (decl && ts.isJSDocSignature(decl)) decl = undefined;
    if (decl) {
      const inTree = resolver.inSource(decl);
      const found = resolver.recordOfDeclaration(decl, checker) ?? (inTree ? this.implementationOf(decl, callee, checker) : null);
      if (found && !abstractMember(decl)) return emitAll([{ record: found }], "static");
      if (inTree && (abstractMember(decl) || isInterfaceMember(decl))) {
        return emitAll(this.interfaceTargets(decl, record), "dynamic");
      }
      if (inTree && isFunctionType(decl)) {
        // A function value: a parameter's callers say what it is.
        this.parameterCall(callee, kind, state, line);
        return [];
      }
      if (!inTree) {
        const ext = this.externalCallee(callee, decl, checker);
        if (ext) return emitAll([{ external: ext }], "static");
        return [];
      }
    }
    if (ts.isIdentifier(callee) && this.parameterCall(callee, kind, state, line)) return [];
    // A value of the tree whose type the checker does not know: what its symbol declares.
    if (ts.isIdentifier(callee) || ts.isPropertyAccessExpression(callee)) {
      const symbol = safe(() => checker.getSymbolAtLocation(callee));
      const found = resolver.recordOfSymbol(symbol, checker);
      if (found && found.kind !== "module") return emitAll([{ record: found }], "static");
    }
    // Without types (dependencies not installed): the import it comes from.
    const ext = resolver.externalOfOrigin(resolver.expressionOrigin(callee, checker));
    return ext ? emitAll([{ external: ext }], "static") : [];
  }

  /**
   * The external a callee declared outside the tree is. A callee rooted at an import is named after the import
   * (express.Router, fs.readFileSync), as it is without types, so ids do not depend on @types being installed.
   * A call through a call signature (an interface whose values are functions) is named after the property or
   * variable called: IRouter.get rather than IRouterMatcher.call.
   */
  externalCallee(callee, decl, checker) {
    const resolver = this.resolver;
    const fromImport = resolver.externalOfOrigin(resolver.expressionOrigin(callee, checker));
    if (fromImport) return fromImport;
    if (ts.isCallSignatureDeclaration(decl) || !decl.name) {
      const nameNode = ts.isPropertyAccessExpression(callee) ? callee.name : callee;
      let symbol = safe(() => checker.getSymbolAtLocation(nameNode));
      if (symbol && symbol.flags & ts.SymbolFlags.Alias) symbol = safe(() => checker.getAliasedSymbol(symbol)) ?? symbol;
      const named = symbol?.declarations?.find(
        (d) => ts.isPropertySignature(d) || ts.isPropertyDeclaration(d) || ts.isVariableDeclaration(d) || ts.isMethodSignature(d),
      );
      if (named && !resolver.inSource(named)) {
        const ext = resolver.externalOfDeclaration(named);
        if (ext) return ext;
      }
    }
    return resolver.externalOfDeclaration(decl);
  }

  /** The implementation of an overloaded function or method whose signature has no body. */
  implementationOf(decl, callee, checker) {
    const name = ts.isPropertyAccessExpression(callee) ? callee.name : callee;
    const symbol = safe(() => checker.getSymbolAtLocation(name));
    return this.resolver.recordOfSymbol(symbol, checker);
  }

  /** For a method called on a union of types, each member type's method; null when the receiver is not a union. */
  unionTargets(callee, checker) {
    const receiver = safe(() => checker.getTypeAtLocation(callee.expression));
    if (!receiver || !receiver.isUnion()) return null;
    const name = ts.isPropertyAccessExpression(callee) ? callee.name.text : stringOf(callee.argumentExpression);
    if (name === null) return null;
    const out = [];
    for (const member of receiver.types) {
      if (member.flags & (ts.TypeFlags.Undefined | ts.TypeFlags.Null)) continue;
      const prop = safe(() => member.getProperty(name));
      for (const decl of prop?.declarations ?? []) {
        const record = this.resolver.recordOfNode(decl);
        if (record) out.push({ record });
        else if (abstractMember(decl) || isInterfaceMember(decl)) out.push(...this.interfaceTargets(decl, null));
      }
    }
    if (out.length === 0) return null;
    return dedupe(out);
  }

  /** The methods that implement an interface member or an abstract method, else the member as an external. */
  interfaceTargets(decl, caller) {
    const owner = decl.parent;
    const name = decl.name && (ts.isIdentifier(decl.name) || ts.isPrivateIdentifier(decl.name) || ts.isStringLiteral(decl.name)) ? decl.name.text : null;
    let targets = [];
    if (name !== null && owner && (ts.isInterfaceDeclaration(owner) || ts.isClassLike(owner))) {
      targets = this.resolver.implementations(owner, name).map((record) => ({ record }));
    }
    // Tests are analysed with the code they test, so their fakes implement the interfaces it uses; production
    // code does not list them.
    if (caller && !caller.test) targets = targets.filter((t) => !t.record.test);
    if (targets.length === 0) {
      const ext = this.resolver.externalOfDeclaration(decl);
      return ext ? [{ external: ext }] : [];
    }
    return dedupe(targets);
  }

  /** Records a call of a parameter of an indexed function, resolved later from what is passed for it. */
  parameterCall(callee, kind, state, line) {
    if (!ts.isIdentifier(callee)) return false;
    const symbol = safe(() => state.checker.getSymbolAtLocation(callee));
    const decl = symbol?.valueDeclaration;
    if (!decl || !ts.isParameter(decl) || !ts.isIdentifier(decl.name)) return false;
    const owner = this.resolver.recordOfNode(decl.parent);
    if (!owner) return false;
    const index = decl.parent.parameters.indexOf(decl);
    this.parameterCalls.push({ caller: state.record, owner, index, line, kind });
    return true;
  }

  flow(callee, index, target) {
    const key = `${callee.id}#${index}`;
    let set = this.flows.get(key);
    if (!set) this.flows.set(key, (set = new Map()));
    set.set(target.id, target);
  }

  /** Resolves the calls of parameters from the functions passed for them. */
  finish() {
    for (const call of this.parameterCalls) {
      const targets = this.flows.get(`${call.owner.id}#${call.index}`);
      if (!targets) continue;
      for (const target of targets.values()) {
        if (!call.caller.test && target.test) continue;
        this.emit(call.caller, { record: target }, call.line, call.kind, "dynamic");
      }
    }
  }

  emit(caller, target, line, kind, resolution) {
    if (target.record) {
      this.out.calls.push({ caller: caller.id, callee: target.record.id, line, kind, resolution, relation: "call" });
    } else if (target.external) {
      this.out.externals.set(target.external.id, target.external);
      this.out.calls.push({ caller: caller.id, callee: target.external.id, line, kind, resolution, relation: "call" });
    }
  }
}

/** A key for a function value at a place, to tell it is accounted for. */
export function nodeKey(node, caller) {
  return `${caller.id}|${node.getSourceFile().fileName}:${node.pos}`;
}

function dedupe(targets) {
  const seen = new Set();
  return targets.filter((t) => {
    const id = t.record ? t.record.id : t.external.id;
    if (seen.has(id)) return false;
    seen.add(id);
    return true;
  });
}

function stringOf(expr) {
  return expr && (ts.isStringLiteral(expr) || ts.isNumericLiteral(expr)) ? expr.text : null;
}

function isStatic(member) {
  return (ts.getCombinedModifierFlags(member) & ts.ModifierFlags.Static) !== 0;
}

function isShortCircuit(kind) {
  return (
    kind === ts.SyntaxKind.AmpersandAmpersandToken ||
    kind === ts.SyntaxKind.BarBarToken ||
    kind === ts.SyntaxKind.QuestionQuestionToken ||
    kind === ts.SyntaxKind.AmpersandAmpersandEqualsToken ||
    kind === ts.SyntaxKind.BarBarEqualsToken ||
    kind === ts.SyntaxKind.QuestionQuestionEqualsToken
  );
}

function isTrue(expr) {
  return unwrap(expr)?.kind === ts.SyntaxKind.TrueKeyword;
}

/** Whether a call is part of an optional chain (a?.b(), a.b?.(), a?.b.c()): it may not happen. */
function optional(node) {
  for (let cur = node; cur; ) {
    if (cur.questionDotToken) return true;
    if (ts.isCallExpression(cur) || ts.isPropertyAccessExpression(cur) || ts.isElementAccessExpression(cur) || ts.isNonNullExpression(cur)) {
      cur = cur.expression;
    } else return false;
  }
  return false;
}

/** The position a call is placed at: the name of the method for a.b.c(), so a chain spread over lines is placed well. */
function calleeStart(expr) {
  const e = unwrap(expr);
  if (ts.isPropertyAccessExpression(e)) return e.name.getStart();
  return e.getStart();
}

/** Whether `statement` may leave the enclosing function or loop early: a return, or a break or continue out of it. */
function mayExit(statement) {
  let found = false;
  const visit = (node, loops, switches) => {
    if (found || ts.isFunctionLike(node) || ts.isClassLike(node)) return;
    if (ts.isReturnStatement(node)) {
      found = true;
      return;
    }
    if (ts.isBreakStatement(node)) {
      if (node.label || (loops === 0 && switches === 0)) found = true;
      return;
    }
    if (ts.isContinueStatement(node)) {
      if (node.label || loops === 0) found = true;
      return;
    }
    const loop = ts.isIterationStatement(node, false) ? 1 : 0;
    const sw = ts.isSwitchStatement(node) ? 1 : 0;
    ts.forEachChild(node, (child) => visit(child, loops + loop, switches + sw));
  };
  visit(statement, 0, 0);
  return found;
}

function isTimer(callee) {
  if (ts.isIdentifier(callee)) return TIMERS.has(callee.text);
  if (ts.isPropertyAccessExpression(callee) && ts.isIdentifier(callee.expression)) {
    const host = callee.expression.text;
    if (host === "process" && callee.name.text === "nextTick") return true;
    return TIMER_HOSTS.has(host) && TIMERS.has(callee.name.text);
  }
  return false;
}

function isPromiseCallback(callee) {
  return ts.isPropertyAccessExpression(callee) && PROMISE_CALLBACKS.has(callee.name.text);
}

/** Whether a declaration is an abstract method, or a member without a body of an abstract class. */
function abstractMember(decl) {
  return (
    (ts.isMethodDeclaration(decl) || ts.isGetAccessorDeclaration(decl) || ts.isPropertyDeclaration(decl)) &&
    (ts.getCombinedModifierFlags(decl) & ts.ModifierFlags.Abstract) !== 0
  );
}

function isInterfaceMember(decl) {
  return (
    (ts.isMethodSignature(decl) || ts.isPropertySignature(decl) || ts.isCallSignatureDeclaration(decl)) &&
    decl.parent !== undefined &&
    (ts.isInterfaceDeclaration(decl.parent) || ts.isTypeLiteralNode(decl.parent))
  );
}

function isFunctionType(decl) {
  return ts.isFunctionTypeNode(decl) || ts.isCallSignatureDeclaration(decl) || ts.isParameter(decl) || ts.isConstructorTypeNode(decl);
}

/**
 * Whether the value of a call is a promise nobody waits for: the call is a statement of its own, under `void`,
 * or the start of a then/catch/finally chain that is.
 */
function discardedPromise(node, checker, resolver) {
  let cur = node;
  for (;;) {
    let parent = cur.parent;
    while (parent && ts.isParenthesizedExpression(parent)) {
      cur = parent;
      parent = parent.parent;
    }
    if (!parent) return false;
    if (ts.isExpressionStatement(parent) || ts.isVoidExpression(parent)) break;
    if (
      ts.isPropertyAccessExpression(parent) &&
      parent.expression === cur &&
      PROMISE_CALLBACKS.has(parent.name.text) &&
      ts.isCallExpression(parent.parent) &&
      parent.parent.expression === parent
    ) {
      cur = parent.parent;
      continue;
    }
    return false;
  }
  return returnsPromise(node, checker, resolver);
}

function returnsPromise(node, checker, resolver) {
  const callee = unwrap(node.expression);
  if (ts.isPropertyAccessExpression(callee) && PROMISE_CALLBACKS.has(callee.name.text)) return true;
  const type = safe(() => checker.getTypeAtLocation(node));
  if (type && !(type.flags & ts.TypeFlags.Any)) {
    const then = safe(() => type.getProperty("then"));
    if (then) return true;
    return false;
  }
  const sig = safe(() => checker.getResolvedSignature(node));
  const decl = sig?.declaration;
  if (decl && !ts.isJSDocSignature(decl)) {
    const record = resolver.recordOfDeclaration(decl, checker);
    const fn = record?.node;
    if (fn && ts.getCombinedModifierFlags(fn) & ts.ModifierFlags.Async) return true;
  }
  return false;
}
