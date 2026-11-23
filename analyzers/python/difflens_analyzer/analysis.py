"""The analyze command: functions, calls, externals and diagnostics of a tree."""

from __future__ import annotations

import ast
import platform
import time
import warnings
from typing import Any, Optional

from difflens_analyzer import protocol
from difflens_analyzer.calls import Emitter, Walker, references
from difflens_analyzer.entrypoints import Detector
from difflens_analyzer.log import Logger
from difflens_analyzer.model import Model, Scope
from difflens_analyzer.project import Project, discover, is_generated

PYTHON = platform.python_version()


def parse_file(path: str, rel: str) -> tuple[Optional[ast.Module], bytes, Optional[dict[str, Any]]]:
    """The syntax of a file, its bytes, and a diagnostic when it cannot be parsed."""
    try:
        with open(path, "rb") as stream:
            source = stream.read()
    except OSError as error:
        return None, b"", protocol.diagnostic("warning", f"cannot read the file: {error.strerror or error}", rel)
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")  # invalid escape sequences and the like
            return ast.parse(source, filename=rel), source, None
    except SyntaxError as error:
        message = f"{error.msg} (parsed with Python {PYTHON})"
        return None, source, protocol.diagnostic("error", message, rel, error.lineno or None)
    except (ValueError, RecursionError, MemoryError) as error:
        return None, source, protocol.diagnostic("error", f"cannot parse: {error} (Python {PYTHON})", rel)


def build_model(project: Project, deadline: Optional[float] = None) -> Model:
    """Parses every file of the project into a model (diagnostics in model.diagnostics)."""
    model = Model(project)
    for source in project.files:
        if deadline is not None and time.monotonic() > deadline:
            model.diagnostics.append(
                protocol.diagnostic("warning", "the analysis ran out of time: some files were not analysed")
            )
            break
        tree, data, problem = parse_file(source.path, source.rel)
        if problem is not None:
            model.diagnostics.append(problem)
        if tree is not None:
            try:
                model.add_module(source, tree, is_generated(data, source.rel))
            except RecursionError:
                model.diagnostics.append(protocol.diagnostic(
                    "warning", "the file was not analysed: its code is nested too deeply", source.rel
                ))
    try:
        model.finish()
    except RecursionError:
        model.diagnostics.append(protocol.diagnostic("warning", "class hierarchies nest too deeply to be followed"))
    return model


def analyze_tree(root: str, timeout: float, log: Logger) -> dict[str, Any]:
    started = time.monotonic()
    deadline = started + max(timeout - min(timeout / 10, 5.0), 1.0)
    project = discover(root, deadline)
    model = build_model(project, deadline)
    emitter = Emitter()
    detector = Detector(model, emitter)
    stopped = False

    skipped: set[str] = set()

    def guarded(what: str, step: Any) -> None:
        # Code nested deeper than the interpreter recurses (a generated
        # expression thousands of operators long) loses its calls, not the run.
        try:
            step()
        except RecursionError:
            skipped.add(what)

    # guarded calls its step before returning, so the lambdas below never
    # outlive the loop iteration whose variables they read (B023).
    for module in model.modules.values():
        mains = [f for f in module.functions if f.kind == "main"]
        whole = any(f.node is module.tree for f in mains)
        if not whole:
            walker = Walker(model, emitter, detector, None, Scope(module, None, None))
            guarded(module.source.rel, lambda: walker.run(module.tree.body, skip=[f.node for f in mains]))  # noqa: B023
    for f in model.functions:
        if time.monotonic() > deadline:
            stopped = True
            break
        walker = Walker(model, emitter, detector, f, f.scope())
        guarded(f.module.source.rel, lambda: walker.run(f.body))  # noqa: B023
    if not stopped:
        for f in model.functions:
            if time.monotonic() > deadline:
                stopped = True
                break
            guarded(f.module.source.rel, lambda: references(model, emitter, f, detector.registered))  # noqa: B023
    guarded("entry points", detector.finish)
    diagnostics = list(model.diagnostics)
    for rel in sorted(skipped):
        diagnostics.append(protocol.diagnostic(
            "warning", "some calls were not analysed: the code is nested too deeply", rel if rel.endswith(".py") else None
        ))
    if stopped:
        diagnostics.append(
            protocol.diagnostic("warning", f"the analysis stopped after {timeout:g}s: some calls were not analysed")
        )

    functions = []
    for f in model.functions:
        entry: dict[str, Any] = {
            "id": f.id,
            "name": f.name,
            "qualifiedName": f"{f.module.name.rsplit('.', 1)[-1]}.{f.name}",
            "container": f.module.name,
            "file": f.module.source.rel,
            "startLine": max(f.start, 1),
            "endLine": max(f.end, f.start, 1),
            "generated": f.generated,
            "test": f.test,
            "complexity": max(f.complexity, 1),
        }
        if f.entry_points:
            entry["entryPoints"] = f.entry_points
        functions.append(entry)
    log.info(
        "analysis done",
        files=len(project.files),
        functions=len(functions),
        calls=len(emitter.calls),
        externals=len(emitter.externals),
        elapsed_ms=int((time.monotonic() - started) * 1000),
    )
    return protocol.analysis(
        functions=functions,
        calls=emitter.calls,
        externals=sorted(emitter.externals.values(), key=lambda e: e["id"]),
        diagnostics=diagnostics,
    )
