"""The test command: the project's tests run once with its own interpreter.

pytest runs them (unittest suites included), with a plugin shipped in
`inproject/` that streams what it collects and how each test ends; without
pytest, unittest suites run through `inproject/_difflens_unittest.py`.
Coverage is measured with coverage.py (`coverage run -m pytest`, then
`coverage json`) when the project's environment has it.

How results are named (README.md, "test"):

- a package is a test module, by the container analyze gives its file
  (`tests.test_orders`); a full run also lists every other module of the
  tree as no_tests;
- a test is a pytest node: its name is the node id without the file
  (`TestOrders::test_total[2-3]`), its function the id analyze gives the
  test function, found by where the function is defined;
- a module that fails to import or collect is build_failed, with pytest's
  report as output;
- a failed test's output is what it printed, then pytest's short traceback,
  so the last lines are the failing assertion.

A selection names packages by container and tests by name; a test with
subtests names `name/subtest`, which is put back together into the node id,
so parametrised cases can be selected one by one. Every test is reported
with one level (`hierarchy` is `[name]`, even when a parametrised id holds a
"/"): neither pytest nor unittest has subtests the core could select.

Each test and package carries the command that runs it again from the root
of the tree: `python -m pytest '<file>::<name>'` and `python -m pytest
'<file>'`, or, for projects run with the unittest fallback, `python -m
unittest <module>.<Class>.<test>` and `python -m unittest <module>`, from the
folder the module name starts at (`cd <folder> && ` when that is not the root).

With --cache-dir, bytecode goes to `<cache-dir>/pycache` instead of the run's
scratch folder, so the standard library and the environment's packages are
compiled once per project rather than on every run (PYTHONPYCACHEPREFIX makes
Python ignore the bytecode next to them). Their paths do not change between
runs; the tree's do, so the bytecode of the tree is removed after the run.
"""

from __future__ import annotations

import ast
import json
import os
import re
import shutil
import tempfile
import time
from dataclasses import dataclass, field
from typing import Any, Optional

from difflens_analyzer import environment, processes, protocol
from difflens_analyzer.analysis import build_model
from difflens_analyzer.log import Logger
from difflens_analyzer.model import Function, Model
from difflens_analyzer.project import Project, discover

INPROJECT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "inproject")
MAX_OUTPUT = 64 << 10
PROGRESS_INTERVAL = 0.25
_TEST_FILE = re.compile(r"^(test_.*|.*_test)\.py$")
_CONFTEST_ERROR = re.compile(r"ImportError while loading conftest '([^']+)'")
_SHELL_SAFE = re.compile(r"^[A-Za-z0-9_@%+=:,./-]+$")
PYTEST, UNITTEST = "pytest", "unittest"


class RunError(Exception):
    """The runner could not run at all (exit 1)."""


def keep_end(text: str, limit: int = MAX_OUTPUT) -> str:
    data = text.encode("utf-8")
    if len(data) <= limit:
        return text
    return data[-limit:].decode("utf-8", "ignore")


@dataclass
class ModuleRun:
    rel: str
    container: str
    items: set[str] = field(default_factory=set)
    finished: set[str] = field(default_factory=set)
    failed: bool = False
    passed: bool = False
    duration: float = 0.0
    error: Optional[str] = None

    @property
    def done(self) -> bool:
        return self.error is not None or (bool(self.items) and self.items <= self.finished)


