import json
import os
import tempfile
import unittest

from difflens_analyzer import testrun
from difflens_analyzer.analysis import build_model
from difflens_analyzer.project import discover
from tests.helpers import Tree, run, schema, validate

TESTS = schema("tests.schema.json")
PROGRESS = schema("tests.schema.json", "progress")

SOURCES = {
    "pyproject.toml": "[tool.pytest.ini_options]\n",
    "calc.py": """
        def add(a, b):
            return a + b


        @staticmethod
        def unused(x):
            y = x * 2
            return y
    """,
    "gen_pb2.py": "X = 1\n",
    "tests/test_calc.py": """
        import unittest

        from calc import add


        class AddCase(unittest.TestCase):
            def test_pass(self):
                self.assertEqual(add(1, 2), 3)

            def test_fail(self):
                print("about to fail")
                self.assertEqual(add(1, 2), 4)
    """,
    "tests/test_broken.py": "import does_not_exist\n",
}


class Events(unittest.TestCase):
    """The session turns the plugin's events into the protocol's lists."""

    def session(self, tree, progress=""):
        project = discover(tree.root)
        return testrun.Session(project, build_model(project), progress, expected=2), project

    def setUp(self):
        self.interval = testrun.PROGRESS_INTERVAL
        testrun.PROGRESS_INTERVAL = 0  # write on every event

    def tearDown(self):
        testrun.PROGRESS_INTERVAL = self.interval

    def test_results_packages_and_progress(self):
        with Tree(SOURCES) as tree:
            progress = tree.path("progress.json")
            session, project = self.session(tree, progress)
            test_file = tree.path("tests/test_calc.py")
            for event in (
                {"e": "collecterror", "nodeid": "tests/test_broken.py", "path": tree.path("tests/test_broken.py"),
                 "output": "ModuleNotFoundError: No module named 'does_not_exist'"},
                {"e": "collected", "items": [["tests/test_calc.py::AddCase::test_pass", test_file],
                                             ["tests/test_calc.py::AddCase::test_fail", test_file]]},
                {"e": "start", "nodeid": "tests/test_calc.py::AddCase::test_pass"},
                {"e": "result", "nodeid": "tests/test_calc.py::AddCase::test_pass", "path": test_file,
                 "outcome": "pass", "duration": 0.002, "output": None, "def_file": test_file, "def_line": 7},
            ):
                session.handle(event)
            with open(progress, encoding="utf-8") as stream:
                written = json.load(stream)
            self.assertEqual(validate(written, PROGRESS), [])
            self.assertEqual((written["packagesDone"], written["packagesTotal"], written["tests"]), (1, 2, 1))
            self.assertEqual(written["current"], "tests.test_calc")
            session.handle({"e": "result", "nodeid": "tests/test_calc.py::AddCase::test_fail", "path": test_file,
                            "outcome": "fail", "duration": 0.001, "output": "E   AssertionError: 3 != 4\n",
                            "def_file": None, "def_line": None})
            packages = {p["container"]: p for p in testrun._packages(session, project, full=True)}
        passed, failed = session.tests
        self.assertEqual(passed["function"], "tests.test_calc.AddCase.test_pass")
        self.assertEqual((passed["file"], passed["line"]), ("tests/test_calc.py", 7))
        self.assertEqual(failed["function"], "tests.test_calc.AddCase.test_fail")  # found by its name
        self.assertEqual(failed["output"], "E   AssertionError: 3 != 4\n")
        self.assertEqual(packages["tests.test_broken"]["outcome"], "build_failed")
        self.assertIn("does_not_exist", packages["tests.test_broken"]["output"])
        self.assertEqual((packages["tests.test_calc"]["outcome"], packages["tests.test_calc"]["output"]), ("fail", None))
        self.assertEqual(packages["calc"]["outcome"], "no_tests")
        self.assertEqual(passed["hierarchy"], ["AddCase::test_pass"])
        self.assertEqual(passed["command"], "python -m pytest 'tests/test_calc.py::AddCase::test_pass'")
        self.assertEqual(packages["tests.test_calc"]["command"], "python -m pytest 'tests/test_calc.py'")
        self.assertEqual(packages["tests.test_broken"]["command"], "python -m pytest 'tests/test_broken.py'")
        self.assertIsNone(packages["calc"]["command"])  # nothing of it ran

    def result(self, session, path, nodeid):
        session.handle({"e": "result", "nodeid": nodeid, "path": path, "outcome": "pass", "duration": 0.0,
                        "output": None, "def_file": None, "def_line": None})
        return session.tests[-1]

    def test_parametrised_ids_are_one_level(self):
        with Tree(SOURCES) as tree:
            session, _ = self.session(tree)
            test_file = tree.path("tests/test_calc.py")
            url = self.result(session, test_file, "tests/test_calc.py::test_url[a/b]")
            quoted = self.result(session, test_file, "tests/test_calc.py::TestQ::test_q[it's]")
        self.assertEqual((url["name"], url["hierarchy"]), ("test_url[a/b]", ["test_url[a/b]"]))
        self.assertEqual(url["command"], "python -m pytest 'tests/test_calc.py::test_url[a/b]'")
        self.assertEqual(quoted["hierarchy"], ["TestQ::test_q[it's]"])
        self.assertEqual(quoted["command"], "python -m pytest 'tests/test_calc.py::TestQ::test_q[it'\\''s]'")
        self.assertEqual(validate(url, schema("tests.schema.json", "test")), [])

    def test_unittest_commands(self):
        files = {
            "calc.py": "",
            "tests/test_calc.py": "",
            "svc/pyproject.toml": "[project]\nname = 'svc'\n",
            "svc/tests/test_api.py": "",
            "lib/pyproject.toml": "[project]\nname = 'lib'\n",
            "lib/src/lib/__init__.py": "",
            "lib/src/lib/tests/test_core.py": "",
            "my tools/pyproject.toml": "[project]\nname = 'tools'\n",
            "my tools/test_cli.py": "",
            "bad-name/test_x.py": "",
        }
        with Tree(files) as tree:
            session, project = self.session(tree)
            session.runner = testrun.UNITTEST
            commands = {}
            for rel in ("tests/test_calc.py", "svc/tests/test_api.py", "lib/src/lib/tests/test_core.py",
                        "my tools/test_cli.py", "bad-name/test_x.py"):
                test = self.result(session, tree.path(rel), f"{rel}::Case::test_it")
                commands[rel] = (test["command"], session.command(session.modules[rel], None))
        self.assertEqual(commands, {
            "tests/test_calc.py": ("python -m unittest tests.test_calc.Case.test_it", "python -m unittest tests.test_calc"),
            "svc/tests/test_api.py": ("cd svc && python -m unittest tests.test_api.Case.test_it",
                                      "cd svc && python -m unittest tests.test_api"),
            "lib/src/lib/tests/test_core.py": ("cd lib/src && python -m unittest lib.tests.test_core.Case.test_it",
                                               "cd lib/src && python -m unittest lib.tests.test_core"),
            "my tools/test_cli.py": ("cd 'my tools' && python -m unittest test_cli.Case.test_it",
                                     "cd 'my tools' && python -m unittest test_cli"),
            "bad-name/test_x.py": (None, None),  # not importable by a dotted name
        })

    def test_selection_targets(self):
        with Tree(SOURCES) as tree:
            project = discover(tree.root)
            diagnostics = []
            targets, wanted = testrun._targets(project, {"packages": [
                {"package": "tests.test_calc", "tests": [
                    {"name": "AddCase::test_pass", "subtests": None},
                    {"name": "test_div[a", "subtests": ["b]", "c]"]},
                    {"name": "test_none", "subtests": []},
                ]},
                {"package": "tests.test_broken", "tests": None},
                {"package": "missing", "tests": None},
            ]}, diagnostics)
        self.assertEqual(targets, [
            "tests/test_calc.py::AddCase::test_pass",
            "tests/test_calc.py::test_div[a/b]",
            "tests/test_calc.py::test_div[a/c]",
            "tests/test_broken.py",
        ])
        self.assertEqual(wanted, ["tests.test_calc", "tests.test_broken"])
        self.assertEqual(diagnostics, ["the package missing was not found, so its tests were not run"])

    def test_coverage_blocks(self):
        with Tree(SOURCES) as tree:
            project = discover(tree.root)
            model = build_model(project)
            data = {"files": {
                "calc.py": {"executed_lines": [1, 2, 5], "missing_lines": [7, 8]},
                tree.path("tests/test_calc.py"): {"executed_lines": [1, 3], "missing_lines": []},
                "gen_pb2.py": {"executed_lines": [1], "missing_lines": []},
                "/elsewhere/lib.py": {"executed_lines": [1], "missing_lines": []},
            }}
            blocks = testrun.coverage_blocks(data, project, model)
        # Lines 1 and 5-6 are def headers (decorator included): they run at import.
        self.assertEqual([(b["file"], b["startLine"], b["count"]) for b in blocks],
                         [("calc.py", 2, 1), ("calc.py", 7, 0), ("calc.py", 8, 0)])


