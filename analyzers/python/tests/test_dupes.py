"""The dupes command: tokens, windows, pairs, groups, and the fixture end to end."""

import json
import os
import sys
import unittest
from typing import Optional
from unittest import mock

from difflens_analyzer import cli, dupes
from difflens_analyzer.dupes import (
    Exclude, FileTokens, Search, Side, count_added, group, is_new, merge_ranges, tokens_of,
)
from tests.helpers import TESTDATA, Tree, run, schema, validate

FIXTURE = os.path.join(TESTDATA, "dupes")
CHANGED = os.path.join(FIXTURE, "changed.json")
DUPES = schema("dupes.schema.json")
# A helper of 11 lines and 110 tokens, enough for the default minimums.
HELPER = "def calculate_shipping():\n" + "".join(f"    v{i} = f(a, b[{i}], c.d)\n" for i in range(10))


def texts(source: str) -> list[list[str]]:
    return [[text for text, _, _ in split] for split in tokens_of(source.encode())]


def search(root: str, changed: dict, *extra: str, exclude: Optional[dict] = None) -> dict:
    path = os.path.join(root, ".changed.json")
    document = {"protocol": 1, "files": [{"path": p, "added": a} for p, a in changed.items()]}
    if exclude is not None:
        document["exclude"] = exclude
    with open(path, "w", encoding="utf-8") as stream:
        json.dump(document, stream)
    code, document, stderr = run(["dupes", "--dir", root, "--changed", path, *extra])
    assert code == 0, stderr
    assert validate(document, DUPES) == [], validate(document, DUPES)
    return document


def ranges(document: dict) -> list[tuple]:
    out = []
    for d in document["duplicates"]:
        block = (d["block"]["file"], d["block"]["startLine"], d["block"]["endLine"])
        others = [(o["file"], o["startLine"], o["endLine"]) for o in d["others"]]
        out.append((d["scope"], d["kind"], block, others, d["lines"]))
    return out


class Tokens(unittest.TestCase):
    def test_normalization(self):
        source = 'def total(xs, *, start=0):  # comment\n    return sum(xs, start) + 1.5 if True else None\n'
        self.assertEqual(texts(source), [[
            "def", "ID", "(", "ID", ",", "*", ",", "ID", "=", "NUM", ")", ":", "NEWLINE",
            "INDENT", "return", "ID", "(", "ID", ",", "ID", ")", "+", "NUM", "if", "True", "else", "None",
            "NEWLINE", "DEDENT",
        ]])
        # Soft keywords are identifiers; strings, bytes, f-strings are one STR each.
        self.assertEqual(texts('match = type = _ = 1\nx = f"a{b!r:>{w}}c" b"d" rb\'e\'\n'), [[
            "ID", "=", "ID", "=", "ID", "=", "NUM", "NEWLINE",
            "ID", "=", "STR", "STR", "STR", "NEWLINE",
        ]])

    def test_lines(self):
        tokens = tokens_of(b'x = """a\nb\nc"""\n\nif x:\n    y = 0x1F\n')[0]
        self.assertEqual(tokens[:3], [("ID", 1, 1), ("=", 1, 1), ("STR", 1, 3)])
        # NEWLINE, INDENT and DEDENT are compared but have no line.
        self.assertEqual([t for t in tokens if t[0] in ("NEWLINE", "INDENT", "DEDENT")], [
            ("NEWLINE", 0, 0), ("NEWLINE", 0, 0), ("INDENT", 0, 0), ("NEWLINE", 0, 0), ("DEDENT", 0, 0),
        ])
        self.assertEqual(tokens[-3], ("NUM", 6, 6))

    @unittest.skipIf(sys.version_info < (3, 10), "match needs Python 3.10")
    def test_a_match_statement_is_identifiers(self):
        self.assertEqual(texts("match x:\n    case 1:\n        pass\n")[0][:3], ["ID", "ID", ":"])

    def test_imports_split_the_file(self):
        source = (
            '"""Doc."""\n'
            "from __future__ import annotations\n"
            "import os, sys\n"
            "x = 1\n"
            "try:\n"
            "    from a import (\n"
            "        b,\n"
            "        c,\n"
            "    )\n"
            "except ImportError:\n"
            "    b = None\n"
            "def f():\n"
            "    yield from g()\n"
            "    raise E from None\n"
            "    import re; y = 2\n"
        )
        self.assertEqual(texts(source), [
            ["STR", "NEWLINE"],
            ["ID", "=", "NUM", "NEWLINE", "try", ":", "NEWLINE", "INDENT"],
            ["DEDENT", "except", "ID", ":", "NEWLINE", "INDENT", "ID", "=", "None", "NEWLINE", "DEDENT",
             "def", "ID", "(", ")", ":", "NEWLINE", "INDENT",
             "yield", "from", "ID", "(", ")", "NEWLINE", "raise", "ID", "from", "None", "NEWLINE"],
            ["ID", "=", "NUM", "NEWLINE", "DEDENT"],
        ])

    def test_a_file_that_does_not_tokenize_is_a_diagnostic(self):
        with self.assertRaises(dupes.TokenizeProblem):
            tokens_of(b'x = """never closed\n')
        with Tree({"ok.py": "x = 1\n", "bad.py": 'x = """never closed\n'}) as tree:
            document = search(tree.root, {"bad.py": [[1, 1]], "ok.py": [[1, 1]]})
        self.assertEqual(document["duplicates"], [])
        self.assertEqual(len(document["diagnostics"]), 1)
        self.assertTrue(document["diagnostics"][0].startswith("bad.py: cannot tokenize the file: "))


