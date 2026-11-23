"""The mutate command: the changed lines the tests run but do not verify.

The steps follow the description of analyzers/protocol/mutation.schema.json
(rules 1 to 7); what is Python's (README.md, "mutate"):

- candidate_lines: the lines --changed adds that a --coverage block with a
  count above 0 spans;
- the mutants of the tree's own files (not tests, not generated files) on
  those lines, from mutator.py, each with the id analyze gives the innermost
  function holding it;
- sample: the cap, round-robin across functions (rule 4);
- selections: the tests each mutated module is run with, the test modules
  that import it, directly or through other modules of the tree (their
  conftest.py files included), else every test;
- Runner: one process per run, with the project's interpreter as `test`
  runs it (pytest, else the unittest fallback), started through
  `inproject/_difflens_mutant.py`, which serves the mutated source of the one
  file from memory: the tree is never changed, so up to --jobs runs share it.
  Each selection runs once unmutated first (the baseline), and each of its
  mutants is stopped after max(--min-mutant-timeout, 3 x the baseline);
- the results kept in `<cache-dir>/mutation/`, by a fingerprint of the
  mutant, its function's source, the source of the test files it runs and
  the engine (rule 6).
"""

from __future__ import annotations

import ast
import hashlib
import json
import os
import shutil
import tempfile
import threading
import time
from collections import deque
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from typing import Any, Optional

from difflens_analyzer import VERSION, environment, processes, protocol, testrun
from difflens_analyzer.analysis import build_model
from difflens_analyzer.log import Logger
from difflens_analyzer.model import Model, Module
from difflens_analyzer.mutator import Mutant, mutants_of, mutated_source
from difflens_analyzer.project import Project, SourceFile, discover

ENGINE = {"name": "difflens-analyzer-python mutator", "version": VERSION}
# Bump when mutants or their results change meaning, so kept results are not reused.
FORMAT = 1
PROGRESS_INTERVAL = 0.25
KEPT = ("killed", "survived", "timeout")  # results worth keeping in the cache


class MutationError(Exception):
    """The mutants could not be run at all (exit 1)."""


@dataclass
class Selection:
    """The tests a mutated module runs with: test modules, or every test (modules empty)."""

    modules: list[SourceFile]
    files: list[str]  # test files and conftest.py files it runs, relative, for fingerprints
    every: bool
    digest: str = ""
    baseline: Optional["Outcome"] = None
    problem: str = ""  # why the baseline failed, "" when it passed
    limit: float = 0.0  # each mutant's time limit


@dataclass
class Candidate:
    """A mutant to report, and its result."""

    source: SourceFile
    mutant: Mutant
    data: bytes
    id: str = ""
    fingerprint: str = ""
    selection: Optional[Selection] = None
    status: Optional[str] = None  # None until it has a result
    killed_by: Optional[str] = None
    duration_ms: int = 0
    cached: bool = False

    @property
    def where(self) -> str:
        return f"{self.source.rel}:{self.mutant.line}"


@dataclass
class Outcome:
    """What one run of a selection's tests did."""

    duration: float
    timed_out: bool = False
    code: int = 0
    tests: int = 0
    failed: Optional[str] = None  # the first failing test's name
    broken: Optional[str] = None  # the first test module that did not import
    output: str = ""


