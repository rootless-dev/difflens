// What expressions denote: the indexed function a symbol or declaration is,
// the external (outside the tree) a callee is, the module an import binding
// comes from, and the classes of the tree that implement an interface or
// extend an abstract class.

import ts from "typescript";

import { containerOf, modulePath } from "../files.js";
import { isFunctionValue, keyOf, unwrap } from "./functions.js";

const MAX_DEPTH = 6;

/** "fs" for "node:fs"; module specifiers name externals. */
export function normalizeModule(specifier) {
  return specifier.startsWith("node:") ? specifier.slice(5) : specifier;
}

export class Resolver {
  /**
   * `index`: { byKey, byId, files: Map<fileName, file info>, checkerOf(node) }, `tree`: the Tree, `rootName`:
   * the container of root files.
   */
  constructor(index, tree, rootName) {
    this.index = index;
    this.tree = tree;
    this.rootName = rootName;
    this.heritage = null;
  }

  checkerOf(node) {
    return this.index.checkerOf(node);
  }

  /** Whether a node is in the tree's own sources (not a declaration file, not under node_modules). */
  inSource(node) {
    const sf = node.getSourceFile();
    if (sf.isDeclarationFile) return false;
    const rel = this.tree.rel(sf.fileName);
    return rel !== null && !rel.split("/").includes("node_modules");
  }

  /** The record of a function-like node, or null. */
  recordOfNode(node) {
    if (!node) return null;
    if (ts.isSourceFile(node)) return this.index.byKey.get(`${node.fileName}:module`) ?? null;
    return this.index.byKey.get(keyOf(node)) ?? null;
  }

  /** The innermost indexed function holding `node` (the module body for top-level code), or null. */
  enclosing(node) {
    for (let cur = node.parent; cur; cur = cur.parent) {
      if (ts.isFunctionLike(cur) || ts.isClassStaticBlockDeclaration(cur)) {
        const record = this.recordOfNode(cur);
        if (record) return record;
      }
      if (ts.isSourceFile(cur)) return this.recordOfNode(cur);
    }
    return null;
  }

  /** The indexed function a symbol denotes (aliases, re-exports and CommonJS assignments followed), or null. */
  recordOfSymbol(symbol, checker, depth = 0) {
    if (!symbol || depth > MAX_DEPTH) return null;
    if (symbol.flags & ts.SymbolFlags.Alias) {
      let aliased;
      try {
        aliased = checker.getAliasedSymbol(symbol);
      } catch {
        aliased = undefined;
      }
      if (aliased && aliased !== symbol && !(aliased.flags & ts.SymbolFlags.Alias && aliased.escapedName === "unknown")) {
        const found = this.recordOfSymbol(aliased, checker, depth + 1);
        if (found) return found;
      }
    }
    const declarations = symbol.declarations ?? [];
    // An implementation, not an overload signature.
    const ordered = [...declarations].sort((a, b) => (a.body ? 0 : 1) - (b.body ? 0 : 1));
    for (const decl of ordered) {
      const found = this.recordOfDeclaration(decl, checker, depth + 1);
      if (found) return found;
    }
    return null;
  }

