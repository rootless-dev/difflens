import ast
import json
import os
import shutil
import tempfile
import unittest

from difflens_analyzer import mutation
from difflens_analyzer.analysis import build_model
from difflens_analyzer.mutator import mutants_of, mutated_source
from difflens_analyzer.project import discover
from tests.helpers import TESTDATA, Tree, run, schema, validate

MUTATION = schema("mutation.schema.json")
PROGRESS = schema("mutation.schema.json", "progress")
PROJECT = os.path.join(TESTDATA, "mutation")


def mutants(text, rel="m.py"):
    """(line, column, endLine, endColumn, original, mutated, operator, function) of a file's mutants."""
    with Tree({rel: text}) as tree:
        project = discover(tree.root)
        model = build_model(project)
        module = next(m for m in model.modules.values() if m.source.rel == rel)
        with open(tree.path(rel), "rb") as stream:
            data = stream.read()
        found = mutants_of(data, module.tree, model, module.name)
    return [(m.line, m.column, m.end_line, m.end_column, m.original, m.mutated, m.operator,
             m.function.id if m.function else None) for m in found], found, data


class Operators(unittest.TestCase):
    def test_comparisons_have_boundary_and_negation(self):
        found, _, _ = mutants("""
            def f(a, b):
                x = a < b
                y = a <= b
                z = a == b or a is not b
                return x, y, z, a not in b
        """)
        self.assertEqual([(f[4], f[5], f[6]) for f in found], [
            ("<", "<=", "relational_boundary"), ("<", ">=", "relational_negation"),
            ("<=", "<", "relational_boundary"), ("<=", ">", "relational_negation"),
            ("==", "!=", "relational_negation"), ("or", "and", "logical"), ("is not", "is", "relational_negation"),
            ("x, y, z, a not in b", "()", "return_value"), ("not in", "in", "relational_negation"),
        ])

    def test_columns_are_code_points_and_spans_are_exact(self):
        found, objects, data = mutants("def saque(saldo_ação, valor):\n    if saldo_ação >= valor:\n        return 1\n")
        self.assertEqual(found[0], (2, 19, 2, 21, ">=", ">", "relational_boundary", "m.saque"))
        text = data.decode("utf-8").split("\n")[1]
        self.assertEqual(text[found[0][1] - 1 : found[0][3] - 1], ">=")
        self.assertEqual(mutated_source(data, objects[0]).decode("utf-8").split("\n")[1], "    if saldo_ação > valor:")

    def test_arithmetic_skips_strings_and_sequences(self):
        found, _, _ = mutants("""
            def f(a, b):
                s = "x" + a
                t = [0] * b
                u = "%s" % a
                v = (a + b) * 2 // a % b
                return None
        """)
        self.assertEqual([(f[4], f[5]) for f in found], [("+", "-"), ("*", "/"), ("//", "*"), ("%", "*")])

    def test_logical_not_and_conditions(self):
        found, _, _ = mutants("""
            def f(a, b):
                if a and not (b):
                    pass
                while b:
                    b = 0
                if a > b:
                    pass
                if not a:
                    pass
                while True:
                    break
        """)
        self.assertEqual([(f[0], f[4], f[5], f[6]) for f in found], [
            (2, "a and not (b)", "not (a and not (b))", "negation_removal"),
            (2, "and", "or", "logical"),
            (2, "not ", "", "negation_removal"),
            (4, "b", "not (b)", "negation_removal"),
            # a single comparison is negated by relational_negation only
            (6, ">", ">=", "relational_boundary"), (6, ">", "<=", "relational_negation"),
            (8, "not ", "", "negation_removal"),
        ])

    def test_return_values(self):
        found, _, _ = mutants("""
            def n(): return 5
            def z(): return 0
            def s(): return 'x'
            def e(): return ''
            def t(): return True
            def l(): return [1]
            def d(): return {1: 2}
            def c(a, b): return a == b
            def k(x) -> int: return x
            def o(x) -> "Thing": return x
            def g(x): return x.y
            def none(): return None
        """)
        self.assertEqual([(f[7], f[4], f[5]) for f in found if f[6] == "return_value"], [
            ("m.n", "5", "0"), ("m.s", "'x'", '""'), ("m.t", "True", "False"), ("m.l", "[1]", "[]"),
            ("m.d", "{1: 2}", "{}"), ("m.c", "a == b", "False"), ("m.k", "x", "0"), ("m.o", "x", "None"),
            ("m.g", "x.y", "None"),
        ])

    def test_call_removal_and_what_it_keeps(self):
        found, objects, data = mutants("""
            import logging
            log = logging.getLogger(__name__)

            def f(self, x):
                save(x)
                self.repo.flush(); x.close()
                print(x)
                log.info("saved")
                logging.warning("w")
                self.assertEqual(x, 1)
                warnings.warn("w")
                x.debug("d")

            def g(x):
                notify(x)  # alone in its block
        """)
        self.assertEqual([(f[0], f[4], f[5]) for f in found], [
            (5, "save(x)", ""), (6, "self.repo.flush()", "pass"), (6, "x.close()", "pass"), (15, "notify(x)", "pass"),
        ])
        removed = mutated_source(data, objects[0]).decode("utf-8").split("\n")[4]
        self.assertEqual(removed, "    ")

    def test_increment_and_what_is_never_mutated(self):
        found, _, _ = mutants("""
            from typing import TYPE_CHECKING
            LIMIT = 10 + 2
            key = lambda x: x - 1
            if TYPE_CHECKING:
                pass

            class Box:
                SIZE = 2 * 3

                def grow(self, by: int = 1 + 1) -> "int | None":
                    self.size += by
                    self.size -= 1

            if __name__ == "__main__":
                Box().grow()
        """)
        self.assertEqual([(f[0], f[4], f[5], f[6], f[7]) for f in found], [
            (3, "-", "+", "arithmetic", "m.key$1"),
            (10, "+", "-", "arithmetic", None),  # a default value belongs to the enclosing scope
            (11, "+=", "-=", "increment", "m.Box.grow"),
            (12, "-=", "+=", "increment", "m.Box.grow"),
            (15, "Box().grow()", "pass", "call_removal", "m.__main__"),
        ])