class Lines(unittest.TestCase):
    def test_merge_ranges(self):
        self.assertEqual(merge_ranges([(5, 9), (1, 3), (3, 4), (11, 12)]), [(1, 4), (5, 9), (11, 12)])
        self.assertEqual(merge_ranges([(5, 9), (1, 4), (11, 12), (9, 8)], adjacent=True), [(1, 9), (11, 12)])

    def test_new_when_at_least_half_the_lines_are_added(self):
        added = [(3, 4), (10, 20)]
        self.assertEqual(count_added(added, 1, 12), 5)
        self.assertTrue(is_new(added, 1, 4))  # 2 of 4
        self.assertFalse(is_new(added, 1, 5))  # 2 of 5
        self.assertFalse(is_new(added, 15, 30))  # 6 of 16
        self.assertTrue(is_new([(15, 22)], 15, 30))  # 8 of 16
        self.assertFalse(is_new([], 1, 1))


class Groups(unittest.TestCase):
    def test_new_sides_of_a_file_merge_into_blocks(self):
        pairs = [
            (Side("a.py", 10, 20, True), Side("old.py", 1, 11, False), 60),
            (Side("a.py", 15, 30, True), Side("old.py", 5, 20, False), 80),
            (Side("a.py", 40, 50, True), Side("old.py", 100, 110, False), 55),
        ]
        self.assertEqual(group(pairs, "production"), [
            {"kind": "copies_existing", "scope": "production", "block": {"file": "a.py", "startLine": 10, "endLine": 30},
             "others": [{"file": "old.py", "startLine": 1, "endLine": 20}], "tokens": 80, "lines": 21},
            {"kind": "copies_existing", "scope": "production", "block": {"file": "a.py", "startLine": 40, "endLine": 50},
             "others": [{"file": "old.py", "startLine": 100, "endLine": 110}], "tokens": 55, "lines": 11},
        ])

    def test_repeated_blocks_join_transitively(self):
        pairs = [
            (Side("b.py", 30, 40, True), Side("c.py", 1, 11, True), 70),
            (Side("a.py", 5, 15, True), Side("b.py", 30, 40, True), 60),
            # A block that also copies existing code is a copy, not a repetition.
            (Side("d.py", 1, 9, True), Side("c.py", 1, 9, True), 90),
            (Side("d.py", 1, 9, True), Side("old.py", 1, 9, False), 90),
        ]
        self.assertEqual(group(pairs, "test"), [
            {"kind": "repeated_in_change", "scope": "test", "block": {"file": "a.py", "startLine": 5, "endLine": 15},
             "others": [{"file": "b.py", "startLine": 30, "endLine": 40}, {"file": "c.py", "startLine": 1, "endLine": 11}],
             "tokens": 70, "lines": 11},
            {"kind": "copies_existing", "scope": "test", "block": {"file": "d.py", "startLine": 1, "endLine": 9},
             "others": [{"file": "old.py", "startLine": 1, "endLine": 9}], "tokens": 90, "lines": 9},
        ])

    def test_a_block_repeating_only_a_copy_is_no_group(self):
        pairs = [
            (Side("a.py", 1, 9, True), Side("b.py", 1, 9, True), 60),
            (Side("b.py", 1, 9, True), Side("old.py", 1, 9, False), 60),
        ]
        self.assertEqual([(g["kind"], g["block"]["file"]) for g in group(pairs, "production")], [("copies_existing", "b.py")])