  /** The indexed function a declaration declares, or null. A class declares its constructor. */
  recordOfDeclaration(decl, checker, depth = 0) {
    if (!decl || depth > MAX_DEPTH) return null;
    const direct = this.recordOfNode(decl);
    if (direct) return direct;
    const viaExpression = (expr) => {
      const e = unwrap(expr);
      if (!e) return null;
      if (isFunctionValue(e)) return this.recordOfNode(e);
      if (ts.isIdentifier(e) || ts.isPropertyAccessExpression(e)) {
        return this.recordOfSymbol(safe(() => checker.getSymbolAtLocation(e)), checker, depth + 1);
      }
      return null;
    };
    switch (decl.kind) {
      case ts.SyntaxKind.VariableDeclaration:
      case ts.SyntaxKind.PropertyAssignment:
      case ts.SyntaxKind.PropertyDeclaration:
        return decl.initializer ? viaExpression(decl.initializer) : null;
      case ts.SyntaxKind.ShorthandPropertyAssignment:
        return this.recordOfSymbol(safe(() => checker.getShorthandAssignmentValueSymbol(decl)), checker, depth + 1);
      case ts.SyntaxKind.BinaryExpression:
        return viaExpression(decl.right);
      case ts.SyntaxKind.PropertyAccessExpression:
      case ts.SyntaxKind.ElementAccessExpression:
        if (ts.isBinaryExpression(decl.parent) && decl.parent.left === decl) return viaExpression(decl.parent.right);
        return null;
      case ts.SyntaxKind.ExportAssignment:
        return viaExpression(decl.expression);
      case ts.SyntaxKind.ExportSpecifier:
        return this.recordOfSymbol(safe(() => checker.getExportSpecifierLocalTargetSymbol(decl)), checker, depth + 1);
      case ts.SyntaxKind.ClassDeclaration:
      case ts.SyntaxKind.ClassExpression: {
        const ctor = decl.members.find((m) => ts.isConstructorDeclaration(m) && m.body);
        return ctor ? this.recordOfNode(ctor) : null;
      }
      default:
        return null;
    }
  }

  /**
   * The indexed functions an expression used as a value denotes, each with the expression that denotes it:
   * a function literal, a function or method name, `f.bind(x)`, a wrapper call such as asyncHandler(fn) (its
   * function arguments), or an array of those.
   */
  valueTargets(expr, checker, depth = 0) {
    const e = unwrap(expr);
    if (!e || depth > 3) return [];
    if (isFunctionValue(e)) {
      const record = this.recordOfNode(e);
      return record ? [{ record, node: e }] : [];
    }
    if (ts.isIdentifier(e) || ts.isPropertyAccessExpression(e)) {
      const record = this.recordOfSymbol(safe(() => checker.getSymbolAtLocation(e)), checker);
      return record && record.kind !== "module" ? [{ record, node: e }] : [];
    }
    if (ts.isArrayLiteralExpression(e)) return e.elements.flatMap((el) => this.valueTargets(el, checker, depth + 1));
    if (ts.isCallExpression(e)) {
      const callee = unwrap(e.expression);
      if (ts.isPropertyAccessExpression(callee) && callee.name.text === "bind") {
        return this.valueTargets(callee.expression, checker, depth + 1);
      }
      return e.arguments.flatMap((arg) => {
        const a = unwrap(arg);
        return isFunctionValue(a) || ts.isIdentifier(a) || ts.isPropertyAccessExpression(a) ? this.valueTargets(a, checker, depth + 1) : [];
      });
    }
    return [];
  }

  /**
   * Where an identifier bound by an import or a require comes from: the module and the names after it, such as
   * { module: "express", names: ["Router"] } for `import { Router } from "express"`. Null for anything else.
   */
  importOrigin(identifier, checker) {
    const symbol = safe(() => checker.getSymbolAtLocation(identifier));
    for (const decl of symbol?.declarations ?? []) {
      const origin = declarationOrigin(decl);
      if (origin) return origin;
    }
    return null;
  }

  /** The origin of an expression such as fs.promises.readFile: the import of its root and the names after it. */
  expressionOrigin(expr, checker) {
    const e = unwrap(expr);
    if (ts.isIdentifier(e)) return this.importOrigin(e, checker);
    if (ts.isPropertyAccessExpression(e) && ts.isIdentifier(e.name)) {
      const head = this.expressionOrigin(e.expression, checker);
      return head && { module: head.module, names: [...head.names, e.name.text] };
    }
    if (ts.isCallExpression(e) && ts.isIdentifier(e.expression) && e.expression.text === "require") {
      const spec = e.arguments[0];
      return spec && ts.isStringLiteralLike(spec) ? { module: normalizeModule(spec.text), names: [] } : null;
    }
    return null;
  }

  /** The external an import origin denotes, or null for a relative import (a file of the tree that did not resolve). */
  externalOfOrigin(origin) {
    if (!origin || origin.module.startsWith(".") || origin.module.startsWith("/")) return null;
    const name = origin.names.length > 0 ? origin.names.join(".") : origin.module;
    const id = origin.names.length > 0 ? `${origin.module}.${name}` : origin.module;
    return { id, name, container: origin.module };
  }

