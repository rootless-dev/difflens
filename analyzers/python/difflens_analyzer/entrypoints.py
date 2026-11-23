"""Entry points and registrations, recognised from the source only.

A registration hands a function to a framework that will call it later; when
it is written inside a function of the tree it becomes a `registration` call
from that function, with evidence such as "route GET /users". Written at
module level (a decorator, a urlpatterns list), it has no caller and only
marks the entry point, as the Go analyzer does for package-level commands.

Recognised (see README.md for labels):

- main: `if __name__ == "__main__":` blocks and `__main__.py` modules (as a
  function `<module>.__main__`), console and GUI scripts of pyproject.toml
  (`[project.scripts]`, `[project.gui-scripts]`, `[tool.poetry.scripts]`) and
  setup.cfg (`console_scripts`).
- http_route: FastAPI and Starlette (`@app.get`, `@router.post`,
  `api_route`, `websocket`, `add_api_route`, `Route(...)`), Flask and Quart
  (`@app.route`, `@bp.get`, `add_url_rule`), aiohttp, Litestar and Bottle
  style decorators, Django `path`/`re_path`/`url` (function views and
  `View.as_view()`, whose HTTP methods are listed), Django REST framework
  `router.register` view sets. A router's `prefix=`/`url_prefix=` is
  prepended when it is a constant of its constructor.
- cli_command: click and rich-click commands and groups (subcommands
  labelled with their group chain), Typer commands, `typer.run`, `fire.Fire`,
  Flask's `@app.cli.command`, argparse `set_defaults(func=...)` with the
  subparser's name.
- message_handler: Celery tasks (`@app.task`, `@shared_task`), Dramatiq
  actors, RQ jobs, Django signal receivers, `signal.signal` handlers.
- scheduled_job: Celery beat schedules, `add_periodic_task`, APScheduler
  (`@scheduled_job`, `add_job`), `schedule.every(...).do(f)`.
- test: pytest tests (`test*` functions, `test*` methods of `Test*` classes
  without `__init__`), unittest `TestCase` methods, xunit setup and teardown
  functions, fixtures, and pytest hooks in conftest.py, in test files.
- exported_api: what a top-level package exports, by its `__init__.py`
  `__all__` or, without one, its public re-exports.

Framework objects are recognised by what they are an instance of (the
external class, from the imports), or, for an object of unknown type (a
factory's `app` parameter), by the module importing that framework.
"""

from __future__ import annotations

import ast
import configparser
import os
from typing import Any, Iterable, Optional

from difflens_analyzer.calls import Emitter
from difflens_analyzer.model import (
    BoundV,
    ClassInfo,
    ClassV,
    ExtInstV,
    ExtV,
    FuncV,
    Function,
    InstV,
    ModV,
    Model,
    Module,
    Scope,
    UnknownV,
    Value,
)

HTTP_VERBS = ("get", "post", "put", "patch", "delete", "head", "options", "trace")
WEB_ROOTS = {
    "fastapi", "starlette", "flask", "quart", "sanic", "aiohttp", "litestar", "ninja", "apiflask",
    "bottle", "falcon", "blacksheep", "connexion", "robyn", "responder", "flask_restx", "flask_smorest",
}
CLICK_ROOTS = {"click", "rich_click", "asyncclick", "cloup"}
TASK_ROOTS = {"celery", "huey"}
HOOKS = {
    "middleware": "middleware", "exception_handler": "exception handler", "errorhandler": "error handler",
    "before_request": "before request hook", "after_request": "after request hook",
    "teardown_request": "teardown hook", "teardown_appcontext": "teardown hook",
    "before_app_request": "before request hook", "on_event": "event hook",
    "context_processor": "context processor", "before_serving": "startup hook", "after_serving": "shutdown hook",
}
DJANGO_ROUTES = {"django.urls.path": "path", "django.urls.re_path": "re_path", "django.conf.urls.url": "re_path"}
VIEWSET_ACTIONS = {
    "list": ("GET", ""), "create": ("POST", ""), "retrieve": ("GET", "{pk}/"),
    "update": ("PUT", "{pk}/"), "partial_update": ("PATCH", "{pk}/"), "destroy": ("DELETE", "{pk}/"),
}
XUNIT_MODULE = {"setup_module", "teardown_module", "setup_function", "teardown_function", "setUpModule", "tearDownModule"}
XUNIT_CLASS = {
    "setup_method", "teardown_method", "setup_class", "teardown_class", "setup", "teardown",
    "setUp", "tearDown", "setUpClass", "tearDownClass", "asyncSetUp", "asyncTearDown",
}
DYNAMIC = "{dynamic}"


