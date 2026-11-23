"""The mutants of a source file: small changes of its code, made on the text.

Each mutant replaces one exact span of the file (the operator token where
there is one, so `>=` becomes `>` and nothing else moves), found from the
`ast` node positions and, for operators, the `tokenize` token between the
operands (parentheses skipped). The operators (mutation.schema.json, rule 2,
in Python):

- relational_boundary: `<` and `<=`, `>` and `>=`, into each other;
- relational_negation: `==` and `!=`, `<` and `>=`, `>` and `<=`, `in` and
  `not in`, `is` and `is not`, into each other;
- arithmetic: `+` and `-` into each other, `*` into `/`, `/` into `*`, `//`
  and `%` into `*`; not when an operand is a string, bytes, f-string, list
  or tuple literal (concatenation, repetition, formatting);
- logical: `and` and `or` into each other;
- negation_removal: `not x` into `x`, and an `if`/`elif`/`while` condition
  `c` into `not (c)` (unless the condition is a single comparison, whose
  relational_negation is the same mutant, a `not`, or a constant such as
  `while True`);
- return_value: `return x` into the zero value: `0` for a number, `""` for a
  string, `b""` for bytes, `False` for `True` and `True` for `False`, an
  empty literal for a list, tuple, dict or set literal; otherwise by the
  return annotation (`int`/`float`/`complex` 0, `str` "", `bytes` b"",
  `bool` False, `list`/`dict`/`set`/`tuple` and their typing aliases empty);
  otherwise `False` for a comparison, `not` or `and`/`or` of those, and
  `None` for anything else. A value that already is that zero, and `return
  None`, are not mutated;
- call_removal: an expression statement that is only a call, removed (the
  replacement is empty, or `pass` when the statement is the only one of its
  block or shares its line with another statement). Not removed: `print`,
  `pprint`, `breakpoint`, logging (any part of the callee's dotted name
  containing `log`, `warnings.*`, and the methods `debug`, `info`,
  `warning`, `warn`, `error`, `exception`, `critical`, `fatal`, `trace`),
  assertions (a callee name starting with `assert`), and `pytest.*`,
  `unittest.*`, `pdb.*`;
- increment: `+=` and `-=` into each other.

Not mutated: type annotations, the `if __name__ == "__main__":` and `if
TYPE_CHECKING:` tests, and assignments outside functions (module and class
constants), except the lambdas they hold. Every mutant is checked to parse;
one that would not is dropped. Which files and lines are mutated, and which
function a mutant belongs to, is the caller's business (mutation.py): this
module only reports every mutant of a file with its innermost function.
"""

from __future__ import annotations

import ast
import io
import tokenize
import warnings
from bisect import bisect_left, bisect_right
from dataclasses import dataclass
from typing import Any, Iterable, Optional

from difflens_analyzer.model import Function, Model

_COMPARE = {
    ast.Lt: "<", ast.LtE: "<=", ast.Gt: ">", ast.GtE: ">=", ast.Eq: "==", ast.NotEq: "!=",
    ast.In: "in", ast.NotIn: "not in", ast.Is: "is", ast.IsNot: "is not",
}
# Per operator, its replacements in order: (operator family, replacement).
_COMPARE_MUTANTS = {
    "<": [("relational_boundary", "<="), ("relational_negation", ">=")],
    "<=": [("relational_boundary", "<"), ("relational_negation", ">")],
    ">": [("relational_boundary", ">="), ("relational_negation", "<=")],
    ">=": [("relational_boundary", ">"), ("relational_negation", "<")],
    "==": [("relational_negation", "!=")],
    "!=": [("relational_negation", "==")],
    "in": [("relational_negation", "not in")],
    "not in": [("relational_negation", "in")],
    "is": [("relational_negation", "is not")],
    "is not": [("relational_negation", "is")],
}
_ARITHMETIC = {ast.Add: ("+", "-"), ast.Sub: ("-", "+"), ast.Mult: ("*", "/"), ast.Div: ("/", "*"),
               ast.FloorDiv: ("//", "*"), ast.Mod: ("%", "*")}
