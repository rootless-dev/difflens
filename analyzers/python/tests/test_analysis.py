import os
import unittest

from tests.helpers import TESTDATA, Tree, analyze, calls, entries, functions, schema, validate

ANALYSIS = schema("analysis.schema.json")


class FixtureShapes(unittest.TestCase):
    """A src-layout package with classes, inheritance, decorators and async code."""

    @classmethod
    def setUpClass(cls):
        cls.doc = analyze(os.path.join(TESTDATA, "shapes"))
        cls.calls = calls(cls.doc)
        cls.functions = functions(cls.doc)

    def test_matches_the_schema(self):
        self.assertEqual(validate(self.doc, ANALYSIS), [])

    def test_ids_names_and_containers(self):
        f = self.functions["shapes.base.Square.of"]
        self.assertEqual(f["name"], "Square.of")
        self.assertEqual(f["qualifiedName"], "base.Square.of")
        self.assertEqual(f["container"], "shapes.base")
        self.assertEqual(f["file"], "src/shapes/base.py")
        self.assertEqual((f["startLine"], f["endLine"]), (29, 31))  # from the decorator
        self.assertIn("shapes.service.retry.decorate.wrapper", self.functions)
        self.assertIn("shapes.service.draw_all$1", self.functions)  # the lambda
        self.assertIn("shapes.cli.__main__", self.functions)

    def test_test_flag(self):
        self.assertTrue(self.functions["tests.test_shapes.test_circle_area"]["test"])
        self.assertFalse(self.functions["shapes.base.area_of"]["test"])

    def test_complexity(self):
        self.assertEqual(self.functions["shapes.base.area_of"]["complexity"], 3)  # if, ternary
        self.assertEqual(self.functions["shapes.service.retry.decorate.wrapper"]["complexity"], 3)  # for, except
        self.assertEqual(self.functions["shapes.service.Drawing.render"]["complexity"], 3)  # comprehension for and if

    def test_self_calls_dispatch_to_overrides(self):
        self.assertIn(("shapes.base.Shape.describe", "shapes.base.Shape.name", "always", "dynamic", "call"), self.calls)
        self.assertIn(("shapes.base.Shape.describe", "shapes.circle.Circle.name", "always", "dynamic", "call"), self.calls)
        # The abstract method is replaced by its implementations.
        self.assertNotIn(("shapes.base.Shape.describe", "shapes.base.Shape.area", "always", "dynamic", "call"), self.calls)
        self.assertIn(("shapes.base.Shape.describe", "shapes.circle.Circle.area", "always", "dynamic", "call"), self.calls)

    def test_static_calls_through_imports_and_classes(self):
        self.assertIn(("shapes.service.Drawing.total", "shapes.base.area_of", "conditional", "static", "call"), self.calls)
        self.assertIn(("shapes.service.fetch", "shapes.base.Square.of", "conditional", "static", "call"), self.calls)
        self.assertIn(("shapes.base.Square.of", "shapes.base.Square.__init__", "always", "static", "call"), self.calls)
        self.assertIn(("shapes.circle.TextRenderer.render", "shapes.base.Shape.describe", "always", "static", "call"), self.calls)
        self.assertIn(("tests.test_shapes.SquareCase.test_describe", "shapes.base.Shape.describe", "always", "static", "call"), self.calls)

    def test_protocol_calls_reach_compatible_implementations(self):
        self.assertIn(("shapes.service.Drawing.render", "shapes.circle.TextRenderer.render", "conditional", "dynamic", "call"), self.calls)
        self.assertNotIn(("shapes.service.Drawing.render", "shapes.service.Drawing.render", "conditional", "dynamic", "call"), self.calls)

    def test_async_calls(self):
        self.assertIn(("shapes.service.draw_all", "shapes.service.fetch", "async", "static", "call"), self.calls)
        self.assertIn(("shapes.service.draw_all", "shapes.service.Drawing.render", "async", "static", "call"), self.calls)  # Thread target
        self.assertIn(("shapes.service.Drawing.render", "shapes.service.Drawing.log", "async", "static", "call"), self.calls)  # submit
        self.assertIn(("shapes.service.draw_all", "asyncio.gather", "always", "static", "call"), self.calls)

    def test_references(self):
        self.assertIn(("shapes.service.draw_all", "shapes.service.draw_all$1", "async", "static", "reference"), self.calls)
        self.assertIn(("shapes.service.retry", "shapes.service.retry.decorate", "async", "static", "reference"), self.calls)

    def test_registrations(self):
        self.assertIn(("shapes.cli.main", "shapes.cli.list_shapes", "async", "static", "registration"), self.calls)
        self.assertIn(("shapes.cli.main", "shapes.cli.on_term", "async", "static", "registration"), self.calls)
        evidence = {c["evidence"] for c in self.doc["calls"] if c["relation"] == "registration"}
        self.assertEqual(evidence, {"command list", "signal SIGTERM"})

    def test_entry_points(self):
        self.assertIn(("main", "shapes"), entries(self.doc, "shapes.cli.main"))
        self.assertIn(("main", "shapes-admin"), entries(self.doc, "shapes.cli.cli"))
        self.assertIn(("cli_command", "cli draw"), entries(self.doc, "shapes.cli.draw_command"))
        self.assertEqual(entries(self.doc, "shapes.cli.__main__"), [("main", "python -m shapes.cli")])
        self.assertEqual(entries(self.doc, "shapes.cli.list_shapes"), [("cli_command", "list")])
        self.assertEqual(entries(self.doc, "shapes.cli.on_term"), [("message_handler", "SIGTERM")])
        self.assertEqual(entries(self.doc, "shapes.base.area_of"), [("exported_api", "shapes.area_of")])
        self.assertEqual(entries(self.doc, "tests.test_shapes.circle"), [("test", "fixture circle")])
        self.assertEqual(entries(self.doc, "tests.test_shapes.TestSquare.test_unit"), [("test", "TestSquare::test_unit")])
        self.assertEqual(entries(self.doc, "tests.test_shapes.SquareCase.setUp"), [("test", "SquareCase::setUp")])

    def test_externals(self):
        externals = {e["id"]: e for e in self.doc["externals"]}
        self.assertEqual(externals["concurrent.futures.ThreadPoolExecutor.submit"]["container"], "concurrent.futures")
        self.assertEqual(externals["concurrent.futures.ThreadPoolExecutor.submit"]["name"], "ThreadPoolExecutor.submit")
        self.assertEqual(externals["builtins.print"]["container"], "builtins")
        self.assertNotIn("builtins.len", externals)
        self.assertNotIn("builtins.sum", externals)