class Detector:
    def __init__(self, model: Model, emitter: Emitter) -> None:
        self.model = model
        self.emitter = emitter
        self.registered: set[int] = set()  # value nodes a registration handed over
        self.beat: list[tuple[str, str, Module, int]] = []  # label, task name, where
        self.tasks: dict[str, Function] = {}  # Celery task names
        self._click: dict[int, Optional[str]] = {}

    # Recording.

    def found(
        self,
        target: Function,
        caller: Optional[Function],
        line: int,
        entry: Optional[tuple[str, str]],
        evidence: str,
        where: Optional[Module] = None,
        value: Optional[ast.AST] = None,
    ) -> None:
        if entry is not None:
            kind, label = entry
            if where is not None:
                target.add_entry(kind, label, file=where.source.rel, line=line)
            else:
                target.add_entry(kind, label)
        if caller is not None and evidence:
            self.emitter.relate(caller, target, line, "registration", evidence)
        if value is not None:
            self.registered.add(id(value))

    # Decorators.

    def decorated(self, node: Any, defined: Optional[Function], scope: Scope, caller: Optional[Function]) -> bool:
        if defined is None:
            return False
        registered = False
        for decorator in node.decorator_list:
            for entry, evidence in self.decorator(decorator, defined, scope):
                self.found(defined, caller, decorator.lineno, entry, evidence)
                registered = registered or bool(evidence)
        return registered

    def decorated_class(self, node: ast.ClassDef, scope: Scope, caller: Optional[Function]) -> None:
        return None

    def decorator(self, decorator: ast.AST, f: Function, scope: Scope) -> list[tuple[Optional[tuple[str, str]], str]]:
        model = self.model
        call = decorator if isinstance(decorator, ast.Call) else None
        target = call.func if call is not None else decorator
        values = model.evaluate(target, scope)
        paths = [v.path for v in values if isinstance(v, ExtV)]
        attr = target.attr if isinstance(target, ast.Attribute) else None
        receiver = model.evaluate(target.value, scope) if isinstance(target, ast.Attribute) else []
        out: list[tuple[Optional[tuple[str, str]], str]] = []

        web = self.framework(receiver, scope.module, WEB_ROOTS) if attr else None
        direct_web = next((p for p in paths if p.split(".", 1)[0] in WEB_ROOTS), None)
        verb_name = attr or (direct_web.rsplit(".", 1)[-1] if direct_web else None)
        if (web or direct_web) and verb_name and call is not None:
            methods = self.route_methods(verb_name, call)
            if methods is not None:
                path = self.route_path(call, 0)
                prefix = self.prefix(target.value, scope) if isinstance(target, ast.Attribute) else ""
                for method in methods:
                    label = f"{method} {prefix}{path}" if method else f"{prefix}{path}"
                    out.append((("http_route", label), f"route {label}"))
                return out
            if attr in HOOKS and web:
                return [(None, HOOKS[attr])]
            if attr == "command" and any(isinstance(v, ExtV) and v.path.endswith(".cli") for v in receiver):
                name = _const_arg(call, 0, "name") or _command_name(f.short, click=True)
                return [(("cli_command", name), f"command {name}")]

        label = self.click_label(decorator, f, scope)
        if label is not None:
            return [(("cli_command", label), f"command {label}")]

        if attr == "task" and self.framework(receiver, scope.module, TASK_ROOTS) or any(
            p in ("celery.shared_task", "celery.task") for p in paths
        ):
            name = _const_arg(call, None, "name") if call is not None else None
            name = name or f"{f.module.name}.{f.short}"
            self.tasks.setdefault(name, f)
            return [(("message_handler", name), f"task {name}")]
        if any(p in ("dramatiq.actor", "rq.decorators.job", "rq.job") for p in paths):
            name = (_const_arg(call, None, "actor_name", "queue_name") if call is not None else None) or f.short
            if call is not None and any(p.startswith("rq") for p in paths):
                name = _const_arg(call, 0, "queue") or name
            return [(("message_handler", name), f"task {name}")]
        if attr in ("periodic_task",) and self.framework(receiver, scope.module, TASK_ROOTS) or any(
            p.endswith("periodic_task") and p.split(".", 1)[0] in TASK_ROOTS for p in paths
        ):
            label = _source(call.args[0]) if call is not None and call.args else f.short
            return [(("scheduled_job", label), f"scheduled job {label}")]
        if attr == "scheduled_job" and self.framework(receiver, scope.module, {"apscheduler"}):
            label = _call_summary(call) if call is not None else "job"
            return [(("scheduled_job", label), f"scheduled job {label}")]
        if any(p == "django.dispatch.receiver" for p in paths) and call is not None and call.args:
            names = _signal_names(call.args[0])
            return [(("message_handler", n), f"signal {n}") for n in names]
        if any(p in ("pytest.fixture", "pytest_asyncio.fixture") for p in paths):
            name = (_const_arg(call, None, "name") if call is not None else None) or f.short
            return [(("test", f"fixture {name}"), "")]
        return out

    def route_methods(self, name: str, call: ast.Call) -> Optional[list[str]]:
        if name in HTTP_VERBS:
            return [name.upper()]
        if name in ("route", "api_route", "view"):
            methods = _const_list(_keyword(call, "methods"))
            return [m.upper() for m in methods] if methods else (["GET"] if name != "api_route" else ["GET"])
        if name in ("websocket", "websocket_route", "ws"):
            return ["WEBSOCKET"]
        return None

    @staticmethod
    def route_path(call: ast.Call, index: int) -> str:
        expr = call.args[index] if len(call.args) > index else _keyword(call, "path") or _keyword(call, "rule")
        if expr is None:
            return "/" if not call.args else DYNAMIC
        return _path_label(expr)

    def prefix(self, receiver: ast.AST, scope: Scope) -> str:
        """The constant prefix= or url_prefix= of the router a name was built with."""
        if not isinstance(receiver, ast.Name):
            return ""
        for binding in self.model.lookup(receiver.id, scope):
            node = binding.node
            if binding.kind == "expr" and isinstance(node, ast.Call):
                for name in ("prefix", "url_prefix"):
                    value = _keyword(node, name)
                    if isinstance(value, ast.Constant) and isinstance(value.value, str):
                        return value.value.rstrip("/") if value.value != "/" else ""
        return ""

    def framework(self, receiver: list[Value], module: Module, roots: set[str]) -> Optional[str]:
        """The framework (by its root package) the receiver object belongs to."""
        unknown = not receiver
        for value in receiver:
            if isinstance(value, (ExtInstV, ExtV)):
                path = value.ext.path if isinstance(value, ExtInstV) else value.path
                root = path.split(".", 1)[0]
                if root in roots:
                    return root
            elif isinstance(value, InstV):
                for base in self.model.external_bases(value.cls):
                    if base.path.split(".", 1)[0] in roots:
                        return base.path.split(".", 1)[0]
            elif isinstance(value, UnknownV):
                unknown = True
        if unknown:
            imported = module.imports & roots
            if imported:
                return sorted(imported)[0]
        return None

    def click_label(self, decorator: ast.AST, f: Function, scope: Scope) -> Optional[str]:
        """The command line of a click or Typer command declared by decorator."""
        model = self.model
        call = decorator if isinstance(decorator, ast.Call) else None
        target = call.func if call is not None else decorator
        values = model.evaluate(target, scope)
        given = _const_arg(call, 0, "name") if call is not None else None
        for value in values:
            if isinstance(value, ExtV):
                root, _, name = value.path.partition(".")
                if root in CLICK_ROOTS and name in ("command", "group", "decorators.command", "decorators.group"):
                    return given or _command_name(f.short, click=True)
        if not isinstance(target, ast.Attribute) or target.attr not in ("command", "group"):
            return None
        for value in model.evaluate(target.value, scope):
            if isinstance(value, FuncV):
                parent = self.function_click_label(value.func)
                if parent is not None:
                    return f"{parent} {given or _command_name(f.short, click=True)}"
            if isinstance(value, ExtInstV) and value.ext.path.split(".", 1)[0] == "typer":
                return given or _command_name(f.short, click=False)
        return None

    def function_click_label(self, f: Function) -> Optional[str]:
        """The label of a function declared as a click group or command."""
        key = id(f)
        if key in self._click:
            return self._click[key]
        self._click[key] = None
        label = None
        if isinstance(f.node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            scope = Scope(f.module, f.parent, f.def_class)
            for decorator in f.node.decorator_list:
                label = self.click_label(decorator, f, scope)
                if label is not None:
                    break
        self._click[key] = label
        return label

    # Calls.

    def call(self, node: ast.Call, scope: Scope, caller: Optional[Function]) -> None:
        model = self.model
        func = node.func
        values = model.evaluate(func, scope)
        paths = {v.path for v in values if isinstance(v, ExtV)}
        attr = func.attr if isinstance(func, ast.Attribute) else None
        receiver = model.evaluate(func.value, scope) if isinstance(func, ast.Attribute) else []
        module = scope.module
        line = node.lineno
        here = module

        def register(expr: Optional[ast.AST], entry: Optional[tuple[str, str]], evidence: str) -> None:
            for target, given in self.functions(expr, scope):
                self.found(target, caller, line, entry, evidence, where=here, value=given)

        web = self.framework(receiver, module, WEB_ROOTS) if attr else None
        if web and attr == "add_url_rule":
            path = self.route_path(node, 0)
            view = _keyword(node, "view_func") or (node.args[2] if len(node.args) > 2 else None)
            methods = [m.upper() for m in _const_list(_keyword(node, "methods"))] or ["GET"]
            self.view(view, path, methods, scope, caller, line)
            return
        if web and attr in ("add_api_route", "add_route", "add_websocket_route"):
            path = self.route_path(node, 0)
            endpoint = node.args[1] if len(node.args) > 1 else _keyword(node, "endpoint") or _keyword(node, "route")
            methods = [m.upper() for m in _const_list(_keyword(node, "methods"))]
            if attr == "add_websocket_route":
                methods = ["WEBSOCKET"]
            self.view(endpoint, path, methods or ["GET"], scope, caller, line)
            return
        if web and attr and attr.startswith("add_") and attr[4:] in HTTP_VERBS:  # aiohttp router.add_get
            self.view(node.args[1] if len(node.args) > 1 else None, self.route_path(node, 0), [attr[4:].upper()], scope, caller, line)
            return
        route_class = next((p for p in paths if p.split(".", 1)[0] in WEB_ROOTS and p.rsplit(".", 1)[-1] in ("Route", "WebSocketRoute")), None)
        if route_class is not None:
            endpoint = node.args[1] if len(node.args) > 1 else _keyword(node, "endpoint")
            methods = [m.upper() for m in _const_list(_keyword(node, "methods"))]
            if route_class.endswith("WebSocketRoute"):
                methods = ["WEBSOCKET"]
            self.view(endpoint, self.route_path(node, 0), methods or ["GET"], scope, caller, line)
            return
        aiohttp_verb = next((p.rsplit(".", 1)[-1] for p in paths if p.startswith("aiohttp.web.") and p.rsplit(".", 1)[-1] in HTTP_VERBS), None)
        if aiohttp_verb is not None:
            self.view(node.args[1] if len(node.args) > 1 else None, self.route_path(node, 0), [aiohttp_verb.upper()], scope, caller, line)
            return
        django = next((DJANGO_ROUTES[p] for p in paths if p in DJANGO_ROUTES), None)
        if django is not None and len(node.args) > 1:
            route = _path_label(node.args[0])
            if django == "path" and not route.startswith("/"):
                route = "/" + route
            self.view(node.args[1], route, [], scope, caller, line, django=True)
            return
        if attr == "register" and self.framework(receiver, module, {"rest_framework"}) and len(node.args) > 1:
            prefix = "/" + _path_label(node.args[0]).strip("/") + "/"
            for value in model.evaluate(node.args[1], scope):
                if isinstance(value, ClassV):
                    for action, (verb, suffix) in VIEWSET_ACTIONS.items():
                        member = model.member(value.cls, action)
                        if isinstance(member, Function):
                            label = f"{verb} {prefix}{suffix}"
                            self.found(member, caller, line, ("http_route", label), f"route {label}", where=here)
            return
        if "signal.signal" in paths and len(node.args) > 1:
            name = _signal_names(node.args[0])[0] if _signal_names(node.args[0]) else DYNAMIC
            register(node.args[1], ("message_handler", name), f"signal {name}")
            return
        if "atexit.register" in paths and node.args:
            register(node.args[0], None, "atexit")
            return
        if attr == "connect" and node.args and any(
            isinstance(v, (ExtV, ExtInstV)) and _path(v).startswith("django.") for v in receiver
        ):
            name = func.value.attr if isinstance(func.value, ast.Attribute) else getattr(func.value, "id", "signal")
            register(node.args[0], ("message_handler", name), f"signal {name}")
            return
        if attr == "add_job" and node.args and self.framework(receiver, module, {"apscheduler"}):
            label = _call_summary(node, skip=1)
            register(node.args[0], ("scheduled_job", label), f"scheduled job {label}")
            return
        if attr == "do" and node.args and _is_schedule_chain(func.value, scope, model):
            label = _schedule_label(func.value)
            register(node.args[0], ("scheduled_job", label), f"scheduled job {label}")
            return
        if attr == "add_periodic_task" and len(node.args) > 1 and self.framework(receiver, module, TASK_ROOTS):
            label = f"every {_source(node.args[0])}"
            task = node.args[1]
            if isinstance(task, ast.Call) and isinstance(task.func, ast.Attribute) and task.func.attr in ("s", "si", "signature"):
                task = task.func.value
            register(task, ("scheduled_job", label), f"scheduled job {label}")
            return
        if paths & {"typer.run", "fire.Fire"} and node.args:
            for target, given in self.functions(node.args[0], scope):
                name = _command_name(target.short, click=False)
                self.found(target, caller, line, ("cli_command", name), f"command {name}", where=here, value=given)
            return
        if attr == "set_defaults":
            label = self.subparser_name(func.value, scope)
            for keyword in node.keywords:
                for target, given in self.functions(keyword.value, scope):
                    name = label or target.short
                    self.found(target, caller, line, ("cli_command", name), f"command {name}", where=here, value=given)

    def view(
        self,
        expr: Optional[ast.AST],
        path: str,
        methods: list[str],
        scope: Scope,
        caller: Optional[Function],
        line: int,
        django: bool = False,
    ) -> None:
        """Registers a view: a function, or a class view's HTTP methods."""
        if expr is None:
            return
        model = self.model
        here = scope.module
        if isinstance(expr, ast.Call) and isinstance(expr.func, ast.Attribute) and expr.func.attr == "as_view":
            mapping = expr.args[0] if expr.args and isinstance(expr.args[0], ast.Dict) else None
            for value in model.evaluate(expr.func.value, scope):
                if not isinstance(value, ClassV):
                    continue
                pairs: list[tuple[str, str]] = []
                if mapping is not None:
                    for k, v in zip(mapping.keys, mapping.values):
                        if isinstance(k, ast.Constant) and isinstance(v, ast.Constant):
                            pairs.append((str(k.value).upper(), str(v.value)))
                else:
                    pairs = [(verb.upper(), verb) for verb in HTTP_VERBS]
                    pairs.append(("", "dispatch_request"))
                for verb, method in pairs:
                    member = model.member(value.cls, method)
                    if isinstance(member, Function):
                        label = f"{verb} {path}" if verb else path
                        self.found(member, caller, line, ("http_route", label), f"route {label}", where=here)
            return
        for target, given in self.functions(expr, scope):
            for method in methods or [""]:
                label = f"{method} {path}" if method else path
                self.found(target, caller, line, ("http_route", label), f"route {label}", where=here, value=given)

    def subparser_name(self, receiver: ast.AST, scope: Scope) -> Optional[str]:
        if not isinstance(receiver, ast.Name):
            return None
        for binding in self.model.lookup(receiver.id, scope):
            node = binding.node
            if binding.kind == "expr" and isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute):
                if node.func.attr == "add_parser":
                    return _const_arg(node, 0, "name")
        return None

    def functions(self, expr: Optional[ast.AST], scope: Scope) -> list[tuple[Function, ast.AST]]:
        """The functions of the tree a value expression denotes."""
        if expr is None:
            return []
        model = self.model
        out: list[tuple[Function, ast.AST]] = []
        if isinstance(expr, ast.Call) and expr.args:
            if any(isinstance(v, ExtV) and v.path == "functools.partial" for v in model.evaluate(expr.func, scope)):
                return self.functions(expr.args[0], scope)
        for value in model.evaluate(expr, scope):
            if isinstance(value, (FuncV, BoundV)) and value.func is not None:
                out.append((value.func, expr))
        return out

    # Dictionaries: Celery beat schedules.

    def dictionary(self, node: ast.Dict, scope: Scope) -> None:
        for key, value in zip(node.keys, node.values):
            if not isinstance(key, ast.Constant) or not isinstance(value, ast.Dict):
                continue
            fields = {k.value: v for k, v in zip(value.keys, value.values) if isinstance(k, ast.Constant)}
            task = fields.get("task")
            if "schedule" in fields and isinstance(task, ast.Constant) and isinstance(task.value, str):
                self.beat.append((str(key.value), task.value, scope.module, key.lineno))

    # After the walk.

    def finish(self) -> None:
        for label, task, module, line in self.beat:
            f = self.tasks.get(task)
            if f is not None:
                f.add_entry("scheduled_job", label, file=module.source.rel, line=line)
        for module in self.model.modules.values():
            if module.source.test:
                self.tests(module)
            elif module.source.is_package and "." not in module.name:
                self.exported(module)
        self.scripts()

    def tests(self, module: Module) -> None:
        conftest = module.source.rel.rsplit("/", 1)[-1] == "conftest.py"
        for f in module.functions:
            if f.kind != "def":
                continue
            if f.parent is None and f.cls is None:
                name = f.short
                if name.startswith("test") or name in XUNIT_MODULE or (conftest and name.startswith("pytest_")):
                    f.add_entry("test", name)
            elif f.cls is not None and f.parent is None and self.is_test_class(f.cls):
                name = f.short
                if name.startswith("test") or name in XUNIT_CLASS:
                    f.add_entry("test", "::".join(_class_chain(f.cls) + [name]))

    def is_test_class(self, cls: ClassInfo) -> bool:
        if any(base.path.rsplit(".", 1)[-1] in ("TestCase", "IsolatedAsyncioTestCase", "TransactionTestCase", "SimpleTestCase", "LiveServerTestCase", "APITestCase") for base in self.model.external_bases(cls)):
            return True
        chain: Optional[ClassInfo] = cls
        while chain is not None:
            if not chain.name.rsplit(".", 1)[-1].startswith("Test") or "__init__" in chain.methods:
                return False
            chain = chain.owner
        return True

    def exported(self, module: Module) -> None:
        names = _dunder_all(module.tree)
        if names is None:
            names = [
                alias.asname or alias.name
                for stmt in module.tree.body if isinstance(stmt, ast.ImportFrom)
                and (stmt.level > 0 or (stmt.module or "").startswith(module.name + "."))
                for alias in stmt.names if alias.name != "*"
            ]
        for name in names:
            if name.startswith("_"):
                continue
            for value in self.model.module_attr(module.name, name):
                if isinstance(value, FuncV):
                    value.func.add_entry("exported_api", f"{module.name}.{name}")
                elif isinstance(value, ClassV):
                    for method_name, method in value.cls.methods.items():
                        if not method_name.startswith("_") or method_name in ("__init__", "__call__"):
                            method.add_entry("exported_api", f"{module.name}.{name}.{method_name}")

    def scripts(self) -> None:
        project = self.model.project
        for folder in project.projects:
            data = project.pyproject(folder)
            entries: dict[str, Any] = {}
            for table in (
                data.get("project", {}).get("scripts"),
                data.get("project", {}).get("gui-scripts"),
                data.get("tool", {}).get("poetry", {}).get("scripts"),
            ):
                if isinstance(table, dict):
                    entries.update(table)
            pyproject = f"{folder}/pyproject.toml" if folder else "pyproject.toml"
            for name, spec in entries.items():
                if isinstance(spec, dict):
                    spec = spec.get("reference") or spec.get("callable")
                if isinstance(spec, str):
                    self.script(name, spec, pyproject)
            setup_cfg = os.path.join(project.root, folder, "setup.cfg")
            if os.path.isfile(setup_cfg):
                parser = configparser.ConfigParser()
                try:
                    parser.read(setup_cfg, encoding="utf-8")
                    text = parser.get("options.entry_points", "console_scripts", fallback="")
                except (configparser.Error, UnicodeDecodeError):
                    text = ""
                for line in text.splitlines():
                    name, _, spec = line.partition("=")
                    if spec.strip():
                        self.script(name.strip(), spec.strip(), f"{folder}/setup.cfg" if folder else "setup.cfg")

    def script(self, name: str, spec: str, file: str) -> None:
        module_name, _, attrs = spec.split("[", 1)[0].strip().partition(":")
        model = self.model
        anchor = next(iter(model.modules.values()), None)
        if anchor is None:
            return
        found = model.tree_module(module_name.strip(), anchor)
        if found is None:
            return
        values: list[Value] = [ModV(found)] if attrs else []
        for part in [p for p in attrs.strip().split(".") if p]:
            nxt: list[Value] = []
            for value in values:
                nxt.extend(model.attribute(value, part))
            values = nxt
        line = _line_of(os.path.join(model.project.root, file), name)
        for value in values:
            if isinstance(value, (FuncV, BoundV)) and value.func is not None:
                value.func.add_entry("main", name, file=file, line=line)


