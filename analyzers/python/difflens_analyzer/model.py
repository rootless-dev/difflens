"""The functions and classes of a tree, and what names and expressions denote.

Function ids are built from syntax only, so they do not depend on the order
of unrelated code (see README.md, "analyze"):

    function       <module>.<name>                    shop.billing.charge
    method         <module>.<Class>.<name>            shop.billing.Invoice.total
    nested         <parent id>.<name>                 shop.billing.charge.retry
    lambda         <parent id>$<k>                    shop.billing.charge$1
    module lambda  <module>.<variable>$<k>            shop.billing.key$1
    main block     <module>.__main__                  shop.cli.__main__

A lambda's k is its position among the lambdas written directly in its
parent, so edits elsewhere never renumber it. A name defined twice in the
same scope gets `#2`, `#3`... for the later definitions.

Resolution is a small abstract interpretation over the syntax: a name is
looked up in the function's scopes, the enclosing functions, the module and
the builtins (LEGB); assignments, parameters with annotations, `self` and
`cls`, imports (absolute and relative, followed across modules of the tree)
and return annotations give the values. Nothing of the project is imported.
"""

from __future__ import annotations

import ast
import builtins
import sys
from dataclasses import dataclass, field
from typing import Any, Iterable, Iterator, Optional, Union

from difflens_analyzer import complexity
from difflens_analyzer.project import Project, SourceFile

DefNode = Union[ast.FunctionDef, ast.AsyncFunctionDef]
DEF_NODES = (ast.FunctionDef, ast.AsyncFunctionDef)
NESTED_SCOPES = (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda)
BUILTIN_NAMES = frozenset(dir(builtins))
STDLIB = frozenset(getattr(sys, "stdlib_module_names", ())) | {"os", "sys", "typing", "asyncio", "json"}
TYPING_MODULES = ("typing", "typing_extensions")


# ---------------------------------------------------------------------------
# Index


@dataclass(eq=False)
class Module:
    source: SourceFile
    tree: ast.Module
    generated: bool
    globals: dict[str, list["Binding"]] = field(default_factory=dict)
    functions: list["Function"] = field(default_factory=list)
    imports: set[str] = field(default_factory=set)  # top-level names of imported modules

    @property
    def name(self) -> str:
        return self.source.module


@dataclass(eq=False)
class ClassInfo:
    id: str
    name: str  # display, such as "Outer.Inner"
    module: Module
    node: ast.ClassDef
    parent: Optional["Function"]  # the function the class is defined in
    owner: Optional["ClassInfo"]  # the class this one is nested in
    methods: dict[str, "Function"] = field(default_factory=dict)
    locals: dict[str, list["Binding"]] = field(default_factory=dict)  # class body names
    attributes: dict[str, list["Binding"]] = field(default_factory=dict)  # self.x = ... in methods
    subclasses: list["ClassInfo"] = field(default_factory=list)
    _bases: Optional[list[Any]] = None
    _mro: Optional[list[Any]] = None

    def scope(self) -> "Scope":
        return Scope(self.module, self.parent, self)


@dataclass(eq=False)
class Function:
    id: str
    name: str
    module: Module
    node: ast.AST  # FunctionDef, AsyncFunctionDef, Lambda, If (main block) or Module (__main__.py)
    kind: str  # "def", "lambda" or "main"
    start: int
    end: int
    parent: Optional["Function"] = None  # enclosing function, for scopes
    cls: Optional[ClassInfo] = None  # the class this is a method of
    def_class: Optional[ClassInfo] = None  # the class body this def is written in
    method_kind: str = ""  # "instance", "static", "class", "property" for methods
    abstract: bool = False
    complexity: int = 1
    locals: Optional[dict[str, list["Binding"]]] = None
    global_names: set[str] = field(default_factory=set)
    entry_points: list[dict[str, Any]] = field(default_factory=list)
    lambdas: int = 0

    @property
    def test(self) -> bool:
        return self.module.source.test

    @property
    def generated(self) -> bool:
        return self.module.generated

    @property
    def body(self) -> list[ast.AST]:
        node = self.node
        if isinstance(node, ast.Lambda):
            return [node.body]
        return list(getattr(node, "body", []))

    @property
    def short(self) -> str:
        """The function's own name: "charge" for "Invoice.charge"."""
        return self.name.rsplit(".", 1)[-1]

    def scope(self) -> "Scope":
        if self.kind == "main":
            return Scope(self.module, None, None)
        return Scope(self.module, self, None)

    def add_entry(self, kind: str, label: str, file: str | None = None, line: int | None = None) -> None:
        """file and line say where it is registered, when that is not the function itself."""
        entry: dict[str, Any] = {"kind": kind, "label": label}
        if file is not None:
            entry["file"] = file
            if line is not None and line >= 1:
                entry["line"] = line
        if entry not in self.entry_points:
            self.entry_points.append(entry)


@dataclass(frozen=True)
class Scope:
    """Where an expression is evaluated: its module, function and class body."""

    module: Module
    function: Optional[Function]
    class_body: Optional[ClassInfo]

    @property
    def enclosing_class(self) -> Optional[ClassInfo]:
        """The class whose methods `self` and `super()` refer to here."""
        f = self.function
        while f is not None:
            if f.cls is not None:
                return f.cls
            if f.kind != "lambda":
                return None
            f = f.parent
        return self.class_body


# ---------------------------------------------------------------------------
# Values


class Value:
    __slots__ = ()

    def key(self) -> tuple[Any, ...]:
        raise NotImplementedError


