// The tokens the duplicate search compares (dupes.schema.json, step 3): a
// file's tokens as the TypeScript parser reads them, comments and whitespace
// dropped, every identifier one placeholder (ID), every string, template piece,
// regular expression and JSX text another (STR), every number another (NUM);
// keywords, operators and punctuation are kept as they are.
//
// The tokens come from the parsed tree, not from a bare scan, so that a `/`
// that starts a regular expression, the `}` that resumes a template and the
// text between JSX tags are read as the parser reads them: leaf nodes are
// tokens, and the punctuation between the children of a node is scanned.
//
// Import declarations, `export ... from` declarations, `import x = require()`
// and top-level statements that only bind or run `require(...)` are left out
// and split the file: no clone spans them.

import ts from "typescript";

import { scriptKind } from "../analyze/functions.js";

const K = ts.SyntaxKind;

/** The token id of every identifier, private name and JSX tag name. */
export const ID = K.Identifier;
/** The token id of every string, template piece, regular expression and JSX text. */
export const STR = K.StringLiteral;
/** The token id of every number and bigint. */
export const NUM = K.NumericLiteral;

/** The id a token of syntax kind `kind` is compared by: its placeholder, or the kind itself (its text). */
export function tokenId(kind) {
  switch (kind) {
    case K.Identifier:
    case K.PrivateIdentifier:
      return ID;
    case K.StringLiteral:
    case K.NoSubstitutionTemplateLiteral:
    case K.TemplateHead:
    case K.TemplateMiddle:
    case K.TemplateTail:
    case K.RegularExpressionLiteral:
    case K.JsxText:
    case K.JsxTextAllWhiteSpaces:
      return STR;
    case K.NumericLiteral:
    case K.BigIntLiteral:
      return NUM;
    default:
      return kind;
  }
}

/** Whether `node` is an import: an import declaration, `import x = ...`, or `export ... from`. */
function isImport(node) {
  return (
    node.kind === K.ImportDeclaration ||
    node.kind === K.ImportEqualsDeclaration ||
    (node.kind === K.ExportDeclaration && node.moduleSpecifier !== undefined)
  );
}

/** Whether `expr` is `require("x")`, or a property of it (`require("x").y`). */
function isRequireCall(expr) {
  while (expr.kind === K.PropertyAccessExpression || expr.kind === K.ParenthesizedExpression) expr = expr.expression;
  return (
    expr.kind === K.CallExpression &&
    expr.expression.kind === K.Identifier &&
    expr.expression.text === "require" &&
    expr.arguments.length === 1 &&
    ts.isStringLiteralLike(expr.arguments[0])
  );
}

/** Whether a top-level statement only binds or runs `require(...)`: `const x = require("y")`, `require("y")`. */
export function isRequireStatement(statement) {
  if (statement.kind === K.ExpressionStatement) return isRequireCall(statement.expression);
  if (statement.kind !== K.VariableStatement) return false;
  const declarations = statement.declarationList.declarations;
  return declarations.length > 0 && declarations.every((d) => d.initializer !== undefined && isRequireCall(d.initializer));
}

/** The index of the line holding `pos` among `lineStarts` (0-based). */
function lineIndex(lineStarts, pos) {
  let lo = 0;
  let hi = lineStarts.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (lineStarts[mid] <= pos) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/** Whether the character code `c` is JavaScript white space or a line break. */
function isSpace(c) {
  return c === 32 || (c >= 9 && c <= 13) || c === 0xa0 || c === 0xfeff || c === 0x2028 || c === 0x2029 || (c > 127 && /\s/.test(String.fromCharCode(c)));
}

/**
 * The tokens of `text`, the contents of the file `rel` (its extension picks TypeScript, TSX, JavaScript or JSX):
 * `{ ids, startLines, endLines, splits, lineCount }`, the first four typed arrays of one entry per token. `ids`
 * are token ids (`tokenId`), the lines are 1-based (the line a token starts on and the one it ends on), and
 * `splits` numbers the stretches of the file between the imports left out: two tokens of one clone always share
 * their split.
 */
export function tokenize(text, rel) {
  const sf = ts.createSourceFile(rel, text, ts.ScriptTarget.Latest, false, scriptKind(rel));
  const scanner = ts.createScanner(ts.ScriptTarget.Latest, true, sf.languageVariant, text);
  const lineStarts = sf.getLineStarts();

  let capacity = 1024;
  let ids = new Int32Array(capacity);
  let startLines = new Int32Array(capacity);
  let endLines = new Int32Array(capacity);
  let splits = new Int32Array(capacity);
  let count = 0;
  let split = 0;

  const push = (kind, start, end) => {
    if (count === capacity) {
      capacity *= 2;
      const grow = (array) => {
        const bigger = new Int32Array(capacity);
        bigger.set(array);
        return bigger;
      };
      ids = grow(ids);
      startLines = grow(startLines);
      endLines = grow(endLines);
      splits = grow(splits);
    }
    ids[count] = tokenId(kind);
    startLines[count] = lineIndex(lineStarts, start) + 1;
    endLines[count] = lineIndex(lineStarts, end - 1) + 1;
    splits[count] = split;
    count++;
  };

  // The punctuation and keywords between two children of a node (and around them), which the tree does not hold.
  const scanGap = (pos, end) => {
    if (pos >= end) return;
    scanner.resetTokenState(pos);
    for (;;) {
      const kind = scanner.scan();
      if (kind === K.EndOfFileToken) return;
      const tokenEnd = scanner.getTokenEnd();
      if (tokenEnd > end) return;
      push(kind, scanner.getTokenStart(), tokenEnd);
      if (tokenEnd === end) return;
    }
  };

  const leaf = (node) => {
    if (node.kind === K.EndOfFileToken) return;
    if (node.kind === K.JsxText || node.kind === K.JsxTextAllWhiteSpaces) {
      // JSX text holds its own white space; the token is the text between it, if any.
      let start = node.pos;
      let end = node.end;
      while (start < end && isSpace(text.charCodeAt(start))) start++;
      while (end > start && isSpace(text.charCodeAt(end - 1))) end--;
      if (start < end) push(node.kind, start, end);
      return;
    }
    const start = node.getStart(sf);
    // A node the parser made up for a syntax error spans no text.
    if (start < node.end) push(node.kind, start, node.end);
  };

  const visit = (node) => {
    if (node.kind <= K.LastToken) {
      leaf(node);
      return;
    }
    if (isImport(node)) {
      split++;
      return;
    }
    let pos = node.pos;
    const child = (c) => {
      scanGap(pos, c.pos);
      visit(c);
      pos = c.end;
    };
    ts.forEachChild(node, child, (list) => {
      for (const c of list) child(c);
    });
    scanGap(pos, node.end);
  };

  let pos = 0;
  for (const statement of sf.statements) {
    scanGap(pos, statement.pos);
    if (isImport(statement) || isRequireStatement(statement)) split++;
    else visit(statement);
    pos = statement.end;
  }
  scanGap(pos, sf.end);

  return {
    ids: ids.slice(0, count),
    startLines: startLines.slice(0, count),
    endLines: endLines.slice(0, count),
    splits: splits.slice(0, count),
    lineCount: lineStarts.length,
  };
}