class Pairs(unittest.TestCase):
    def files(self, sources: list[tuple[str, str, list]], size: int) -> tuple[Search, list[FileTokens]]:
        found = Search(size)
        files = [
            FileTokens(index, rel, tokens_of(source.encode()), found.kinds, added)
            for index, (rel, source, added) in enumerate(sources)
        ]
        for tokens in files:
            found.add_candidates(tokens)
        for tokens in files:
            found.collect(tokens)
        return found, files

    def test_a_long_clone_is_one_maximal_pair(self):
        body = "".join(f"    v{i} = f(a, b[{i}], c.d)\n" for i in range(200))
        found, files = self.files([
            ("new.py", f"def g():\n{body}", [(1, 201)]),
            ("old.py", f"class K:\n    pass\ndef h():\n{body}x = 1\n", []),
        ], 50)
        pairs = found.pairs(float("inf"))
        self.assertEqual(len(pairs), 1)
        a, first_a, last_a, b, first_b, last_b = pairs[0]
        self.assertEqual((a.side(first_a, last_a), b.side(first_b, last_b)), (
            Side("new.py", 1, 201, True), Side("old.py", 3, 203, False)
        ))

    def test_the_sides_of_a_pair_never_overlap(self):
        step = "x = f(a, b, c, d, e)\n"
        found, files = self.files([("new.py", step * 40, [(1, 40)])], 30)
        for a, _first_a, last_a, b, first_b, _last_b in found.pairs(float("inf")):
            self.assertIs(a, b)
            self.assertLess(last_a, first_b)

    def test_out_of_time_reports_what_was_found(self):
        found, _ = self.files([("new.py", "x = f(a, b, c)\n" * 4, [(1, 4)])], 8)
        self.assertEqual(found.pairs(0.0), [])
        self.assertTrue(found.stopped)


class Fixture(unittest.TestCase):
    """testdata/dupes: what the branch adds, per changed.json."""

    def dupes(self, *extra: str) -> dict:
        code, document, stderr = run(["dupes", "--dir", FIXTURE, "--changed", CHANGED, *extra])
        self.assertEqual(code, 0, stderr)
        self.assertEqual(validate(document, schema("dupes.schema.json")), [])
        return document

    def test_the_changed_file_matches_the_schema(self):
        with open(CHANGED, encoding="utf-8") as stream:
            self.assertEqual(validate(json.load(stream), schema("dupes.schema.json", "changed")), [])

    def test_copies_and_repetitions(self):
        document = self.dupes()
        self.assertEqual(
            {k: document[k] for k in ("protocol", "language", "minTokens", "minLines", "diagnostics")},
            {"protocol": 1, "language": "python", "minTokens": 50, "minLines": 6, "diagnostics": []},
        )
        # Not reported: the helper copied below the thresholds (summarize: 3
        # lines; vat: 20 tokens), legacy.py's two identical functions (no
        # added line), the import blocks reports.py and legacy.py share, and
        # api_client.py (generated).
        self.assertEqual(ranges(document), [
            ("production", "copies_existing", ("shop/orders.py", 17, 27), [("shop/pricing.py", 13, 23)], 11),
            ("production", "repeated_in_change", ("shop/reports.py", 25, 32),
             [("shop/reports.py", 38, 45), ("shop/reports.py", 52, 59)], 8),
        ])
        self.assertEqual([d["tokens"] for d in document["duplicates"]], [108, 102])

    def test_the_minimums_change_the_result(self):
        self.assertEqual(ranges(self.dupes("--min-tokens", "500"))[:1], [])
        loose = ranges(self.dupes("--min-lines", "3"))
        self.assertIn(("production", "copies_existing", ("shop/orders.py", 33, 35), [("shop/pricing.py", 30, 32)], 3), loose)
        self.assertEqual(len(loose), 3)
        self.assertEqual(self.dupes("--min-lines=3", "--min-tokens=50")["minLines"], 3)