class Selection(unittest.TestCase):
    def test_candidate_lines(self):
        changed = {"a.py": [(1, 5), (9, 9)], "b.py": [(1, 3)]}
        coverage = [
            {"file": "a.py", "startLine": 2, "endLine": 3, "statements": 2, "count": 1},
            {"file": "a.py", "startLine": 4, "endLine": 4, "statements": 1, "count": 0},
            {"file": "a.py", "startLine": 9, "endLine": 10, "statements": 1, "count": 3},
            {"file": "b.py", "startLine": 1, "endLine": 1, "statements": 1, "count": 0},
            {"file": "c.py", "startLine": 1, "endLine": 1, "statements": 1, "count": 1},
        ]
        self.assertEqual(mutation.candidate_lines(changed, coverage), {"a.py": {2, 3, 9}})

    def test_round_robin_sample(self):
        groups = [["a1", "a2", "a3"], ["b1"], ["c1", "c2"]]
        self.assertEqual(mutation.sample(groups, 4), ["a1", "b1", "c1", "a2"])
        self.assertEqual(mutation.sample(groups, 10), ["a1", "b1", "c1", "a2", "c2", "a3"])

    def test_tests_that_import_a_module(self):
        files = {
            "app/__init__.py": "",
            "app/core.py": "def f():\n    return 1\n",
            "app/service.py": "from . import core\n",
            "app/alone.py": "X = 1\n",
            "tests/conftest.py": "",
            "tests/test_service.py": "from app.service import core\n",
            "tests/test_core.py": "import app.core\n",
            "tests/test_other.py": "import json\n",
        }
        with Tree(files) as tree:
            project = discover(tree.root)
            model = build_model(project)
            tests = [s for s in project.files if s.test and s.rel.rsplit("/", 1)[-1].startswith("test_")]
            found = mutation._selections(project, model, tests, {"app.core", "app.alone"})
        self.assertEqual([s.rel for s in found["app.core"].modules], ["tests/test_core.py", "tests/test_service.py"])
        self.assertEqual(found["app.core"].files, ["tests/conftest.py", "tests/test_core.py", "tests/test_service.py"])
        self.assertFalse(found["app.core"].every)
        self.assertTrue(found["app.alone"].every)  # no test imports it: every test