def run_mutations(
    root: str,
    timeout: float,
    changed: dict[str, list[tuple[int, int]]],
    coverage: list[dict[str, Any]],
    max_mutants: int,
    jobs: int,
    min_timeout: float,
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

    lines = candidate_lines(changed, coverage)
    candidates = _candidates(project, model, lines, diagnostics)
    groups = _groups(candidates)
    chosen = sample(groups, max_mutants)
    total = len(candidates)
    _identify(chosen)
    progress = Progress(progress_file, len(chosen))

    def finish() -> dict[str, Any]:
        reported = [c for c in chosen if c.status is not None]
        left = len(chosen) - len(reported)
        if left:
            what = "1 mutant was" if left == 1 else f"{left} mutants were"
            diagnostics.append(f"the run stopped after {timeout:g}s: {what} not tested")
        reported.sort(key=lambda c: (c.source.rel, c.mutant.line, c.mutant.column, c.mutant.end_line,
                                     c.mutant.end_column, c.mutant.operator, c.mutant.mutated))
        mutants = [_document(c) for c in reported]
        counts = {status: sum(1 for c in reported if c.status == status)
                  for status in ("killed", "survived", "timeout", "error", "no_coverage")}
        summary = {
            "candidates": total, "sampled": total > max_mutants, "maxMutants": max_mutants,
            "killed": counts["killed"], "survived": counts["survived"], "timeout": counts["timeout"],
            "error": counts["error"], "noCoverage": counts["no_coverage"],
            "cached": sum(1 for c in reported if c.cached),
            "durationMs": int((time.monotonic() - started) * 1000),
        }
        log.info("mutation run done", candidates=total, mutants=len(mutants), killed=counts["killed"],
                 survived=counts["survived"], timeout=counts["timeout"], error=counts["error"],
                 cached=summary["cached"], elapsed_ms=summary["durationMs"])
        progress.current.clear()
        progress.phase = "mutating"
        progress.write(force=True)
        return protocol.mutation(ENGINE, mutants, summary, diagnostics)

    if not chosen:
        return finish()

    test_modules = [s for s in project.files if s.test and testrun._TEST_FILE.match(s.rel.rsplit("/", 1)[-1])]
    selections = _selections(project, model, test_modules, {c.source.module for c in chosen})
    for c in chosen:
        c.selection = selections[c.source.module]
        c.fingerprint = _fingerprint(c, project)
        kept = _load(cache_dir, c.fingerprint)
        if kept is not None:
            c.status, c.killed_by, c.duration_ms, c.cached = kept["status"], kept["killedBy"], kept["durationMs"], True
            progress.result(c)
    progress.write(force=True)
    pending = [c for c in chosen if c.status is None]
    if not pending:
        return finish()

    runner = _runner(project, offline, cache_dir, deadline, diagnostics)
    try:
        with ThreadPoolExecutor(max_workers=max(jobs, 1)) as pool:
            needed: list[Selection] = []
            for c in pending:
                if c.selection is not None and all(c.selection is not s for s in needed):
                    needed.append(c.selection)
            for selection, outcome in zip(needed, pool.map(lambda s: runner.run(s, None, None, deadline), needed)):
                _baseline(selection, outcome, min_timeout)
            for selection in needed:
                if selection.problem:
                    count = sum(1 for c in pending if c.selection is selection)
                    diagnostics.append(f"{_describe(selection)} fail on the unmodified code ({selection.problem}),"
                                       f" so their {count} mutants could not be tested")
            progress.phase = "mutating"
            progress.write(force=True)

            def test(c: Candidate) -> None:
                _test(c, runner, deadline, progress, cache_dir)

            list(pool.map(test, pending))
    finally:
        runner.close()
    return finish()


# Which mutants.


def candidate_lines(changed: dict[str, list[tuple[int, int]]], coverage: list[dict[str, Any]]) -> dict[str, set[int]]:
    """Rule 1: the added lines that a block with a count above 0 spans, by file."""
    covered: dict[str, set[int]] = {}
    for block in coverage:
        if block["count"] > 0 and block["file"] in changed:
            covered.setdefault(block["file"], set()).update(range(block["startLine"], block["endLine"] + 1))
    lines: dict[str, set[int]] = {}
    for rel, ranges in changed.items():
        ran = covered.get(rel)
        if not ran:
            continue
        added = {n for start, end in ranges for n in range(start, end + 1) if n in ran}
        if added:
            lines[rel] = added
    return lines


def _candidates(project: Project, model: Model, lines: dict[str, set[int]], diagnostics: list[str]) -> list[Candidate]:
    modules = {m.source.rel: m for m in model.modules.values()}
    problems = {d.get("file"): d for d in model.diagnostics if d.get("file")}
    found = []
    for rel in sorted(lines):
        source = project.by_rel.get(rel)
        if source is None or source.test:
            continue
        module: Optional[Module] = modules.get(rel)
        if module is None:
            problem = problems.get(rel)
            if problem is not None:
                at = f":{problem['line']}" if problem.get("line") else ""
                diagnostics.append(f"{rel}{at}: {problem['message']}, so it was not mutated")
            continue
        if module.generated:
            continue
        try:
            with open(source.path, "rb") as stream:
                data = stream.read()
            mutants = mutants_of(data, module.tree, model, source.module)
        except (OSError, SyntaxError, UnicodeDecodeError, RecursionError) as error:
            diagnostics.append(f"{rel}: cannot be mutated: {error}")
            continue
        found.extend(Candidate(source, m, data) for m in mutants if m.line in lines[rel])
    return found


def _groups(candidates: list[Candidate]) -> list[list[Candidate]]:
    """Rule 4's groups: one per function, one per file for code outside any,
    ordered by file then start line (a file's top-level group by its first
    mutant); each group's mutants in source order."""
    by_key: dict[tuple[str, str], list[Candidate]] = {}
    for c in candidates:
        f = c.mutant.function
        by_key.setdefault((c.source.rel, f.id if f is not None else ""), []).append(c)

    def start(group: list[Candidate]) -> tuple[str, int, int]:
        f = group[0].mutant.function
        return group[0].source.rel, f.start if f is not None else group[0].mutant.line, 0 if f is not None else 1

    groups = sorted(by_key.values(), key=start)
    for group in groups:
        group.sort(key=lambda c: (c.mutant.start, c.mutant.end))
    return groups


def sample(groups: list[list[Any]], cap: int) -> list[Any]:
    """Rule 4: one of each group in turn until cap are taken."""
    queues = [deque(group) for group in groups if group]
    taken: list[Any] = []
    while queues and len(taken) < cap:
        for queue in list(queues):
            if len(taken) >= cap:
                break
            taken.append(queue.popleft())
            if not queue:
                queues.remove(queue)
    return taken


def _identify(chosen: list[Candidate]) -> None:
    """Stable ids: the file, the function, the mutant and its position inside the function."""
    seen: set[str] = set()
    for c in chosen:
        digest = hashlib.sha256("\0".join(_identity(c)).encode("utf-8")).hexdigest()[:16]
        id, n = digest, 1
        while id in seen:
            n += 1
            id = f"{digest}-{n}"
        seen.add(id)
        c.id = id


def _identity(c: Candidate) -> list[str]:
    m, f = c.mutant, c.mutant.function
    base = f.start if f is not None else 0
    return [c.source.rel, f.id if f is not None else "", m.operator, m.original, m.mutated,
            str(m.line - base), str(m.column), str(m.end_line - base), str(m.end_column)]


# Which tests.


def _selections(
    project: Project, model: Model, test_modules: list[SourceFile], mutated: set[str]
) -> dict[str, Selection]:
    graph = {m.source.module: _imports(m.source, m.tree, project) for m in model.modules.values()}
    conftests = [s for s in project.files if s.rel.rsplit("/", 1)[-1] == "conftest.py"]
    reach: dict[str, set[str]] = {}
    for test in test_modules:
        start =[test.module] + [c.module for c in conftests if _applies(c.rel, test.rel)]
        seen: set[str] = set()
        queue = deque(start)
        while queue:
            name = queue.popleft()
            if name in seen:
                continue
            seen.add(name)
            queue.extend(graph.get(name, ()))
        reach[test.rel] = seen
    every = Selection(modules=list(test_modules), every=True,
                      files=sorted({s.rel for s in test_modules} | {c.rel for c in conftests}))
    by_key: dict[tuple[str, ...], Selection] = {}
    out: dict[str, Selection] = {}
    for name in sorted(mutated):
        modules = [t for t in test_modules if name in reach[t.rel]]
        if not modules:
            out[name] = every
            continue
        key = tuple(t.rel for t in modules)
        if key not in by_key:
            files = {t.rel for t in modules} | {c.rel for c in conftests if any(_applies(c.rel, t.rel) for t in modules)}
            by_key[key] = Selection(modules=modules, every=False, files=sorted(files))
        out[name] = by_key[key]
    for selection in list(by_key.values()) + [every]:
        selection.digest = _files_digest(project, selection.files)
    return out


def _applies(conftest: str, test: str) -> bool:
    """A conftest.py applies to the tests in its folder and below."""
    folder = conftest.rsplit("/", 1)[0] if "/" in conftest else ""
    return not folder or test.startswith(folder + "/")


def _imports(source: SourceFile, tree: ast.Module, project: Project) -> set[str]:
    """The modules of the tree a module imports, anywhere in it: `import a.b`
    (a and a.b), `from a.b import x` (a, a.b, and a.b.x when it is a module),
    relative imports resolved from the module's package."""
    out: set[str] = set()

    def add(name: str) -> None:
        parts = name.split(".")
        for n in range(1, len(parts) + 1):
            prefix = ".".join(parts[:n])
            if prefix in project.by_module:
                out.add(prefix)

    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                add(alias.name)
        elif isinstance(node, ast.ImportFrom):
            if node.level:
                parts = source.package.split(".") if source.package else []
                if node.level - 1 > len(parts):
                    continue
                parts = parts[: len(parts) - (node.level - 1)]
                base = ".".join(parts + ([node.module] if node.module else []))
            else:
                base = node.module or ""
            if base:
                add(base)
            for alias in node.names:
                if alias.name != "*":
                    add(f"{base}.{alias.name}" if base else alias.name)
    out.discard(source.module)
    return out


def _describe(selection: Selection) -> str:
    if selection.every:
        return "the tests"
    names = [s.module for s in selection.modules]
    shown = ", ".join(names[:3]) + (f" and {len(names) - 3} more" if len(names) > 3 else "")
    return f"the tests of {shown}"


# Fingerprints and the kept results.


def _files_digest(project: Project, files: list[str]) -> str:
    digest = hashlib.sha256()
    for rel in files:
        digest.update(rel.encode("utf-8") + b"\0")
        try:
            with open(os.path.join(project.root, rel), "rb") as stream:
                digest.update(hashlib.sha256(stream.read()).digest())
        except OSError:
            digest.update(b"missing")
    return digest.hexdigest()


def _fingerprint(c: Candidate, project: Project) -> str:
    """Rule 6: the function's source, the tests it runs, the engine, the mutant."""
    f = c.mutant.function
    if f is not None:
        text = c.data.split(b"\n")[f.start - 1 : f.end]
        body = b"\n".join(text)
    else:
        body = c.data
    assert c.selection is not None
    digest = hashlib.sha256()
    for part in [str(FORMAT), ENGINE["name"], ENGINE["version"], c.selection.digest] + _identity(c):
        digest.update(part.encode("utf-8") + b"\0")
    digest.update(hashlib.sha256(body).digest())
    return digest.hexdigest()


def _cache_path(cache_dir: str, fingerprint: str) -> str:
    return os.path.join(cache_dir, "mutation", fingerprint[:2], f"{fingerprint}.json")


def _load(cache_dir: str, fingerprint: str) -> Optional[dict[str, Any]]:
    if not cache_dir:
        return None
    try:
        with open(_cache_path(cache_dir, fingerprint), encoding="utf-8") as stream:
            kept = json.load(stream)
    except (OSError, ValueError):
        return None
    if (
        not isinstance(kept, dict) or kept.get("status") not in KEPT
        or not isinstance(kept.get("durationMs"), int) or not isinstance(kept.get("killedBy"), (str, type(None)))
    ):
        return None
    return kept


def _store(cache_dir: str, c: Candidate) -> None:
    """Keeps a result, written whole (a complete file renamed into place)."""
    if not cache_dir or c.status not in KEPT:
        return
    path = _cache_path(cache_dir, c.fingerprint)
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        handle, temporary = tempfile.mkstemp(dir=os.path.dirname(path), prefix=".result-")
        with os.fdopen(handle, "w", encoding="utf-8") as stream:
            json.dump({"status": c.status, "killedBy": c.killed_by, "durationMs": c.duration_ms}, stream)
        os.replace(temporary, path)
    except OSError:
        pass


# Running.


class Runner:
    """Runs a selection's tests in the project's interpreter, with or without a mutant."""

    def __init__(
        self, project: Project, python: str, env: dict[str, str], scratch: str, pytest: bool,
        options: list[str], pycache: str, cache_dir: str,
    ) -> None:
        self.project = project
        self.python = python
        self.env = env
        self.scratch = scratch
        self.pytest = pytest
        self.options = options  # pytest options the environment needs (--no-cov, -n 0)
        self.pycache = pycache
        self.cache_dir = cache_dir
        self.specs: dict[int, str] = {}
        self.lock = threading.Lock()

    def close(self) -> None:
        shutil.rmtree(self.scratch, ignore_errors=True)
        if self.cache_dir:
            testrun.drop_tree_bytecode(self.pycache, self.project.root)

    def command(self, selection: Selection, job: str) -> list[str]:
        if self.pytest:
            targets = [] if selection.every else [s.rel for s in selection.modules]
            return [self.python, "-m", "_difflens_mutant", "pytest", "-p", "_difflens_pytest",
                    "-o", f"cache_dir={os.path.join(job, 'pytest-cache')}", "-x", "-q", "--tb=short",
                    "--color=no"] + self.options + targets
        return [self.python, "-m", "_difflens_mutant", "_difflens_unittest", self.spec(selection)]

    def spec(self, selection: Selection) -> str:
        """The unittest fallback's spec of a selection, written once."""
        with self.lock:
            path = self.specs.get(id(selection))
            if path is None:
                modules = []
                for source in selection.modules:
                    base, name = testrun._import_name(source.path)
                    modules.append({"module": name, "sys_path": base, "path": source.rel, "abs": source.path,
                                    "names": None})
                path = os.path.join(self.scratch, f"unittest-{len(self.specs)}.json")
                with open(path, "w", encoding="utf-8") as stream:
                    json.dump({"modules": modules, "failfast": True}, stream)
                self.specs[id(selection)] = path
            return path

    def run(self, selection: Selection, target: Optional[str], mutated: Optional[bytes], deadline: float,
            limit: Optional[float] = None) -> Optional[Outcome]:
        """One run, stopped after limit seconds (the time left without one).
        None when the command's time runs out before the run ends."""
        left = deadline - time.monotonic()
        if left < 0.5:
            return None
        job = tempfile.mkdtemp(prefix="job-", dir=self.scratch)
        try:
            env = dict(self.env)
            events = os.path.join(job, "events.jsonl")
            os.makedirs(os.path.join(job, "tmp"))
            env["DIFFLENS_TEST_EVENTS"] = events
            env["TMPDIR"] = os.path.join(job, "tmp")
            env.pop("DIFFLENS_MUTANT_TARGET", None)
            env.pop("DIFFLENS_MUTANT_SOURCE", None)
            if target is not None and mutated is not None:
                path = os.path.join(job, "mutant.py")
                with open(path, "wb") as stream:
                    stream.write(mutated)
                env["DIFFLENS_MUTANT_TARGET"] = target
                env["DIFFLENS_MUTANT_SOURCE"] = path
            allowed = min(limit, left) if limit is not None else left
            began = time.monotonic()
            result = processes.run(self.command(selection, job), self.project.root, env, allowed)
            outcome = Outcome(duration=time.monotonic() - began, timed_out=result.timed_out, code=result.code,
                              output=testrun.keep_end((result.stdout + result.stderr).strip(), 4 << 10))
            if result.timed_out and (limit is None or allowed < limit):
                return None  # stopped by the command's deadline, not by its own limit
            _read_events(events, outcome)
            return outcome
        finally:
            shutil.rmtree(job, ignore_errors=True)


def _read_events(path: str, outcome: Outcome) -> None:
    try:
        with open(path, encoding="utf-8") as stream:
            lines = stream.read().splitlines()
    except OSError:
        return
    for line in lines:
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if not isinstance(event, dict):
            continue
        kind = event.get("e")
        if kind == "result":
            outcome.tests += 1
            if event.get("outcome") not in ("pass", "skip") and outcome.failed is None:
                nodeid = event.get("nodeid") or ""
                outcome.failed = nodeid.split("::", 1)[1] if "::" in nodeid else nodeid.rsplit("/", 1)[-1]
        elif kind == "collecterror" and outcome.broken is None:
            outcome.broken = event.get("nodeid") or event.get("path") or "a test module"


def _runner(project: Project, offline: bool, cache_dir: str, deadline: float, diagnostics: list[str]) -> Runner:
    root = project.root
    plan = environment.make_plan(root)
    interpreter = environment.find_interpreter(root, plan)
    if interpreter is None:
        raise MutationError("no Python interpreter for the project: no prepared environment, no virtual environment"
                            " in the tree, no python3 on PATH")
    info = environment.probe(interpreter, timeout=min(60.0, processes.remaining(deadline)))
    if info is None:
        raise MutationError(f"the project's interpreter {interpreter.python} does not run")
    if plan.kind != "none" and interpreter.source == "path":
        diagnostics.append(
            f"the project's dependencies were not prepared, so the tests ran with {interpreter.python}"
            f" (Python {info.get('python')}); some may fail to import"
        )
    pytest = environment.has_module(info, "pytest")
    if not pytest:
        diagnostics.append(
            f"pytest is not installed in the project's environment ({interpreter.python}), so only unittest suites ran"
        )
    options = []
    if environment.has_module(info, "pytest_cov"):
        options.append("--no-cov")
    if environment.has_module(info, "xdist"):
        options += ["-n", "0"]
    scratch = tempfile.mkdtemp(prefix="difflens-mutate-")
    pycache = os.path.join(cache_dir, "pycache") if cache_dir else os.path.join(scratch, "pycache")
    env = testrun._environment(project, interpreter, scratch, os.path.join(scratch, "events.jsonl"), offline, pycache)
    return Runner(project, interpreter.python, env, scratch, pytest, options, pycache, cache_dir)


def _baseline(selection: Selection, outcome: Optional[Outcome], min_timeout: float) -> None:
    """Records the unmutated run of a selection: its mutants' time limit, or why they cannot be tested."""
    selection.baseline = outcome
    if outcome is None:
        return  # the command ran out of time: its mutants are not tested
    if outcome.failed is not None:
        selection.problem = f"{outcome.failed} failed"
    elif outcome.broken is not None:
        selection.problem = f"{outcome.broken} did not import"
    elif outcome.tests == 0:
        selection.problem = "no test ran"
    elif outcome.code != 0:
        lines = [line for line in outcome.output.splitlines() if line.strip()]
        selection.problem = f"the runner exited with status {outcome.code}" + (f": {lines[-1].strip()}" if lines else "")
    selection.limit = max(min_timeout, 3 * outcome.duration)


def _test(c: Candidate, runner: Runner, deadline: float, progress: "Progress", cache_dir: str) -> None:
    selection = c.selection
    assert selection is not None
    if selection.baseline is None:
        return
    if selection.problem:
        c.status = "error"
        progress.result(c)
        return
    progress.start(c)
    try:
        outcome = runner.run(selection, c.source.path, mutated_source(c.data, c.mutant), deadline, selection.limit)
    finally:
        progress.stop(c)
    if outcome is None:
        return
    c.duration_ms = max(int(outcome.duration * 1000), 0)
    if outcome.timed_out:
        c.status = "timeout"
    elif outcome.failed is not None:
        c.status, c.killed_by = "killed", outcome.failed
    elif outcome.broken is not None or outcome.tests == 0 or outcome.code != 0:
        c.status = "error"  # it does not import, or the runner crashed
    else:
        c.status = "survived"
    _store(cache_dir, c)
    progress.result(c)


# Reporting.


class Progress:
    """The --progress file ($defs/progress), replaced whole on each change."""

    def __init__(self, file: str, total: int) -> None:
        self.file = file
        self.total = total
        self.phase = "baseline"
        self.counts = {"killed": 0, "survived": 0, "timeout": 0, "error": 0}
        self.done = 0
        self.current: list[str] = []
        self.lock = threading.Lock()
        self.written = 0.0

    def result(self, c: Candidate) -> None:
        with self.lock:
            self.done += 1
            status = c.status if c.status in self.counts else "error"
            self.counts[status] += 1
        self.write()

    def start(self, c: Candidate) -> None:
        with self.lock:
            self.current.append(c.where)
        self.write()

    def stop(self, c: Candidate) -> None:
        with self.lock:
            if c.where in self.current:
                self.current.remove(c.where)

    def write(self, force: bool = False) -> None:
        if not self.file:
            return
        from difflens_analyzer.cli import write_progress

        with self.lock:
            now = time.monotonic()
            if not force and now - self.written < PROGRESS_INTERVAL:
                return
            self.written = now
            document = protocol.mutation_progress(
                self.phase, self.total, self.done, self.counts["killed"], self.counts["survived"],
                self.counts["timeout"], self.counts["error"], self.current[0] if self.current else None,
            )
            write_progress(self.file, document)


def _document(c: Candidate) -> dict[str, Any]:
    m = c.mutant
    return {
        "id": c.id,
        "file": c.source.rel,
        "line": m.line,
        "column": m.column,
        "endLine": m.end_line,
        "endColumn": m.end_column,
        "original": m.original,
        "mutated": m.mutated,
        "operator": m.operator,
        "description": m.description,
        "function": m.function.id if m.function is not None else None,
        "status": c.status,
        "killedBy": c.killed_by if c.status == "killed" else None,
        "durationMs": c.duration_ms,
        "cached": c.cached,
    }