_AUGMENTED = {ast.Add: ("+=", "-="), ast.Sub: ("-=", "+=")}
_SEQUENCE_LITERALS = (ast.JoinedStr, ast.List, ast.Tuple, ast.ListComp)
_ANNOTATION_FIELDS = {"annotation", "returns", "type_comment", "type_params"}
_ASSIGNMENTS = (ast.Assign, ast.AnnAssign, ast.AugAssign)
_KEPT_CALLS = {"print", "pprint", "breakpoint"}
_KEPT_MODULES = {"warnings", "pytest", "unittest", "pdb"}
_LOG_METHODS = {"debug", "info", "warning", "warn", "error", "exception", "critical", "fatal", "trace"}
_ZERO_BY_ANNOTATION = {
    "int": "0", "float": "0", "complex": "0", "str": '""', "bytes": 'b""', "bool": "False",
    "list": "[]", "List": "[]", "Sequence": "[]", "MutableSequence": "[]",
    "dict": "{}", "Dict": "{}", "Mapping": "{}", "MutableMapping": "{}",
    "set": "set()", "Set": "set()", "frozenset": "frozenset()", "FrozenSet": "frozenset()", "tuple": "()", "Tuple": "()",
}
_TOKEN_KINDS = {tokenize.OP, tokenize.NAME}


@dataclass
class Mutant:
    """One mutant of a file. Positions are 1-based, in code points; the end
    column is exclusive. start and end are the offsets in the file's text."""

    line: int
    column: int
    end_line: int
    end_column: int
    start: int
    end: int
    original: str
    mutated: str
    operator: str
    description: str
    function: Optional[Function]


class SourceText:
    """A file's text with the conversions between ast positions (UTF-8 byte
    columns), code point columns and offsets in the text."""

    def __init__(self, data: bytes) -> None:
        self.data = data
        self.encoding = tokenize.detect_encoding(io.BytesIO(data).readline)[0]
        self.text = data.decode(self.encoding)
        self.lines = self.text.split("\n")
        self.starts: list[int] = []
        offset = 0
        for line in self.lines:
            self.starts.append(offset)
            offset += len(line) + 1

    def offset(self, line: int, byte_column: int) -> int:
        """The text offset of an ast position (1-based line, UTF-8 byte column)."""
        text = self.lines[line - 1] if 0 < line <= len(self.lines) else ""
        if text.isascii():
            column = byte_column
        else:
            column = len(text.encode("utf-8")[:byte_column].decode("utf-8", "ignore"))
        return self.starts[line - 1] + column

    def span(self, node: ast.AST) -> tuple[int, int]:
        return (self.offset(node.lineno, node.col_offset),  # type: ignore[attr-defined]
                self.offset(node.end_lineno, node.end_col_offset))  # type: ignore[attr-defined]

    def position(self, offset: int) -> tuple[int, int]:
        """1-based line and code point column of an offset."""
        index = bisect_right(self.starts, offset) - 1
        return index + 1, offset - self.starts[index] + 1

    def replace(self, start: int, end: int, text: str) -> str:
        return self.text[:start] + text + self.text[end:]

    def encode(self, text: str) -> Optional[bytes]:
        """The text in the file's encoding, None when it cannot hold it."""
        try:
            return text.encode(self.encoding)
        except UnicodeEncodeError:
            return None


def _tokens(source: SourceText) -> list[tuple[int, int, str]]:
    """The operator and name tokens of the file: (start offset, end offset, text).
    A file tokenize rejects has none (its operator mutants are skipped)."""
    out = []
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        try:
            for token in tokenize.tokenize(io.BytesIO(source.data).readline):
                if token.type in _TOKEN_KINDS:
                    (row, column), (end_row, end_column) = token.start, token.end
                    out.append((source.starts[row - 1] + column, source.starts[end_row - 1] + end_column, token.string))
        except (tokenize.TokenError, SyntaxError, IndexError):
            return []
    return out


def mutants_of(data: bytes, tree: ast.Module, model: Model, where: str) -> list[Mutant]:
    """Every mutant of a file (its bytes and syntax), in source order. where
    names the file's module in descriptions of code outside any function."""
    source = SourceText(data)
    walker = _Walker(source, _tokens(source), model, where)
    main = model.by_node.get(id(tree))
    walker.block(tree.body, main, module_level=main is None)
    found = []
    ordered = sorted(enumerate(walker.found), key=lambda pair: (pair[1].start, pair[1].end, pair[0]))
    for _, mutant in ordered:
        mutated = source.replace(mutant.start, mutant.end, mutant.mutated)
        if source.encode(mutated) is None or not _parses(mutated):
            continue
        found.append(mutant)
    return found