class Limits(unittest.TestCase):
    STATEMENT = "total = compute(first, second, third)\n"  # 11 tokens

    def copies(self, existing: int) -> dict:
        files = {f"old{i}.py": self.STATEMENT for i in range(existing)}
        files["new.py"] = self.STATEMENT
        with Tree(files) as tree:
            return search(tree.root, {"new.py": [[1, 1]]}, "--min-tokens", "11", "--min-lines", "1")

    def test_a_bucket_of_more_than_64_windows_is_boilerplate(self):
        kept = self.copies(63)["duplicates"]
        self.assertEqual(len(kept), 1)
        self.assertEqual((kept[0]["kind"], len(kept[0]["others"]), kept[0]["tokens"]), ("copies_existing", 63, 11))
        self.assertEqual(self.copies(64)["duplicates"], [])

    def test_both_sides_must_reach_the_minimum_lines(self):
        spread = "total = compute(\n    first,\n    second,\n    third\n)\n"  # 5 lines, same tokens
        with Tree({"old.py": spread, "new.py": self.STATEMENT}) as tree:
            flags = ("--min-tokens", "11")
            self.assertEqual(search(tree.root, {"new.py": [[1, 1]]}, *flags, "--min-lines", "2")["duplicates"], [])
            found = search(tree.root, {"new.py": [[1, 1]]}, *flags, "--min-lines", "1")["duplicates"]
        self.assertEqual(ranges({"duplicates": found}),
                         [("production", "copies_existing", ("new.py", 1, 1), [("old.py", 1, 5)], 1)])

    def test_large_and_generated_files_are_left_out(self):
        with Tree({
            "old.py": HELPER,
            "big.py": HELPER + "#" * (1 << 20),
            "gen.py": f"# Code generated by hand. DO NOT EDIT.\n{HELPER}",
            "new.py": HELPER,
        }) as tree:
            document = search(tree.root, {"new.py": [[1, 11]], "big.py": [[1, 11]], "gen.py": [[1, 12]]})
        self.assertEqual(ranges(document), [("production", "copies_existing", ("new.py", 1, 11), [("old.py", 1, 11)], 11)])

    def test_the_cap_is_per_scope(self):
        other = HELPER.replace("calculate_shipping():", "calculate_tax():\n    total = 0").replace("c.d", "c[d]")
        with Tree({
            "shop/a.py": HELPER, "shop/new_a.py": HELPER,
            "shop/b.py": other, "shop/new_b.py": other,
            "tests/test_a.py": HELPER, "tests/test_new.py": HELPER,
        }) as tree:
            added = {"shop/new_a.py": [[1, 11]], "shop/new_b.py": [[1, 12]], "tests/test_new.py": [[1, 11]]}
            self.assertEqual([d["scope"] for d in search(tree.root, added)["duplicates"]],
                             ["production", "production", "test"])
            with mock.patch.object(dupes, "MAX_GROUPS", 1):
                document = search(tree.root, added)
        self.assertEqual(ranges(document), [
            ("production", "copies_existing", ("shop/new_a.py", 1, 11), [("shop/a.py", 1, 11)], 11),
            ("test", "copies_existing", ("tests/test_new.py", 1, 11), [("tests/test_a.py", 1, 11)], 11),
        ])
        self.assertEqual(document["diagnostics"], ["2 production duplicates found: only the first 1 are reported"])