class Session:
    """What the event stream said so far."""

    def __init__(self, project: Project, model: Model, progress_file: str, expected: int) -> None:
        self.project = project
        self.model = model
        self.progress_file = progress_file
        self.expected = expected
        self.modules: dict[str, ModuleRun] = {}
        self.tests: list[dict[str, Any]] = []
        self.failed = 0
        self.current: Optional[str] = None
        self.collected = False
        self.status: Optional[int] = None
        self.runner = PYTEST  # or UNITTEST, for the commands
        self.written = 0.0
        self.by_position: dict[tuple[str, int], Function] = {}
        for f in model.functions:
            if f.kind == "def":
                self.by_position[(f.module.source.rel, f.start)] = f
                self.by_position.setdefault((f.module.source.rel, getattr(f.node, "lineno", f.start)), f)

    def module(self, path: Optional[str], nodeid: str) -> ModuleRun:
        rel = self.project.rel(path) if path else None
        if rel is None:
            rel = nodeid.split("::", 1)[0]
        run = self.modules.get(rel)
        if run is None:
            source = self.project.by_rel.get(rel)
            container = source.module if source else rel[:-3].replace("/", ".") if rel.endswith(".py") else rel
            run = self.modules[rel] = ModuleRun(rel=rel, container=container)
        return run

    def handle(self, event: dict[str, Any]) -> None:
        kind = event.get("e")
        if kind == "collected":
            self.collected = True
            for nodeid, path in event.get("items", []):
                self.module(path, nodeid).items.add(nodeid)
        elif kind == "collecterror":
            run = self.module(event.get("path"), event.get("nodeid") or "")
            run.error = keep_end((run.error or "") + (event.get("output") or ""))
        elif kind == "start":
            nodeid = event.get("nodeid") or ""
            self.current = self.module(None, nodeid).container
        elif kind == "result":
            self.result(event)
        elif kind == "done":
            self.status = event.get("status")
        self.report(force=kind in ("collected", "collecterror", "done"))

    def result(self, event: dict[str, Any]) -> None:
        nodeid = event.get("nodeid") or ""
        run = self.module(event.get("path"), nodeid)
        run.items.add(nodeid)
        was_done = run.done
        run.finished.add(nodeid)
        outcome = event.get("outcome") if event.get("outcome") in ("pass", "fail", "skip") else "fail"
        duration = float(event.get("duration") or 0.0)
        run.duration += duration
        run.failed = run.failed or outcome == "fail"
        run.passed = True
        self.failed += outcome == "fail"
        node = nodeid.split("::", 1)[1] if "::" in nodeid else None
        name = node if node is not None else nodeid.rsplit("/", 1)[-1]
        f = self.function_of(event, run, name)
        output = event.get("output") if outcome == "fail" else None
        self.tests.append({
            "container": f.module.name if f else run.container,
            "package": run.container,
            "name": name,
            "hierarchy": [name],
            "function": f.id if f else None,
            "file": f.module.source.rel if f else None,
            "line": f.start if f else None,
            "outcome": outcome,
            "durationMs": max(int(duration * 1000), 0),
            "output": keep_end(output or "") if outcome == "fail" else None,
            "command": self.command(run, node),
        })
        self.report(force=run.done and not was_done)

    def command(self, run: ModuleRun, node: Optional[str]) -> Optional[str]:
        """The command that runs a test (node: its name in the module) or,
        with node None, the module's tests, as this run ran them."""
        if self.runner == UNITTEST:
            return unittest_command(self.project, run.rel, node)
        return pytest_command(run.rel, node)

    def function_of(self, event: dict[str, Any], run: ModuleRun, name: str) -> Optional[Function]:
        file, line = event.get("def_file"), event.get("def_line")
        if file and isinstance(line, int):
            rel = self.project.rel(file)
            if rel is not None:
                f = self.by_position.get((rel, line))
                if f is not None:
                    return f
        qualified = re.sub(r"\[.*$", "", name).replace("::", ".")
        return self.model.by_id.get(f"{run.container}.{qualified}")

    def report(self, force: bool = False) -> None:
        if not self.progress_file:
            return
        now = time.monotonic()
        if not force and now - self.written < PROGRESS_INTERVAL:
            return
        self.written = now
        done = sum(1 for m in self.modules.values() if m.done)
        total = len(self.modules) if self.collected else max(self.expected, len(self.modules))
        current = self.current
        if current is not None and all(m.done for m in self.modules.values() if m.container == current):
            current = None
        from difflens_analyzer.cli import write_progress

        write_progress(self.progress_file, protocol.progress(done, max(total, done), len(self.tests), self.failed, current))