def coverage_of(root):
    """Coverage blocks for the testdata project: every line of the functions' bodies
    ran, except never_called's, as `test` reports them."""
    path = os.path.join(root, "bank", "account.py")
    with open(path, encoding="utf-8") as stream:
        tree = ast.parse(stream.read())
    blocks = []
    for node in tree.body:
        if isinstance(node, ast.FunctionDef):
            for statement in ast.walk(node):
                if isinstance(statement, ast.stmt) and statement is not node:
                    count = 0 if node.name == "never_called" else 1
                    blocks.append({"file": "bank/account.py", "startLine": statement.lineno,
                                   "endLine": statement.lineno, "statements": 1, "count": count})
    blocks.append({"file": "tests/test_account.py", "startLine": 1, "endLine": 21, "statements": 9, "count": 1})
    return {"protocol": 1, "blocks": blocks}


class RealRun(unittest.TestCase):
    """mutate end to end on testdata/mutation, with the python3 on PATH (pytest or unittest)."""

    def setUp(self):
        self.scratch = tempfile.mkdtemp(prefix="difflens-mutate-test-")
        self.root = os.path.join(self.scratch, "tree")
        shutil.copytree(PROJECT, self.root)
        self.coverage = os.path.join(self.scratch, "coverage.json")
        with open(self.coverage, "w", encoding="utf-8") as stream:
            json.dump(coverage_of(self.root), stream)

    def tearDown(self):
        shutil.rmtree(self.scratch, ignore_errors=True)

    def mutate(self, *extra):
        code, document, stderr = run(["mutate", "--dir", self.root, "--timeout", "120s",
                                      "--changed", os.path.join(self.root, "changed.json"), "--coverage", self.coverage,
                                      "--min-mutant-timeout", "2s", "--jobs", "4"] + list(extra))
        self.assertEqual(code, 0, stderr)
        self.assertEqual(validate(document, MUTATION), [])
        return document

    def test_a_full_run(self):
        progress = os.path.join(self.scratch, "progress.json")
        document = self.mutate("--progress", progress)
        with open(progress, encoding="utf-8") as stream:
            written = json.load(stream)
        self.assertEqual(validate(written, PROGRESS), [])
        self.assertEqual((written["phase"], written["total"], written["done"], written["current"]),
                         ("mutating", 12, 12, None))
        found = {(m["line"], m["original"], m["mutated"]): m for m in document["mutants"]}
        survivor = found[(6, ">=", ">")]
        self.assertEqual((survivor["status"], survivor["killedBy"], survivor["function"]),
                         ("survived", None, "bank.account.can_withdraw"))
        self.assertEqual(survivor["description"], "replace >= with > in can_withdraw")
        self.assertEqual((found[(6, ">=", "<")]["status"], found[(6, ">=", "<")]["killedBy"]),
                         ("killed", "CanWithdraw::test_enough"))
        self.assertEqual({m["status"] for m in document["mutants"] if m["function"] == "bank.account.apply_fee"},
                         {"killed"})
        self.assertEqual(found[(23, "+=", "-=")]["status"], "timeout")  # i -= 1: the loop never ends
        self.assertFalse(any(m["line"] == 29 for m in document["mutants"]))  # changed but not covered
        self.assertFalse(any(m["file"].startswith("tests/") for m in document["mutants"]))
        summary = document["summary"]
        self.assertEqual((summary["candidates"], summary["sampled"], summary["killed"], summary["survived"],
                          summary["timeout"], summary["error"], summary["cached"]), (12, False, 10, 1, 1, 0, 0))
        positions = [(m["file"], m["line"], m["column"]) for m in document["mutants"]]
        self.assertEqual(positions, sorted(positions))
        self.assertEqual(len({m["id"] for m in document["mutants"]}), 12)
        with open(os.path.join(PROJECT, "bank", "account.py"), "rb") as original, \
                open(os.path.join(self.root, "bank", "account.py"), "rb") as copy:
            self.assertEqual(original.read(), copy.read())  # the tree is never changed

    def test_the_cap_samples_round_robin_and_results_are_kept(self):
        cache = os.path.join(self.scratch, "cache")
        first = self.mutate("--max-mutants", "3", "--cache-dir", cache)
        self.assertEqual([(m["function"], m["original"], m["mutated"]) for m in first["mutants"]], [
            ("bank.account.can_withdraw", ">=", ">"),
            ("bank.account.apply_fee", "*", "/"),
            ("bank.account.double_steps", "<", "<="),
        ])
        self.assertEqual((first["summary"]["candidates"], first["summary"]["sampled"], first["summary"]["maxMutants"]),
                         (12, True, 3))
        self.assertTrue(os.path.isdir(os.path.join(cache, "mutation")))
        second = self.mutate("--max-mutants", "3", "--cache-dir", cache)
        self.assertEqual([m["cached"] for m in second["mutants"]], [True, True, True])
        self.assertEqual([(m["id"], m["status"], m["killedBy"], m["durationMs"]) for m in second["mutants"]],
                         [(m["id"], m["status"], m["killedBy"], m["durationMs"]) for m in first["mutants"]])
        self.assertEqual(second["summary"]["cached"], 3)
        # A changed function is tested again; the others are still reused.
        path = os.path.join(self.root, "bank", "account.py")
        with open(path, encoding="utf-8") as stream:
            text = stream.read()
        with open(path, "w", encoding="utf-8") as stream:
            stream.write(text.replace("fee = amount * rate", "fee = amount * rate  # changed"))
        third = self.mutate("--max-mutants", "3", "--cache-dir", cache)
        self.assertEqual([m["cached"] for m in third["mutants"]], [True, False, True])

    def test_a_failing_baseline_makes_errors(self):
        path = os.path.join(self.root, "tests", "test_account.py")
        with open(path, "a", encoding="utf-8") as stream:
            stream.write("\n\nclass Broken(unittest.TestCase):\n    def test_broken(self):\n        self.fail('no')\n")
        document = self.mutate("--max-mutants", "2")
        self.assertEqual([m["status"] for m in document["mutants"]], ["error", "error"])
        self.assertTrue(any("fail on the unmodified code (Broken::test_broken failed)" in d
                            for d in document["diagnostics"]), document["diagnostics"])

    def test_the_timeout_leaves_out_what_was_not_tested(self):
        code, document, stderr = run(["mutate", "--dir", self.root, "--timeout", "4s",
                                      "--changed", os.path.join(self.root, "changed.json"), "--coverage", self.coverage,
                                      "--min-mutant-timeout", "60s", "--jobs", "4"])
        self.assertEqual(code, 0, stderr)
        self.assertEqual(validate(document, MUTATION), [])
        self.assertFalse(any(m["line"] == 23 for m in document["mutants"]))  # the endless loop was cut
        self.assertTrue(any(d.startswith("the run stopped after 4s: ") for d in document["diagnostics"]),
                        document["diagnostics"])
        self.assertLess(document["summary"]["durationMs"], 4000)

    def test_nothing_to_mutate(self):
        with open(self.coverage, "w", encoding="utf-8") as stream:
            json.dump({"protocol": 1, "blocks": []}, stream)
        document = self.mutate()
        self.assertEqual((document["mutants"], document["summary"]["candidates"]), ([], 0))

    def test_usage(self):
        changed = os.path.join(self.root, "changed.json")
        for args in (
            ["mutate", "--dir", self.root, "--changed", changed],
            ["mutate", "--dir", self.root, "--changed", changed, "--coverage", changed],
            ["mutate", "--dir", self.root, "--changed", changed, "--coverage", self.coverage, "--jobs", "0"],
        ):
            code, _, _ = run(args)
            self.assertEqual(code, 2, args)
        with Tree({"app.py": "X = 1\n"}) as tree:
            code, document, stderr = run(["prepare", "--dir", tree.root, "--tools", os.path.join(self.scratch, "tools")])
        self.assertEqual((code, document), (0, {"protocol": 1, "writable": []}), stderr)
        self.assertFalse(os.path.exists(os.path.join(self.scratch, "tools")))  # Python needs no engine


if __name__ == "__main__":
    unittest.main()
