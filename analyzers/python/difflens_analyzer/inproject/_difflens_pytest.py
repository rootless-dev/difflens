"""A pytest plugin DiffLens loads with `-p _difflens_pytest` in the project's
own interpreter. It streams what pytest collects and how each test ends, as
JSON lines, to the file named by DIFFLENS_TEST_EVENTS. It writes nothing
else and changes nothing in the run. Python 3.7 and pytest 6 or later.
"""

import inspect
import json
import os

_PATH = os.environ.get("DIFFLENS_TEST_EVENTS", "")
_MAX_OUTPUT = 64 * 1024
_stream = None
_paths = {}
_definitions = {}
_tests = {}


def _emit(event):
    global _stream
    if not _PATH:
        return
    if _stream is None:
        _stream = open(_PATH, "a", encoding="utf-8")
    _stream.write(json.dumps(event) + "\n")
    _stream.flush()


def _item_path(item):
    path = getattr(item, "path", None) or getattr(item, "fspath", None)
    return str(path) if path is not None else None


def _definition(item):
    """Where the test function is defined: file and first line (decorators included)."""
    obj = getattr(item, "obj", None) or getattr(item, "function", None)
    try:
        obj = inspect.unwrap(obj)
        code = obj.__code__
        return [code.co_filename, code.co_firstlineno]
    except Exception:
        return [None, None]


def pytest_collectreport(report):
    if report.failed:
        path = getattr(report, "fspath", None)
        _emit({"e": "collecterror", "nodeid": report.nodeid, "path": str(path) if path else None,
               "output": _limit(report.longreprtext)})


def pytest_collection_finish(session):
    items = []
    for item in session.items:
        path = _item_path(item)
        _paths[item.nodeid] = path
        _definitions[item.nodeid] = _definition(item)
        items.append([item.nodeid, path])
    _emit({"e": "collected", "items": items})


def pytest_runtest_logstart(nodeid, location):
    _emit({"e": "start", "nodeid": nodeid})


def pytest_runtest_logreport(report):
    state = _tests.setdefault(report.nodeid, {"outcome": "pass", "duration": 0.0, "sections": [], "failures": []})
    state["duration"] += getattr(report, "duration", 0.0) or 0.0
    if report.failed:
        state["outcome"] = "fail"
        state["failures"].append(report.longreprtext)
    elif report.skipped and state["outcome"] != "fail":
        state["outcome"] = "skip"
    for title, content in getattr(report, "sections", []) or []:
        entry = (title, content)
        if entry not in state["sections"]:
            state["sections"].append(entry)


def pytest_runtest_logfinish(nodeid, location):
    state = _tests.pop(nodeid, None)
    if state is None:
        return
    output = None
    if state["outcome"] == "fail":
        parts = ["----- %s -----\n%s" % (title, content.rstrip("\n")) for title, content in state["sections"]]
        parts.extend(text.rstrip("\n") for text in state["failures"])
        output = _limit("\n".join(parts) + "\n")
    definition = _definitions.get(nodeid) or [None, None]
    _emit({
        "e": "result", "nodeid": nodeid, "path": _paths.get(nodeid) or (location[0] if location else None),
        "outcome": state["outcome"], "duration": state["duration"], "output": output,
        "def_file": definition[0], "def_line": definition[1],
    })


def pytest_sessionfinish(session, exitstatus):
    _emit({"e": "done", "status": int(exitstatus)})


def _limit(text):
    text = text or ""
    if len(text.encode("utf-8")) <= _MAX_OUTPUT:
        return text
    return text.encode("utf-8")[-_MAX_OUTPUT:].decode("utf-8", "ignore")
