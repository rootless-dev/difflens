"""The check command: ruff and mypy, as the project uses them, in that order.

A checker runs when the project uses it: configured (`[tool.ruff]`,
`ruff.toml`, `.ruff.toml`; `[tool.mypy]`, `mypy.ini`, `.mypy.ini`,
setup.cfg `[mypy]`) or installed in the project's environment (environment.py
says which one). It is looked for in that environment, then on PATH; a
checker the project does not use, or that is not installed, is
`unavailable` with the reason.

Both only read the code and write nothing inside the tree: ruff runs with
`--no-cache` (its cache is keyed by absolute paths, and every tree is a copy
at a new path), mypy with its incremental cache in `<--cache-dir>/mypy`, or
in a temporary folder without --cache-dir. That cache stays correct across
copies: mypy is given absolute paths (the configured `files` too), and it
rehashes a module whose recorded path differs, so the tree's modules are
rechecked when their contents changed, while those of the stable project
environment and the standard library are reused. mypy uses the
project's interpreter (`--python-executable`) so that installed packages
resolve; with no project environment it is told to ignore missing imports,
and a diagnostic says so. The core runs `prepare` on the tree first, so the
environment is there. Findings are ruff's diagnostics (rule: its code;
severity warning, error for a syntax error) and mypy's errors and warnings
(rule: its error code; severity as mypy says); notes are left out.
"""

from __future__ import annotations

import configparser
import glob
import json
import os
import re
import shutil
import tempfile
import time
from typing import Any, Optional

from difflens_analyzer import environment, processes, protocol
from difflens_analyzer.log import Logger
from difflens_analyzer.project import Project, discover

RUFF, MYPY = "ruff", "mypy"
_MYPY_LINE = re.compile(
    r"^(?P<file>.+?):(?P<line>-?\d+)(?::-?\d+)?(?::\d+:\d+)?: (?P<severity>error|warning|note): "
    r"(?P<message>.*?)(?:  \[(?P<code>[a-z0-9-]+)\])?$"
)
# What ruff reports for code that does not parse: no rule in recent
# versions, "invalid-syntax" or E999 in older ones.
_RUFF_SYNTAX = (None, "", "invalid-syntax", "E999")


class Check:
    def __init__(self, tool: str) -> None:
        self.tool = tool
        self.status = "clean"
        self.message: Optional[str] = None
        self.findings: list[dict[str, Any]] = []

    def unavailable(self, why: str) -> "Check":
        self.status, self.message = "unavailable", why
        return self

    def failed(self, why: str) -> "Check":
        self.status, self.message = "failed", why
        return self

    def add(self, file: str, line: int, message: str, rule: Optional[str], severity: str) -> None:
        """severity is "error", "warning" or "info" (checks.schema.json)."""
        self.findings.append({"file": file, "line": max(line, 1), "message": message, "rule": rule, "severity": severity})

    def document(self) -> dict[str, Any]:
        findings = sorted(
            {(f["file"], f["line"], f["rule"] or "", f["message"]): f for f in self.findings}.values(),
            key=lambda f: (f["file"], f["line"], f["rule"] or "", f["message"]),
        )
        status = self.status
        if status == "clean" and findings:
            status = "findings"
        if status == "unavailable":
            findings = []
        return protocol.check(self.tool, status, self.message if status in ("unavailable", "failed") else None, findings)


def _configured(project: Project, tool: str) -> bool:
    for folder in project.projects or [""]:
        base = os.path.join(project.root, folder)
        if tool in (project.pyproject(folder).get("tool") or {}):
            return True
        files = ("ruff.toml", ".ruff.toml") if tool == RUFF else ("mypy.ini", ".mypy.ini")
        if any(os.path.isfile(os.path.join(base, f)) for f in files):
            return True
        if tool == MYPY and os.path.isfile(os.path.join(base, "setup.cfg")):
            parser = configparser.ConfigParser()
            try:
                parser.read(os.path.join(base, "setup.cfg"), encoding="utf-8")
            except (configparser.Error, UnicodeDecodeError):
                continue
            if parser.has_section("mypy"):
                return True
    return False


