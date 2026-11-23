"""The documents of the analyzer protocol, version 1, as plain dicts.

The schemas are in analyzers/protocol/ of the DiffLens repository:
analysis.schema.json (analyze), checks.schema.json (check) and
tests.schema.json (prepare, test, the --selection file and the --progress
file), dupes.schema.json (dupes and its --changed file), mutation.schema.json
(mutate, its --coverage and --progress files). The core rejects a
document with a field the schema does not list.
"""

from __future__ import annotations

from typing import Any

from difflens_analyzer import LANGUAGE, NAME, VERSION

PROTOCOL = 1


def identity() -> dict[str, str]:
    return {"name": NAME, "version": VERSION}


def analysis(
    functions: list[dict[str, Any]] | None = None,
    calls: list[dict[str, Any]] | None = None,
    externals: list[dict[str, Any]] | None = None,
    diagnostics: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    """What `analyze` prints (analysis.schema.json)."""
    return {
        "protocol": PROTOCOL,
        "language": LANGUAGE,
        "analyzer": identity(),
        "functions": functions or [],
        "calls": calls or [],
        "externals": externals or [],
        "diagnostics": diagnostics or [],
    }


def diagnostic(level: str, message: str, file: str | None = None, line: int | None = None) -> dict[str, Any]:
    """A problem that did not stop the analysis; level is "warning" or "error"."""
    out: dict[str, Any] = {"level": level, "message": message}
    if file:
        out["file"] = file
        if line is not None and line >= 1:
            out["line"] = line
    return out


def prepare(writable: list[str] | None = None) -> dict[str, Any]:
    """What `prepare` prints (tests.schema.json, $defs/prepare)."""
    return {"protocol": PROTOCOL, "writable": writable or []}


def test_run(
    race: bool = False,
    tests: list[dict[str, Any]] | None = None,
    packages: list[dict[str, Any]] | None = None,
    coverage: dict[str, Any] | None = None,
    races: list[dict[str, Any]] | None = None,
    diagnostics: list[str] | None = None,
) -> dict[str, Any]:
    """What `test` prints (tests.schema.json)."""
    return {
        "protocol": PROTOCOL,
        "language": LANGUAGE,
        "runner": identity(),
        "race": race,
        "tests": tests or [],
        "packages": packages or [],
        "coverage": coverage,
        "races": races or [],
        "diagnostics": diagnostics or [],
    }


def progress(
    packages_done: int = 0,
    packages_total: int = 0,
    tests: int = 0,
    failed: int = 0,
    current: str | None = None,
) -> dict[str, Any]:
    """The --progress file (tests.schema.json, $defs/progress)."""
    return {
        "protocol": PROTOCOL,
        "packagesDone": packages_done,
        "packagesTotal": packages_total,
        "tests": tests,
        "failed": failed,
        "current": current,
    }


def check_run(
    checks: list[dict[str, Any]] | None = None, diagnostics: list[str] | None = None
) -> dict[str, Any]:
    """What `check` prints (checks.schema.json)."""
    return {
        "protocol": PROTOCOL,
        "language": LANGUAGE,
        "runner": identity(),
        "checks": checks or [],
        "diagnostics": diagnostics or [],
    }


def check(
    tool: str, status: str, message: str | None = None, findings: list[dict[str, Any]] | None = None
) -> dict[str, Any]:
    """One checker; status is "clean", "findings", "unavailable" or "failed"."""
    return {"tool": tool, "status": status, "message": message, "findings": findings or []}


def mutation(
    engine: dict[str, str],
    mutants: list[dict[str, Any]],
    summary: dict[str, Any],
    diagnostics: list[str] | None = None,
) -> dict[str, Any]:
    """What `mutate` prints (mutation.schema.json)."""
    return {
        "protocol": PROTOCOL,
        "language": LANGUAGE,
        "runner": identity(),
        "engine": engine,
        "mutants": mutants,
        "summary": summary,
        "diagnostics": diagnostics or [],
    }


def mutation_progress(
    phase: str,
    total: int = 0,
    done: int = 0,
    killed: int = 0,
    survived: int = 0,
    timeout: int = 0,
    error: int = 0,
    current: str | None = None,
) -> dict[str, Any]:
    """The --progress file of `mutate` (mutation.schema.json, $defs/progress);
    phase is "baseline" or "mutating", error counts no_coverage too."""
    return {
        "protocol": PROTOCOL,
        "phase": phase,
        "total": total,
        "done": done,
        "killed": killed,
        "survived": survived,
        "timeout": timeout,
        "error": error,
        "current": current,
    }


def dupes(
    min_tokens: int,
    min_lines: int,
    duplicates: list[dict[str, Any]] | None = None,
    diagnostics: list[str] | None = None,
) -> dict[str, Any]:
    """What `dupes` prints (dupes.schema.json)."""
    return {
        "protocol": PROTOCOL,
        "language": LANGUAGE,
        "runner": identity(),
        "minTokens": min_tokens,
        "minLines": min_lines,
        "duplicates": duplicates or [],
        "diagnostics": diagnostics or [],
    }
