"""The project's own Python environment, never the analyzer's.

`prepare` builds the environment the tests and checkers use, outside the
tree, in a cache keyed by the project's dependency files (README.md,
"Project environments"). `test` and `check` find the interpreter in this
order:

1. the environment `prepare` built for these dependency files;
2. a virtual environment inside the tree: `.venv/`, `venv/`, `env/` or
   `.env/` (a folder with a `pyvenv.cfg`);
3. `python3` on PATH, then `python`.

How `prepare` installs, by what the project uses:

- a `uv.lock` and `uv` on PATH: `uv sync --frozen` into the cached
  environment (`UV_PROJECT_ENVIRONMENT`), with the default groups plus the
  test, dev, lint and typing groups and extras the project declares, and
  without installing the project itself (`--no-install-workspace`);
- a `poetry.lock` and `poetry` on PATH: a virtual environment, then
  `poetry install --no-root` into it;
- otherwise a virtual environment (`uv venv`, honouring `.python-version` or
  `requires-python`, else `python3 -m venv`) and `uv pip install` or `pip
  install` of what the project lists: `requirements*.txt` files (also under
  `requirements/`), `[project]` dependencies with those groups and extras,
  `[dependency-groups]`, Poetry's tables, a Pipfile, setup.cfg's
  `install_requires`. Editable and local-path requirements are dropped,
  since building the project would run its code.

Whatever the project uses, `prepare` then adds coverage.py to the
environment it built when the project does not declare it, so the test step
can measure coverage (`prepare.add_coverage`); failing to add it does not fail
prepare. Environments in the tree and interpreters on PATH are never changed.

The project's own code is never installed: the test step puts its source
roots on PYTHONPATH, so it always tests the tree it was given.

Several prepares may build the same environment at once (the core prepares
one copy of a commit for its checks while another is prepared for a test
run): each holds the environment's lock file, `.locks/<key>.lock` in the
cache, while it validates or builds it, so the second one waits and then
reuses the first one's. An environment is used only once its ready marker
is written, the last step of the build, so a half-built one never is.
"""

from __future__ import annotations

import contextlib
import fcntl
import hashlib
import json
import os
import re
import shutil
import sys
import tempfile
import time
from dataclasses import dataclass, field
from typing import Any, Iterator, Optional

from difflens_analyzer import processes, toml

FORMAT = 2  # bump to rebuild every cached environment (2: coverage.py added)
READY = "difflens-ready.json"
LOCKS = ".locks"
LOCK_POLL = 0.2
VENV_DIRS = (".venv", "venv", "env", ".env")
TEST_GROUPS = {
    "dev", "test", "tests", "testing", "lint", "linting", "typing", "types", "type", "typecheck",
    "type-check", "mypy", "ruff", "style", "qa", "check", "checks", "ci", "coverage",
}
PRUNE_AFTER = 30 * 24 * 3600

_REQUIREMENT_FILE = re.compile(r"^(.*[-_.])?requirements([-_.].*)?\.(txt|in)$|^requirements\.txt$", re.IGNORECASE)


def cache_root() -> str:
    override = os.environ.get("DIFFLENS_PYTHON_ENVS")
    if override:
        return os.path.abspath(override)
    home = os.path.expanduser("~")
    if sys.platform == "darwin":
        return os.path.join(home, "Library", "Caches", "dev.difflens.app", "python-envs")
    base = os.environ.get("XDG_CACHE_HOME") or os.path.join(home, ".cache")
    return os.path.join(base, "difflens", "python-envs")


@dataclass
class Plan:
    """How the project's dependencies are installed, and the cache key of the result."""

    kind: str  # "uv", "poetry", "pip" or "none"
    folder: str  # the project folder, relative to the tree
    files: list[str] = field(default_factory=list)  # dependency files, relative to the tree
    requirements: list[str] = field(default_factory=list)  # for pip: requirement strings
    requirement_files: list[str] = field(default_factory=list)  # for pip: absolute paths
    groups: list[str] = field(default_factory=list)
    extras: list[str] = field(default_factory=list)
    python: Optional[str] = None  # a version request, such as "3.12"
    key: str = ""

    @property
    def env_dir(self) -> str:
        return os.path.join(cache_root(), self.key)


@dataclass
class Interpreter:
    python: str
    source: str  # "prepared", "tree" or "path"
    env: Optional[str] = None  # the environment folder, for prepared and tree ones

    @property
    def bin_dir(self) -> Optional[str]:
        return os.path.dirname(self.python) if self.env else None

    def tool(self, name: str) -> Optional[str]:
        """An executable of the environment, such as ruff."""
        if self.bin_dir is None:
            return None
        path = os.path.join(self.bin_dir, name)
        return path if os.path.isfile(path) and os.access(path, os.X_OK) else None