class Scopes(unittest.TestCase):
    """Production and test code are searched apart: no clone pairs the two."""

    def test_a_test_copying_production_code_is_no_duplicate(self):
        for files, added in (
            ({"shop/rates.py": HELPER, "tests/test_rates.py": HELPER}, {"tests/test_rates.py": [[1, 11]]}),
            ({"conftest.py": HELPER, "shop/rates.py": HELPER}, {"shop/rates.py": [[1, 11]]}),
            ({"shop/rates.py": HELPER, "rates_test.py": HELPER}, {"shop/rates.py": [[1, 11]], "rates_test.py": [[1, 11]]}),
        ):
            with self.subTest(files=sorted(files)), Tree(files) as tree:
                self.assertEqual(search(tree.root, added)["duplicates"], [])

    def test_two_tests_copying_each_other_are_a_test_duplicate(self):
        # The production copy is existing code, yet in the other scope: the
        # two new test files repeat each other, they do not copy it.
        with Tree({"shop/rates.py": HELPER, "tests/test_a.py": HELPER, "test/test_b.py": HELPER}) as tree:
            document = search(tree.root, {"tests/test_a.py": [[1, 11]], "test/test_b.py": [[1, 11]]})
        self.assertEqual(ranges(document), [
            ("test", "repeated_in_change", ("test/test_b.py", 1, 11), [("tests/test_a.py", 1, 11)], 11),
        ])

    def test_production_groups_come_first(self):
        # By file the test group (conftest.py) would sort first.
        with Tree({"a_test.py": HELPER, "conftest.py": HELPER, "shop/a.py": HELPER, "shop/b.py": HELPER}) as tree:
            document = search(tree.root, {"conftest.py": [[1, 11]], "shop/b.py": [[1, 11]]})
        self.assertEqual(ranges(document), [
            ("production", "copies_existing", ("shop/b.py", 1, 11), [("shop/a.py", 1, 11)], 11),
            ("test", "copies_existing", ("conftest.py", 1, 11), [("a_test.py", 1, 11)], 11),
        ])


class Excluded(unittest.TestCase):
    EXCLUDE = {"dirs": ["locales"], "extensions": [".l10n.py"]}

    def test_the_rule(self):
        excluded = Exclude({"dirs": ["Locales", "i18n"], "extensions": [".L10N.py"]})
        for rel in ("locales/a.py", "web/LOCALES/b.py", "web/i18n/x/c.py", "labels.l10n.py", "web/Labels.L10N.PY"):
            self.assertTrue(excluded(rel), rel)
        for rel in ("locales.py", "web/locales_old/a.py", "l10n.py", "web/labels.py"):
            self.assertFalse(excluded(rel), rel)
        self.assertFalse(Exclude(None)("locales/a.py"))
        self.assertFalse(Exclude({"dirs": [], "extensions": []})("locales/a.py"))

    def test_excluded_files_are_left_out_of_the_search(self):
        with Tree({
            "new.py": HELPER,
            "locales/en.py": HELPER,
            "web/Locales/pt.py": HELPER,
            "shop/labels.L10N.py": HELPER,
            "locales.py": HELPER,  # the file name is not a folder
        }) as tree:
            added = {"new.py": [[1, 11]], "locales/en.py": [[1, 11]], "shop/labels.L10N.py": [[1, 11]]}
            document = search(tree.root, added, exclude=self.EXCLUDE)
            kept = search(tree.root, added)
        self.assertEqual(ranges(document), [
            ("production", "copies_existing", ("new.py", 1, 11), [("locales.py", 1, 11)], 11),
        ])
        # Without exclude, the catalogs are code like any other.
        self.assertEqual([d["block"]["file"] for d in kept["duplicates"]], ["locales/en.py", "new.py", "shop/labels.L10N.py"])

    def test_mutate_reads_and_ignores_exclude(self):
        with Tree({
            "locales/en.py": "X = 1\n",
            "changed.json": json.dumps({
                "protocol": 1, "exclude": self.EXCLUDE, "files": [{"path": "locales/en.py", "added": [[1, 2]]}],
            }),
            "coverage.json": '{"protocol": 1, "blocks": []}',
        }) as tree:
            self.assertEqual(cli.read_changed(tree.path("changed.json"), "mutate"),
                             ({"locales/en.py": [(1, 2)]}, self.EXCLUDE))
            # Nothing covered, so nothing runs: the changed file was accepted.
            code, document, stderr = run(["mutate", "--dir", tree.root, "--changed", tree.path("changed.json"),
                                          "--coverage", tree.path("coverage.json")])
        self.assertEqual((code, document["mutants"]), (0, []), stderr)