def run_tests(
    root: str,
    timeout: float,
    cover: bool,
    selection: Optional[dict[str, Any]],
    progress_file: str,
    offline: bool,
    log: Logger,
    cache_dir: str = "",
) -> dict[str, Any]:
    started = time.monotonic()
    deadline = started + max(timeout - min(timeout / 10, 5.0), 1.0)
    project = discover(root)
    model = build_model(project)
    diagnostics: list[str] = []
    if not cover:
        diagnostics.append("coverage was not measured: --no-cover was given")

    test_modules = [s for s in project.files if s.test and _TEST_FILE.match(s.rel.rsplit("/", 1)[-1])]
    targets, wanted = _targets(project, selection, diagnostics)
    expected = len(wanted) if selection is not None else len(test_modules)
    session = Session(project, model, progress_file, expected)
    session.report(force=True)

    def finish(coverage: Optional[dict[str, Any]], ran: bool) -> dict[str, Any]:
        packages = _packages(session, project, selection is None)
        log.info("test run done", tests=len(session.tests), failed=session.failed, packages=len(packages),
                 partial=selection is not None, elapsed_ms=int((time.monotonic() - started) * 1000))
        session.current = None
        session.report(force=True)
        return protocol.test_run(race=False, tests=session.tests, packages=packages, coverage=coverage,
                                 diagnostics=diagnostics)

    if selection is not None and not targets:
        return finish(None, False)
    if selection is None and not test_modules:
        if cover:
            diagnostics.append("coverage was not measured: the tree has no tests")
        return finish(None, False)

    plan = environment.make_plan(root)
    interpreter = environment.find_interpreter(root, plan)
    if interpreter is None:
        raise RunError("no Python interpreter for the project: no prepared environment, no virtual environment in the tree, no python3 on PATH")
    info = environment.probe(interpreter, timeout=min(60.0, processes.remaining(deadline)))
    if info is None:
        raise RunError(f"the project's interpreter {interpreter.python} does not run")
    if plan.kind != "none" and interpreter.source == "path":
        diagnostics.append(
            f"the project's dependencies were not prepared, so the tests ran with {interpreter.python}"
            f" (Python {info.get('python')}); some may fail to import"
        )
    pytest = environment.has_module(info, "pytest")
    measure = cover and environment.has_module(info, "coverage")
    if cover and not measure:
        diagnostics.append(
            f"coverage was not measured: coverage.py is not installed in the project's environment ({interpreter.python})"
        )
    if not pytest:
        session.runner = UNITTEST
        diagnostics.append(
            f"pytest is not installed in the project's environment ({interpreter.python}), so only unittest suites ran"
        )

    scratch = tempfile.mkdtemp(prefix="difflens-test-")
    pycache = os.path.join(cache_dir, "pycache") if cache_dir else os.path.join(scratch, "pycache")
    try:
        events = os.path.join(scratch, "events.jsonl")
        env = _environment(project, interpreter, scratch, events, offline, pycache)
        if pytest:
            # The cache provider stays (a project's addopts may use --lf), with
            # its folder in the run's scratch space.
            command = ["-m", "pytest", "-p", "_difflens_pytest", "-o", f"cache_dir={os.path.join(scratch, 'pytest-cache')}",
                       "--continue-on-collection-errors", "--tb=short", "-q", "--color=no"]
            if measure and environment.has_module(info, "pytest_cov"):
                command.append("--no-cov")
            if measure and environment.has_module(info, "xdist"):
                command += ["-n", "0"]
            command += targets
        else:
            spec = os.path.join(scratch, "unittest.json")
            _unittest_spec(project, spec, selection, wanted, test_modules)
            command = ["-m", "_difflens_unittest", spec]
        if measure:
            command = ["-m", "coverage", "run", f"--source={root}",
                       "--omit=*/.venv/*,*/venv/*,*/site-packages/*"] + command
        stopped, output = _run(interpreter.python, command, root, env, events, session, deadline)
        if stopped:
            diagnostics.append(
                f"the run was stopped after {timeout:g}s: results are partial and coverage was not measured"
            )
        _conftest_errors(output, session, project, test_modules)
        if not session.tests and not session.modules and session.status is None and not stopped:
            raise RunError("the tests could not run:\n" + keep_end(output.strip(), 4 << 10))
        if not session.tests and session.status in (3, 4) and not any(m.error for m in session.modules.values()):
            raise RunError("pytest could not run:\n" + keep_end(output.strip(), 4 << 10))
        coverage = None
        if measure and not stopped:
            coverage = _coverage(interpreter.python, root, env, scratch, project, model, deadline, diagnostics)
        return finish(coverage, True)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
        if cache_dir:
            drop_tree_bytecode(pycache, root)


