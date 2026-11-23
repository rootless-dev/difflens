"""The small pieces: TOML, complexity, module names, plans, the command line."""

import ast
import io
import json
import os
import shutil
import stat
import sys
import tempfile
import threading
import time
import unittest

from difflens_analyzer import NAME, VERSION, checks, cli, complexity, environment, prepare, toml
from difflens_analyzer.checks import _MYPY_LINE, Check
from difflens_analyzer.log import Logger
from difflens_analyzer.project import discover, is_generated, is_test_path
from tests.helpers import Tree, run, schema, validate


class Toml(unittest.TestCase):
    DOCUMENT = '''
# comment
title = "x" # trailing
[project]
name = 'orders'
version = "1.0"
dependencies = [
    "a>=1",  # pinned
    'b',
]
"quoted key" = { inline = true, n = 3, list = [1, 2.5] }
[project.scripts]
orders = "orders.cli:main"
[[tool.items]]
x = 1
[[tool.items]]
x = 2
[tool.ruff.lint]
select = ["E", "F"]
multi = """
line \\
  joined"""
raw = \'\'\'C:\\path\'\'\'
when = 1979-05-27T07:32:00Z
'''

    def test_fallback_reader_agrees_with_tomllib(self):
        parsed = toml.parse(self.DOCUMENT)
        self.assertEqual(parsed["project"]["dependencies"], ["a>=1", "b"])
        self.assertEqual(parsed["project"]["quoted key"], {"inline": True, "n": 3, "list": [1, 2.5]})
        self.assertEqual(parsed["project"]["scripts"]["orders"], "orders.cli:main")
        self.assertEqual([i["x"] for i in parsed["tool"]["items"]], [1, 2])
        self.assertEqual(parsed["tool"]["ruff"]["lint"]["multi"], "line joined")
        self.assertEqual(parsed["tool"]["ruff"]["lint"]["raw"], "C:\\path")
        if sys.version_info >= (3, 11):
            import tomllib

            expected = tomllib.loads(self.DOCUMENT)
            expected["tool"]["ruff"]["lint"]["when"] = parsed["tool"]["ruff"]["lint"]["when"]
            self.assertEqual(parsed, expected)

    def test_bad_toml_is_an_error_and_missing_files_are_empty(self):
        with self.assertRaises(toml.TOMLError):
            toml.parse("[unclosed\n")
        self.assertEqual(toml.load_file("/nonexistent/pyproject.toml"), {})


class Complexity(unittest.TestCase):
    def of(self, source: str) -> int:
        node = ast.parse(source).body[0]
        return complexity.of_function(node)  # type: ignore[arg-type]

    def test_rules(self):
        self.assertEqual(self.of("def f():\n    return 1\n"), 1)
        self.assertEqual(self.of("def f(a, b, c):\n    if a and b or c:\n        pass\n    elif b:\n        pass\n"), 5)
        self.assertEqual(self.of("def f(xs):\n    return [x for x in xs if x if x > 1]\n"), 4)
        self.assertEqual(self.of("def f(x):\n    try:\n        pass\n    except A:\n        pass\n    except B:\n        pass\n"), 3)
        self.assertEqual(self.of("def f():\n    while x:\n        pass\n    return 1 if y else 2\n"), 3)
        # Nested functions and lambdas are not counted.
        self.assertEqual(self.of("def f():\n    def g():\n        if x: pass\n    h = lambda: 1 if x else 2\n"), 1)

    @unittest.skipIf(sys.version_info < (3, 10), "match needs Python 3.10")
    def test_match_cases_but_the_default(self):
        source = "def f(x):\n    match x:\n        case 1: pass\n        case [a, b]: pass\n        case _: pass\n"
        self.assertEqual(self.of(source), 3)