class FuncV(Value):
    __slots__ = ("func",)

    def __init__(self, func: Function) -> None:
        self.func = func

    def key(self) -> tuple[Any, ...]:
        return ("f", id(self.func))


class ClassV(Value):
    """A class; exact is false for `cls`, which may be a subclass."""

    __slots__ = ("cls", "exact")

    def __init__(self, cls: ClassInfo, exact: bool = True) -> None:
        self.cls, self.exact = cls, exact

    def key(self) -> tuple[Any, ...]:
        return ("c", id(self.cls), self.exact)


class InstV(Value):
    """An instance of a class of the tree; exact is false when it may be a subclass."""

    __slots__ = ("cls", "exact")

    def __init__(self, cls: ClassInfo, exact: bool) -> None:
        self.cls, self.exact = cls, exact

    def key(self) -> tuple[Any, ...]:
        return ("i", id(self.cls), self.exact)


class ModV(Value):
    """A module or namespace package of the tree."""

    __slots__ = ("name",)

    def __init__(self, name: str) -> None:
        self.name = name

    def key(self) -> tuple[Any, ...]:
        return ("m", self.name)


class ExtV(Value):
    """Something outside the tree, by dotted path. module is how many leading
    parts are known to be a module; extend says whether lower-case parts
    after them may be submodules too (after `import os`, but not after `from
    os import path`)."""

    __slots__ = ("path", "module", "extend")

    def __init__(self, path: str, module: int, extend: bool) -> None:
        self.path, self.module, self.extend = path, module, extend

    def key(self) -> tuple[Any, ...]:
        return ("e", self.path)

    def attr(self, name: str) -> "ExtV":
        return ExtV(f"{self.path}.{name}", self.module, self.extend)

    def container(self) -> tuple[str, str]:
        parts = self.path.split(".")
        size = max(1, min(self.module, len(parts) - 1)) if len(parts) > 1 else 1
        if self.extend:
            while size < len(parts) - 1 and parts[size][:1].islower():
                size += 1
        return ".".join(parts[:size]), ".".join(parts[size:]) or parts[-1]


class ExtInstV(Value):
    """An instance of an external class."""

    __slots__ = ("ext",)

    def __init__(self, ext: ExtV) -> None:
        self.ext = ext

    def key(self) -> tuple[Any, ...]:
        return ("ei", self.ext.path)


class BoundV(Value):
    """A method looked up on an instance or class: func may be None when the
    class does not define it (a mixin), exact says whether subclasses are
    excluded."""

    __slots__ = ("func", "recv", "exact", "attr")

    def __init__(self, func: Optional[Function], recv: ClassInfo, exact: bool, attr: str) -> None:
        self.func, self.recv, self.exact, self.attr = func, recv, exact, attr

    def key(self) -> tuple[Any, ...]:
        return ("b", id(self.func), id(self.recv), self.exact, self.attr)


class SuperV(Value):
    __slots__ = ("cls",)

    def __init__(self, cls: ClassInfo) -> None:
        self.cls = cls

    def key(self) -> tuple[Any, ...]:
        return ("s", id(self.cls))


class DuckV(Value):
    """A method of an object whose type is not known."""

    __slots__ = ("attr",)

    def __init__(self, attr: str) -> None:
        self.attr = attr

    def key(self) -> tuple[Any, ...]:
        return ("d", self.attr)


class UnknownV(Value):
    """Any object: an unannotated parameter, the result of an unknown call."""

    __slots__ = ()

    def key(self) -> tuple[Any, ...]:
        return ("u",)


class OpaqueV(Value):
    """An object known to come from outside the tree, of an unknown type."""

    __slots__ = ()

    def key(self) -> tuple[Any, ...]:
        return ("o",)


UNKNOWN = UnknownV()
OPAQUE = OpaqueV()


# Bindings: how a name got its value, evaluated on demand.


class Binding:
    __slots__ = ("kind", "node", "scope", "extra", "_values")

    def __init__(self, kind: str, node: Any, scope: Optional[Scope], extra: Any = None) -> None:
        self.kind, self.node, self.scope, self.extra = kind, node, scope, extra
        self._values: Optional[list[Value]] = None


def value_binding(value: Value) -> Binding:
    binding = Binding("value", None, None)
    binding._values = [value]
    return binding


# ---------------------------------------------------------------------------
# The model


