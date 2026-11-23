"""The prepare command: the project's dependencies installed outside the tree.

It may use the network and never runs project code: the project itself is
not built or installed (see environment.py). The environment is only read
by the test step, so it is not reported as writable; nothing else is
written outside the run's own folders, so `writable` is empty.

Two prepares of copies with the same dependency files build one
environment: the first holds its lock while building, the second waits for
it and finds it ready (environment.build_lock).
"""

from __future__ import annotations

import os
import shutil
import tempfile
import time

from difflens_analyzer import environment, processes
from difflens_analyzer.environment import Plan
from difflens_analyzer.log import Logger


class PrepareError(Exception):
    """The dependencies could not be installed; the message says why."""


def prepare_tree(root: str, timeout: float, log: Logger) -> list[str]:
    deadline = time.monotonic() + max(timeout - min(timeout / 10, 10.0), 1.0)
    plan = environment.make_plan(root)
    if plan.kind == "none":
        log.info("prepare done: no dependencies declared", dir=root)
        return []
    env = plan.env_dir
    os.makedirs(environment.cache_root(), exist_ok=True)
    # Another prepare may be building this environment for another copy of
    # the project: wait for it, then reuse what it built.
    try:
        with environment.build_lock(plan.key, deadline):
            if environment.prepared(plan) is not None:
                os.utime(os.path.join(env, environment.READY))
                log.info("prepare done: environment ready", env=env, kind=plan.kind)
                return []
            started = time.monotonic()
            # Whatever is there has no ready marker: a build that failed or was killed.
            shutil.rmtree(env, ignore_errors=True)
            try:
                _build(root, plan, env, deadline)
            except PrepareError:
                shutil.rmtree(env, ignore_errors=True)
                raise
            environment.mark_ready(env, plan, os.path.join(env, "bin", "python"))
            environment.prune(env)
    except environment.LockTimeout as error:
        raise PrepareError(str(error)) from error
    log.info("prepare done", env=env, kind=plan.kind, elapsed_ms=int((time.monotonic() - started) * 1000))
    return []


def _build(root: str, plan: Plan, env: str, deadline: float) -> None:
    """Installs the project's dependencies into env, which does not exist yet,
    then coverage.py when the project does not declare it."""
    if plan.kind == "uv":
        _uv_sync(root, plan, env, deadline)
    elif plan.kind == "poetry":
        _venv(root, plan, env, deadline)
        _poetry(root, plan, env, deadline)
    else:
        _venv(root, plan, env, deadline)
        _pip(root, plan, env, deadline)
    add_coverage(root, plan, env, deadline)


# What the test step measures coverage with. Few projects declare it, and
# without it every run reports no coverage, so the Quality tab has no
# coverage and no CRAP score for any function.
COVERAGE_REQUIREMENT = "coverage>=7"


def add_coverage(root: str, plan: Plan, env: str, deadline: float) -> bool:
    """Installs coverage.py into the environment env when it lacks it, so the
    test step can measure coverage. Only the measuring tool is added; the
    project's own requirements are left as they were installed. Returns
    whether the environment has coverage.py afterwards; failing to install it
    does not fail prepare (the tests still run, the run then says coverage
    was not measured and why)."""
    python = os.path.join(env, "bin", "python")
    interpreter = environment.Interpreter(python, "prepared", env)
    if environment.has_module(environment.probe(interpreter, timeout=min(30.0, processes.remaining(deadline))), "coverage"):
        return True
    if shutil.which("uv"):
        cmd = ["uv", "pip", "install", "--no-config", "--python", python, "--compile-bytecode", COVERAGE_REQUIREMENT]
    else:
        cmd = [python, "-I", "-m", "pip", "install", "--disable-pip-version-check", "--no-input", COVERAGE_REQUIREMENT]
    result = processes.run(cmd, os.path.join(root, plan.folder), _environ(), processes.remaining(deadline))
    return result.code == 0 and not result.timed_out and not result.missing


def _environ(**extra: str) -> dict[str, str]:
    env = dict(os.environ)
    for name in ("PYTHONHOME", "PYTHONPATH", "VIRTUAL_ENV", "CONDA_PREFIX", "UV_PROJECT_ENVIRONMENT"):
        env.pop(name, None)
    env.update(extra)
    return env


