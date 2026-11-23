"""Call sites, references and their kinds.

Kind of a call (README.md, "analyze"), decided on the syntax with the rule
the Go analyzer applies to its control flow graph:

- always: on every path from the start of the function to a normal return.
  Paths that end in an exception are ignored, as Go ignores panics: a call
  after `if not ok: raise ...` is always, and so is everything in a `try`
  body, its `else` and its `finally`. A function that never returns (a
  `while True:` server loop) counts its loop iterations instead, so the
  calls on every iteration are always.
- conditional: in a branch (`if`, `elif`, `else`, `except`, `case`, the
  right side of `and`/`or`, either side of `x if c else y`), in a loop body
  that may run zero times, in a comprehension past its first iterable, or
  after a statement that may return, `break` or `continue` early.
- async: started without waiting for it. The coroutine handed to
  `asyncio.create_task`, `ensure_future`, `run_coroutine_threadsafe` or a
  task group's `create_task`; the function given to `threading.Thread`,
  `Timer`, `multiprocessing.Process`, an executor's `submit`, an event
  loop's `call_soon`/`call_later`/`call_at`/`run_in_executor`, AnyIO's
  `start_soon` or Starlette's and FastAPI's `add_task`; a Celery task sent
  with `.delay()` or `.apply_async()`. `await` is a call: it waits.

Resolution: static when the callee is known from the code (a function, an
imported function, a method of a known class or of `self`/`cls` that no
subclass of the tree overrides, `super()`); dynamic when it is a method a
subclass overrides (each implementation gets its own call, abstract and
Protocol methods replaced by their implementations), or a method called on
an object of unknown type, resolved by name to the methods of the tree that
have it, when at most four classes do and the name is not one of the common
names of the standard library's types (`get`, `append`, `close`...).
"""

from __future__ import annotations

import ast
from typing import Any, Iterable, Optional, Sequence

from difflens_analyzer.model import (
    DEF_NODES,
    BoundV,
    ClassV,
    DuckV,
    ExtV,
    FuncV,
    Function,
    InstV,
    Model,
    Scope,
    Value,
)

KIND_ALWAYS, KIND_CONDITIONAL, KIND_ASYNC = "always", "conditional", "async"
STATIC, DYNAMIC = "static", "dynamic"

# Builtins worth showing as external callees; the rest (len, isinstance,
# str...) are like Go's builtins, which are not reported.
KEPT_BUILTINS = {"print", "open", "input", "exec", "eval", "compile", "__import__", "breakpoint", "exit", "quit"}
SKIPPED_EXTERNAL_ROOTS = {"typing", "typing_extensions"}

CORO_FUNCTIONS = {
    "asyncio.create_task", "asyncio.ensure_future", "asyncio.run_coroutine_threadsafe",
    "asyncio.tasks.create_task", "asyncio.tasks.ensure_future",
}
CORO_METHODS = {"create_task", "ensure_future", "spawn"}
# External callables that run a function argument later: keyword, position.
FUNC_FUNCTIONS = {
    "threading.Thread": ("target", 1),
    "threading.Timer": ("function", 1),
    "multiprocessing.Process": ("target", 1),
    "multiprocessing.context.Process": ("target", 1),
    "_thread.start_new_thread": (None, 0),
}
FUNC_METHODS = {
    "submit": 0, "call_soon": 0, "call_soon_threadsafe": 0, "call_later": 1, "call_at": 1,
    "run_in_executor": 1, "start_soon": 0, "add_task": 0, "apply_async": 0,
}
CELERY_SEND = {"delay", "apply_async"}

MAX_DUCK_TARGETS = 4
# Method names too common on standard types (str, list, dict, files,
# loggers, sockets...) to resolve by name alone.
COMMON_METHODS = frozenset(
    """
    get set add append extend insert remove pop clear copy update items keys values setdefault
    sort reverse index count join split rsplit strip lstrip rstrip replace format startswith endswith
    lower upper title encode decode find rfind read readline readlines write writelines close open
    flush seek tell send recv connect bind listen accept start stop run wait cancel result done
    put put_nowait get_nowait join_thread info debug warning warn error exception critical log
    match search sub fullmatch group groups findall finditer exists mkdir unlink resolve iterdir
    is_file is_dir glob rglob with_suffix fetchone fetchall fetchmany execute executemany commit rollback
    cursor filter exclude first last all order_by save delete create acquire release lock notify
    send_message emit dump dumps load loads parse render to_dict dict json copy deepcopy next
    isoformat strftime timestamp total_seconds hex digest hexdigest
    """.split()
)