def run_checks(root: str, timeout: float, log: Logger, cache_dir: str = "") -> dict[str, Any]:
    deadline = time.monotonic() + max(timeout - min(timeout / 10, 5.0), 1.0)
    project = discover(root, deadline)
    diagnostics: list[str] = []
    interpreter = environment.find_interpreter(root)
    project_env = interpreter if interpreter is not None and interpreter.source != "path" else None
    info = environment.probe(project_env) if project_env is not None else None
    checks = [
        ruff(project, project_env, deadline),
        mypy(project, project_env, info, deadline, diagnostics, cache_dir),
    ]
    documents = [c.document() for c in checks]
    log.info("checks done", **{d["tool"]: d["status"] for d in documents})
    return protocol.check_run(checks=documents, diagnostics=diagnostics)


def ruff(project: Project, env: Optional[environment.Interpreter], deadline: float) -> Check:
    check = Check(RUFF)
    in_env = env.tool("ruff") if env is not None else None
    configured = _configured(project, RUFF)
    if in_env is None and not configured:
        return check.unavailable("the project does not use ruff (no ruff configuration, not in its environment)")
    executable = in_env or shutil.which("ruff")
    if executable is None:
        return check.unavailable("ruff is not installed (not in the project's environment nor on PATH)")
    base = [executable, "check", "--no-cache", "--exit-zero", "--quiet"]
    result = processes.run(base + ["--output-format", "json", "."], project.root, _environ(), processes.remaining(deadline))
    if result.code != 0 and "output-format" in result.stderr:
        result = processes.run(base + ["--format", "json", "."], project.root, _environ(), processes.remaining(deadline))
    if result.timed_out:
        return check.failed("ruff did not finish in time")
    if result.code != 0:
        return check.failed(f"ruff could not check the code: {result.first_line()}")
    try:
        problems = json.loads(result.stdout or "[]")
    except ValueError:
        return check.failed("ruff printed something that is not JSON")
    for problem in problems if isinstance(problems, list) else []:
        rel = project.rel(problem.get("filename") or "")
        if rel is None:
            continue
        location = problem.get("location") or {}
        code = problem.get("code") or None
        check.add(rel, int(location.get("row") or 1), str(problem.get("message") or ""), code, ruff_severity(code))
    return check


def ruff_severity(code: Optional[str]) -> str:
    """ruff has no levels of its own: a syntax error is an error, every rule a warning."""
    return "error" if code in _RUFF_SYNTAX else "warning"


def mypy(
    project: Project,
    env: Optional[environment.Interpreter],
    info: Optional[dict[str, Any]],
    deadline: float,
    diagnostics: list[str],
    cache_dir: str = "",
) -> Check:
    check = Check(MYPY)
    in_env = env is not None and environment.has_module(info, "mypy")
    configured = _configured(project, MYPY)
    if not in_env and not configured:
        return check.unavailable("the project does not use mypy (no mypy configuration, not in its environment)")
    if in_env:
        assert env is not None
        command = [env.python, "-m", "mypy"]
    else:
        executable = shutil.which("mypy")
        if executable is None:
            return check.unavailable("mypy is not installed (not in the project's environment nor on PATH)")
        command = [executable]
        if env is not None:
            command += ["--python-executable", env.python]
        else:
            command.append("--ignore-missing-imports")
            diagnostics.append("mypy ran without the project's environment, so missing imports were ignored")
    cache, temporary = mypy_cache(cache_dir)
    try:
        command += [
            "--cache-dir", cache, "--show-error-codes", "--no-error-summary", "--no-pretty", "--no-color-output",
            "--show-absolute-path", "--hide-error-context", "--explicit-package-bases",
        ]
        # Absolute paths, so that mypy's cache records where each module of the
        # tree was: with a relative path, a copy holding other code at the same
        # place, written in the same second with the same size, would pass for
        # unchanged. Modules mypy finds by name are recorded absolute already.
        configured = _mypy_files_configured(project)
        targets = _mypy_config_files(project)
        if not targets and not configured:
            targets = [os.path.join(project.root, t) for t in _mypy_targets(project)]
        if not targets and not configured:
            return check
        environ = _environ()
        environ["MYPYPATH"] = os.pathsep.join(os.path.join(project.root, r) for r in project.roots if r)
        result = processes.run(command + targets, project.root, environ, processes.remaining(deadline))
    finally:
        if temporary:
            shutil.rmtree(cache, ignore_errors=True)
    if result.timed_out:
        return check.failed("mypy did not finish in time")
    if result.code not in (0, 1):
        return check.failed(f"mypy could not check the code: {result.first_line()}")
    for line in result.stdout.splitlines():
        match = _MYPY_LINE.match(line.strip())
        if match is None or match.group("severity") == "note":
            continue
        rel = project.rel(match.group("file"))
        if rel is None:
            continue
        check.add(rel, int(match.group("line")), match.group("message"), match.group("code"), match.group("severity"))
    if result.code == 1 and not check.findings:
        return check.failed(f"mypy failed: {result.first_line()}")
    return check