def drop_tree_bytecode(pycache: str, root: str) -> None:
    """Removes the bytecode of the tree from a kept PYTHONPYCACHEPREFIX folder:
    the next run's tree is a copy at another path, so it would never be read."""
    for path in dict.fromkeys([os.path.abspath(root), os.path.realpath(root)]):
        mirror = os.path.join(pycache, os.path.splitdrive(path)[1].lstrip(os.sep))
        if os.path.normpath(mirror) != os.path.normpath(pycache):
            shutil.rmtree(mirror, ignore_errors=True)


def _targets(project: Project, selection: Optional[dict[str, Any]], diagnostics: list[str]) -> tuple[list[str], list[str]]:
    """pytest arguments for a selection, and the containers it names that exist."""
    if selection is None:
        return [], []
    targets: list[str] = []
    wanted: list[str] = []
    for package in selection.get("packages", []):
        container = package.get("package")
        source = project.by_module.get(container) if isinstance(container, str) else None
        if source is None:
            diagnostics.append(f"the package {container} was not found, so its tests were not run")
            continue
        wanted.append(container)
        tests = package.get("tests")
        if tests is None:
            targets.append(source.rel)
            continue
        for test in tests:
            name = test.get("name") or ""
            subtests = test.get("subtests")
            if subtests is None:
                targets.append(f"{source.rel}::{name}")
            else:
                targets.extend(f"{source.rel}::{name}/{sub}" for sub in subtests)
    return list(dict.fromkeys(targets)), wanted


def _unittest_spec(
    project: Project, path: str, selection: Optional[dict[str, Any]], wanted: list[str], test_modules: list[Any]
) -> None:
    modules = []
    if selection is None:
        chosen = [(s, None) for s in test_modules]
    else:
        chosen = []
        for package in selection.get("packages", []):
            source = project.by_module.get(package.get("package"))
            if source is None:
                continue
            tests = package.get("tests")
            names = None if tests is None else [t.get("name", "").split("[", 1)[0].replace("::", ".") for t in tests]
            chosen.append((source, names))
    for source, names in chosen:
        base, name = _import_name(source.path)
        modules.append({"module": name, "sys_path": base, "path": source.rel, "abs": source.path, "names": names})
    with open(path, "w", encoding="utf-8") as stream:
        json.dump({"modules": modules}, stream)


def _import_name(path: str) -> tuple[str, str]:
    """Where a test module is imported from and by which name, as pytest does
    (its "prepend" import mode): the first folder up without an __init__.py
    goes on sys.path, so a tests/ folder without one is never confused with
    an installed package called tests."""
    folder = os.path.dirname(path)
    parts = [os.path.basename(path)[:-3]]
    while os.path.isfile(os.path.join(folder, "__init__.py")):
        parts.insert(0, os.path.basename(folder))
        folder = os.path.dirname(folder)
    return folder, ".".join(parts)


def _environment(
    project: Project, interpreter: environment.Interpreter, scratch: str, events: str, offline: bool, pycache: str
) -> dict[str, str]:
    env = dict(os.environ)
    env.pop("PYTHONHOME", None)
    metadata = os.path.join(scratch, "metadata")
    _write_metadata(project, metadata)
    roots = sorted(project.roots, key=lambda r: -r.count("/") - (1 if r else 0))
    paths = [INPROJECT, metadata] + [os.path.join(project.root, r) for r in roots]
    if env.get("PYTHONPATH"):
        paths.append(env["PYTHONPATH"])
    env["PYTHONPATH"] = os.pathsep.join(paths)
    env["DIFFLENS_TEST_EVENTS"] = events
    env["COVERAGE_FILE"] = os.path.join(scratch, "coverage", ".coverage")
    os.makedirs(os.path.join(scratch, "coverage"), exist_ok=True)
    # Bytecode of the analyzer's plugin, the project and its tests goes to the
    # run's own folder or the kept cache folder, never next to the sources.
    env["PYTHONPYCACHEPREFIX"] = pycache
    env["PY_COLORS"] = "0"
    env["NO_COLOR"] = "1"
    if interpreter.bin_dir:
        env["VIRTUAL_ENV"] = interpreter.env or ""
        env["PATH"] = interpreter.bin_dir + os.pathsep + env.get("PATH", "")
    if offline:
        env["PIP_NO_INDEX"] = "1"
        env["UV_OFFLINE"] = "1"
    return env