class Emitter:
    """The calls and externals found so far."""

    def __init__(self) -> None:
        self.calls: list[dict[str, Any]] = []
        self.externals: dict[str, dict[str, str]] = {}
        self.covered: set[tuple[str, str, int]] = set()

    def call(self, caller: Function, target: "Target", line: int, kind: str, resolution: str) -> None:
        if target.external is not None:
            self.externals[target.id] = target.external
        self.calls.append(
            {"caller": caller.id, "callee": target.id, "line": line, "kind": kind,
             "resolution": resolution, "relation": "call"}
        )
        self.covered.add((caller.id, target.id, line))

    def relate(self, caller: Function, callee: Function, line: int, relation: str, evidence: str = "") -> None:
        """A registration or a reference: always async and static."""
        key = (caller.id, callee.id, line)
        if caller is callee or (relation == "reference" and key in self.covered):
            return
        call: dict[str, Any] = {
            "caller": caller.id, "callee": callee.id, "line": line, "kind": KIND_ASYNC,
            "resolution": STATIC, "relation": relation,
        }
        if evidence:
            call["evidence"] = evidence
        self.calls.append(call)
        self.covered.add(key)


class Target:
    __slots__ = ("id", "external", "func")

    def __init__(self, id: str, external: Optional[dict[str, str]] = None, func: Optional[Function] = None) -> None:
        self.id, self.external, self.func = id, external, func


def external_target(value: ExtV) -> Optional[Target]:
    root = value.path.split(".", 1)[0]
    if root in SKIPPED_EXTERNAL_ROOTS:
        return None
    if root == "builtins":
        name = value.path.split(".", 1)[1] if "." in value.path else ""
        if name not in KEPT_BUILTINS:
            return None
    container, name = value.container()
    return Target(value.path, {"id": value.path, "name": name, "container": container})


class Resolver:
    """Turns the values a callee expression has into call targets."""

    def __init__(self, model: Model) -> None:
        self.model = model

    def targets(self, values: list[Value], caller: Optional[Function]) -> tuple[list[Target], str]:
        found: list[Target] = []
        dynamic = False
        for value in values:
            targets, is_dynamic = self.of(value, caller)
            dynamic = dynamic or is_dynamic
            found.extend(targets)
        unique: dict[str, Target] = {}
        for t in found:
            unique.setdefault(t.id, t)
        result = sorted(unique.values(), key=lambda t: t.id)
        if len(result) > 1:
            dynamic = True
        return result, DYNAMIC if dynamic else STATIC

    def of(self, value: Value, caller: Optional[Function]) -> tuple[list[Target], bool]:
        model = self.model
        if isinstance(value, FuncV):
            return [Target(value.func.id, func=value.func)], False
        if isinstance(value, BoundV):
            return self.method(value.func, value.recv, value.exact, value.attr, caller)
        if isinstance(value, ClassV):
            init = model.member(value.cls, "__init__")
            if isinstance(init, Function):
                return self.method(init, value.cls, value.exact, "__init__", caller)
            if isinstance(init, ExtV):
                return [], False
            return [], False
        if isinstance(value, InstV):
            call = model.member(value.cls, "__call__")
            if isinstance(call, Function):
                return self.method(call, value.cls, value.exact, "__call__", caller)
            return [], False
        if isinstance(value, ExtV):
            t = external_target(value)
            return ([t] if t is not None else []), False
        if isinstance(value, DuckV):
            return self.duck(value.attr, caller), True
        return [], False

    def method(
        self, func: Optional[Function], recv: Any, exact: bool, attr: str, caller: Optional[Function]
    ) -> tuple[list[Target], bool]:
        if exact:
            return ([Target(func.id, func=func)] if func is not None else []), False
        model = self.model
        protocol = model.is_protocol(recv) or (func is not None and func.cls is not None and model.is_protocol(func.cls))
        if protocol:
            # Structural: every class of the tree with a compatible method.
            impls = [
                c.methods[attr] for c in model.classes
                if attr in c.methods and not model.is_protocol(c) and c is not recv
                and (func is None or _compatible(c.methods[attr], func))
            ]
        else:
            impls = [sub.methods[attr] for sub in model.descendants(recv) if attr in sub.methods]
        impls = [f for f in impls if f is not func and self.visible(f, caller)]
        base = [func] if func is not None and not func.abstract and not protocol else []
        chosen = base + impls
        if not chosen and func is not None:
            chosen = [func]
        dynamic = bool(impls)
        return [Target(f.id, func=f) for f in chosen], dynamic

    def duck(self, attr: str, caller: Optional[Function]) -> list[Target]:
        if attr.startswith("__") or attr in COMMON_METHODS:
            return []
        candidates = [f for f in self.model.methods_by_name.get(attr, []) if self.visible(f, caller)]
        if not candidates or len(candidates) > MAX_DUCK_TARGETS:
            return []
        return [Target(f.id, func=f) for f in candidates]

    @staticmethod
    def visible(f: Function, caller: Optional[Function]) -> bool:
        """A production caller does not list targets declared in test code (stubs and fakes)."""
        return not f.test or (caller is not None and caller.test)


