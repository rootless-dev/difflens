"""The command line every DiffLens analyzer implements (see README.md):

    difflens-analyzer-python analyze --dir <source tree> [--timeout 120s]
    difflens-analyzer-python prepare --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--tools <folder>]
    difflens-analyzer-python test --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--offline]
        [--no-race] [--no-cover] [--selection <file>] [--progress <file>]
    difflens-analyzer-python check --dir <source tree> [--timeout 5m] [--cache-dir <folder>]
    difflens-analyzer-python dupes --dir <source tree> --changed <file> [--timeout 120s]
        [--min-tokens 50] [--min-lines 6]
    difflens-analyzer-python mutate --dir <source tree> --changed <file> --coverage <file> [--timeout 10m]
        [--cache-dir <folder>] [--tools <folder>] [--max-mutants 200] [--jobs <n>]
        [--min-mutant-timeout 10s] [--progress <file>] [--offline]
    difflens-analyzer-python --version

Each command's work lives in its own module (analysis, prepare, testrun,
checks, dupes, mutation), imported when the command runs; this module keeps
the parsing, the exit codes and the single document on stdout.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import tempfile
from typing import Any, Callable, TextIO

from difflens_analyzer import NAME, VERSION, protocol
from difflens_analyzer.log import Logger

EXIT_OK = 0
# The command itself failed; stderr says why.
EXIT_FAILED = 1
# Bad command line, or a selection or changed-lines file that cannot be read.
EXIT_USAGE = 2

USAGE = f"""usage: {NAME} analyze --dir <source tree> [--timeout 120s]
       {NAME} prepare --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--tools <folder>]
       {NAME} test --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--offline] [--no-race]
           [--no-cover] [--selection <file>] [--progress <file>]
       {NAME} check --dir <source tree> [--timeout 5m] [--cache-dir <folder>]
       {NAME} dupes --dir <source tree> --changed <file> [--timeout 120s]
           [--min-tokens 50] [--min-lines 6]
       {NAME} mutate --dir <source tree> --changed <file> --coverage <file> [--timeout 10m]
           [--cache-dir <folder>] [--tools <folder>] [--max-mutants 200] [--jobs <n>]
           [--min-mutant-timeout 10s] [--progress <file>] [--offline]
       {NAME} --version
"""

DEFAULT_TIMEOUTS = {"analyze": "120s", "prepare": "10m", "test": "10m", "check": "5m", "dupes": "120s", "mutate": "10m"}
# The commands given the core's per-project cache folder (--cache-dir).
CACHED = ("prepare", "test", "check", "mutate")


class UsageError(Exception):
    """A command line the analyzer does not accept: exit 2."""


class _Parser(argparse.ArgumentParser):
    def error(self, message: str) -> None:  # type: ignore[override]
        raise UsageError(f"{self.prog}: {message}")


_DURATION_PART = re.compile(r"(\d+(?:\.\d*)?|\.\d+)(ns|us|µs|ms|s|m|h)")
_UNIT_SECONDS = {"ns": 1e-9, "us": 1e-6, "µs": 1e-6, "ms": 1e-3, "s": 1.0, "m": 60.0, "h": 3600.0}


def parse_duration(text: str) -> float:
    """Seconds in a Go duration such as "120s" or "1m30s", the form --timeout takes."""
    position, seconds = 0, 0.0
    while position < len(text):
        part = _DURATION_PART.match(text, position)
        if part is None:
            raise UsageError(f"invalid duration {text!r}")
        seconds += float(part.group(1)) * _UNIT_SECONDS[part.group(2)]
        position = part.end()
    if not text:
        raise UsageError("invalid duration ''")
    return seconds


def _parser(command: str) -> _Parser:
    parser = _Parser(prog=command, add_help=False, allow_abbrev=False)
    parser.add_argument("--dir", default="")
    parser.add_argument("--timeout", default=DEFAULT_TIMEOUTS[command])
    if command in CACHED:
        parser.add_argument("--cache-dir", default="")
    if command == "test":
        parser.add_argument("--offline", action="store_true")
        parser.add_argument("--no-race", action="store_true")
        parser.add_argument("--no-cover", action="store_true")
        parser.add_argument("--selection", default="")
        parser.add_argument("--progress", default="")
    if command in ("prepare", "mutate"):
        # Where the mutation engine is installed; Python needs none (mutator.py).
        parser.add_argument("--tools", default="")
    if command in ("dupes", "mutate"):
        parser.add_argument("--changed", default="")
    if command == "dupes":
        parser.add_argument("--min-tokens", type=int, default=50)
        parser.add_argument("--min-lines", type=int, default=6)
    if command == "mutate":
        parser.add_argument("--coverage", default="")
        parser.add_argument("--max-mutants", type=int, default=200)
        parser.add_argument("--jobs", type=int, default=max((os.cpu_count() or 2) // 2, 1))
        parser.add_argument("--min-mutant-timeout", default="10s")
        parser.add_argument("--progress", default="")
        parser.add_argument("--offline", action="store_true")
    return parser


def _tree(directory: str) -> str:
    """The source tree to read; a missing one fails the command (exit 1)."""
    if not os.path.isdir(directory):
        raise FileNotFoundError(f"{directory} is not a directory")
    return os.path.abspath(directory)


def cache_dir(options: argparse.Namespace) -> str:
    """The --cache-dir folder, absolute, or "" without the flag. It is the
    core's, kept between runs of one project, outside the tree."""
    folder = getattr(options, "cache_dir", "")
    return os.path.abspath(folder) if folder else ""