class Model:
    def __init__(self, project: Project) -> None:
        self.project = project
        self.modules: dict[str, Module] = {}
        self.functions: list[Function] = []
        self.by_id: dict[str, Function] = {}
        self.by_node: dict[int, Function] = {}
        self.classes: list[ClassInfo] = []
        self.class_by_node: dict[int, ClassInfo] = {}
        self.methods_by_name: dict[str, list[Function]] = {}
        self.diagnostics: list[dict[str, Any]] = []
        self._evaluating: set[int] = set()

    # Building.

    def add_module(self, source: SourceFile, tree: ast.Module, generated: bool) -> Module:
        module = Module(source=source, tree=tree, generated=generated)
        self.modules[source.module] = module
        _Indexer(self, module).run()
        return module

    def finish(self) -> None:
        """Links classes to their subclasses once every module is indexed."""
        for cls in self.classes:
            for base in self.bases(cls):
                if isinstance(base, ClassInfo) and cls not in base.subclasses:
                    base.subclasses.append(cls)
        for f in self.functions:
            if f.cls is not None and f.kind == "def":
                self.methods_by_name.setdefault(f.short, []).append(f)
                scope = self._definition_scope(f)
                f.abstract = any(self.is_ext(d, "abc.abstractmethod", scope=scope) for d in _decorators(f.node))

    def register(self, f: Function) -> Function:
        base, n = f.id, 1
        while f.id in self.by_id:
            n += 1
            f.id = f"{base}#{n}"
            f.name = f"{f.name.split('#')[0]}#{n}"
        self.functions.append(f)
        self.by_id[f.id] = f
        self.by_node[id(f.node)] = f
        f.module.functions.append(f)
        return f

    # Scopes.

    def locals_of(self, f: Function) -> dict[str, list[Binding]]:
        if f.locals is None:
            f.locals = {}
            scope = f.scope()
            node = f.node
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)):
                self._bind_parameters(f, node.args, scope)
            if not isinstance(node, ast.Lambda) and f.kind != "main":
                _collect(node.body, f.locals, scope, f.global_names)
        return f.locals

    def _bind_parameters(self, f: Function, args: ast.arguments, scope: Scope) -> None:
        assert f.locals is not None
        positional = list(getattr(args, "posonlyargs", [])) + list(args.args)
        everything = positional + list(args.kwonlyargs)
        if args.vararg:
            everything.append(args.vararg)
        if args.kwarg:
            everything.append(args.kwarg)
        outer = self._definition_scope(f)
        for arg in everything:
            if arg.annotation is not None and arg is not args.vararg and arg is not args.kwarg:
                f.locals[arg.arg] = [Binding("annotation", arg.annotation, outer)]
            else:
                f.locals[arg.arg] = [value_binding(UNKNOWN)]
        if f.cls is not None and positional and f.method_kind in ("instance", "property", "class"):
            first = positional[0].arg
            if f.method_kind == "class" or f.short == "__new__":
                f.locals[first] = [value_binding(ClassV(f.cls, exact=False))]
            else:
                f.locals[first] = [value_binding(InstV(f.cls, exact=False))]

    def _definition_scope(self, f: Function) -> Scope:
        """Where a def's annotations, defaults and decorators are evaluated."""
        return Scope(f.module, f.parent, f.def_class)

    def lookup(self, name: str, scope: Scope) -> list[Binding]:
        if scope.class_body is not None and name in scope.class_body.locals:
            return scope.class_body.locals[name]
        f = scope.function
        while f is not None:
            if f.kind == "main":
                break
            if name in f.global_names:
                break
            bindings = self.locals_of(f).get(name)
            if bindings is not None:
                return bindings
            f = f.parent
        found = scope.module.globals.get(name)
        if found is not None:
            return found
        starred = self._star_values(scope.module, name, set())
        if starred:
            return [value_binding(v) for v in starred]
        if name in BUILTIN_NAMES:
            return [value_binding(ExtV(f"builtins.{name}", 1, False))]
        return []

    def values(self, binding: Binding) -> list[Value]:
        if binding._values is not None:
            return binding._values
        marker = id(binding)
        if marker in self._evaluating:
            return []
        self._evaluating.add(marker)
        try:
            values = self._compute(binding)
        finally:
            self._evaluating.discard(marker)
        binding._values = values
        return values

    def _compute(self, binding: Binding) -> list[Value]:
        kind, scope = binding.kind, binding.scope
        if kind == "def":
            f = self.by_node.get(id(binding.node))
            if f is not None:
                return [FuncV(f)]
            cls = self.class_by_node.get(id(binding.node))
            return [ClassV(cls)] if cls is not None else []
        if kind == "expr":
            return self.evaluate(binding.node, scope)
        if kind == "annotation":
            return self.annotation(binding.node, scope)
        if kind == "import":
            return self._import_values(binding.extra, scope)
        if kind == "from":
            module, attr = binding.extra
            return self._from_values(module, attr, scope)
        if kind == "unpack":
            # a, b = x, y: the element at index extra of a tuple value.
            node = binding.node
            if isinstance(node, (ast.Tuple, ast.List)) and binding.extra < len(node.elts):
                return self.evaluate(node.elts[binding.extra], scope)
            return [UNKNOWN]
        return [UNKNOWN]

    # Imports.

    def _import_values(self, name: str, scope: Scope) -> list[Value]:
        found = self.tree_module(name, scope.module)
        if found is not None:
            return [ModV(found)]
        return [ExtV(name, name.count(".") + 1, True)]

    def _from_values(self, module: str, attr: str, scope: Scope) -> list[Value]:
        found = self.tree_module(module, scope.module)
        if found is None:
            if attr == "*":
                return []
            return [ExtV(f"{module}.{attr}", module.count(".") + 1, False)]
        return self.module_attr(found, attr)

    def tree_module(self, name: str, importer: Module) -> Optional[str]:
        """The tree module or namespace a dotted import names, or None."""
        project = self.project
        if not name:
            return None
        if project.is_tree_module(name):
            return name
        first = name.split(".", 1)[0]
        if "." not in name or first in STDLIB:
            return None
        # A tree whose roots were not all found: match the end of a module
        # name, preferring the importer's own project.
        suffix = "." + name
        candidates = [m for m in project.by_module if m.endswith(suffix)]
        if len(candidates) > 1:
            folder = project.project_folder(importer.source.rel)
            candidates = [m for m in candidates if project.project_folder(project.by_module[m].rel) == folder]
        return candidates[0] if len(candidates) == 1 else None

    def module_attr(self, name: str, attr: str, seen: Optional[set[str]] = None) -> list[Value]:
        module = self.modules.get(name)
        if module is not None and attr in module.globals:
            found: list[Value] = []
            for binding in module.globals[attr]:
                found.extend(self.values(binding))
            if found:
                return _unique(found)
        sub = f"{name}.{attr}"
        if self.project.is_tree_module(sub):
            return [ModV(sub)]
        if module is not None and not attr.startswith("_"):
            return self._star_values(module, attr, seen if seen is not None else set())
        return []

    def _star_values(self, module: Module, name: str, seen: set[str]) -> list[Value]:
        """name as a `from m import *` of a tree module brings it in."""
        if module.name in seen:
            return []
        seen.add(module.name)
        for key in list(module.globals):
            if not key.startswith("*"):
                continue
            target = self.tree_module(key[1:], module)
            if target is not None:
                found = self.module_attr(target, name, seen)
                if found:
                    return found
        return []

    # Expressions.

    def evaluate(self, expr: Optional[ast.AST], scope: Optional[Scope]) -> list[Value]:
        if expr is None or scope is None:
            return []
        if isinstance(expr, ast.Name):
            found: list[Value] = []
            for binding in self.lookup(expr.id, scope):
                found.extend(self.values(binding))
            return _unique(found)
        if isinstance(expr, ast.Attribute):
            out: list[Value] = []
            for base in self.evaluate(expr.value, scope):
                out.extend(self.attribute(base, expr.attr))
            return _unique(out)
        if isinstance(expr, ast.Call):
            if isinstance(expr.func, ast.Name) and expr.func.id == "super":
                cls = scope.enclosing_class
                return [SuperV(cls)] if cls is not None else []
            out = []
            for callee in self.evaluate(expr.func, scope):
                out.extend(self.result(callee))
            return _unique(out)
        if isinstance(expr, ast.Await):
            return self.evaluate(expr.value, scope)
        if isinstance(expr, ast.IfExp):
            return _unique(self.evaluate(expr.body, scope) + self.evaluate(expr.orelse, scope))
        if isinstance(expr, ast.BoolOp):
            out = []
            for value in expr.values:
                out.extend(self.evaluate(value, scope))
            return _unique(out)
        if isinstance(expr, ast.NamedExpr):
            return self.evaluate(expr.value, scope)
        if isinstance(expr, ast.Lambda):
            f = self.by_node.get(id(expr))
            return [FuncV(f)] if f is not None else []
        if isinstance(expr, ast.Subscript):
            return [UNKNOWN]
        return []

    def attribute(self, base: Value, attr: str) -> list[Value]:
        if isinstance(base, ModV):
            return self.module_attr(base.name, attr)
        if isinstance(base, ExtV):
            return [base.attr(attr)]
        if isinstance(base, ExtInstV):
            return [base.ext.attr(attr)]
        if isinstance(base, ClassV):
            return self._class_attr(base.cls, attr, base.exact)
        if isinstance(base, InstV):
            return self._instance_attr(base.cls, attr, base.exact)
        if isinstance(base, SuperV):
            return self._super_attr(base.cls, attr)
        if isinstance(base, UnknownV):
            return [DuckV(attr)]
        return []

    def _class_attr(self, cls: ClassInfo, attr: str, exact: bool) -> list[Value]:
        member = self.member(cls, attr)
        if isinstance(member, Function):
            if member.method_kind in ("class", "static") or not exact:
                return [BoundV(member, cls, exact, attr)]
            return [FuncV(member)]
        if isinstance(member, ClassInfo):
            return [ClassV(member)]
        if isinstance(member, list):
            return member
        external = self.external_member(cls, attr)
        if external is not None:
            return [external]
        return [] if exact else [BoundV(None, cls, False, attr)]

    def _instance_attr(self, cls: ClassInfo, attr: str, exact: bool) -> list[Value]:
        member = self.member(cls, attr)
        if isinstance(member, Function):
            if member.method_kind == "property":
                return self.result(FuncV(member))
            return [BoundV(member, cls, exact, attr)]
        if isinstance(member, ClassInfo):
            return [ClassV(member)]
        if isinstance(member, list):
            return member
        types = self.instance_attribute(cls, attr)
        if types:
            return types
        external = self.external_member(cls, attr)
        if external is not None:
            return [external]
        return [] if exact else [BoundV(None, cls, False, attr)]

    def _super_attr(self, cls: ClassInfo, attr: str) -> list[Value]:
        mro = self.mro(cls)
        for entry in mro[1:]:
            if isinstance(entry, ClassInfo):
                f = entry.methods.get(attr)
                if f is not None:
                    return [BoundV(f, entry, True, attr)]
            elif isinstance(entry, ExtV):
                return [entry.attr(attr)]
        return []

    def external_member(self, cls: ClassInfo, attr: str) -> Optional[ExtV]:
        """attr of the first external class in cls's MRO, such as unittest.TestCase.assertIn."""
        for entry in self.mro(cls):
            if isinstance(entry, ExtV):
                return entry.attr(attr)
        return None

    def member(self, cls: ClassInfo, attr: str) -> Any:
        """What attr is on cls by the classes of the tree in its MRO: a method,
        a nested class, the values of a class attribute (a list), or None."""
        for entry in self.mro(cls):
            if isinstance(entry, ClassInfo):
                if attr in entry.methods:
                    return entry.methods[attr]
                bindings = entry.locals.get(attr)
                if bindings:
                    values: list[Value] = []
                    for binding in bindings:
                        values.extend(self.values(binding))
                    for value in values:
                        if isinstance(value, ClassV):
                            return value.cls
                    if values:
                        return _unique(values)
        return None

    def instance_attribute(self, cls: ClassInfo, attr: str) -> list[Value]:
        for entry in self.mro(cls):
            if isinstance(entry, ClassInfo) and attr in entry.attributes:
                out: list[Value] = []
                for binding in entry.attributes[attr]:
                    out.extend(self.values(binding))
                if out:
                    return _unique(out)
        return []

    def result(self, callee: Value) -> list[Value]:
        """What calling callee returns."""
        if isinstance(callee, ClassV):
            return [InstV(callee.cls, callee.exact)]
        if isinstance(callee, ExtV):
            last = callee.path.rsplit(".", 1)[-1]
            if callee.path.startswith("builtins."):
                return [OPAQUE]
            return [ExtInstV(callee)] if last[:1].isupper() else [OPAQUE]
        if isinstance(callee, (FuncV, BoundV)):
            f = callee.func
            if f is None or not isinstance(f.node, DEF_NODES) or f.node.returns is None:
                return [UNKNOWN]
            returns = f.node.returns
            if isinstance(callee, BoundV) and _names_self(returns):
                return [InstV(callee.recv, callee.exact)]
            values = self.annotation(returns, self._definition_scope(f))
            return values or [UNKNOWN]
        if isinstance(callee, OpaqueV):
            return [OPAQUE]
        return [UNKNOWN]

    def annotation(self, expr: Optional[ast.AST], scope: Optional[Scope]) -> list[Value]:
        """The instances an annotation says a value is."""
        if expr is None or scope is None:
            return []
        if isinstance(expr, ast.Constant):
            if isinstance(expr.value, str):
                try:
                    parsed = ast.parse(expr.value, mode="eval").body
                except SyntaxError:
                    return []
                return self.annotation(parsed, scope)
            return []
        if isinstance(expr, ast.BinOp) and isinstance(expr.op, ast.BitOr):
            return _unique(self.annotation(expr.left, scope) + self.annotation(expr.right, scope))
        if isinstance(expr, ast.Subscript):
            base = self.evaluate(expr.value, scope)
            inner = expr.slice
            if isinstance(inner, ast.Index):  # pragma: no cover - Python 3.8
                inner = inner.value  # type: ignore[attr-defined]
            elements = list(inner.elts) if isinstance(inner, ast.Tuple) else [inner]
            for value in base:
                if isinstance(value, ExtV):
                    name = value.path.rsplit(".", 1)[-1]
                    if name in ("Optional", "Union"):
                        out: list[Value] = []
                        for element in elements:
                            out.extend(self.annotation(element, scope))
                        return _unique(out)
                    if name in ("Annotated", "Final", "ClassVar", "Required", "NotRequired", "ReadOnly"):
                        return self.annotation(elements[0], scope)
                    if name in ("Type", "type"):
                        return [ClassV(v.cls, exact=False) for v in self.annotation(elements[0], scope) if isinstance(v, InstV)]
                if isinstance(value, ClassV):
                    return [InstV(value.cls, exact=False)]
            return []
        if _names_self(expr):
            cls = scope.enclosing_class
            return [InstV(cls, exact=False)] if cls is not None else []
        out = []
        for value in self.evaluate(expr, scope):
            if isinstance(value, ClassV):
                out.append(InstV(value.cls, exact=False))
            elif isinstance(value, ExtV):
                module = value.path.split(".", 1)[0]
                if module not in TYPING_MODULES and not value.path.startswith("builtins."):
                    out.append(ExtInstV(value))
        return _unique(out)

    # Classes.

    def bases(self, cls: ClassInfo) -> list[Any]:
        """The bases of cls: classes of the tree, or ExtV for external ones."""
        if cls._bases is None:
            cls._bases = []
            outer = Scope(cls.module, cls.parent, cls.owner)
            for base in cls.node.bases:
                for value in self.evaluate(base, outer):
                    if isinstance(value, ClassV) and value.cls is not cls:
                        cls._bases.append(value.cls)
                    elif isinstance(value, ExtV) and value.path not in ("builtins.object",):
                        cls._bases.append(value)
        return cls._bases

    def mro(self, cls: ClassInfo) -> list[Any]:
        """A C3 linearisation, or left-to-right depth-first when C3 fails."""
        if cls._mro is not None:
            return cls._mro
        cls._mro = [cls]  # guards against cycles while computing
        lists = [list(self.mro(b)) if isinstance(b, ClassInfo) else [b] for b in self.bases(cls)]
        lists.append(list(self.bases(cls)))
        merged = _c3(lists)
        if merged is None:
            merged = []
            for entries in lists[:-1]:
                for entry in entries:
                    if all(_key(entry) != _key(e) for e in merged):
                        merged.append(entry)
        cls._mro = [cls] + merged
        return cls._mro

    def external_bases(self, cls: ClassInfo) -> list[ExtV]:
        return [entry for entry in self.mro(cls) if isinstance(entry, ExtV)]

    def is_protocol(self, cls: ClassInfo) -> bool:
        return any(
            isinstance(base, ExtV) and base.path.rsplit(".", 1)[-1] == "Protocol"
            and base.path.split(".", 1)[0] in TYPING_MODULES
            for base in self.bases(cls)
        )

    def descendants(self, cls: ClassInfo) -> list[ClassInfo]:
        seen: list[ClassInfo] = []
        stack = list(cls.subclasses)
        while stack:
            sub = stack.pop()
            if sub in seen:
                continue
            seen.append(sub)
            stack.extend(sub.subclasses)
        return seen

    # Helpers for detectors.

    def is_ext(self, expr: ast.AST, *paths: str, scope: Optional[Scope] = None, module: Optional[Module] = None) -> bool:
        """Whether expr evaluates to one of the external paths given."""
        if scope is None:
            if module is None:
                return False
            scope = Scope(module, None, None)
        return any(isinstance(v, ExtV) and v.path in paths for v in self.evaluate(expr, scope))