def _read(path: str) -> str:
    try:
        with open(path, encoding="utf-8") as stream:
            return stream.read()
    except (OSError, UnicodeDecodeError):
        return ""


def project_folder(root: str) -> str:
    """The folder whose dependencies are installed: the tree, or one folder down."""
    if _has_dependencies(root):
        return ""
    try:
        names = sorted(os.listdir(root))
    except OSError:
        return ""
    for name in names:
        if name.startswith(".") or name in ("node_modules", "venv", "env"):
            continue
        if os.path.isdir(os.path.join(root, name)) and _has_dependencies(os.path.join(root, name)):
            return name
    return ""


def _has_dependencies(folder: str) -> bool:
    try:
        names = os.listdir(folder)
    except OSError:
        return False
    return any(
        n in ("pyproject.toml", "setup.cfg", "setup.py", "Pipfile", "uv.lock", "poetry.lock") or _REQUIREMENT_FILE.match(n)
        for n in names
    )


def make_plan(root: str) -> Plan:
    folder = project_folder(root)
    base = os.path.join(root, folder)
    plan = Plan(kind="none", folder=folder)
    files = []
    for name in ("pyproject.toml", "uv.lock", "poetry.lock", "Pipfile", "Pipfile.lock", "setup.cfg", ".python-version"):
        if os.path.isfile(os.path.join(base, name)):
            files.append(name)
    requirement_files = _requirement_files(base)
    files.extend(os.path.relpath(f, base).replace(os.sep, "/") for f in requirement_files)
    plan.files = [f"{folder}/{f}" if folder else f for f in files]
    pyproject = toml.load_file(os.path.join(base, "pyproject.toml"))
    plan.python = _python_request(base, pyproject)
    plan.groups = [g for g in (pyproject.get("dependency-groups") or {}) if g.lower() in TEST_GROUPS]
    extras = (pyproject.get("project") or {}).get("optional-dependencies") or {}
    plan.extras = [e for e in extras if e.lower() in TEST_GROUPS]

    if os.path.isfile(os.path.join(base, "uv.lock")) and shutil.which("uv"):
        plan.kind = "uv"
    elif os.path.isfile(os.path.join(base, "poetry.lock")) and shutil.which("poetry"):
        plan.kind = "poetry"
        poetry_groups = ((pyproject.get("tool") or {}).get("poetry") or {}).get("group") or {}
        plan.groups = [g for g in poetry_groups if g.lower() in TEST_GROUPS]
    else:
        plan.requirements = _declared_requirements(base, pyproject)
        plan.requirement_files = requirement_files
        if plan.requirements or plan.requirement_files:
            plan.kind = "pip"
    digest = hashlib.sha256()
    digest.update(f"{FORMAT}\n{plan.kind}\n{plan.python}\n{sys.platform}\n".encode())
    for rel in sorted(plan.files):
        digest.update(rel.encode() + b"\0")
        try:
            with open(os.path.join(root, rel), "rb") as stream:
                digest.update(hashlib.sha256(stream.read()).digest())
        except OSError:
            pass
    plan.key = digest.hexdigest()[:20]
    return plan


def _python_request(base: str, pyproject: dict[str, Any]) -> Optional[str]:
    pinned = _read(os.path.join(base, ".python-version")).strip().splitlines()
    if pinned and pinned[0].strip():
        return pinned[0].strip()
    requires = (pyproject.get("project") or {}).get("requires-python")
    return requires if isinstance(requires, str) else None


def _requirement_files(base: str) -> list[str]:
    found = []
    for folder in (base, os.path.join(base, "requirements")):
        try:
            names = sorted(os.listdir(folder))
        except OSError:
            continue
        for name in names:
            path = os.path.join(folder, name)
            inside = folder != base
            if os.path.isfile(path) and (_REQUIREMENT_FILE.match(name) or (inside and name.endswith(".txt"))):
                if name.endswith(".in") and os.path.isfile(path[:-3] + ".txt"):
                    continue  # the compiled .txt is pinned
                found.append(path)
    return found