def _write_metadata(project: Project, folder: str) -> None:
    """Distribution metadata for the projects of the tree, which are not
    installed, so that importlib.metadata.version() and entry points work."""
    os.makedirs(folder, exist_ok=True)
    for rel in project.projects:
        data = project.pyproject(rel).get("project") or {}
        name, version = data.get("name"), data.get("version")
        if not isinstance(name, str) or not name:
            continue
        if not isinstance(version, str):
            version = "0.0.0"
        normalized = re.sub(r"[-_.]+", "_", name).lower()
        info = os.path.join(folder, f"{normalized}-{version}.dist-info")
        os.makedirs(info, exist_ok=True)
        with open(os.path.join(info, "METADATA"), "w", encoding="utf-8") as stream:
            stream.write(f"Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n")
        groups: dict[str, dict[str, str]] = {}
        for group, table in (("console_scripts", data.get("scripts")), ("gui_scripts", data.get("gui-scripts"))):
            if isinstance(table, dict):
                groups[group] = {k: v for k, v in table.items() if isinstance(v, str)}
        for group, table in (data.get("entry-points") or {}).items():
            if isinstance(table, dict):
                groups[group] = {k: v for k, v in table.items() if isinstance(v, str)}
        if groups:
            with open(os.path.join(info, "entry_points.txt"), "w", encoding="utf-8") as stream:
                for group, entries in groups.items():
                    stream.write(f"[{group}]\n")
                    for key, value in entries.items():
                        stream.write(f"{key} = {value}\n")
                    stream.write("\n")


def _run(
    python: str, command: list[str], root: str, env: dict[str, str], events: str, session: Session, deadline: float
) -> tuple[bool, str]:
    """Runs the tests, feeding the session as events arrive; whether it was stopped, and its output."""
    open(events, "a", encoding="utf-8").close()
    output = processes.Tail(1 << 20)
    try:
        process = processes.start([python] + command, root, env)
    except OSError as error:
        raise RunError(f"cannot start {python}: {error}") from error
    readers = [processes.pump(process.stdout, output), processes.pump(process.stderr, output)]  # type: ignore[arg-type]
    stopped = False
    with open(events, encoding="utf-8") as stream:
        pending = ""
        while True:
            finished = process.poll() is not None
            chunk = stream.read()
            if chunk:
                pending += chunk
                lines = pending.split("\n")
                pending = lines.pop()
                for line in lines:
                    _feed(session, line)
            if finished:
                break
            if time.monotonic() > deadline:
                stopped = True
                processes.stop(process)
                continue
            session.report()
            time.sleep(0.1)
        rest = pending + stream.read()
        for line in rest.split("\n"):
            _feed(session, line)
    for reader in readers:
        reader.join(5)
    return stopped, output.text()


def _feed(session: Session, line: str) -> None:
    line = line.strip()
    if not line:
        return
    try:
        event = json.loads(line)
    except ValueError:
        return
    if isinstance(event, dict):
        session.handle(event)


def _conftest_errors(output: str, session: Session, project: Project, test_modules: list[Any]) -> None:
    """A conftest.py that does not import stops pytest before it collects:
    every test module under it did not build."""
    match = _CONFTEST_ERROR.search(output)
    if match is None or session.tests:
        return
    rel = project.rel(match.group(1))
    folder = rel.rsplit("/", 1)[0] if rel and "/" in rel else ""
    start = output.find(match.group(0))
    report = keep_end(output[start:].strip())
    for source in test_modules:
        if not folder or source.rel.startswith(folder + "/"):
            run = session.module(source.path, source.rel)
            run.error = report


def _packages(session: Session, project: Project, full: bool) -> list[dict[str, Any]]:
    out = []
    seen = set()
    for run in sorted(session.modules.values(), key=lambda m: m.rel):
        seen.add(run.container)
        if run.error is not None:
            outcome, output = "build_failed", run.error
        elif run.items and not run.items <= run.finished:
            outcome = "fail"
            output = "not every test of the module ran: the run stopped before them\n"
        elif run.failed:
            outcome, output = "fail", None
        elif run.passed:
            outcome, output = "pass", None
        else:
            outcome, output = "no_tests", None
        out.append({"container": run.container, "outcome": outcome, "durationMs": int(run.duration * 1000),
                    "output": keep_end(output) if output is not None else None,
                    "command": session.command(run, None)})
    if full:
        # Modules nothing ran in have no tests to run again: no command.
        for source in project.files:
            if source.module not in seen:
                seen.add(source.module)
                out.append({"container": source.module, "outcome": "no_tests", "durationMs": 0, "output": None,
                            "command": None})
    return out