class FixtureWebapp(unittest.TestCase):
    """Routes, tasks and commands found from the source alone."""

    @classmethod
    def setUpClass(cls):
        cls.doc = analyze(os.path.join(TESTDATA, "webapp"))
        cls.calls = calls(cls.doc)

    def test_matches_the_schema(self):
        self.assertEqual(validate(self.doc, ANALYSIS), [])

    def test_fastapi(self):
        self.assertEqual(entries(self.doc, "app.api.get_order"), [("http_route", "GET /orders/{order_id}")])
        self.assertEqual(entries(self.doc, "app.api.create_order"), [("http_route", "POST /orders")])
        self.assertEqual(entries(self.doc, "app.api.create_app.health"), [("http_route", "GET /health")])
        self.assertEqual(entries(self.doc, "app.api.version"), [("http_route", "GET /version")])
        self.assertIn(("app.api.create_app", "app.api.create_app.health", "async", "static", "registration"), self.calls)
        self.assertIn(("app.api.create_order", "app.api.notify", "async", "static", "call"), self.calls)
        self.assertIn(("app.api.create_order", "app.tasks.send_receipt", "async", "static", "call"), self.calls)

    def test_flask(self):
        self.assertEqual(entries(self.doc, "app.web.index"), [("http_route", "GET /")])
        self.assertEqual(entries(self.doc, "app.web.users"), [("http_route", "GET /admin/users"), ("http_route", "POST /admin/users")])
        self.assertEqual(entries(self.doc, "app.web.ItemView.delete"), [("http_route", "DELETE /items/<int:item_id>")])
        self.assertEqual(entries(self.doc, "app.web.seed"), [("cli_command", "seed")])
        self.assertEqual(entries(self.doc, "app.web.authenticate"), [])

    def test_django(self):
        self.assertEqual(entries(self.doc, "app.views.home"), [("http_route", "/")])
        self.assertEqual(entries(self.doc, "app.views.ProfileView.post"), [("http_route", "POST /profiles/<int:pk>/")])
        self.assertEqual(entries(self.doc, "app.views.audit"), [("message_handler", "post_save")])
        route = functions(self.doc)["app.views.home"]["entryPoints"][0]
        self.assertEqual((route["file"], route["line"]), ("app/urls.py", 6))

    def test_celery_and_typer(self):
        self.assertEqual(entries(self.doc, "app.tasks.send_receipt"), [("message_handler", "receipts.send")])
        self.assertEqual(
            entries(self.doc, "app.tasks.cleanup"),
            [("message_handler", "app.tasks.cleanup"), ("scheduled_job", "nightly-cleanup")],
        )
        self.assertEqual(entries(self.doc, "app.manage.import_orders"), [("cli_command", "import-orders")])
        self.assertEqual(entries(self.doc, "app.manage.export_orders"), [("cli_command", "export")])