# ---------------------------------------------------------------------------
# Indexing one module


class _Indexer:
    def __init__(self, model: Model, module: Module) -> None:
        self.model = model
        self.module = module
        self.prefix = module.name

    def run(self) -> None:
        module = self.module
        scope = Scope(module, None, None)
        self.has_lambdas = any(isinstance(node, ast.Lambda) for node in ast.walk(module.tree))
        _collect(module.tree.body, module.globals, scope, set(), module.imports)
        self.block(module.tree.body, parent=None, owner=None, id_prefix=self.prefix, name_prefix="")
        self.main_blocks()

    def block(
        self,
        stmts: Iterable[ast.AST],
        parent: Optional[Function],
        owner: Optional[ClassInfo],
        id_prefix: str,
        name_prefix: str,
    ) -> None:
        """Indexes the defs, classes and lambdas of statements of one scope."""
        for stmt in stmts:
            if isinstance(stmt, DEF_NODES):
                self.function(stmt, parent, owner, id_prefix, name_prefix)
            elif isinstance(stmt, ast.ClassDef):
                self.klass(stmt, parent, owner, id_prefix, name_prefix)
            else:
                self.lambdas_in_statement(stmt, parent, owner, id_prefix, name_prefix)
                for child in _child_blocks(stmt):
                    self.block(child, parent, owner, id_prefix, name_prefix)

    def function(
        self, node: DefNode, parent: Optional[Function], owner: Optional[ClassInfo], id_prefix: str, name_prefix: str
    ) -> Function:
        start = min([node.lineno] + [d.lineno for d in node.decorator_list])
        f = Function(
            id=f"{id_prefix}.{node.name}",
            name=f"{name_prefix}{node.name}",
            module=self.module,
            node=node,
            kind="def",
            start=start,
            end=getattr(node, "end_lineno", None) or node.lineno,
            parent=parent,
            cls=owner,
            def_class=owner,
            complexity=complexity.of_function(node),
        )
        if owner is not None:
            f.method_kind = self.method_kind(node, owner)
        self.model.register(f)
        if owner is not None:
            owner.methods.setdefault(node.name, f)
        # Lambdas in decorators and defaults belong to the enclosing scope.
        for expr in list(node.decorator_list) + list(node.args.defaults) + [d for d in node.args.kw_defaults if d]:
            self.lambdas(expr, parent, f"{id_prefix}", name_prefix.rstrip("."), owner)
        self.block(node.body, parent=f, owner=None, id_prefix=f.id, name_prefix=f"{f.name}.")
        return f

    def method_kind(self, node: DefNode, owner: ClassInfo) -> str:
        kind = "instance"
        for decorator in node.decorator_list:
            target = decorator.func if isinstance(decorator, ast.Call) else decorator
            name = target.attr if isinstance(target, ast.Attribute) else getattr(target, "id", "")
            if name == "staticmethod":
                return "static"
            if name == "classmethod":
                kind = "class"
            elif name in ("property", "cached_property") or (
                isinstance(target, ast.Attribute) and target.attr in ("setter", "getter", "deleter")
            ):
                kind = "property" if kind == "instance" else kind
        if node.name in ("__init_subclass__", "__class_getitem__"):
            return "class"
        return kind

    def klass(
        self, node: ast.ClassDef, parent: Optional[Function], owner: Optional[ClassInfo], id_prefix: str, name_prefix: str
    ) -> ClassInfo:
        cls = ClassInfo(
            id=f"{id_prefix}.{node.name}",
            name=f"{name_prefix}{node.name}",
            module=self.module,
            node=node,
            parent=parent,
            owner=owner,
        )
        self.model.classes.append(cls)
        self.model.class_by_node[id(node)] = cls
        _collect(node.body, cls.locals, Scope(self.module, parent, cls), set())
        for expr in list(node.decorator_list) + list(node.bases):
            self.lambdas(expr, parent, id_prefix, name_prefix.rstrip("."), owner)
        self.block(node.body, parent=parent, owner=cls, id_prefix=cls.id, name_prefix=f"{cls.name}.")
        for method in cls.methods.values():
            _collect_self_attributes(method, cls)
        return cls

    def lambdas_in_statement(
        self, stmt: ast.AST, parent: Optional[Function], owner: Optional[ClassInfo], id_prefix: str, name_prefix: str
    ) -> None:
        if parent is not None:
            for expr in _own_expressions(stmt):
                self.lambdas(expr, parent, parent.id, parent.name, None)
            return
        # At module or class level only `name = lambda ...` is indexed, under
        # the variable's name, as the Go analyzer does for var closures.
        if isinstance(stmt, (ast.Assign, ast.AnnAssign)) and stmt.value is not None:
            targets = stmt.targets if isinstance(stmt, ast.Assign) else [stmt.target]
            if len(targets) == 1 and isinstance(targets[0], ast.Name):
                holder = _Holder(f"{id_prefix}.{targets[0].id}", f"{name_prefix}{targets[0].id}")
                self.lambdas(stmt.value, None, holder.id, holder.name, owner, holder)

    def lambdas(
        self,
        expr: ast.AST,
        parent: Optional[Function],
        parent_id: str,
        parent_name: str,
        owner: Optional[ClassInfo],
        holder: Optional["_Holder"] = None,
    ) -> None:
        """Indexes the lambdas written directly in expr (not inside other lambdas), then theirs."""
        if (parent is None and holder is None) or not self.has_lambdas:
            return
        counter = holder if holder is not None else parent
        for node in _walk_no_nested(expr):
            if not isinstance(node, ast.Lambda):
                continue
            assert counter is not None
            counter.lambdas += 1
            f = Function(
                id=f"{parent_id}${counter.lambdas}",
                name=f"{parent_name}${counter.lambdas}",
                module=self.module,
                node=node,
                kind="lambda",
                start=node.lineno,
                end=getattr(node, "end_lineno", None) or node.lineno,
                parent=parent,
                def_class=owner if parent is None else None,
                complexity=complexity.of_function(node),
            )
            self.model.register(f)
            self.lambdas(node.body, f, f.id, f.name, None)

    def main_blocks(self) -> None:
        module = self.module
        blocks = [stmt for stmt in module.tree.body if isinstance(stmt, ast.If) and _is_main_test(stmt.test)]
        nodes: list[ast.AST] = list(blocks)
        if not nodes and module.name.rsplit(".", 1)[-1] == "__main__":
            nodes = [module.tree]
        for node in nodes:
            if isinstance(node, ast.Module):
                end = max([getattr(s, "end_lineno", 1) or 1 for s in node.body] + [1])
                start, body = 1, node.body
            else:
                start, end, body = node.lineno, getattr(node, "end_lineno", None) or node.lineno, node.body  # type: ignore[attr-defined]
            f = Function(
                id=f"{module.name}.__main__",
                name="__main__",
                module=module,
                node=node,
                kind="main",
                start=start,
                end=end,
                complexity=complexity.cyclomatic(
                    [s for s in body if not isinstance(s, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef))]
                ),
            )
            self.model.register(f)
            package = module.name[: -len(".__main__")] if module.name.endswith(".__main__") else module.name
            f.add_entry("main", f"python -m {package}")