def mypy_cache(cache_dir: str) -> tuple[str, bool]:
    """mypy's --cache-dir, and whether it is a temporary folder to remove.

    With the core's cache folder it is `<cache_dir>/mypy`, kept between runs.
    mypy validates every entry before reusing it (mypy's version, its options,
    the module's size, then its hash when its path or mtime differ) and writes
    each file by renaming a complete one, so two checks of the project may
    share it."""
    if cache_dir:
        folder = os.path.join(cache_dir, "mypy")
        try:
            os.makedirs(folder, exist_ok=True)
            return folder, False
        except OSError:
            pass
    return tempfile.mkdtemp(prefix="difflens-mypy-"), True


def _mypy_files_configured(project: Project) -> bool:
    for folder in project.projects or [""]:
        settings = (project.pyproject(folder).get("tool") or {}).get("mypy") or {}
        if settings.get("files") or settings.get("packages") or settings.get("modules"):
            return True
        for name in ("mypy.ini", ".mypy.ini", "setup.cfg"):
            parser = configparser.ConfigParser()
            try:
                parser.read(os.path.join(project.root, folder, name), encoding="utf-8")
            except (configparser.Error, UnicodeDecodeError):
                continue
            if parser.has_option("mypy", "files") or parser.has_option("mypy", "packages"):
                return True
    return False


def _mypy_config_files(project: Project) -> list[str]:
    """The `files` of the mypy configuration mypy reads at the root of the
    tree (mypy.ini, .mypy.ini, pyproject.toml, setup.cfg, the first with a
    mypy section), expanded as mypy expands them and made absolute; given on
    the command line, they replace the configured ones. Empty without one."""
    entries: list[str] = []
    for name in ("mypy.ini", ".mypy.ini", "pyproject.toml", "setup.cfg"):
        path = os.path.join(project.root, name)
        if not os.path.isfile(path):
            continue
        if name == "pyproject.toml":
            settings = (project.pyproject("").get("tool") or {}).get("mypy")
            if not isinstance(settings, dict):
                continue
            value = settings.get("files") or []
            entries = [value] if isinstance(value, str) else [v for v in value if isinstance(v, str)]
        else:
            parser = configparser.ConfigParser()
            try:
                parser.read(path, encoding="utf-8")
            except (configparser.Error, UnicodeDecodeError):
                continue
            if not parser.has_section("mypy"):
                continue
            entries = [parser.get("mypy", "files", fallback="")]
        break
    files: list[str] = []
    for entry in (part for value in entries for part in value.split(",")):
        pattern = os.path.expandvars(os.path.expanduser(entry.strip()))
        if not pattern:
            continue
        # Relative to the folder mypy runs in, the root, whose own name is no pattern.
        anchored = pattern if os.path.isabs(pattern) else os.path.join(glob.escape(project.root), pattern)
        found = sorted(glob.glob(anchored, recursive=True))
        files.extend(found or [os.path.join(project.root, pattern)])
    return files


def _mypy_targets(project: Project) -> list[str]:
    """The top-level packages and modules of the source roots, tests included."""
    targets: list[str] = []
    for source in project.files:
        root = project.source_root(source.rel)
        inner = source.rel[len(root) + 1 :] if root else source.rel
        top = inner.split("/", 1)[0]
        target = f"{root}/{top}" if root else top
        if target not in targets:
            targets.append(target)
    return targets


def _environ() -> dict[str, str]:
    env = dict(os.environ)
    for name in ("PYTHONHOME", "PYTHONPATH"):
        env.pop(name, None)
    return env
