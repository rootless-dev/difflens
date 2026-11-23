"""Runs unittest suites when pytest is not installed, in the project's own
interpreter: `python -m _difflens_unittest <spec.json>`. The spec names the
tree and the test modules to load (all their tests, or some of them, by
"Class.method"). Events go to DIFFLENS_TEST_EVENTS as the pytest plugin
writes them, with pytest-style node ids ("tests/test_x.py::Case::test_y").
With "failfast" true in the spec, the run stops at the first failure
(mutate's runs, like pytest -x).
"""

import inspect
import json
import os
import sys
import time
import traceback
import unittest

_PATH = os.environ.get("DIFFLENS_TEST_EVENTS", "")
_MAX_OUTPUT = 64 * 1024


class _Events:
    def __init__(self, path):
        self.stream = open(path, "a", encoding="utf-8") if path else None

    def emit(self, event):
        if self.stream is not None:
            self.stream.write(json.dumps(event) + "\n")
            self.stream.flush()


class _Result(unittest.TestResult):
    def __init__(self, events, ids):
        super().__init__()
        self.events = events
        self.ids = ids
        self.started = {}
        self.outcomes = {}
        self.buffer = True

    def _key(self, test):
        return self.ids.get(id(test)) or (None, test.id())

    def startTest(self, test):
        super().startTest(test)
        self.started[id(test)] = time.monotonic()
        self.outcomes[id(test)] = ("pass", None)
        self.events.emit({"e": "start", "nodeid": self._key(test)[1]})

    def _set(self, test, outcome, err=None):
        text = None
        if err is not None:
            # With buffer on, this includes what the test printed.
            text = self._exc_info_to_string(err, test)
        self.outcomes[id(test)] = (outcome, text)

    def addError(self, test, err):
        super().addError(test, err)
        self._set(test, "fail", err)

    def addFailure(self, test, err):
        super().addFailure(test, err)
        self._set(test, "fail", err)

    def addSkip(self, test, reason):
        super().addSkip(test, reason)
        self._set(test, "skip")

    def addExpectedFailure(self, test, err):
        super().addExpectedFailure(test, err)
        self._set(test, "skip")

    def addUnexpectedSuccess(self, test):
        super().addUnexpectedSuccess(test)
        self.outcomes[id(test)] = ("fail", "unexpected success\n")

    def addSubTest(self, test, subtest, err):
        super().addSubTest(test, subtest, err)
        if err is not None:
            self._set(test, "fail", err)

    def stopTest(self, test):
        super().stopTest(test)
        outcome, output = self.outcomes.pop(id(test), ("pass", None))
        path, nodeid = self._key(test)
        definition = _definition(test)
        if output is not None:
            output = output.encode("utf-8")[-_MAX_OUTPUT:].decode("utf-8", "ignore")
        self.events.emit({
            "e": "result", "nodeid": nodeid, "path": path, "outcome": outcome,
            "duration": time.monotonic() - self.started.pop(id(test), time.monotonic()),
            "output": output, "def_file": definition[0], "def_line": definition[1],
        })


def _definition(test):
    method = getattr(test, getattr(test, "_testMethodName", ""), None)
    try:
        code = inspect.unwrap(method).__code__
        return [code.co_filename, code.co_firstlineno]
    except Exception:
        return [None, None]


def _flatten(suite):
    for test in suite:
        if isinstance(test, unittest.TestSuite):
            for inner in _flatten(test):
                yield inner
        else:
            yield test


def main(argv):
    with open(argv[0], encoding="utf-8") as stream:
        spec = json.load(stream)
    events = _Events(_PATH)
    loader = unittest.TestLoader()
    suite = unittest.TestSuite()
    ids = {}
    items = []
    for module in spec["modules"]:
        rel = module["path"]
        if module.get("sys_path") and module["sys_path"] not in sys.path:
            sys.path.insert(0, module["sys_path"])
        try:
            if module.get("names"):
                loaded = loader.loadTestsFromNames(module["names"], __import__(module["module"], fromlist=["_"]))
            else:
                loaded = loader.loadTestsFromName(module["module"])
        except Exception:
            events.emit({"e": "collecterror", "nodeid": rel, "path": module["abs"], "output": traceback.format_exc()})
            continue
        for test in _flatten(loaded):
            if type(test).__name__ == "_FailedTest":
                error = getattr(test, "_exception", None)
                text = "".join(traceback.format_exception(type(error), error, error.__traceback__)) if error else str(test)
                events.emit({"e": "collecterror", "nodeid": rel, "path": module["abs"], "output": text})
                continue
            nodeid = "%s::%s::%s" % (rel, type(test).__name__, getattr(test, "_testMethodName", test.id()))
            ids[id(test)] = (module["abs"], nodeid)
            items.append([nodeid, module["abs"]])
            suite.addTest(test)
    events.emit({"e": "collected", "items": items})
    result = _Result(events, ids)
    result.failfast = bool(spec.get("failfast"))
    suite.run(result)
    status = 0 if result.wasSuccessful() else 1
    events.emit({"e": "done", "status": status})
    return status


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