def _declared_requirements(base: str, pyproject: dict[str, Any]) -> list[str]:
    out: list[str] = []
    project = pyproject.get("project") or {}
    out.extend(r for r in project.get("dependencies") or [] if isinstance(r, str))
    for extra, items in (project.get("optional-dependencies") or {}).items():
        if extra.lower() in TEST_GROUPS:
            out.extend(r for r in items if isinstance(r, str))
    groups = pyproject.get("dependency-groups") or {}
    for group in groups:
        if group.lower() in TEST_GROUPS:
            out.extend(_group(groups, group, set()))
    poetry = (pyproject.get("tool") or {}).get("poetry") or {}
    tables = [poetry.get("dependencies") or {}, poetry.get("dev-dependencies") or {}]
    for name, group in (poetry.get("group") or {}).items():
        if isinstance(group, dict) and (name.lower() in TEST_GROUPS or not group.get("optional")):
            tables.append(group.get("dependencies") or {})
    for table in tables:
        for name, spec in table.items():
            if name.lower() != "python":
                requirement = _poetry_requirement(name, spec)
                if requirement:
                    out.append(requirement)
    pipfile = toml.load_file(os.path.join(base, "Pipfile"))
    for table in (pipfile.get("packages") or {}, pipfile.get("dev-packages") or {}):
        for name, spec in table.items():
            version = spec if isinstance(spec, str) else (spec.get("version") if isinstance(spec, dict) else None)
            if isinstance(spec, dict) and ("path" in spec or "editable" in spec or "git" in spec):
                continue
            out.append(name if version in (None, "*") else f"{name}{version}")
    out.extend(_setup_cfg_requirements(os.path.join(base, "setup.cfg")))
    seen: set[str] = set()
    unique = []
    for requirement in out:
        requirement = requirement.strip()
        if requirement and not _local(requirement) and requirement not in seen:
            seen.add(requirement)
            unique.append(requirement)
    return unique


def _group(groups: dict[str, Any], name: str, seen: set[str]) -> list[str]:
    if name in seen:
        return []
    seen.add(name)
    out = []
    for item in groups.get(name) or []:
        if isinstance(item, str):
            out.append(item)
        elif isinstance(item, dict) and isinstance(item.get("include-group"), str):
            out.extend(_group(groups, item["include-group"], seen))
    return out


def _poetry_requirement(name: str, spec: Any) -> Optional[str]:
    if isinstance(spec, dict):
        if any(k in spec for k in ("path", "git", "url", "develop")):
            return None
        if spec.get("optional"):
            return None
        spec = spec.get("version", "*")
    if not isinstance(spec, str):
        return name
    spec = spec.strip()
    if spec in ("", "*"):
        return name
    if spec[0] in "^~" and spec[1:2] != "=":
        return f"{name}{_caret(spec[1:], spec[0] == '^')}"
    if spec[0].isdigit():
        return f"{name}=={spec}"
    return f"{name}{spec}"


def _caret(version: str, caret: bool) -> str:
    """Poetry's ^ and ~ constraints as PEP 440 ranges: ^1.2 is >=1.2,<2; ~1.2.3 is >=1.2.3,<1.3."""
    parts = [int(p) if p.isdigit() else 0 for p in version.split(".")[:3]]
    if caret:
        index = next((i for i, p in enumerate(parts) if p != 0), len(parts) - 1)
    else:
        index = 1 if len(parts) > 1 else 0
    upper = parts[: index + 1]
    upper[-1] += 1
    return f">={version},<{'.'.join(str(p) for p in upper)}"


def _setup_cfg_requirements(path: str) -> list[str]:
    import configparser

    if not os.path.isfile(path):
        return []
    parser = configparser.ConfigParser()
    try:
        parser.read(path, encoding="utf-8")
    except (configparser.Error, UnicodeDecodeError):
        return []
    out = [line.strip() for line in parser.get("options", "install_requires", fallback="").splitlines()]
    if parser.has_section("options.extras_require"):
        for extra, value in parser.items("options.extras_require"):
            if extra.lower() in TEST_GROUPS:
                out.extend(line.strip() for line in value.splitlines())
    return [line for line in out if line and not line.startswith("#")]


def _local(requirement: str) -> bool:
    text = requirement.strip()
    return (
        text.startswith(("-e", "--editable", ".", "/", "file:", "~"))
        or " @ file:" in text
        or text.endswith((".", "/"))
    )


# Finding the interpreter.


def tree_venv(root: str) -> Optional[str]:
    folder = project_folder(root)
    for base in dict.fromkeys([root, os.path.join(root, folder)]):
        for name in VENV_DIRS:
            env = os.path.join(base, name)
            python = os.path.join(env, "bin", "python")
            if os.path.isfile(os.path.join(env, "pyvenv.cfg")) and os.path.exists(python):
                return env
    return None


def prepared(plan: Plan) -> Optional[str]:
    env = plan.env_dir
    if plan.kind != "none" and os.path.isfile(os.path.join(env, READY)):
        return env
    return None