def shell_quote(text: str) -> str:
    """text as one single-quoted shell word ("'" becomes "'\\''")."""
    return "'" + text.replace("'", "'\\''") + "'"


def pytest_command(rel: str, node: Optional[str]) -> str:
    """pytest runs from the root of the tree, so the file is relative to it."""
    return "python -m pytest " + shell_quote(f"{rel}::{node}" if node else rel)


def unittest_command(project: Project, rel: str, node: Optional[str]) -> Optional[str]:
    """`python -m unittest` with the module's dotted name (and the test's
    Class.method), run from the folder that name starts at: the module's
    source root, or the tree. None when the module has no importable name."""
    source = project.by_rel.get(rel)
    if source is None:
        return None
    dotted = source.module
    if node:
        dotted += "." + node.replace("::", ".")
    if not all(part.isidentifier() for part in dotted.split(".")):
        return None
    for base in dict.fromkeys([project.source_root(rel), ""]):
        inner = rel[len(base) + 1 :] if base else rel
        parts = inner[:-3].split("/") if inner.endswith(".py") else []
        if parts and parts[-1] == "__init__":
            parts.pop()
        if ".".join(parts) == source.module:
            prefix = f"cd {base if _SHELL_SAFE.match(base) else shell_quote(base)} && " if base else ""
            return f"{prefix}python -m unittest {dotted}"
    return None


def _coverage(
    python: str, root: str, env: dict[str, str], scratch: str, project: Project, model: Model,
    deadline: float, diagnostics: list[str],
) -> Optional[dict[str, Any]]:
    data_dir = os.path.dirname(env["COVERAGE_FILE"])
    names = os.listdir(data_dir) if os.path.isdir(data_dir) else []
    if any(n.startswith(".coverage.") for n in names):
        processes.run([python, "-m", "coverage", "combine", "--keep"], root, env, processes.remaining(deadline))
        names = os.listdir(data_dir)
    if ".coverage" not in names:
        diagnostics.append("coverage was not measured: coverage.py wrote no data")
        return None
    report = os.path.join(scratch, "coverage.json")
    result = processes.run([python, "-m", "coverage", "json", "-q", "-o", report, "--ignore-errors"], root, env,
                           processes.remaining(deadline))
    try:
        with open(report, encoding="utf-8") as stream:
            data = json.load(stream)
    except (OSError, ValueError):
        diagnostics.append(f"coverage was not measured: coverage json failed: {result.first_line()}")
        return None
    return {"blocks": coverage_blocks(data, project, model)}


def coverage_blocks(data: dict[str, Any], project: Project, model: Model) -> list[dict[str, Any]]:
    """One block per statement line of the project's own code (not tests,
    not generated files). The lines of def statements and their decorators
    run at import, not in the function, so they are left out."""
    blocks = []
    modules = {m.source.rel: m for m in model.modules.values()}
    for name, entry in (data.get("files") or {}).items():
        rel = project.rel(name if os.path.isabs(name) else os.path.join(project.root, name))
        source = project.by_rel.get(rel) if rel else None
        if source is None or source.test:
            continue
        module = modules.get(rel)
        if module is not None and module.generated:
            continue
        headers = _header_lines(module.tree) if module is not None else set()
        counts: dict[int, int] = {}
        for line in entry.get("executed_lines") or []:
            counts[int(line)] = 1
        for line in entry.get("missing_lines") or []:
            counts.setdefault(int(line), 0)
        for line in sorted(counts):
            if line >= 1 and line not in headers:
                blocks.append({"file": rel, "startLine": line, "endLine": line, "statements": 1, "count": counts[line]})
    blocks.sort(key=lambda b: (b["file"], b["startLine"]))
    return blocks


def _header_lines(tree: ast.Module) -> set[int]:
    lines: set[int] = set()
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            start = min([node.lineno] + [d.lineno for d in node.decorator_list])
            first = node.body[0].lineno if node.body else node.lineno
            end = first - 1 if first > node.lineno else node.lineno
            lines.update(range(start, end + 1))
    return lines