  /** The external a declaration outside the tree (or a member without a body in it) is, named like ids. */
  externalOfDeclaration(decl) {
    const sf = decl.getSourceFile();
    const name = memberPath(decl);
    if (name === null) return null;
    const rel = this.tree.rel(sf.fileName);
    if (rel !== null && !rel.split("/").includes("node_modules")) {
      return { id: `${modulePath(rel)}.${name}`, name, container: containerOf(rel, this.rootName) };
    }
    const container = externalContainer(sf, decl);
    return { id: `${container}.${name}`, name, container };
  }

  /**
   * The methods of the tree that a call of `member` through the interface or abstract class declared by
   * `typeDecl` may run: the member of every class of the tree that implements or extends it (directly or
   * through other classes and interfaces).
   */
  implementations(typeDecl, member) {
    const heritage = this.heritageIndex();
    const key = keyOf(typeDecl);
    const out = [];
    for (const cls of heritage.classes) {
      if (!heritage.supers(cls).has(key)) continue;
      const checker = this.checkerOf(cls);
      if (!checker) continue;
      const symbol = cls.name ? safe(() => checker.getSymbolAtLocation(cls.name)) : safe(() => checker.getTypeAtLocation(cls).symbol);
      if (!symbol) continue;
      const type = safe(() => checker.getDeclaredTypeOfSymbol(symbol));
      const prop = type && safe(() => type.getProperty(member));
      if (!prop) continue;
      for (const decl of prop.declarations ?? []) {
        const record = this.recordOfNode(decl);
        if (record) out.push(record);
      }
    }
    return out;
  }

  /** The classes of the tree with, for each, the keys of every type it inherits from. */
  heritageIndex() {
    if (this.heritage) return this.heritage;
    const classes = [];
    for (const file of this.index.files.values()) {
      const visit = (node) => {
        if (ts.isClassLike(node)) classes.push(node);
        ts.forEachChild(node, visit);
      };
      visit(file.sf);
    }
    const direct = new Map();
    const directSupers = (decl) => {
      const key = keyOf(decl);
      if (direct.has(key)) return direct.get(key);
      const out = [];
      direct.set(key, out);
      const checker = this.checkerOf(decl);
      if (!checker) return out;
      for (const clause of decl.heritageClauses ?? []) {
        for (const type of clause.types) {
          let symbol = safe(() => checker.getSymbolAtLocation(type.expression));
          if (symbol && symbol.flags & ts.SymbolFlags.Alias) symbol = safe(() => checker.getAliasedSymbol(symbol)) ?? symbol;
          for (const d of symbol?.declarations ?? []) {
            if (ts.isClassLike(d) || ts.isInterfaceDeclaration(d)) out.push(d);
          }
        }
      }
      return out;
    };
    const cache = new Map();
    const supers = (decl) => {
      const key = keyOf(decl);
      if (cache.has(key)) return cache.get(key);
      const all = new Set();
      cache.set(key, all);
      for (const parent of directSupers(decl)) {
        all.add(keyOf(parent));
        for (const k of supers(parent)) all.add(k);
      }
      return all;
    };
    this.heritage = { classes, supers };
    return this.heritage;
  }
}

/** Runs `f`, turning a compiler exception into undefined (the checker throws on some broken code). */
export function safe(f) {
  try {
    return f();
  } catch {
    return undefined;
  }
}