def mutated_source(data: bytes, mutant: Mutant) -> bytes:
    """The file's bytes with the mutant applied."""
    source = SourceText(data)
    encoded = source.encode(source.replace(mutant.start, mutant.end, mutant.mutated))
    assert encoded is not None
    return encoded


def _parses(text: str) -> bool:
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            ast.parse(text)
        return True
    except (SyntaxError, ValueError, RecursionError, MemoryError):
        return False


class _Walker:
    def __init__(self, source: SourceText, tokens: list[tuple[int, int, str]], model: Model, where: str) -> None:
        self.source = source
        self.tokens = tokens
        self.token_starts = [t[0] for t in tokens]
        self.model = model
        self.where = where
        self.found: list[Mutant] = []

    # Walking.

    def block(self, stmts: list[ast.stmt], function: Optional[Function], module_level: bool) -> None:
        for stmt in stmts:
            self.statement(stmt, function, module_level, alone=len(stmts) == 1)

    def statement(self, stmt: ast.stmt, function: Optional[Function], module_level: bool, alone: bool) -> None:
        if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
            self.expressions(list(stmt.decorator_list) + _defaults(stmt.args), function)
            inner = self.model.by_node.get(id(stmt)) or function
            self.block(stmt.body, inner, module_level=False)
            return
        if isinstance(stmt, ast.ClassDef):
            self.expressions(list(stmt.decorator_list) + list(stmt.bases) + [k.value for k in stmt.keywords], function)
            # A class body is constants and methods, like a module.
            self.block(stmt.body, function, module_level=function is None or module_level)
            return
        if module_level and isinstance(stmt, _ASSIGNMENTS):
            # A constant: only the functions it holds (`key = lambda ...`).
            for node in ast.walk(stmt):
                if isinstance(node, ast.Lambda):
                    lam = self.model.by_node.get(id(node))
                    if lam is not None:
                        self.expression(node.body, lam)
            return
        main = self.model.by_node.get(id(stmt))
        if main is not None and main.kind == "main":
            self.block(stmt.body, main, module_level=False)  # type: ignore[attr-defined]
            self.block(stmt.orelse, function, module_level)  # type: ignore[attr-defined]
            return
        if isinstance(stmt, (ast.If, ast.While)):
            self.condition(stmt, function)
        elif isinstance(stmt, ast.Return) and stmt.value is not None:
            self.return_value(stmt, function)
        elif isinstance(stmt, ast.Expr) and isinstance(stmt.value, ast.Call):
            self.call_removal(stmt, function, alone)
        elif isinstance(stmt, ast.AugAssign) and type(stmt.op) in _AUGMENTED:
            original, mutated = _AUGMENTED[type(stmt.op)]
            self.operator_token(stmt.target, stmt.value, [original], mutated, "increment", function)
        for name, value in ast.iter_fields(stmt):
            if name in _ANNOTATION_FIELDS:
                continue
            if isinstance(value, list) and value and isinstance(value[0], ast.stmt):
                self.block(value, function, module_level)
            elif isinstance(value, list):
                for item in value:
                    self.child(item, function, module_level)
            elif isinstance(value, ast.AST):
                self.child(value, function, module_level)

    def child(self, node: Any, function: Optional[Function], module_level: bool) -> None:
        if isinstance(node, ast.excepthandler):
            if node.type is not None:  # type: ignore[attr-defined]
                self.expression(node.type, function)  # type: ignore[attr-defined]
            self.block(node.body, function, module_level)  # type: ignore[attr-defined]
        elif isinstance(node, ast.stmt):
            self.statement(node, function, module_level, alone=False)
        elif hasattr(ast, "match_case") and isinstance(node, ast.match_case):
            if node.guard is not None:
                self.expression(node.guard, function)
            self.block(node.body, function, module_level)
        elif isinstance(node, ast.expr):
            self.expression(node, function)
        elif isinstance(node, ast.AST):
            # withitem, comprehension, keyword, arguments and the like.
            for name, value in ast.iter_fields(node):
                if name in _ANNOTATION_FIELDS:
                    continue
                for item in value if isinstance(value, list) else [value]:
                    if isinstance(item, ast.AST):
                        self.child(item, function, module_level)

    def expressions(self, nodes: Iterable[ast.AST], function: Optional[Function]) -> None:
        for node in nodes:
            self.expression(node, function)

    def expression(self, node: ast.AST, function: Optional[Function]) -> None:
        if isinstance(node, ast.Lambda):
            self.expressions(_defaults(node.args), function)
            self.expression(node.body, self.model.by_node.get(id(node)) or function)
            return
        if isinstance(node, ast.Compare):
            self.compare(node, function)
        elif isinstance(node, ast.BinOp) and type(node.op) in _ARITHMETIC:
            self.arithmetic(node, function)
        elif isinstance(node, ast.BoolOp):
            word = "and" if isinstance(node.op, ast.And) else "or"
            other = "or" if word == "and" else "and"
            for left, right in zip(node.values, node.values[1:]):
                self.operator_token(left, right, [word], other, "logical", function)
        elif isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.Not):
            self.not_removal(node, function)
        for name, value in ast.iter_fields(node):
            if name in _ANNOTATION_FIELDS:
                continue
            for item in value if isinstance(value, list) else [value]:
                if isinstance(item, ast.expr):
                    self.expression(item, function)
                elif isinstance(item, ast.AST):
                    self.child(item, function, False)

    # Operators.

    def compare(self, node: ast.Compare, function: Optional[Function]) -> None:
        if isinstance(node.left, ast.Name) and node.left.id == "__name__":
            return
        left: ast.AST = node.left
        for op, right in zip(node.ops, node.comparators):
            text = _COMPARE[type(op)]
            for family, replacement in _COMPARE_MUTANTS[text]:
                self.operator_token(left, right, text.split(), replacement, family, function)
            left = right

    def arithmetic(self, node: ast.BinOp, function: Optional[Function]) -> None:
        original, mutated = _ARITHMETIC[type(node.op)]
        if _sequence_like(node.left) or (not isinstance(node.op, ast.Mod) and _sequence_like(node.right)):
            return
        self.operator_token(node.left, node.right, [original], mutated, "arithmetic", function)

    def operator_token(
        self, left: ast.AST, right: ast.AST, expected: list[str], mutated: str, family: str, function: Optional[Function]
    ) -> None:
        """The operator written between two operands, as the token(s) expected, replaced."""
        after, before = self.source.span(left)[1], self.source.span(right)[0]
        first = bisect_left(self.token_starts, after)
        words = []
        for start, end, text in self.tokens[first:]:
            if start >= before:
                break
            if text not in ("(", ")") and end <= before:
                words.append((start, end, text))
        if [w[2] for w in words] != expected:
            return
        start, end = words[0][0], words[-1][1]
        original = self.source.text[start:end]
        self.add(start, end, mutated, family, f"replace {original} with {mutated}", function)

    def not_removal(self, node: ast.UnaryOp, function: Optional[Function]) -> None:
        start = self.source.span(node)[0]
        text = self.source.text
        if text[start : start + 3] != "not":
            return
        end = start + 3
        while end < len(text) and text[end] in " \t":
            end += 1
        self.add(start, end, "", "negation_removal", "remove not", function)

    def condition(self, stmt: Any, function: Optional[Function]) -> None:
        test = stmt.test
        if isinstance(test, ast.Constant) or (isinstance(test, ast.UnaryOp) and isinstance(test.op, ast.Not)):
            return
        if isinstance(test, ast.Compare) and len(test.ops) == 1:
            return
        if isinstance(test, ast.Name) and test.id == "TYPE_CHECKING":
            return
        if isinstance(test, ast.Attribute) and test.attr == "TYPE_CHECKING":
            return
        start, end = self.source.span(test)
        original = self.source.text[start:end]
        keyword = "while" if isinstance(stmt, ast.While) else "if"
        self.add(start, end, f"not ({original})", "negation_removal", f"negate the {keyword} condition", function)

    def return_value(self, stmt: ast.Return, function: Optional[Function]) -> None:
        value = stmt.value
        assert value is not None
        if isinstance(value, ast.Constant) and value.value is None:
            return
        zero = _zero(value, function)
        start, end = self.source.span(value)
        original = self.source.text[start:end]
        if zero is None or zero == original:
            return
        self.add(start, end, zero, "return_value", f"return {zero} instead", function)

    def call_removal(self, stmt: ast.Expr, function: Optional[Function], alone: bool) -> None:
        call = stmt.value
        assert isinstance(call, ast.Call)
        name = _dotted(call.func)
        parts = name.split(".") if name else []
        if parts and (
            parts[-1] in _KEPT_CALLS
            or parts[-1].startswith("assert")
            or parts[-1] in _LOG_METHODS
            or parts[0] in _KEPT_MODULES
            or any("log" in part.lower() for part in parts)
        ):
            return
        start, end = self.source.span(stmt)
        text = self.source.text
        line_start = text.rfind("\n", 0, start) + 1
        line_end = text.find("\n", end)
        line_end = len(text) if line_end < 0 else line_end
        rest = text[end:line_end].strip()
        own_line = not text[line_start:start].strip() and (not rest or rest.startswith("#"))
        replacement = "" if own_line and not alone else "pass"
        self.add(start, end, replacement, "call_removal", f"remove the call to {name or 'a function'}", function)

    def add(self, start: int, end: int, mutated: str, family: str, what: str, function: Optional[Function]) -> None:
        original = self.source.text[start:end]
        if original == mutated:
            return
        line, column = self.source.position(start)
        end_line, end_column = self.source.position(end)
        where = function.name if function is not None else f"module {self.where}"
        self.found.append(Mutant(
            line=line, column=column, end_line=end_line, end_column=end_column, start=start, end=end,
            original=original, mutated=mutated, operator=family, description=f"{what} in {where}", function=function,
        ))