# ---------------------------------------------------------------------------
# Helpers


def _path(value: Value) -> str:
    if isinstance(value, ExtInstV):
        return value.ext.path
    if isinstance(value, ExtV):
        return value.path
    return ""


def _keyword(call: Optional[ast.Call], name: str) -> Optional[ast.AST]:
    if call is None:
        return None
    for keyword in call.keywords:
        if keyword.arg == name:
            return keyword.value
    return None


def _const_arg(call: Optional[ast.Call], index: Optional[int], *names: str) -> Optional[str]:
    if call is None:
        return None
    if index is not None and len(call.args) > index:
        arg = call.args[index]
        if isinstance(arg, ast.Constant) and isinstance(arg.value, str):
            return arg.value
    for name in names:
        value = _keyword(call, name)
        if isinstance(value, ast.Constant) and isinstance(value.value, str):
            return value.value
    return None


def _const_list(expr: Optional[ast.AST]) -> list[str]:
    if isinstance(expr, (ast.List, ast.Tuple, ast.Set)):
        return [e.value for e in expr.elts if isinstance(e, ast.Constant) and isinstance(e.value, str)]
    if isinstance(expr, ast.Constant) and isinstance(expr.value, str):
        return [expr.value]
    return []


def _path_label(expr: ast.AST) -> str:
    """A route path: a constant, a concatenation or f-string with {dynamic} for what is not."""
    if isinstance(expr, ast.Constant) and isinstance(expr.value, str):
        return expr.value
    if isinstance(expr, ast.BinOp) and isinstance(expr.op, ast.Add):
        return _path_label(expr.left) + _path_label(expr.right)
    if isinstance(expr, ast.JoinedStr):
        return "".join(
            v.value if isinstance(v, ast.Constant) and isinstance(v.value, str) else DYNAMIC for v in expr.values
        )
    return DYNAMIC