def analyze(options: argparse.Namespace, log: Logger) -> dict[str, Any]:
    """Functions and calls of the tree (analysis.schema.json). Must not write inside --dir."""
    from difflens_analyzer.analysis import analyze_tree

    return analyze_tree(_tree(options.dir), parse_duration(options.timeout), log)


def prepare(options: argparse.Namespace, log: Logger) -> dict[str, Any]:
    """Fetches what the tests need, so the test step can run offline; never runs project code.
    --cache-dir is accepted and not used: the environment is shared by every
    project with the same dependency files, so it lives in the analyzer's own cache.
    --tools is accepted and not used: mutate's engine is the analyzer itself."""
    from difflens_analyzer.prepare import prepare_tree

    return protocol.prepare(writable=prepare_tree(_tree(options.dir), parse_duration(options.timeout), log))


def run_tests(options: argparse.Namespace, log: Logger) -> dict[str, Any]:
    """Runs the project's tests once (tests.schema.json). Failing tests are results, not a failure."""
    from difflens_analyzer.testrun import run_tests as run

    tree = _tree(options.dir)
    selection = read_selection(options.selection)
    return run(
        tree,
        parse_duration(options.timeout),
        cover=not options.no_cover,
        selection=selection,
        progress_file=options.progress,
        offline=options.offline,
        log=log,
        cache_dir=cache_dir(options),
    )


def check(options: argparse.Namespace, log: Logger) -> dict[str, Any]:
    """Static checkers on the tree (checks.schema.json); they read the code and never run it."""
    from difflens_analyzer.checks import run_checks

    return run_checks(_tree(options.dir), parse_duration(options.timeout), log, cache_dir=cache_dir(options))


def dupes(options: argparse.Namespace, log: Logger) -> dict[str, Any]:
    """Duplicated code the change introduces (dupes.schema.json); reads the code only."""
    from difflens_analyzer.dupes import find_duplicates

    if not options.changed:
        raise UsageError("dupes: --changed is required")
    if options.min_tokens < 1 or options.min_lines < 1:
        raise UsageError("dupes: --min-tokens and --min-lines must be at least 1")
    tree = _tree(options.dir)
    changed, exclude = read_changed(options.changed)
    return find_duplicates(
        tree, changed, options.min_tokens, options.min_lines, parse_duration(options.timeout), log, exclude=exclude
    )