class CommandLine(unittest.TestCase):
    def test_usage_errors(self):
        self.assertEqual(run(["dupes", "--dir", FIXTURE])[0], 2)
        self.assertEqual(run(["dupes", "--dir", FIXTURE, "--changed", "/nonexistent.json"])[0], 2)
        for flag in ("--min-tokens", "--min-lines"):
            self.assertEqual(run(["dupes", "--dir", FIXTURE, "--changed", CHANGED, flag, "0"])[0], 2)
            self.assertEqual(run(["dupes", "--dir", FIXTURE, "--changed", CHANGED, flag, "x"])[0], 2)
        self.assertEqual(run(["dupes", "--dir", FIXTURE, "--changed", CHANGED, "--timeout", "0s"])[0], 2)

    def test_invalid_changed_files(self):
        cases = {
            "not json": "dupes: cannot read the changed lines",
            '{"protocol": 2, "files": []}': "$.protocol is 2, not 1",
            '{"protocol": 1}': "$.files is missing",
            '{"protocol": 1, "files": [], "extra": 1}': "$.extra is not allowed",
            '{"protocol": 1, "files": [{"path": "a.py", "added": [], "x": 1}]}': "$.files[0].x is not allowed",
            '{"protocol": 1, "files": [{"path": "a.py", "added": [[0, 2]]}]}': "is not a [start, end] pair",
            '{"protocol": 1, "files": [{"path": "a.py", "added": [[1, 2, 3]]}]}': "is not a [start, end] pair",
            '{"protocol": true, "files": []}': "$.protocol is True",
            '{"protocol": 1, "files": [], "exclude": []}': "$.exclude is not an object",
            '{"protocol": 1, "files": [], "exclude": {"dirs": []}}': "$.exclude.extensions is missing",
            '{"protocol": 1, "files": [], "exclude": {"dirs": [], "extensions": [], "x": 1}}': "$.exclude.x is not allowed",
            '{"protocol": 1, "files": [], "exclude": {"dirs": [1], "extensions": []}}': "$.exclude.dirs is not an array",
            '{"protocol": 1, "files": [], "exclude": {"dirs": [], "extensions": "x"}}': "$.exclude.extensions is not an",
        }
        for text, message in cases.items():
            with Tree({"changed.json": text}) as tree:
                code, _, stderr = run(["dupes", "--dir", tree.root, "--changed", tree.path("changed.json")])
            self.assertEqual(code, 2, text)
            self.assertIn(message, stderr, text)

    def test_missing_tree_fails(self):
        code, _, stderr = run(["dupes", "--dir", "/nonexistent/tree", "--changed", CHANGED])
        self.assertEqual(code, 1)
        self.assertIn("is not a directory", stderr)

    def test_nothing_changed_is_nothing_found(self):
        with Tree({"a.py": "x = 1\n", "changed.json": '{"protocol": 1, "files": []}'}) as tree:
            code, document, _ = run(["dupes", "--dir", tree.root, "--changed", tree.path("changed.json")])
        self.assertEqual((code, document["duplicates"], document["diagnostics"]), (0, [], []))
        self.assertEqual(validate(document, schema("dupes.schema.json")), [])


if __name__ == "__main__":
    unittest.main()