class _Holder:
    """Counts the lambdas of a module- or class-level variable."""

    def __init__(self, id: str, name: str) -> None:
        self.id, self.name, self.lambdas = id, name, 0


# ---------------------------------------------------------------------------
# Syntax helpers


def _decorators(node: ast.AST) -> list[ast.expr]:
    return list(getattr(node, "decorator_list", []))


def _names_self(expr: ast.AST) -> bool:
    if isinstance(expr, ast.Constant) and isinstance(expr.value, str):
        return expr.value == "Self"
    return (isinstance(expr, ast.Name) and expr.id == "Self") or (
        isinstance(expr, ast.Attribute) and expr.attr == "Self"
    )


def _is_main_test(test: ast.AST) -> bool:
    if not isinstance(test, ast.Compare) or len(test.ops) != 1 or not isinstance(test.ops[0], ast.Eq):
        return False
    sides = [test.left, test.comparators[0]]
    names = [s for s in sides if isinstance(s, ast.Name) and s.id == "__name__"]
    consts = [s for s in sides if isinstance(s, ast.Constant) and s.value == "__main__"]
    return len(names) == 1 and len(consts) == 1


def _child_blocks(stmt: ast.AST) -> list[list[ast.AST]]:
    """The statement lists inside a compound statement (not inside nested scopes)."""
    blocks: list[list[ast.AST]] = []
    for name in ("body", "orelse", "finalbody"):
        value = getattr(stmt, name, None)
        if isinstance(value, list) and value and isinstance(value[0], ast.stmt):
            blocks.append(value)
    for handler in getattr(stmt, "handlers", []) or []:
        blocks.append(handler.body)
    for case in getattr(stmt, "cases", []) or []:
        blocks.append(case.body)
    return blocks