function declarationOrigin(decl) {
  const fromSpecifier = (spec) => (spec && ts.isStringLiteralLike(spec) ? normalizeModule(spec.text) : null);
  if (ts.isImportSpecifier(decl)) {
    const module = fromSpecifier(decl.parent.parent.parent.moduleSpecifier);
    const imported = (decl.propertyName ?? decl.name).text;
    return module && { module, names: imported === "default" ? [] : [imported] };
  }
  if (ts.isImportClause(decl)) {
    const module = fromSpecifier(decl.parent.moduleSpecifier);
    return module && { module, names: [] };
  }
  if (ts.isNamespaceImport(decl)) {
    const module = fromSpecifier(decl.parent.parent.moduleSpecifier);
    return module && { module, names: [] };
  }
  if (ts.isImportEqualsDeclaration(decl) && ts.isExternalModuleReference(decl.moduleReference)) {
    const module = fromSpecifier(decl.moduleReference.expression);
    return module && { module, names: [] };
  }
  if (ts.isVariableDeclaration(decl) && decl.initializer) {
    const module = requiredModule(decl.initializer);
    if (module && ts.isIdentifier(decl.name)) return { module, names: [] };
  }
  if (ts.isBindingElement(decl) && ts.isObjectBindingPattern(decl.parent) && ts.isVariableDeclaration(decl.parent.parent)) {
    const module = decl.parent.parent.initializer && requiredModule(decl.parent.parent.initializer);
    const name = decl.propertyName ?? decl.name;
    if (module && ts.isIdentifier(name)) return { module, names: [name.text] };
  }
  return null;
}

/** The module of require("m") or require("m").x, or null. */
function requiredModule(expr) {
  let e = unwrap(expr);
  const names = [];
  while (ts.isPropertyAccessExpression(e)) {
    names.unshift(e.name.text);
    e = unwrap(e.expression);
  }
  if (ts.isCallExpression(e) && ts.isIdentifier(e.expression) && e.expression.text === "require") {
    const spec = e.arguments[0];
    if (spec && ts.isStringLiteralLike(spec)) return names.length === 0 ? normalizeModule(spec.text) : null;
  }
  return null;
}

/** The name of a declaration with the type or namespace that holds it: "Console.log", "readFileSync". */
function memberPath(decl) {
  const nameNode = decl.name;
  let name = null;
  if (nameNode && (ts.isIdentifier(nameNode) || ts.isPrivateIdentifier(nameNode) || ts.isStringLiteral(nameNode))) name = nameNode.text;
  else if (ts.isConstructorDeclaration(decl) || ts.isConstructSignatureDeclaration(decl)) name = "constructor";
  else if (ts.isCallSignatureDeclaration(decl)) name = "call";
  if (name === null) return null;
  let owner = decl.parent;
  if (owner && ts.isVariableDeclarationList(owner)) owner = null;
  const parts = [name];
  for (let cur = owner; cur; cur = cur.parent) {
    if ((ts.isInterfaceDeclaration(cur) || ts.isClassLike(cur)) && cur.name) {
      parts.unshift(cur.name.text.replace(/Constructor$/, "") || cur.name.text);
      break;
    }
    if (ts.isTypeLiteralNode(cur)) {
      // A member of an anonymous object type ({ run(): void }) must not be named like a function of the module.
      parts.unshift(cur.parent && ts.isTypeAliasDeclaration(cur.parent) ? cur.parent.name.text : "{}");
      break;
    }
    if (ts.isModuleDeclaration(cur) && ts.isIdentifier(cur.name)) parts.unshift(cur.name.text);
    if (ts.isModuleDeclaration(cur) && ts.isStringLiteral(cur.name)) break;
    if (ts.isSourceFile(cur) || ts.isBlock(cur)) break;
  }
  return parts.join(".");
}

/** The package or module a declaration outside the tree belongs to: "fs", "express", "lib" for the JavaScript library. */
function externalContainer(sf, decl) {
  for (let cur = decl.parent; cur; cur = cur.parent) {
    if (ts.isModuleDeclaration(cur) && ts.isStringLiteral(cur.name)) return normalizeModule(cur.name.text);
  }
  const file = sf.fileName;
  if (/\/typescript\/lib\/lib\.[^/]*\.d\.ts$/.test(file) || sf.hasNoDefaultLib) return "lib";
  const at = file.lastIndexOf("/node_modules/");
  if (at >= 0) {
    const parts = file.slice(at + "/node_modules/".length).split("/");
    let name = parts[0].startsWith("@") ? `${parts[0]}/${parts[1]}` : parts[0];
    if (name.startsWith("@types/")) {
      name = name.slice("@types/".length);
      if (name.includes("__")) name = `@${name.replace("__", "/")}`;
    }
    return name;
  }
  return file.split("/").at(-1).replace(/\.d\.[cm]?ts$|\.[cm]?[jt]sx?$/, "");
}