def _check(result: processes.Result, what: str) -> None:
    if result.timed_out:
        raise PrepareError(f"{what} did not finish in time")
    if result.missing:
        raise PrepareError(f"{what} could not start: {result.stderr}")
    if result.code != 0:
        detail = (result.stderr or result.stdout).strip().splitlines()[-12:]
        raise PrepareError(f"{what} failed (exit status {result.code}):\n" + "\n".join(detail))


def _uv_sync(root: str, plan: Plan, env: str, deadline: float) -> None:
    folder = os.path.join(root, plan.folder)
    base = ["uv", "sync", "--frozen", "--compile-bytecode", "--no-progress"]
    for group in plan.groups:
        base += ["--group", group]
    for extra in plan.extras:
        base += ["--extra", extra]
    environ = _environ(UV_PROJECT_ENVIRONMENT=env)
    result = processes.run(base + ["--no-install-workspace"], folder, environ, processes.remaining(deadline))
    if result.code != 0 and "no-install-workspace" in result.stderr:
        result = processes.run(base + ["--no-install-project"], folder, environ, processes.remaining(deadline))
    _check(result, "uv sync")


def _venv(root: str, plan: Plan, env: str, deadline: float) -> None:
    folder = os.path.join(root, plan.folder)
    if shutil.which("uv"):
        cmd = ["uv", "venv", "--no-config", "--no-project", env]
        if plan.python:
            cmd[2:2] = ["--python", plan.python]
        result = processes.run(cmd, folder, _environ(), processes.remaining(deadline))
        if result.code != 0 and plan.python:
            # A version uv cannot find or fetch: the default interpreter will do.
            result = processes.run(cmd[:2] + cmd[4:], folder, _environ(), processes.remaining(deadline))
        _check(result, "uv venv")
        return
    python = shutil.which("python3") or shutil.which("python")
    if python is None:
        raise PrepareError("python3 is not on PATH")
    _check(processes.run([python, "-I", "-m", "venv", env], folder, _environ(), processes.remaining(deadline)), "python3 -m venv")


def _pip(root: str, plan: Plan, env: str, deadline: float) -> None:
    python = os.path.join(env, "bin", "python")
    scratch = tempfile.mkdtemp(prefix="difflens-prepare-")
    try:
        args: list[str] = []
        for path in plan.requirement_files:
            args += ["-r", _filtered(path, scratch, set())]
        args += plan.requirements
        if not args:
            return
        if shutil.which("uv"):
            cmd = ["uv", "pip", "install", "--no-config", "--python", python, "--compile-bytecode"] + args
            what = "uv pip install"
        else:
            cmd = [python, "-I", "-m", "pip", "install", "--disable-pip-version-check", "--no-input"] + args
            what = "pip install"
        _check(processes.run(cmd, os.path.join(root, plan.folder), _environ(), processes.remaining(deadline)), what)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


def _filtered(path: str, scratch: str, seen: set[str]) -> str:
    """A copy of a requirements file without editable and local-path lines,
    with its -r and -c includes copied the same way."""
    seen.add(os.path.realpath(path))
    lines = []
    try:
        with open(path, encoding="utf-8") as stream:
            text = stream.read()
    except (OSError, UnicodeDecodeError):
        text = ""
    for raw in text.splitlines():
        line = raw.split(" #", 1)[0].strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith(("-e ", "--editable", "-e.")) or environment._local(line):
            continue
        for flag in ("-r", "--requirement", "-c", "--constraint"):
            if line.startswith(flag + " ") or line.startswith(flag + "="):
                include = line[len(flag) :].lstrip(" =")
                target = os.path.join(os.path.dirname(path), include)
                if os.path.realpath(target) in seen or not os.path.isfile(target):
                    line = ""
                else:
                    line = f"{flag} {_filtered(target, scratch, seen)}"
                break
        if line:
            lines.append(line)
    handle, copy = tempfile.mkstemp(dir=scratch, suffix=".txt")
    with os.fdopen(handle, "w", encoding="utf-8") as stream:
        stream.write("\n".join(lines) + "\n")
    return copy


def _poetry(root: str, plan: Plan, env: str, deadline: float) -> None:
    cmd = ["poetry", "install", "--no-root", "--no-interaction", "--no-ansi"]
    if plan.groups:
        cmd += ["--with", ",".join(plan.groups)]
    environ = _environ(
        VIRTUAL_ENV=env,
        POETRY_VIRTUALENVS_CREATE="false",
        PATH=os.path.join(env, "bin") + os.pathsep + os.environ.get("PATH", ""),
    )
    _check(processes.run(cmd, os.path.join(root, plan.folder), environ, processes.remaining(deadline)), "poetry install")