def mutate(options: argparse.Namespace, log: Logger) -> dict[str, Any]:
    """Mutants of the covered lines the change adds, and whether the tests catch them
    (mutation.schema.json). Surviving mutants are results, not a failure."""
    from difflens_analyzer.mutation import run_mutations

    if not options.changed or not options.coverage:
        raise UsageError("mutate: --changed and --coverage are required")
    if options.max_mutants < 1 or options.jobs < 1:
        raise UsageError("mutate: --max-mutants and --jobs must be at least 1")
    min_timeout = parse_duration(options.min_mutant_timeout)
    if min_timeout <= 0:
        raise UsageError("mutate: --min-mutant-timeout must be positive")
    tree = _tree(options.dir)
    changed, _ = read_changed(options.changed, "mutate")  # exclude is dupes' only
    coverage = read_coverage(options.coverage)
    return run_mutations(
        tree,
        parse_duration(options.timeout),
        changed,
        coverage,
        max_mutants=options.max_mutants,
        jobs=options.jobs,
        min_timeout=min_timeout,
        progress_file=options.progress,
        offline=options.offline,
        log=log,
        cache_dir=cache_dir(options),
    )


def read_changed(
    file: str, command: str = "dupes"
) -> tuple[dict[str, list[tuple[int, int]]], dict[str, list[str]] | None]:
    """The --changed file (dupes.schema.json, $defs/changed): the added line ranges by path,
    and its exclude ({"dirs": [...], "extensions": [...]}, None when absent)."""
    try:
        with open(file, encoding="utf-8") as stream:
            document = json.load(stream)
    except (OSError, ValueError) as error:
        raise UsageError(f"{command}: cannot read the changed lines: {error}") from error
    problem = _changed_problem(document)
    if problem:
        raise UsageError(f"{command}: cannot read the changed lines: {problem}")
    changed: dict[str, list[tuple[int, int]]] = {}
    for entry in document["files"]:
        changed.setdefault(entry["path"], []).extend((start, end) for start, end in entry["added"])
    return changed, document.get("exclude")


def read_coverage(file: str) -> list[dict[str, Any]]:
    """The blocks of mutate's --coverage file (mutation.schema.json, $defs/coverage)."""
    try:
        with open(file, encoding="utf-8") as stream:
            document = json.load(stream)
    except (OSError, ValueError) as error:
        raise UsageError(f"mutate: cannot read the coverage: {error}") from error
    problem = _fields(document, {"protocol", "blocks"}, "$")
    if not problem and (not _is_int(document["protocol"]) or document["protocol"] != protocol.PROTOCOL):
        problem = f"$.protocol is {document['protocol']!r}, not {protocol.PROTOCOL}"
    if not problem and not isinstance(document["blocks"], list):
        problem = "$.blocks is not an array"
    for index, block in enumerate(document["blocks"] if not problem else []):
        where = f"$.blocks[{index}]"
        problem = _fields(block, {"file", "startLine", "endLine", "statements", "count"}, where)
        if not problem and not isinstance(block["file"], str):
            problem = f"{where}.file is not a string"
        for name, least in (("startLine", 1), ("endLine", 1), ("statements", 0), ("count", 0)):
            if not problem and not (_is_int(block[name]) and block[name] >= least):
                problem = f"{where}.{name} is not an integer of at least {least}"
        if problem:
            break
    if problem:
        raise UsageError(f"mutate: cannot read the coverage: {problem}")
    return document["blocks"]


def _fields(value: Any, required: set[str], where: str) -> str:
    """What makes value not an object with exactly the required keys, "" when nothing does."""
    if not isinstance(value, dict):
        return f"{where} is not an object"
    missing, unknown = sorted(required - value.keys()), sorted(value.keys() - required)
    if missing:
        return f"{where}.{missing[0]} is missing"
    if unknown:
        return f"{where}.{unknown[0]} is not allowed"
    return ""