class Project(unittest.TestCase):
    def test_module_names_by_source_root(self):
        with Tree({
            "pyproject.toml": "[project]\nname = 'x'\n",
            "src/pkg/__init__.py": "",
            "src/pkg/mod.py": "",
            "tests/test_mod.py": "",
            "scripts/tool.py": "",
            ".venv/lib/site.py": "",
            "node_modules/x.py": "",
            "build/out.py": "",
            "services/api/pyproject.toml": "",
            "services/api/app/main.py": "",
            "services/web/pyproject.toml": "",
            "services/web/app/main.py": "",
        }) as tree:
            project = discover(tree.root)
        names = {s.rel: s.module for s in project.files}
        self.assertEqual(names["src/pkg/__init__.py"], "pkg")
        self.assertEqual(names["src/pkg/mod.py"], "pkg.mod")
        self.assertEqual(names["tests/test_mod.py"], "tests.test_mod")
        self.assertEqual(names["scripts/tool.py"], "scripts.tool")
        # Two projects with the same package are named from the tree root.
        self.assertEqual(names["services/api/app/main.py"], "services.api.app.main")
        self.assertEqual(names["services/web/app/main.py"], "services.web.app.main")
        self.assertNotIn(".venv/lib/site.py", names)
        self.assertNotIn("node_modules/x.py", names)
        self.assertNotIn("build/out.py", names)

    def test_test_and_generated_conventions(self):
        for rel in ("test_a.py", "a_test.py", "conftest.py", "tests/helpers.py", "pkg/test/util.py"):
            self.assertTrue(is_test_path(rel), rel)
        for rel in ("app.py", "testing.py", "contest.py", "pkg/tests_utils.py"):
            self.assertFalse(is_test_path(rel), rel)
        self.assertTrue(is_generated(b"# -*- coding: utf-8 -*-\n# Generated by the protocol buffer compiler.  DO NOT EDIT!\n"))
        self.assertTrue(is_generated(b"", "api/v1/service_pb2.py"))
        self.assertFalse(is_generated(b"import os\n# generated by hand? no: this is code\n"))


class Plans(unittest.TestCase):
    def test_requirements_from_every_source(self):
        with Tree({
            "pyproject.toml": """
                [project]
                name = "x"
                dependencies = ["requests>=2"]
                [project.optional-dependencies]
                test = ["pytest"]
                docs = ["sphinx"]
                [dependency-groups]
                dev = ["ruff", {include-group = "typing"}]
                typing = ["mypy"]
                [tool.poetry.dependencies]
                python = "^3.9"
                attrs = "^23.1"
                local = {path = "../local"}
            """,
            "requirements-dev.txt": "coverage\n",
            "requirements/ci.txt": "tox\n",
        }) as tree:
            plan = environment.make_plan(tree.root)
        self.assertEqual(plan.kind, "pip")
        self.assertEqual(plan.requirements, ["requests>=2", "pytest", "ruff", "mypy", "attrs>=23.1,<24"])
        self.assertEqual([os.path.basename(f) for f in plan.requirement_files], ["requirements-dev.txt", "ci.txt"])
        self.assertIn("requirements-dev.txt", plan.files)
        self.assertEqual(len(plan.key), 20)

    def test_poetry_constraints(self):
        convert = environment._poetry_requirement
        self.assertEqual(convert("a", "^1.2.3"), "a>=1.2.3,<2")
        self.assertEqual(convert("a", "^0.2.3"), "a>=0.2.3,<0.3")
        self.assertEqual(convert("a", "~1.2.3"), "a>=1.2.3,<1.3")
        self.assertEqual(convert("a", "1.0"), "a==1.0")
        self.assertEqual(convert("a", ">=1,<2"), "a>=1,<2")
        self.assertEqual(convert("a", "*"), "a")
        self.assertIsNone(convert("a", {"git": "https://x"}))

    def test_no_dependencies_and_tree_venvs(self):
        with Tree({"app.py": "", ".venv/pyvenv.cfg": "home = /usr/bin\n", ".venv/bin/python": ""}) as tree:
            plan = environment.make_plan(tree.root)
            self.assertEqual(plan.kind, "none")
            interpreter = environment.find_interpreter(tree.root, plan)
            self.assertEqual(interpreter.source, "tree")
            self.assertEqual(interpreter.python, os.path.join(tree.root, ".venv", "bin", "python"))

    def test_the_key_follows_the_dependency_files(self):
        with Tree({"requirements.txt": "a\n"}) as one, Tree({"requirements.txt": "b\n"}) as two, Tree({"requirements.txt": "a\n"}) as three:
            keys = [environment.make_plan(t.root).key for t in (one, two, three)]
        self.assertNotEqual(keys[0], keys[1])
        self.assertEqual(keys[0], keys[2])