class Walker:
    """Walks one function's body (or module-level code, with no caller),
    emitting its calls and handing registrations to the detector."""

    def __init__(
        self, model: Model, emitter: Emitter, detector: Any, caller: Optional[Function], scope: Scope
    ) -> None:
        self.model = model
        self.emitter = emitter
        self.detector = detector
        self.caller = caller
        self.scope = scope
        self.resolver = Resolver(model)

    # Statements.

    def run(self, stmts: Sequence[ast.AST], skip: Iterable[ast.AST] = ()) -> None:
        skipped = {id(s) for s in skip}
        self.block([s for s in stmts if id(s) not in skipped], True)

    def block(self, stmts: Sequence[ast.AST], always: bool) -> tuple[set[str], bool]:
        current = always
        exits: set[str] = set()
        terminated = False
        for stmt in stmts:
            e, t = self.stmt(stmt, current)
            exits |= e
            terminated = terminated or t
            if t or e:
                current = False
        return exits, terminated

    def stmt(self, node: ast.AST, always: bool) -> tuple[set[str], bool]:
        if isinstance(node, ast.Return):
            self.expr(node.value, always)
            return {"return"}, True
        if isinstance(node, ast.Raise):
            self.expr(node.exc, always)
            self.expr(node.cause, always)
            return set(), True
        if isinstance(node, ast.Break):
            return {"break"}, True
        if isinstance(node, ast.Continue):
            return {"continue"}, True
        if isinstance(node, ast.If):
            self.expr(node.test, always)
            e1, t1 = self.block(node.body, False)
            e2, t2 = self.block(node.orelse, False)
            return e1 | e2, t1 and t2 and bool(node.orelse)
        if isinstance(node, (ast.For, ast.AsyncFor)):
            self.expr(node.iter, always)
            e, _ = self.block(node.body, False)
            e2, _ = self.block(node.orelse, False)
            return (e - {"break", "continue"}) | e2, False
        if isinstance(node, ast.While):
            self.expr(node.test, always)
            endless = isinstance(node.test, ast.Constant) and bool(node.test.value)
            e, _ = self.block(node.body, always if endless else False)
            e2, _ = self.block(node.orelse, False)
            return (e - {"break", "continue"}) | e2, endless and "break" not in e
        if isinstance(node, ast.Try) or type(node).__name__ == "TryStar":
            e1, t1 = self.block(node.body, always)  # type: ignore[attr-defined]
            handler_ends = []
            exits = set(e1)
            for handler in node.handlers:  # type: ignore[attr-defined]
                self.expr(handler.type, False)
                e, t = self.block(handler.body, False)
                exits |= e
                handler_ends.append(t)
            e3, t3 = self.block(node.orelse, always and not e1 and not t1)  # type: ignore[attr-defined]
            e4, t4 = self.block(node.finalbody, always)  # type: ignore[attr-defined]
            exits |= e3 | e4
            body_ends = (t1 or t3) and all(handler_ends)
            return exits, body_ends or t4
        if isinstance(node, (ast.With, ast.AsyncWith)):
            for item in node.items:
                self.expr(item.context_expr, always)
            return self.block(node.body, always)
        if type(node).__name__ == "Match":
            self.expr(node.subject, always)  # type: ignore[attr-defined]
            exits = set()
            for case in node.cases:  # type: ignore[attr-defined]
                self.expr(case.guard, False)
                e, _ = self.block(case.body, False)
                exits |= e
            return exits, False
        if isinstance(node, DEF_NODES):
            self.definition(node, always)
            return set(), False
        if isinstance(node, ast.ClassDef):
            for expr in list(node.decorator_list) + list(node.bases) + [k.value for k in node.keywords]:
                self.expr(expr, always)
            saved = self.scope
            cls = self.model.class_by_node.get(id(node))
            if cls is not None:
                self.scope = Scope(saved.module, saved.function, cls)
            try:
                self.block(node.body, always)
            finally:
                self.scope = saved
            self.detector.decorated_class(node, self.scope, self.caller)
            return set(), False
        if isinstance(node, ast.Assert):
            self.expr(node.test, always)
            self.expr(node.msg, False)
            return set(), False
        for child in ast.iter_child_nodes(node):
            self.expr(child, always)
        return set(), False

    def definition(self, node: Any, always: bool) -> None:
        """A def runs its decorators and defaults where it is written."""
        args = node.args
        for expr in list(args.defaults) + [d for d in args.kw_defaults if d is not None]:
            self.expr(expr, always)
        defined = self.model.by_node.get(id(node))
        registered = self.detector.decorated(node, defined, self.scope, self.caller)
        for decorator in node.decorator_list:
            self.expr(decorator, always)
            if isinstance(decorator, ast.Call) or self.caller is None:
                continue
            # A bare decorator is called with the function.
            targets, resolution = self.resolver.targets(self.model.evaluate(decorator, self.scope), self.caller)
            for t in targets:
                self.emitter.call(self.caller, t, decorator.lineno, _kind(always), resolution)
            if defined is not None and not registered and any(t.func is not None for t in targets):
                self.emitter.relate(self.caller, defined, decorator.lineno, "reference")

    # Expressions.

    def expr(self, node: Optional[ast.AST], always: bool, run_later: bool = False) -> None:
        if node is None:
            return
        if isinstance(node, ast.Call):
            self.call(node, always, run_later)
            return
        if isinstance(node, (ast.Lambda, ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            return  # a function of its own
        if isinstance(node, ast.BoolOp):
            self.expr(node.values[0], always)
            for value in node.values[1:]:
                self.expr(value, False)
            return
        if isinstance(node, ast.IfExp):
            self.expr(node.test, always)
            self.expr(node.body, False)
            self.expr(node.orelse, False)
            return
        if isinstance(node, (ast.ListComp, ast.SetComp, ast.GeneratorExp, ast.DictComp)):
            for i, generator in enumerate(node.generators):
                self.expr(generator.iter, always and i == 0)
                for condition in generator.ifs:
                    self.expr(condition, False)
            if isinstance(node, ast.DictComp):
                self.expr(node.key, False)
                self.expr(node.value, False)
            else:
                self.expr(node.elt, False)
            return
        if isinstance(node, ast.Dict):
            self.detector.dictionary(node, self.scope)
        for child in ast.iter_child_nodes(node):
            self.expr(child, always)

    def call(self, node: ast.Call, always: bool, run_later: bool) -> None:
        model, caller = self.model, self.caller
        line = node.lineno
        kind = KIND_ASYNC if run_later else _kind(always)
        func = node.func
        self.detector.call(node, self.scope, caller)
        values = model.evaluate(func, self.scope)

        if isinstance(func, ast.Attribute) and func.attr in CELERY_SEND:
            tasks = [v.func for v in model.evaluate(func.value, self.scope) if isinstance(v, (FuncV, BoundV)) and v.func]
            if tasks:
                if caller is not None:
                    for task in tasks:
                        self.emitter.call(caller, Target(task.id, func=task), line, KIND_ASYNC, STATIC)
                self.arguments(node, always, False)
                return

        if caller is not None:
            targets, resolution = self.resolver.targets(values, caller)
            for t in targets:
                self.emitter.call(caller, t, line, kind, resolution)

        paths = {v.path for v in values if isinstance(v, ExtV)}
        method = func.attr if isinstance(func, ast.Attribute) else None
        in_tree = any(isinstance(v, (FuncV, BoundV, ClassV)) for v in values)
        coroutine_later = bool(paths & CORO_FUNCTIONS) or (method in CORO_METHODS and not in_tree)
        self.expr(func, always)
        self.arguments(node, always, coroutine_later)

        if caller is None:
            return
        for arg in self.function_arguments(node, paths, method, in_tree):
            for value in model.evaluate(arg, self.scope):
                target = value.func if isinstance(value, (FuncV, BoundV)) else None
                if target is None and isinstance(arg, ast.Call):
                    # functools.partial(f, ...)
                    if arg.args and any(isinstance(v, ExtV) and v.path == "functools.partial" for v in model.evaluate(arg.func, self.scope)):
                        inner = model.evaluate(arg.args[0], self.scope)
                        target = next((v.func for v in inner if isinstance(v, (FuncV, BoundV)) and v.func), None)
                if target is not None:
                    self.emitter.call(caller, Target(target.id, func=target), line, KIND_ASYNC, STATIC)

    def arguments(self, node: ast.Call, always: bool, first_later: bool) -> None:
        for i, arg in enumerate(node.args):
            self.expr(arg, always, run_later=first_later and i == 0)
        for keyword in node.keywords:
            self.expr(keyword.value, always)

    @staticmethod
    def function_arguments(node: ast.Call, paths: set[str], method: Optional[str], in_tree: bool) -> list[ast.AST]:
        out: list[ast.AST] = []
        for path in paths:
            spec = FUNC_FUNCTIONS.get(path)
            if spec is None:
                continue
            keyword, position = spec
            for k in node.keywords:
                if keyword is not None and k.arg == keyword:
                    out.append(k.value)
            if position is not None and len(node.args) > position:
                out.append(node.args[position])
        if method in FUNC_METHODS and not in_tree:
            position = FUNC_METHODS[method]
            if len(node.args) > position:
                out.append(node.args[position])
        return out


def _kind(always: bool) -> str:
    return KIND_ALWAYS if always else KIND_CONDITIONAL


def _arity(f: Function) -> tuple[int, float]:
    """How many positional arguments a method takes besides self: least, most."""
    node = f.node
    if not isinstance(node, DEF_NODES):
        return 0, float("inf")
    args = node.args
    positional = len(getattr(args, "posonlyargs", [])) + len(args.args)
    if f.method_kind in ("instance", "class", "property") and positional:
        positional -= 1
    required = positional - len(args.defaults)
    return max(required, 0), float("inf") if args.vararg else positional


def _compatible(impl: Function, protocol: Function) -> bool:
    least, most = _arity(impl)
    wanted, _ = _arity(protocol)
    return least <= wanted <= most


def references(model: Model, emitter: Emitter, f: Function, registered: set[int]) -> None:
    """A reference from f to each function of the tree it uses as a value
    without calling it: passed as an argument, stored, returned, a lambda
    that is not called right away. Values a registration handed over, and a
    function referring to itself, are skipped."""
    scope = f.scope()
    for node, parent in _own_nodes(f):
        if id(node) in registered:
            continue
        if isinstance(parent, ast.Call) and parent.func is node:
            continue
        callee: Optional[Function] = None
        if isinstance(node, ast.Lambda):
            callee = model.by_node.get(id(node))
        elif isinstance(node, (ast.Name, ast.Attribute)) and isinstance(node.ctx, ast.Load):
            if isinstance(parent, ast.Attribute) and parent.value is node:
                continue
            if isinstance(parent, DEF_NODES + (ast.ClassDef,)) and node in getattr(parent, "decorator_list", []):
                continue
            found = [
                v.func for v in model.evaluate(node, scope)
                if isinstance(v, (FuncV, BoundV)) and v.func is not None
            ]
            if len(found) == 1:
                callee = found[0]
        if callee is not None and callee is not f:
            emitter.relate(f, callee, getattr(node, "lineno", f.start), "reference")


def _own_nodes(f: Function) -> Iterable[tuple[ast.AST, Optional[ast.AST]]]:
    """The nodes of f's own code with their parents: nested defs give only
    their decorators and defaults, lambdas only themselves."""
    roots: list[ast.AST] = f.body
    skip: set[int] = set()
    if f.kind == "main":
        roots = [s for s in roots]
    stack: list[tuple[ast.AST, Optional[ast.AST]]] = [(r, None) for r in reversed(roots)]
    while stack:
        node, parent = stack.pop()
        if id(node) in skip:
            continue
        yield node, parent
        if isinstance(node, ast.Lambda):
            continue
        if isinstance(node, DEF_NODES):
            children: list[ast.AST] = list(node.decorator_list) + list(node.args.defaults)
            children += [d for d in node.args.kw_defaults if d is not None]
        else:
            children = list(ast.iter_child_nodes(node))
        for child in reversed(children):
            stack.append((child, node))