class RealRun(unittest.TestCase):
    """The test command end to end, with the python3 on PATH (pytest or unittest)."""

    def test_a_run(self):
        with Tree(SOURCES) as tree:
            progress = tree.path("progress.json")
            code, document, stderr = run(["test", "--dir", tree.root, "--timeout", "120s", "--no-cover",
                                          "--no-race", "--progress", progress])
            self.assertEqual(code, 0, stderr)
            self.assertEqual(validate(document, TESTS), [])
            with open(progress, encoding="utf-8") as stream:
                self.assertEqual(validate(json.load(stream), PROGRESS), [])
        outcomes = {t["name"]: t["outcome"] for t in document["tests"]}
        self.assertEqual(outcomes, {"AddCase::test_pass": "pass", "AddCase::test_fail": "fail"})
        failed = next(t for t in document["tests"] if t["outcome"] == "fail")
        self.assertIn("4", failed["output"])
        self.assertEqual(failed["function"], "tests.test_calc.AddCase.test_fail")
        packages = {p["container"]: p["outcome"] for p in document["packages"]}
        self.assertEqual(packages["tests.test_broken"], "build_failed")
        self.assertEqual(packages["tests.test_calc"], "fail")
        self.assertIn("coverage was not measured: --no-cover was given", document["diagnostics"])
        self.assertIsNone(document["coverage"])
        self.assertEqual(failed["hierarchy"], ["AddCase::test_fail"])
        commands = {p["container"]: p["command"] for p in document["packages"]}
        if any("only unittest suites ran" in d for d in document["diagnostics"]):
            self.assertEqual(failed["command"], "python -m unittest tests.test_calc.AddCase.test_fail")
            self.assertEqual(commands["tests.test_calc"], "python -m unittest tests.test_calc")
        else:
            self.assertEqual(failed["command"], "python -m pytest 'tests/test_calc.py::AddCase::test_fail'")
            self.assertEqual(commands["tests.test_calc"], "python -m pytest 'tests/test_calc.py'")

    def test_the_kept_bytecode_leaves_out_the_tree(self):
        with Tree(SOURCES) as tree, tempfile.TemporaryDirectory(prefix="difflens-cache-") as cache:
            code, _, stderr = run(["test", "--dir", tree.root, "--no-cover", "--cache-dir", cache])
            self.assertEqual(code, 0, stderr)
            pycache = os.path.join(cache, "pycache")
            compiled = [os.path.join(folder, name) for folder, _, names in os.walk(pycache) for name in names]
            self.assertTrue(any(path.endswith(".pyc") for path in compiled))  # the standard library's, at least
            for root in (tree.root, os.path.realpath(tree.root)):
                self.assertFalse(os.path.exists(os.path.join(pycache, root.lstrip(os.sep))), root)

    def test_a_selection(self):
        with Tree(SOURCES) as tree:
            selection = tree.path("selection.json")
            with open(selection, "w", encoding="utf-8") as stream:
                json.dump({"packages": [{"package": "tests.test_calc",
                                         "tests": [{"name": "AddCase::test_pass", "subtests": None}]}]}, stream)
            code, document, stderr = run(["test", "--dir", tree.root, "--no-cover", "--selection", selection])
        self.assertEqual(code, 0, stderr)
        self.assertEqual([(t["name"], t["outcome"]) for t in document["tests"]], [("AddCase::test_pass", "pass")])
        self.assertEqual([p["container"] for p in document["packages"]], ["tests.test_calc"])

    def test_the_timeout_stops_the_run(self):
        files = dict(SOURCES)
        files["tests/test_slow.py"] = """
            import time
            import unittest


            class Slow(unittest.TestCase):
                def test_sleeps(self):
                    time.sleep(60)
        """
        del files["tests/test_calc.py"]
        with Tree(files) as tree:
            code, document, stderr = run(["test", "--dir", tree.root, "--timeout", "4s", "--no-cover"])
        self.assertEqual(code, 0, stderr)
        self.assertTrue(any("the run was stopped after 4s" in d for d in document["diagnostics"]))
        packages = {p["container"]: p for p in document["packages"]}
        self.assertEqual(packages["tests.test_slow"]["outcome"], "fail")

    def test_a_tree_without_tests(self):
        with Tree({"app.py": "def main():\n    pass\n"}) as tree:
            code, document, _ = run(["test", "--dir", tree.root, "--progress", tree.path("p.json")])
            self.assertTrue(os.path.exists(tree.path("p.json")))
        self.assertEqual(code, 0)
        self.assertEqual(document["tests"], [])
        self.assertEqual(document["packages"], [{"container": "app", "outcome": "no_tests", "durationMs": 0, "output": None,
                                                 "command": None}])
        self.assertEqual(validate(document, TESTS), [])


if __name__ == "__main__":
    unittest.main()