class Environments(unittest.TestCase):
    """prepare builds one environment per key, even for two copies at once."""

    def setUp(self):
        self.cache = tempfile.TemporaryDirectory(prefix="difflens-envs-")
        self.saved_root = os.environ.get("DIFFLENS_PYTHON_ENVS")
        os.environ["DIFFLENS_PYTHON_ENVS"] = self.cache.name
        self.saved_build = prepare._build
        self.log = Logger(io.StringIO(), NAME, "error")

    def tearDown(self):
        prepare._build = self.saved_build
        if self.saved_root is None:
            os.environ.pop("DIFFLENS_PYTHON_ENVS", None)
        else:
            os.environ["DIFFLENS_PYTHON_ENVS"] = self.saved_root
        self.cache.cleanup()

    def fake_build(self, builds, seen_during_build):
        def build(root, plan, env, deadline):
            builds.append(root)
            time.sleep(0.5)
            # Half built: nothing may use it yet.
            seen_during_build.append(environment.prepared(plan))
            os.makedirs(os.path.join(env, "bin"))
            with open(os.path.join(env, "bin", "python"), "w", encoding="utf-8"):
                pass
        return build

    def test_two_prepares_of_one_key_build_it_once(self):
        builds, seen = [], []
        prepare._build = self.fake_build(builds, seen)
        errors = []
        with Tree({"requirements.txt": "six\n"}) as one, Tree({"requirements.txt": "six\n"}) as two:
            def prepare_copy(root):
                try:
                    prepare.prepare_tree(root, 30.0, self.log)
                except Exception as error:  # noqa: BLE001 - reported below
                    errors.append(error)
            threads = [threading.Thread(target=prepare_copy, args=(t.root,)) for t in (one, two)]
            for thread in threads:
                thread.start()
            for thread in threads:
                thread.join(20)
            plan = environment.make_plan(one.root)
            self.assertEqual(environment.make_plan(two.root).key, plan.key)
        self.assertEqual(errors, [])
        self.assertEqual(len(builds), 1)
        self.assertEqual(seen, [None])
        self.assertEqual(environment.prepared(plan), plan.env_dir)
        with open(os.path.join(plan.env_dir, environment.READY), encoding="utf-8") as stream:
            self.assertEqual(json.load(stream)["kind"], "pip")

    def test_a_half_built_environment_is_rebuilt(self):
        builds = []
        prepare._build = self.fake_build(builds, [])
        with Tree({"requirements.txt": "six\n"}) as tree:
            plan = environment.make_plan(tree.root)
            os.makedirs(os.path.join(plan.env_dir, "lib"))  # a killed build: no ready marker
            self.assertIsNone(environment.prepared(plan))
            prepare.prepare_tree(tree.root, 30.0, self.log)
        self.assertEqual(len(builds), 1)
        self.assertFalse(os.path.exists(os.path.join(plan.env_dir, "lib")))
        self.assertEqual(environment.prepared(plan), plan.env_dir)

    def test_waiting_past_the_deadline_fails(self):
        prepare._build = self.fake_build([], [])
        with Tree({"requirements.txt": "six\n"}) as tree:
            plan = environment.make_plan(tree.root)
            with environment.build_lock(plan.key, time.monotonic() + 1):
                with self.assertRaises(prepare.PrepareError) as raised:
                    prepare.prepare_tree(tree.root, 1.0, self.log)
        self.assertIn("did not finish in time", str(raised.exception))

    def fake_tools(self, modules, code=0):
        """Stands in for the probe and for processes.run; returns the commands run."""
        commands = []
        saved_probe, saved_run = environment.probe, prepare.processes.run

        def probe(interpreter, timeout=30.0):
            return {"python": "3.12.0", "modules": dict(modules)}

        def run(cmd, cwd, env=None, timeout=None):
            commands.append(cmd)
            return prepare.processes.Result(code=code, stdout="", stderr="no network" if code else "")

        environment.probe, prepare.processes.run = probe, run
        self.addCleanup(setattr, environment, "probe", saved_probe)
        self.addCleanup(setattr, prepare.processes, "run", saved_run)
        return commands

    def test_coverage_is_added_to_an_environment_without_it(self):
        # The regression behind "coverage and CRAP always show a dash": a uv
        # project that does not declare coverage.py got an environment without
        # it, so no run ever measured coverage.
        commands = self.fake_tools({"pytest": "8.0", "coverage": None})
        with Tree({"pyproject.toml": "[project]\nname = 'acme'\n", "uv.lock": ""}) as tree:
            plan = environment.make_plan(tree.root)
            added = prepare.add_coverage(tree.root, plan, plan.env_dir, time.monotonic() + 30)
        self.assertTrue(added)
        self.assertEqual(len(commands), 1)
        install = commands[0]
        self.assertIn(prepare.COVERAGE_REQUIREMENT, install)
        self.assertIn(os.path.join(plan.env_dir, "bin", "python"), install)
        self.assertIn("install", install)

    def test_coverage_already_there_is_left_alone(self):
        commands = self.fake_tools({"pytest": "8.0", "coverage": "7.6.1"})
        with Tree({"requirements.txt": "coverage\n"}) as tree:
            plan = environment.make_plan(tree.root)
            self.assertTrue(prepare.add_coverage(tree.root, plan, plan.env_dir, time.monotonic() + 30))
        self.assertEqual(commands, [])

    def test_failing_to_add_coverage_does_not_fail_prepare(self):
        commands = self.fake_tools({"coverage": None}, code=1)
        with Tree({"requirements.txt": "six\n"}) as tree:
            plan = environment.make_plan(tree.root)
            self.assertFalse(prepare.add_coverage(tree.root, plan, plan.env_dir, time.monotonic() + 30))
        self.assertEqual(len(commands), 1)

    def test_every_kind_of_build_ends_by_adding_coverage(self):
        calls = []
        saved = {name: getattr(prepare, name) for name in ("_uv_sync", "_venv", "_pip", "_poetry", "add_coverage")}
        for name in ("_uv_sync", "_venv", "_pip", "_poetry", "add_coverage"):
            setattr(prepare, name, lambda *args, name=name: calls.append(name))
        try:
            for kind in ("uv", "poetry", "pip"):
                calls.clear()
                prepare._build("/tree", environment.Plan(kind=kind, folder=""), "/env", time.monotonic() + 30)
                self.assertEqual(calls[-1], "add_coverage", kind)
        finally:
            for name, value in saved.items():
                setattr(prepare, name, value)

    def test_environments_built_before_coverage_was_added_are_rebuilt(self):
        # FORMAT is in the key: an environment built without coverage.py is
        # never reused.
        self.assertGreaterEqual(environment.FORMAT, 2)

    def test_prune_skips_environments_being_built(self):
        old = time.time() - environment.PRUNE_AFTER - 60
        stale, busy = (os.path.join(self.cache.name, name) for name in ("stale", "busy"))
        for env in (stale, busy):
            os.makedirs(env)
            os.utime(env, (old, old))
        with environment.build_lock("busy", time.monotonic() + 1):
            environment.prune(keep="")
        self.assertFalse(os.path.exists(stale))
        self.assertTrue(os.path.exists(busy))


