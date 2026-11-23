"""Shared helpers: temporary trees and running the analyzer in-process."""

from __future__ import annotations

import io
import json
import os
import tempfile
import textwrap
from typing import Any

from difflens_analyzer import cli

HERE = os.path.dirname(os.path.abspath(__file__))
TESTDATA = os.path.join(os.path.dirname(HERE), "testdata")
SCHEMAS = os.path.join(os.path.dirname(os.path.dirname(HERE)), "protocol")


class Tree:
    """A temporary source tree written from a {path: text} mapping."""

    def __init__(self, files: dict[str, str]) -> None:
        self._dir = tempfile.TemporaryDirectory(prefix="difflens-py-test-")
        self.root = self._dir.name
        for rel, text in files.items():
            path = os.path.join(self.root, rel)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w", encoding="utf-8") as stream:
                stream.write(textwrap.dedent(text).lstrip("\n"))

    def __enter__(self) -> "Tree":
        return self

    def __exit__(self, *exc: Any) -> None:
        self._dir.cleanup()

    def path(self, rel: str) -> str:
        return os.path.join(self.root, rel)


def run(args: list[str], env: dict[str, str] | None = None) -> tuple[int, Any, str]:
    """Runs the command line in-process: exit code, parsed stdout (or text), stderr."""
    stdout, stderr = io.StringIO(), io.StringIO()
    saved = dict(os.environ)
    try:
        os.environ["DIFFLENS_LOG_LEVEL"] = "error"
        if env:
            os.environ.update(env)
        code = cli.main(args, stdout, stderr)
    finally:
        os.environ.clear()
        os.environ.update(saved)
    text = stdout.getvalue()
    try:
        parsed: Any = json.loads(text) if text.strip().startswith("{") else text
    except ValueError:
        parsed = text
    return code, parsed, stderr.getvalue()


def analyze(root: str) -> dict[str, Any]:
    code, document, stderr = run(["analyze", "--dir", root])
    assert code == 0, stderr
    return document


def calls(document: dict[str, Any]) -> list[tuple[str, str, str, str, str]]:
    return [(c["caller"], c["callee"], c["kind"], c["resolution"], c["relation"]) for c in document["calls"]]


def functions(document: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {f["id"]: f for f in document["functions"]}


def entries(document: dict[str, Any], function_id: str) -> list[tuple[str, str]]:
    return [(e["kind"], e["label"]) for e in functions(document)[function_id].get("entryPoints", [])]


_TYPES = {
    "object": dict, "array": list, "string": str, "boolean": bool, "null": type(None),
    "integer": int, "number": (int, float),
}


def schema(name: str, definition: str | None = None) -> dict[str, Any]:
    with open(os.path.join(SCHEMAS, name), encoding="utf-8") as stream:
        root = json.load(stream)
    return {"root": root, "node": root["$defs"][definition] if definition else root}


def validate(document: Any, loaded: dict[str, Any]) -> list[str]:
    """Errors of document against the subset of JSON Schema the protocol uses."""
    root = loaded["root"]
    errors: list[str] = []

    def check(value: Any, node: dict[str, Any], where: str) -> None:
        if "$ref" in node:
            target = root
            for part in node["$ref"].lstrip("#/").split("/"):
                target = target[part]
            check(value, target, where)
            return
        if "oneOf" in node:
            matches = [option for option in node["oneOf"] if not _errors(value, option)]
            if len(matches) != 1:
                errors.append(f"{where}: matches {len(matches)} of oneOf")
            return
        if "const" in node and value != node["const"]:
            errors.append(f"{where}: {value!r} is not {node['const']!r}")
        if "enum" in node and value not in node["enum"]:
            errors.append(f"{where}: {value!r} not in {node['enum']}")
        kinds = node.get("type")
        if kinds is not None:
            kinds = kinds if isinstance(kinds, list) else [kinds]
            ok = any(
                isinstance(value, _TYPES[k]) and not (k in ("integer", "number") and isinstance(value, bool))
                for k in kinds
            )
            if not ok:
                errors.append(f"{where}: {value!r} is not {kinds}")
                return
        if isinstance(value, (int, float)) and not isinstance(value, bool) and "minimum" in node:
            if value < node["minimum"]:
                errors.append(f"{where}: {value} < {node['minimum']}")
        if isinstance(value, dict):
            for key in node.get("required", []):
                if key not in value:
                    errors.append(f"{where}: {key} is missing")
            properties = node.get("properties", {})
            for key, item in value.items():
                if key in properties:
                    check(item, properties[key], f"{where}.{key}")
                elif node.get("additionalProperties") is False:
                    errors.append(f"{where}: {key} is not allowed")
        if isinstance(value, list):
            if len(value) < node.get("minItems", 0):
                errors.append(f"{where}: fewer than {node['minItems']} items")
            if "maxItems" in node and len(value) > node["maxItems"]:
                errors.append(f"{where}: more than {node['maxItems']} items")
            prefix = node.get("prefixItems", [])
            for index, item in enumerate(value):
                if index < len(prefix):
                    check(item, prefix[index], f"{where}[{index}]")
                elif "items" in node:
                    check(item, node["items"], f"{where}[{index}]")

    def _errors(value: Any, node: dict[str, Any]) -> list[str]:
        saved = list(errors)
        check(value, node, "")
        found = errors[len(saved):]
        del errors[len(saved):]
        return found

    check(document, loaded["node"], "$")
    return errors