class Flow(unittest.TestCase):
    """always, conditional and async, by the Go analyzer's rule."""

    def kinds(self, body: str) -> dict[str, str]:
        with Tree({"m.py": body}) as tree:
            doc = analyze(tree.root)
        return {c["callee"].rsplit(".", 1)[-1]: c["kind"] for c in doc["calls"] if c["caller"] == "m.f"}

    def test_early_return_and_raise(self):
        kinds = self.kinds("""
            def a(): pass
            def b(): pass
            def c(): pass
            def d(): pass
            def f(x):
                a()
                if not x:
                    raise ValueError()
                b()
                if x > 1:
                    return c()
                d()
        """)
        self.assertEqual(kinds, {"a": "always", "b": "always", "c": "conditional", "d": "conditional"})

    def test_branches_loops_and_operators(self):
        kinds = self.kinds("""
            def a(): pass
            def b(): pass
            def c(): pass
            def d(): pass
            def e(): pass
            def g(): pass
            def f(xs):
                for x in a():
                    b()
                ok = c() and d()
                y = e() if ok else g()
        """)
        self.assertEqual(kinds, {"a": "always", "b": "conditional", "c": "always", "d": "conditional",
                                 "e": "conditional", "g": "conditional"})

    def test_try_and_endless_loops(self):
        kinds = self.kinds("""
            def a(): pass
            def b(): pass
            def c(): pass
            def d(): pass
            def e(): pass
            def f():
                try:
                    a()
                except OSError:
                    b()
                finally:
                    c()
                while True:
                    d()
                    if e():
                        continue
        """)
        self.assertEqual(kinds, {"a": "always", "b": "conditional", "c": "always", "d": "always", "e": "always"})

    def test_duck_typing_is_dynamic_and_bounded(self):
        with Tree({
            "repo.py": """
                class Sql:
                    def find_user(self, key): ...
                class Memory:
                    def find_user(self, key): ...
                    def get(self, key): ...
            """,
            "svc.py": """
                def f(repo):
                    repo.find_user(1)
                    repo.get(1)
            """,
        }) as tree:
            doc = analyze(tree.root)
        found = sorted((c["callee"], c["resolution"]) for c in doc["calls"] if c["caller"] == "svc.f")
        self.assertEqual(found, [("repo.Memory.find_user", "dynamic"), ("repo.Sql.find_user", "dynamic")])

    def test_relative_imports_star_and_annotations(self):
        with Tree({
            "pkg/__init__.py": "from .models import *\n",
            "pkg/models.py": """
                class User:
                    def save(self): ...
                    def rename(self, name): ...
                def make() -> "User":
                    return User()
            """,
            "pkg/service.py": """
                from typing import Optional
                from . import make, User
                def f(user: Optional[User]):
                    user.rename("x")
                    make().rename("y")
            """,
        }) as tree:
            doc = analyze(tree.root)
        found = sorted(c["callee"] for c in doc["calls"] if c["caller"] == "pkg.service.f")
        self.assertEqual(found, ["pkg.models.User.rename", "pkg.models.User.rename", "pkg.models.make"])

    def test_syntax_errors_are_diagnostics(self):
        with Tree({"ok.py": "def f():\n    return 1\n", "bad.py": "def g(:\n"}) as tree:
            doc = analyze(tree.root)
        self.assertEqual([f["id"] for f in doc["functions"]], ["ok.f"])
        (diagnostic,) = doc["diagnostics"]
        self.assertEqual((diagnostic["level"], diagnostic["file"], diagnostic["line"]), ("error", "bad.py", 1))
        self.assertIn("parsed with Python", diagnostic["message"])

    def test_ids_survive_moves_and_duplicates_are_numbered(self):
        before = {"m.py": "def a():\n    return lambda: 1\n\ndef b():\n    pass\n"}
        after = {"m.py": "def b():\n    x = 1\n    return x\n\n\ndef a():\n    return lambda: 2\n"}
        with Tree(before) as one, Tree(after) as two:
            ids_before = sorted(functions(analyze(one.root)))
            ids_after = sorted(functions(analyze(two.root)))
        self.assertEqual(ids_before, ids_after)
        self.assertEqual(ids_before, ["m.a", "m.a$1", "m.b"])
        with Tree({"m.py": "try:\n    def f(): pass\nexcept ImportError:\n    def f(): pass\n"}) as tree:
            self.assertEqual(sorted(functions(analyze(tree.root))), ["m.f", "m.f#2"])

    def test_generated_files(self):
        with Tree({
            "gen.py": "# Code generated by protoc-gen. DO NOT EDIT.\ndef f():\n    pass\n",
            "migrations/0001_initial.py": "# Generated by Django 5.1 on 2024-01-01\ndef g():\n    pass\n",
            "own.py": "# Not generated: written by hand\ndef h():\n    pass\n",
        }) as tree:
            doc = functions(analyze(tree.root))
        self.assertTrue(doc["gen.f"]["generated"])
        self.assertTrue(doc["migrations.0001_initial.g"]["generated"])
        self.assertFalse(doc["own.h"]["generated"])

    def test_deeply_nested_code_loses_its_calls_not_the_run(self):
        deep = "def f():\n    return g(" + "+".join(["1"] * 1500) + ")\n\ndef g(x):\n    return x\n"
        with Tree({"deep.py": deep, "ok.py": "import deep\ndef h():\n    deep.g(1)\n"}) as tree:
            doc = analyze(tree.root)
        self.assertIn(("ok.h", "deep.g", "always", "static", "call"), calls(doc))
        self.assertIn("deep.f", functions(doc))

    def test_analyze_does_not_write_in_the_tree(self):
        with Tree({"a.py": "import b\ndef f():\n    b.g()\n", "b.py": "def g():\n    pass\n"}) as tree:
            before = sorted(os.listdir(tree.root))
            doc = analyze(tree.root)
            self.assertEqual(sorted(os.listdir(tree.root)), before)
        self.assertIn(("a.f", "b.g", "always", "static", "call"), calls(doc))


if __name__ == "__main__":
    unittest.main()