class Checks(unittest.TestCase):
    def test_mypy_lines(self):
        match = _MYPY_LINE.match('/t/a.py:10:5: error: Incompatible return value type (got "str", expected "int")  [return-value]')
        self.assertEqual((match["file"], match["line"], match["code"]), ("/t/a.py", "10", "return-value"))
        self.assertEqual(_MYPY_LINE.match("/t/a.py:3: note: See https://x")["severity"], "note")

    def test_statuses(self):
        check = Check("ruff")
        self.assertEqual(check.document()["status"], "clean")
        check.add("a.py", 0, "x", None, "warning")
        document = check.document()
        self.assertEqual((document["status"], document["findings"][0]["line"]), ("findings", 1))
        self.assertEqual(document["findings"][0]["severity"], "warning")
        self.assertEqual(Check("mypy").unavailable("not installed").document()["findings"], [])

    def test_ruff_severity(self):
        for code in (None, "", "invalid-syntax", "E999"):
            self.assertEqual(checks.ruff_severity(code), "error", code)
        self.assertEqual(checks.ruff_severity("F401"), "warning")

    # Stand-ins for ruff and mypy: they print fixed findings and record their arguments.
    FAKE_RUFF = """\
        #!/bin/sh
        cat <<'EOF'
        [{"filename": "app.py", "location": {"row": 1}, "message": "`os` imported but unused", "code": "F401"},
         {"filename": "app.py", "location": {"row": 3}, "message": "SyntaxError: Expected an expression", "code": null}]
        EOF
        """
    FAKE_MYPY = """\
        #!/bin/sh
        echo "$@" > "$(dirname "$0")/mypy-args"
        echo "$PWD/app.py:2: error: Incompatible types in assignment  [assignment]"
        echo "$PWD/app.py:4: warning: Unused \\"type: ignore\\" comment  [unused-ignore]"
        echo "$PWD/app.py:4: note: See https://mypy.readthedocs.io"
        exit 1
        """

    def fake_tools(self, folder):
        for name, text in (("ruff", self.FAKE_RUFF), ("mypy", self.FAKE_MYPY)):
            path = os.path.join(folder, name)
            with open(path, "w", encoding="utf-8") as stream:
                stream.write("\n".join(line[8:] for line in text.splitlines()) + "\n")
            os.chmod(path, os.stat(path).st_mode | stat.S_IXUSR)

    def test_severities_and_the_mypy_cache(self):
        files = {"pyproject.toml": "[tool.ruff]\n[tool.mypy]\n", "app.py": "import os\nx: int = 'a'\n"}
        with Tree(files) as tree, tempfile.TemporaryDirectory(prefix="difflens-bin-") as tools, \
                tempfile.TemporaryDirectory(prefix="difflens-cache-") as cache:
            self.fake_tools(tools)
            path = {"PATH": tools + os.pathsep + os.environ.get("PATH", "")}
            code, document, stderr = run(["check", "--dir", tree.root, "--cache-dir", cache], env=path)
            self.assertEqual(code, 0, stderr)
            with open(os.path.join(tools, "mypy-args"), encoding="utf-8") as stream:
                args = stream.read().split()
            self.assertEqual(args[args.index("--cache-dir") + 1], os.path.join(cache, "mypy"))
            self.assertTrue(os.path.isdir(os.path.join(cache, "mypy")))
            # Without --cache-dir: a temporary folder, removed afterwards.
            code, _, stderr = run(["check", "--dir", tree.root], env=path)
            self.assertEqual(code, 0, stderr)
            with open(os.path.join(tools, "mypy-args"), encoding="utf-8") as stream:
                args = stream.read().split()
            temporary = args[args.index("--cache-dir") + 1]
            self.assertNotEqual(temporary, os.path.join(cache, "mypy"))
            self.assertFalse(os.path.exists(temporary))
        self.assertEqual(validate(document, schema("checks.schema.json")), [])
        found = {c["tool"]: [(f["line"], f["rule"], f["severity"]) for f in c["findings"]] for c in document["checks"]}
        self.assertEqual(found["ruff"], [(1, "F401", "warning"), (3, None, "error")])
        self.assertEqual(found["mypy"], [(2, "assignment", "error"), (4, "unused-ignore", "warning")])

    @unittest.skipUnless(shutil.which("mypy"), "mypy is not on PATH")
    def test_the_kept_mypy_cache_follows_each_copy(self):
        """Copies at new paths share the cache; mypy rechecks what changed,
        even a change of the same size in a file with the same mtime."""
        for config in ("[tool.mypy]\n", '[tool.mypy]\nfiles = ["pkg/*.py"]\n'):
            good = {"pyproject.toml": config, "pkg/__init__.py": "", "pkg/a.py": "x: int = 1\n"}
            bad = dict(good, **{"pkg/a.py": "x: str = 1\n"})
            with tempfile.TemporaryDirectory(prefix="difflens-cache-") as cache:
                results = []
                for files in (good, bad, good):
                    with Tree(files) as tree:
                        os.utime(tree.path("pkg/a.py"), (1_700_000_000, 1_700_000_000))
                        code, document, stderr = run(["check", "--dir", tree.root, "--cache-dir", cache])
                        self.assertEqual(code, 0, stderr)
                    mypy = next(c for c in document["checks"] if c["tool"] == "mypy")
                    results.append([(f["file"], f["rule"]) for f in mypy["findings"]])
            self.assertEqual(results, [[], [("pkg/a.py", "assignment")], []], config)

    def test_configured_mypy_files_are_absolute(self):
        with Tree({"mypy.ini": "[mypy]\nfiles = pkg/*.py, ~/x.py, missing.py\n", "pkg/a.py": "", "pkg/b.py": ""}) as tree:
            project = discover(tree.root)
            self.assertEqual(checks._mypy_config_files(project), [
                tree.path("pkg/a.py"), tree.path("pkg/b.py"), os.path.expanduser("~/x.py"), tree.path("missing.py"),
            ])
        with Tree({"pyproject.toml": "[tool.mypy]\nstrict = true\n"}) as tree:
            self.assertEqual(checks._mypy_config_files(discover(tree.root)), [])

    def test_unused_checkers_are_unavailable(self):
        with Tree({"app.py": "import os\n"}) as tree:
            code, document, _ = run(["check", "--dir", tree.root])
        self.assertEqual(code, 0)
        self.assertEqual([(c["tool"], c["status"]) for c in document["checks"]], [("ruff", "unavailable"), ("mypy", "unavailable")])
        self.assertEqual(validate(document, schema("checks.schema.json")), [])