def _own_expressions(stmt: ast.AST) -> list[ast.AST]:
    """The expressions of a statement itself, not of the statements it holds."""
    out: list[ast.AST] = []
    for name, value in ast.iter_fields(stmt):
        if name in ("body", "orelse", "finalbody", "handlers", "cases"):
            if name == "cases":
                for case in value or []:
                    if case.guard is not None:
                        out.append(case.guard)
            continue
        if isinstance(value, ast.AST):
            out.append(value)
        elif isinstance(value, list):
            out.extend(v for v in value if isinstance(v, ast.AST))
    return out


def _walk_no_nested(node: ast.AST) -> Iterator[ast.AST]:
    """node and its descendants, not entering lambdas, defs and classes below node."""
    stack = [node]
    while stack:
        current = stack.pop()
        yield current
        if current is not node and isinstance(current, NESTED_SCOPES):
            continue
        stack.extend(reversed(list(ast.iter_child_nodes(current))))


def _collect(
    stmts: Iterable[ast.AST],
    into: dict[str, list[Binding]],
    scope: Scope,
    global_names: set[str],
    imports: Optional[set[str]] = None,
) -> None:
    """The bindings the statements of one scope make, in nested blocks too."""

    def add(name: str, binding: Binding) -> None:
        into.setdefault(name, []).append(binding)

    def target(node: ast.AST, value: Optional[ast.AST], index: Optional[int] = None) -> None:
        if isinstance(node, ast.Name):
            if value is None:
                add(node.id, value_binding(UNKNOWN))
            elif index is not None:
                add(node.id, Binding("unpack", value, scope, index))
            else:
                add(node.id, Binding("expr", value, scope))
        elif isinstance(node, (ast.Tuple, ast.List)):
            for i, element in enumerate(node.elts):
                if isinstance(element, ast.Starred):
                    target(element.value, None)
                elif isinstance(value, (ast.Tuple, ast.List)):
                    target(element, value, i)
                else:
                    target(element, None)

    for stmt in stmts:
        if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            add(stmt.name, Binding("def", stmt, scope))
            continue
        if isinstance(stmt, ast.Assign):
            for t in stmt.targets:
                target(t, stmt.value)
        elif isinstance(stmt, ast.AnnAssign) and isinstance(stmt.target, ast.Name):
            add(stmt.target.id, Binding("annotation", stmt.annotation, scope))
            if stmt.value is not None:
                add(stmt.target.id, Binding("expr", stmt.value, scope))
        elif isinstance(stmt, (ast.For, ast.AsyncFor)):
            target(stmt.target, None)
        elif isinstance(stmt, (ast.With, ast.AsyncWith)):
            for item in stmt.items:
                if item.optional_vars is not None:
                    target(item.optional_vars, item.context_expr)
        elif isinstance(stmt, ast.Import):
            for alias in stmt.names:
                if alias.asname:
                    add(alias.asname, Binding("import", None, scope, alias.name))
                else:
                    top = alias.name.split(".", 1)[0]
                    add(top, Binding("import", None, scope, top))
                if imports is not None:
                    imports.add(alias.name.split(".", 1)[0])
        elif isinstance(stmt, ast.ImportFrom):
            module = _absolute(stmt, scope.module)
            if imports is not None and stmt.level == 0 and stmt.module:
                imports.add(stmt.module.split(".", 1)[0])
            for alias in stmt.names:
                if alias.name == "*":
                    _star_import(module, scope, add)
                    continue
                add(alias.asname or alias.name, Binding("from", None, scope, (module, alias.name)))
        elif isinstance(stmt, (ast.Global, ast.Nonlocal)):
            if isinstance(stmt, ast.Global):
                global_names.update(stmt.names)
        elif isinstance(stmt, ast.Try) or type(stmt).__name__ == "TryStar":
            for handler in stmt.handlers:  # type: ignore[attr-defined]
                if handler.name:
                    add(handler.name, value_binding(UNKNOWN))
        for block in _child_blocks(stmt):
            _collect(block, into, scope, global_names, imports)