def _command_name(name: str, click: bool) -> str:
    """The command a click (8.2 rules) or Typer function name gives."""
    if click:
        for suffix in ("_command", "_cmd", "_group", "_grp"):
            if name.endswith(suffix) and len(name) > len(suffix):
                name = name[: -len(suffix)]
                break
    return name.replace("_", "-").lower() if click else name.replace("_", "-")


def _signal_names(expr: ast.AST) -> list[str]:
    if isinstance(expr, (ast.List, ast.Tuple)):
        out: list[str] = []
        for element in expr.elts:
            out.extend(_signal_names(element))
        return out
    if isinstance(expr, ast.Attribute):
        return [expr.attr]
    if isinstance(expr, ast.Name):
        return [expr.id]
    return []


def _source(expr: ast.AST) -> str:
    try:
        return ast.unparse(expr)
    except Exception:  # noqa: BLE001 - only a label
        return DYNAMIC


def _call_summary(call: ast.Call, skip: int = 0) -> str:
    """'cron hour=3' for scheduled_job("cron", hour=3)."""
    parts = []
    for arg in call.args[skip:]:
        parts.append(arg.value if isinstance(arg, ast.Constant) and isinstance(arg.value, str) else _source(arg))
    for keyword in call.keywords:
        if keyword.arg in ("id", "name", "replace_existing", "args", "kwargs", "misfire_grace_time", "max_instances"):
            continue
        parts.append(f"{keyword.arg}={_source(keyword.value)}" if keyword.arg else _source(keyword.value))
    return " ".join(str(p) for p in parts) or "job"