class CommandLine(unittest.TestCase):
    def test_version(self):
        for flag in ("--version", "-version", "version"):
            self.assertEqual(run([flag])[:2], (0, f"{NAME} {VERSION}\n"))

    def test_usage_errors(self):
        self.assertEqual(run([])[0], 2)
        self.assertEqual(run(["analyze"])[0], 2)
        self.assertEqual(run(["analyze", "--dir", ".", "--timeout", "0s"])[0], 2)
        self.assertEqual(run(["analyze", "--dir", ".", "--bogus"])[0], 2)
        self.assertEqual(run(["analyze", "--dir", ".", "extra"])[0], 2)
        self.assertEqual(run(["test", "--dir", ".", "--selection", "/nonexistent.json"])[0], 2)

    def test_cache_dir(self):
        for command in cli.CACHED:
            for args in (["--cache-dir", "/c/python"], ["--cache-dir=/c/python"]):
                options = cli._parser(command).parse_args(["--dir", "."] + args)
                self.assertEqual(cli.cache_dir(options), "/c/python", (command, args))
            self.assertEqual(cli.cache_dir(cli._parser(command).parse_args(["--dir", "."])), "")
        self.assertEqual(cli.cache_dir(cli._parser("test").parse_args(["--dir", ".", "--cache-dir", "rel"])),
                         os.path.abspath("rel"))
        self.assertEqual(run(["analyze", "--dir", ".", "--cache-dir", "/c"])[0], 2)

    def test_missing_tree_fails(self):
        code, _, stderr = run(["analyze", "--dir=/nonexistent/tree"])
        self.assertEqual(code, 1)
        self.assertIn("is not a directory", stderr)

    def test_durations(self):
        self.assertEqual(cli.parse_duration("1m30s"), 90)
        self.assertEqual(cli.parse_duration("120s"), 120)
        self.assertAlmostEqual(cli.parse_duration("1.5h"), 5400)
        with self.assertRaises(cli.UsageError):
            cli.parse_duration("ten")

    def test_prepare_without_dependencies(self):
        with Tree({"app.py": "def main():\n    pass\n"}) as tree:
            code, document, _ = run(["prepare", "--dir", tree.root])
        self.assertEqual((code, document), (0, {"protocol": 1, "writable": []}))
        self.assertEqual(validate(document, schema("tests.schema.json", "prepare")), [])


if __name__ == "__main__":
    unittest.main()
