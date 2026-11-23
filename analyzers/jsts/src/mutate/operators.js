// StrykerJS's mutators and statuses in the words of mutation.schema.json.
//
//   EqualityOperator       relational_boundary (< and <=, > and >=) or relational_negation (< and >=,
//                          > and <=, == and !=, === and !==), by the operators before and after
//   ArithmeticOperator     arithmetic
//   LogicalOperator        logical
//   UpdateOperator         increment (++ and --)
//   AssignmentOperator     increment (+= and -=), arithmetic (*= /= %=), logical (&&= ||= ??=)
//   BooleanLiteral         negation_removal for !x into x, else other (true and false)
//   ConditionalExpression  negation_removal for the condition of an if, a loop or a ?: forced to true or
//                          false; return_value for a returned condition into false; else other
//   ArrowFunction          return_value (() => x into () => undefined)
//   CallExpression         call_removal for a call statement removed, else other (a throw removed,
//                          a call in an expression into void 0)
//   anything else          other (OptionalChaining, MethodExpression, UnaryOperator, ...)
//
// Mutators left out of every run (EXCLUDED_MUTATORS), so the runs stay short
// and the mutants are those of the schema's families: StringLiteral, Regex,
// ObjectLiteral, ArrayDeclaration (they change data rather than logic) and
// BlockStatement (it empties whole bodies: StrykerJS has no statement-level
// call removal, and an emptied body is reported by the other mutants of it).

import ts from "typescript";

export const EXCLUDED_MUTATORS = ["StringLiteral", "Regex", "ObjectLiteral", "ArrayDeclaration", "BlockStatement"];

const BOUNDARY = new Map([
  ["<", "<="],
  ["<=", "<"],
  [">", ">="],
  [">=", ">"],
]);
const NEGATION = new Map([
  ["<", ">="],
  [">=", "<"],
  [">", "<="],
  ["<=", ">"],
  ["==", "!="],
  ["!=", "=="],
  ["===", "!=="],
  ["!==", "==="],
]);

/** The operator of `text` when it is one binary or assignment expression ("a >= b" -> ">="), else null. */
export function binaryOperator(text) {
  const sf = ts.createSourceFile("expression.ts", `(${text}\n);`, ts.ScriptTarget.Latest, false, ts.ScriptKind.TS);
  const statement = sf.statements[0];
  if (sf.statements.length !== 1 || !statement || !ts.isExpressionStatement(statement)) return null;
  let e = statement.expression;
  while (ts.isParenthesizedExpression(e)) e = e.expression;
  if (ts.isBinaryExpression(e)) return ts.tokenToString(e.operatorToken.kind) ?? null;
  return null;
}

/** relational_boundary or relational_negation for a comparison operator changed from `from` to `to`, else other. */
export function relationalFamily(from, to) {
  if (BOUNDARY.get(from) === to) return "relational_boundary";
  if (NEGATION.get(from) === to) return "relational_negation";
  return "other";
}

/**
 * The schema's operator family of a mutant made by `mutator`, which replaces `original` with `mutated`; `role` is
 * what the replaced expression is to its parent: "condition" (of an if, a loop, a ?:), "return" (a returned
 * value or an arrow function's body), or null.
 */
export function operatorOf(mutator, original, mutated, role = null) {
  switch (mutator) {
    case "EqualityOperator":
      return relationalFamily(binaryOperator(original), binaryOperator(mutated));
    case "ArithmeticOperator":
      return "arithmetic";
    case "LogicalOperator":
      return "logical";
    case "UpdateOperator":
      return "increment";
    case "AssignmentOperator": {
      const op = binaryOperator(original);
      if (op === "+=" || op === "-=") return "increment";
      if (op === "*=" || op === "/=" || op === "%=") return "arithmetic";
      if (op === "&&=" || op === "||=" || op === "??=") return "logical";
      return "other";
    }
    case "BooleanLiteral": {
      const text = original.trim();
      return text.startsWith("!") && text.slice(1).trim() === mutated.trim() ? "negation_removal" : "other";
    }
    case "ConditionalExpression":
      if (role === "condition") return "negation_removal";
      if (role === "return" && mutated.trim() === "false") return "return_value";
      return "other";
    case "ArrowFunction":
      return "return_value";
    case "CallExpression":
      // StrykerJS's EmptyExpression mutator: a call statement removed (`f(x);` into `;`), a `throw new E()`
      // removed, or a call in an expression into `void 0`.
      return mutated.trim() === ";" && !/^throw\b/.test(original.trim()) ? "call_removal" : "other";
    default:
      return "other";
  }
}

const STATUSES = {
  Killed: "killed",
  Survived: "survived",
  Timeout: "timeout",
  NoCoverage: "no_coverage",
  CompileError: "error",
  RuntimeError: "error",
};

/** The schema's status for a StrykerJS status; null for those left out of the report (Ignored, Pending). */
export function statusOf(status) {
  return STATUSES[status] ?? null;
}