def _star_import(module: str, scope: Scope, add: Any) -> None:
    """`from m import *`: the names are known only once m is indexed, so a
    marker is bound that the lookup expands on first use."""
    add("*" + module, Binding("star", None, scope, module))


def _absolute(stmt: ast.ImportFrom, module: Module) -> str:
    if not stmt.level:
        return stmt.module or ""
    package = module.source.package
    parts = package.split(".") if package else []
    if stmt.level > 1:
        parts = parts[: len(parts) - (stmt.level - 1)] if stmt.level - 1 <= len(parts) else []
    base = ".".join(parts)
    if stmt.module:
        return f"{base}.{stmt.module}" if base else stmt.module
    return base


def _collect_self_attributes(method: Function, cls: ClassInfo) -> None:
    """`self.x = value` and `self.x: T = ...` in a method give the instance attribute x."""
    node = method.node
    if not isinstance(node, DEF_NODES) or method.method_kind != "instance":
        return
    args = list(getattr(node.args, "posonlyargs", [])) + list(node.args.args)
    if not args:
        return
    me = args[0].arg
    scope = Scope(method.module, method, None)
    stack: list[ast.AST] = list(node.body)
    while stack:
        child = stack.pop()
        if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            continue
        for block in _child_blocks(child):
            stack.extend(block)
        if isinstance(child, ast.Assign):
            for t in child.targets:
                if _is_self_attr(t, me):
                    cls.attributes.setdefault(t.attr, []).append(Binding("expr", child.value, scope))  # type: ignore[attr-defined]
        elif isinstance(child, ast.AnnAssign) and _is_self_attr(child.target, me):
            cls.attributes.setdefault(child.target.attr, []).append(Binding("annotation", child.annotation, scope))  # type: ignore[attr-defined]


def _is_self_attr(node: ast.AST, me: str) -> bool:
    return isinstance(node, ast.Attribute) and isinstance(node.value, ast.Name) and node.value.id == me


def _key(entry: Any) -> Any:
    return ("e", entry.path) if isinstance(entry, ExtV) else ("c", id(entry))


def _c3(lists: list[list[Any]]) -> Optional[list[Any]]:
    lists = [list(entries) for entries in lists if entries]
    out: list[Any] = []
    while lists:
        for entries in lists:
            head = entries[0]
            if not any(_key(head) in [_key(e) for e in other[1:]] for other in lists):
                break
        else:
            return None
        out.append(head)
        lists = [[e for e in entries if _key(e) != _key(head)] for entries in lists]
        lists = [entries for entries in lists if entries]
    return out


def _unique(values: list[Value]) -> list[Value]:
    seen: set[tuple[Any, ...]] = set()
    out = []
    for value in values:
        k = value.key()
        if k not in seen:
            seen.add(k)
            out.append(value)
    return out
