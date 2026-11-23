"""Cyclomatic complexity from Python syntax, by the rules the Go analyzer uses.

It is 1 plus one for each decision point of the function's own code:

- `if` and each `elif` (an `elif` is an `if` in the `else` of another);
- `for`, `async for` and `while` loops;
- each `except` clause of a `try`;
- each `case` of a `match`, except an irrefutable last one (`case _:` or a
  bare capture, like Go's `default:`);
- each `and` / `or` operator (`a and b and c` counts two);
- each conditional expression (`x if c else y`);
- in comprehensions, each `for` (a loop) and each `if`.

`else`, `finally`, `with`, `assert`, `return` and `raise` are not decision
points. Nested functions, lambdas and the methods of nested classes are
functions of their own and are not counted in the enclosing one.
"""

from __future__ import annotations

import ast
from typing import Iterable, Union

FunctionNode = Union[ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda]
_NESTED = (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)


def cyclomatic(body: Iterable[ast.AST]) -> int:
    """Complexity of the statements (or expression) of one function body."""
    count = 1
    stack = list(body)
    while stack:
        node = stack.pop()
        if isinstance(node, _NESTED):
            continue
        count += _decisions(node)
        stack.extend(ast.iter_child_nodes(node))
    return count


def of_function(node: FunctionNode) -> int:
    if isinstance(node, ast.Lambda):
        return cyclomatic([node.body])
    return cyclomatic(node.body)


def _decisions(node: ast.AST) -> int:
    if isinstance(node, (ast.If, ast.For, ast.AsyncFor, ast.While, ast.IfExp, ast.ExceptHandler)):
        return 1
    if isinstance(node, ast.BoolOp):
        return len(node.values) - 1
    if isinstance(node, ast.comprehension):
        return 1 + len(node.ifs)
    match = getattr(ast, "Match", None)
    if match is not None and isinstance(node, match):
        cases = node.cases  # type: ignore[attr-defined]
        counted = len(cases)
        if cases and _irrefutable(cases[-1]):
            counted -= 1
        return counted
    return 0


def _irrefutable(case: ast.AST) -> bool:
    if getattr(case, "guard", None) is not None:
        return False
    pattern = getattr(case, "pattern", None)
    while isinstance(pattern, getattr(ast, "MatchAs", ())) and pattern.pattern is not None:  # type: ignore[union-attr]
        pattern = pattern.pattern  # type: ignore[union-attr]
    return isinstance(pattern, getattr(ast, "MatchAs", ())) and pattern.pattern is None  # type: ignore[union-attr]