def find_interpreter(root: str, plan: Optional[Plan] = None) -> Optional[Interpreter]:
    plan = plan or make_plan(root)
    env = prepared(plan)
    if env is not None:
        return Interpreter(os.path.join(env, "bin", "python"), "prepared", env)
    env = tree_venv(root)
    if env is not None:
        return Interpreter(os.path.join(env, "bin", "python"), "tree", env)
    for name in ("python3", "python"):
        found = shutil.which(name)
        if found:
            return Interpreter(found, "path")
    return None


PROBE = r"""
import importlib.util, json, sys
def has(name):
    try:
        return importlib.util.find_spec(name) is not None
    except Exception:
        return False
def version(name):
    try:
        from importlib.metadata import version as v
        return v(name)
    except Exception:
        return None
mods = {"pytest": "pytest", "coverage": "coverage", "pytest_cov": "pytest-cov", "xdist": "pytest-xdist", "mypy": "mypy"}
print(json.dumps({
    "python": "%d.%d.%d" % sys.version_info[:3],
    "executable": sys.executable,
    "modules": {m: (version(d) or "") if has(m) else None for m, d in mods.items()},
}))
"""


def probe(interpreter: Interpreter, timeout: float = 30.0) -> Optional[dict[str, Any]]:
    """The interpreter's version and which tools it can import; None when it does not run."""
    result = processes.run([interpreter.python, "-I", "-c", PROBE], cwd=os.path.dirname(interpreter.python) or "/", timeout=timeout)
    if result.code != 0:
        return None
    try:
        found = json.loads(result.stdout.strip().splitlines()[-1])
    except (ValueError, IndexError):
        return None
    return found if isinstance(found, dict) else None


def has_module(info: Optional[dict[str, Any]], name: str) -> bool:
    return bool(info) and info.get("modules", {}).get(name) is not None  # type: ignore[union-attr]


def mark_ready(env: str, plan: Plan, python: str) -> None:
    """Writes the ready marker whole (a complete file renamed into place), last."""
    handle, temporary = tempfile.mkstemp(dir=env, prefix=".ready-")
    with os.fdopen(handle, "w", encoding="utf-8") as stream:
        json.dump({"kind": plan.kind, "python": python, "files": plan.files, "created": time.time()}, stream)
    os.replace(temporary, os.path.join(env, READY))


class LockTimeout(Exception):
    """Another process held the environment's lock past the deadline."""


def _lock_path(key: str) -> str:
    return os.path.join(cache_root(), LOCKS, f"{key}.lock")


def _try_lock(path: str) -> Optional[int]:
    """An open descriptor holding the exclusive lock on path, or None when
    another one holds it. A lock file can be removed between opening and
    locking it (prune does), so the lock counts only if the file locked is
    still the one at path."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    while True:
        handle = os.open(path, os.O_RDWR | os.O_CREAT, 0o644)
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            os.close(handle)
            return None
        try:
            same = os.path.samestat(os.fstat(handle), os.stat(path))
        except OSError:
            same = False
        if same:
            return handle
        os.close(handle)


@contextlib.contextmanager
def build_lock(key: str, deadline: float) -> Iterator[None]:
    """Holds the environment's lock while it is validated or built; waits for
    another holder until deadline (time.monotonic()), then raises LockTimeout."""
    path = _lock_path(key)
    while True:
        handle = _try_lock(path)
        if handle is not None:
            break
        if time.monotonic() >= deadline:
            raise LockTimeout(f"another prepare of the environment {key} did not finish in time")
        time.sleep(LOCK_POLL)
    try:
        yield
    finally:
        os.close(handle)  # releases the lock


def prune(keep: str) -> None:
    """Removes cached environments not prepared again for a month, skipping
    any another prepare holds."""
    root = cache_root()
    try:
        names = os.listdir(root)
    except OSError:
        return
    now = time.time()
    for name in names:
        env = os.path.join(root, name)
        if env == keep or name == LOCKS or not os.path.isdir(env):
            continue
        marker = os.path.join(env, READY)
        try:
            stamp = os.path.getmtime(marker if os.path.exists(marker) else env)
        except OSError:
            continue
        if now - stamp <= PRUNE_AFTER:
            continue
        path = _lock_path(name)
        try:
            handle = _try_lock(path)
        except OSError:
            continue
        if handle is None:
            continue
        try:
            shutil.rmtree(env, ignore_errors=True)
            with contextlib.suppress(OSError):
                os.unlink(path)
        finally:
            os.close(handle)