def _is_int(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _changed_problem(document: Any) -> str:
    """What makes document not a $defs/changed object, "" when nothing does."""
    if isinstance(document, dict) and "exclude" in document:
        problem = _exclude_problem(document["exclude"])
        if problem:
            return problem
        document = {key: value for key, value in document.items() if key != "exclude"}
    problem = _fields(document, {"protocol", "files"}, "$")
    if problem:
        return problem
    if not _is_int(document["protocol"]) or document["protocol"] != protocol.PROTOCOL:
        return f"$.protocol is {document['protocol']!r}, not {protocol.PROTOCOL}"
    if not isinstance(document["files"], list):
        return "$.files is not an array"
    for index, entry in enumerate(document["files"]):
        where = f"$.files[{index}]"
        problem = _fields(entry, {"path", "added"}, where)
        if problem:
            return problem
        if not isinstance(entry["path"], str):
            return f"{where}.path is not a string"
        if not isinstance(entry["added"], list):
            return f"{where}.added is not an array"
        for at, lines in enumerate(entry["added"]):
            if not (isinstance(lines, list) and len(lines) == 2 and all(_is_int(n) and n >= 1 for n in lines)):
                return f"{where}.added[{at}] is not a [start, end] pair of lines"
    return ""


def _exclude_problem(exclude: Any) -> str:
    """What makes exclude not the exclude of a $defs/changed object, "" when nothing does."""
    problem = _fields(exclude, {"dirs", "extensions"}, "$.exclude")
    if problem:
        return problem
    for name in ("dirs", "extensions"):
        if not (isinstance(exclude[name], list) and all(isinstance(item, str) for item in exclude[name])):
            return f"$.exclude.{name} is not an array of strings"
    return ""


def read_selection(file: str) -> dict[str, Any] | None:
    """The --selection file (tests.schema.json, $defs/selection); "" selects everything."""
    if not file:
        return None
    try:
        with open(file, encoding="utf-8") as stream:
            selection = json.load(stream)
    except (OSError, ValueError) as error:
        raise UsageError(f"test: cannot read the selection: {error}") from error
    if not isinstance(selection, dict) or not isinstance(selection.get("packages"), list):
        raise UsageError("test: cannot read the selection: packages is missing")
    return selection


def write_progress(file: str, progress: dict[str, Any]) -> None:
    """Replaces the --progress file whole, so the core never reads half of it."""
    folder = os.path.dirname(os.path.abspath(file))
    handle, temporary = tempfile.mkstemp(dir=folder, prefix=".progress-")
    with os.fdopen(handle, "w", encoding="utf-8") as stream:
        json.dump(progress, stream)
    os.replace(temporary, file)


COMMANDS: dict[str, Callable[[argparse.Namespace, Logger], dict[str, Any]]] = {
    "analyze": analyze,
    "prepare": prepare,
    "test": run_tests,
    "check": check,
    "dupes": dupes,
    "mutate": mutate,
}


def main(args: list[str], stdout: TextIO, stderr: TextIO) -> int:
    if args and args[0] in ("--version", "-version", "version"):
        stdout.write(f"{NAME} {VERSION}\n")
        return EXIT_OK
    if not args or args[0] not in COMMANDS:
        stderr.write(USAGE)
        return EXIT_USAGE
    command = args[0]
    log = Logger(stderr, NAME, os.environ.get("DIFFLENS_LOG_LEVEL", ""))
    try:
        options = _parser(command).parse_args(args[1:])
        if not options.dir or parse_duration(options.timeout) <= 0:
            raise UsageError(
                f"{command}: --dir is required, --timeout must be positive"
                " and no positional arguments are accepted"
            )
    except UsageError as error:
        stderr.write(f"{error}\n")
        return EXIT_USAGE
    try:
        document = COMMANDS[command](options, log)
    except UsageError as error:
        stderr.write(f"{error}\n")
        return EXIT_USAGE
    except Exception as error:  # noqa: BLE001 - any failure is the command's failure
        log.error(f"{command} failed", dir=options.dir, error=str(error), type=type(error).__name__)
        # The core shows the last lines of stderr: end with the reason, as text.
        stderr.write(f"{error}\n")
        stderr.flush()
        return EXIT_FAILED
    stdout.write(json.dumps(document, ensure_ascii=False, separators=(",", ":")) + "\n")
    stdout.flush()
    return EXIT_OK