def _is_schedule_chain(expr: ast.AST, scope: Scope, model: Model) -> bool:
    node = expr
    while True:
        if isinstance(node, ast.Call):
            if any(isinstance(v, ExtV) and v.path == "schedule.every" for v in model.evaluate(node.func, scope)):
                return True
            node = node.func
        elif isinstance(node, ast.Attribute):
            node = node.value
        else:
            return False


def _schedule_label(expr: ast.AST) -> str:
    """'every 10 minutes', 'every day at 10:30' for schedule's fluent API."""
    parts: list[str] = []
    node = expr
    while True:
        if isinstance(node, ast.Call):
            args = " ".join(_source(a).strip("'\"") for a in node.args)
            if isinstance(node.func, ast.Attribute):
                parts.append(f"{node.func.attr} {args}".strip())
                node = node.func.value
            else:
                parts.append(f"every {args}".strip())
                break
        elif isinstance(node, ast.Attribute):
            if node.attr == "every" and isinstance(node.value, ast.Name):
                break
            parts.append(node.attr)
            node = node.value
        else:
            break
    return " ".join(w for w in reversed(parts) if w)


def _class_chain(cls: ClassInfo) -> list[str]:
    chain: list[str] = []
    current: Optional[ClassInfo] = cls
    while current is not None:
        chain.append(current.name.rsplit(".", 1)[-1])
        current = current.owner
    return list(reversed(chain))


def _dunder_all(tree: ast.Module) -> Optional[list[str]]:
    for stmt in tree.body:
        targets: Iterable[ast.AST] = []
        if isinstance(stmt, ast.Assign):
            targets = stmt.targets
        elif isinstance(stmt, ast.AnnAssign):
            targets = [stmt.target]
        if any(isinstance(t, ast.Name) and t.id == "__all__" for t in targets):
            return _const_list(stmt.value)  # type: ignore[attr-defined]
    return None


def _line_of(path: str, needle: str) -> Optional[int]:
    try:
        with open(path, encoding="utf-8") as stream:
            for number, line in enumerate(stream, 1):
                stripped = line.strip().strip('"').strip("'")
                if stripped.startswith(needle) and "=" in line:
                    return number
    except (OSError, UnicodeDecodeError):
        return None
    return None