def _defaults(args: ast.arguments) -> list[ast.AST]:
    return list(args.defaults) + [d for d in args.kw_defaults if d is not None]


def _sequence_like(node: ast.AST) -> bool:
    """A literal whose + * % mean concatenation, repetition or formatting."""
    if isinstance(node, ast.Constant):
        return isinstance(node.value, (str, bytes))
    return isinstance(node, _SEQUENCE_LITERALS)


def _dotted(node: ast.AST) -> str:
    """a.b.c for a callee written as a dotted name, the last name otherwise ("" when none)."""
    parts = []
    while isinstance(node, ast.Attribute):
        parts.append(node.attr)
        node = node.value
    if isinstance(node, ast.Name):
        parts.append(node.id)
    elif isinstance(node, ast.Call):
        inner = _dotted(node.func)
        if inner:
            parts.append(inner + "()")
    return ".".join(reversed(parts))


def _zero(value: ast.AST, function: Optional[Function]) -> Optional[str]:
    """The zero value a return statement's value is mutated into (module
    docstring), None when the value already is it."""
    if isinstance(value, ast.Constant):
        literal = value.value
        if isinstance(literal, bool):
            return "False" if literal else "True"
        if not isinstance(literal, (int, float, complex, str, bytes)) or not literal:
            return None
        if isinstance(literal, str):
            return '""'
        return 'b""' if isinstance(literal, bytes) else "0"
    if isinstance(value, (ast.List, ast.Tuple, ast.Set)) and not value.elts:
        return None
    if isinstance(value, ast.Dict) and not value.keys:
        return None
    if isinstance(value, (ast.List, ast.ListComp)):
        return "[]"
    if isinstance(value, (ast.Dict, ast.DictComp)):
        return "{}"
    if isinstance(value, (ast.Set, ast.SetComp)):
        return "set()"
    if isinstance(value, ast.Tuple):
        return "()"
    if isinstance(value, ast.JoinedStr):
        return '""'
    node = function.node if function is not None else None
    annotation = getattr(node, "returns", None)
    if annotation is not None:
        if isinstance(annotation, ast.Subscript):
            annotation = annotation.value
        name = annotation.attr if isinstance(annotation, ast.Attribute) else getattr(annotation, "id", None)
        if isinstance(name, str) and name in _ZERO_BY_ANNOTATION:
            return _ZERO_BY_ANNOTATION[name]
        if name is not None:
            return "None"
    if _boolean(value):
        return "False"
    return "None"


def _boolean(node: ast.AST) -> bool:
    if isinstance(node, ast.Compare):
        return True
    if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.Not):
        return True
    if isinstance(node, ast.BoolOp):
        return all(_boolean(v) for v in node.values)
    return False
