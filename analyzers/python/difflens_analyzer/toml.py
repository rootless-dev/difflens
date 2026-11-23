"""Reading TOML files (pyproject.toml, uv.lock, ruff.toml, Pipfile).

Python 3.11 and later have `tomllib`; on 3.9 and 3.10 a small reader of the
same language is used instead, so the analyzer needs no dependency. It reads
everything these files use: tables, arrays of tables, dotted and quoted
keys, strings of the four kinds, numbers, booleans, arrays and inline
tables. Dates are kept as strings. A file that cannot be read is an empty
table: the callers treat a missing setting and an unreadable one alike.
"""

from __future__ import annotations

import re
from typing import Any

try:  # Python 3.11+
    import tomllib as _tomllib
except ImportError:  # pragma: no cover - exercised on old interpreters only
    _tomllib = None  # type: ignore[assignment]


class TOMLError(ValueError):
    """The text is not TOML the reader understands."""


def loads(text: str) -> dict[str, Any]:
    if _tomllib is not None:
        try:
            return _tomllib.loads(text)
        except _tomllib.TOMLDecodeError as error:
            raise TOMLError(str(error)) from error
    return parse(text)


def load_file(path: str) -> dict[str, Any]:
    """The table in the file at path, or {} when it is missing or unreadable."""
    try:
        with open(path, encoding="utf-8") as stream:
            return loads(stream.read())
    except (OSError, UnicodeDecodeError, TOMLError):
        return {}


def parse(text: str) -> dict[str, Any]:
    """The fallback reader, also used by the tests on recent interpreters."""
    return _Reader(text).document()


_BARE_KEY = re.compile(r"[A-Za-z0-9_-]+")
_NUMBER = re.compile(r"[+-]?(0x[0-9A-Fa-f_]+|0o[0-7_]+|0b[01_]+|inf|nan|[0-9_]+(\.[0-9_]+)?([eE][+-]?[0-9_]+)?)")
_DATE = re.compile(r"\d{4}-\d{2}-\d{2}([Tt ]\d{2}:\d{2}(:\d{2}(\.\d+)?)?([Zz]|[+-]\d{2}:\d{2})?)?|\d{2}:\d{2}:\d{2}(\.\d+)?")
_ESCAPES = {"b": "\b", "t": "\t", "n": "\n", "f": "\f", "r": "\r", '"': '"', "\\": "\\"}


class _Reader:
    def __init__(self, text: str) -> None:
        self.text = text
        self.pos = 0

    # Low-level helpers.

    def error(self, what: str) -> TOMLError:
        line = self.text.count("\n", 0, self.pos) + 1
        return TOMLError(f"{what} at line {line}")

    def peek(self, n: int = 1) -> str:
        return self.text[self.pos : self.pos + n]

    def skip_spaces(self) -> None:
        while self.pos < len(self.text) and self.text[self.pos] in " \t":
            self.pos += 1

    def skip_comment(self) -> None:
        if self.peek() == "#":
            end = self.text.find("\n", self.pos)
            self.pos = len(self.text) if end < 0 else end

    def skip_blank(self) -> None:
        """Spaces, newlines and comments, as allowed inside arrays."""
        while True:
            self.skip_spaces()
            self.skip_comment()
            if self.peek() in ("\n", "\r"):
                self.pos += 1
                continue
            return

    def end_of_line(self) -> None:
        self.skip_spaces()
        self.skip_comment()
        if self.pos < len(self.text):
            if self.peek(2) == "\r\n":
                self.pos += 2
            elif self.peek() == "\n":
                self.pos += 1
            else:
                raise self.error("expected the end of the line")

    # Structure.

    def document(self) -> dict[str, Any]:
        root: dict[str, Any] = {}
        current = root
        while True:
            self.skip_blank()
            if self.pos >= len(self.text):
                return root
            if self.peek(2) == "[[":
                self.pos += 2
                keys = self.key()
                self.expect("]]")
                parent = self.table_at(root, keys[:-1])
                items = parent.setdefault(keys[-1], [])
                if not isinstance(items, list):
                    raise self.error("not an array of tables")
                current = {}
                items.append(current)
            elif self.peek() == "[":
                self.pos += 1
                keys = self.key()
                self.expect("]")
                current = self.table_at(root, keys)
            else:
                keys = self.key()
                self.skip_spaces()
                self.expect("=")
                self.skip_spaces()
                value = self.value()
                self.table_at(current, keys[:-1])[keys[-1]] = value
            self.end_of_line()

    def expect(self, token: str) -> None:
        self.skip_spaces()
        if not self.text.startswith(token, self.pos):
            raise self.error(f"expected {token!r}")
        self.pos += len(token)

    def table_at(self, root: dict[str, Any], keys: list[str]) -> dict[str, Any]:
        table = root
        for key in keys:
            child = table.setdefault(key, {})
            if isinstance(child, list) and child and isinstance(child[-1], dict):
                child = child[-1]
            if not isinstance(child, dict):
                raise self.error(f"{key} is not a table")
            table = child
        return table

    def key(self) -> list[str]:
        keys = []
        while True:
            self.skip_spaces()
            char = self.peek()
            if char == '"':
                keys.append(self.basic_string())
            elif char == "'":
                keys.append(self.literal_string())
            else:
                match = _BARE_KEY.match(self.text, self.pos)
                if match is None:
                    raise self.error("expected a key")
                keys.append(match.group())
                self.pos = match.end()
            self.skip_spaces()
            if self.peek() != ".":
                return keys
            self.pos += 1

    # Values.

    def value(self) -> Any:
        char = self.peek()
        if self.text.startswith('"""', self.pos):
            return self.multiline_basic()
        if self.text.startswith("'''", self.pos):
            return self.multiline_literal()
        if char == '"':
            return self.basic_string()
        if char == "'":
            return self.literal_string()
        if char == "[":
            return self.array()
        if char == "{":
            return self.inline_table()
        for word, value in (("true", True), ("false", False)):
            if self.text.startswith(word, self.pos):
                self.pos += len(word)
                return value
        date = _DATE.match(self.text, self.pos)
        if date is not None:
            self.pos = date.end()
            return date.group()
        number = _NUMBER.match(self.text, self.pos)
        if number is not None and number.group():
            self.pos = number.end()
            return _number(number.group())
        raise self.error("expected a value")

    def array(self) -> list[Any]:
        self.pos += 1
        items = []
        while True:
            self.skip_blank()
            if self.peek() == "]":
                self.pos += 1
                return items
            items.append(self.value())
            self.skip_blank()
            if self.peek() == ",":
                self.pos += 1
            elif self.peek() != "]":
                raise self.error("expected ',' or ']'")

    def inline_table(self) -> dict[str, Any]:
        self.pos += 1
        table: dict[str, Any] = {}
        while True:
            self.skip_spaces()
            if self.peek() == "}":
                self.pos += 1
                return table
            keys = self.key()
            self.expect("=")
            self.skip_spaces()
            self.table_at(table, keys[:-1])[keys[-1]] = self.value()
            self.skip_spaces()
            if self.peek() == ",":
                self.pos += 1
            elif self.peek() != "}":
                raise self.error("expected ',' or '}'")

    def basic_string(self) -> str:
        self.pos += 1
        out = []
        while True:
            if self.pos >= len(self.text) or self.peek() == "\n":
                raise self.error("unterminated string")
            char = self.text[self.pos]
            if char == '"':
                self.pos += 1
                return "".join(out)
            if char == "\\":
                out.append(self.escape())
                continue
            out.append(char)
            self.pos += 1

    def escape(self) -> str:
        code = self.text[self.pos + 1 : self.pos + 2]
        if code in _ESCAPES:
            self.pos += 2
            return _ESCAPES[code]
        if code in ("u", "U"):
            size = 4 if code == "u" else 8
            digits = self.text[self.pos + 2 : self.pos + 2 + size]
            self.pos += 2 + size
            try:
                return chr(int(digits, 16))
            except ValueError as error:
                raise self.error("bad unicode escape") from error
        raise self.error("bad escape")

    def literal_string(self) -> str:
        end = self.text.find("'", self.pos + 1)
        newline = self.text.find("\n", self.pos + 1)
        if end < 0 or (0 <= newline < end):
            raise self.error("unterminated string")
        value = self.text[self.pos + 1 : end]
        self.pos = end + 1
        return value

    def multiline_basic(self) -> str:
        self.pos += 3
        if self.peek() == "\n":
            self.pos += 1
        elif self.peek(2) == "\r\n":
            self.pos += 2
        out = []
        while True:
            if self.pos >= len(self.text):
                raise self.error("unterminated string")
            if self.text.startswith('"""', self.pos):
                # Up to two quotes may end the content before the delimiter.
                extra = 0
                while self.text.startswith('"', self.pos + 3 + extra) and extra < 2:
                    extra += 1
                out.append('"' * extra)
                self.pos += 3 + extra
                return "".join(out)
            char = self.text[self.pos]
            if char == "\\":
                rest = self.text[self.pos + 1 :]
                stripped = rest.lstrip(" \t")
                if stripped.startswith("\n") or stripped.startswith("\r\n"):
                    # A line-ending backslash trims the whitespace that follows.
                    self.pos = len(self.text) - len(rest.lstrip(" \t\r\n"))
                    continue
                out.append(self.escape())
                continue
            out.append(char)
            self.pos += 1

    def multiline_literal(self) -> str:
        start = self.pos + 3
        if self.text.startswith("\n", start):
            start += 1
        elif self.text.startswith("\r\n", start):
            start += 2
        end = self.text.find("'''", start)
        if end < 0:
            raise self.error("unterminated string")
        extra = 0
        while self.text.startswith("'", end + 3 + extra) and extra < 2:
            extra += 1
        end += extra
        value = self.text[start:end]
        self.pos = end + 3
        return value


def _number(text: str) -> Any:
    clean = text.replace("_", "")
    sign = -1 if clean.startswith("-") else 1
    body = clean.lstrip("+-")
    if body in ("inf", "nan"):
        return sign * float(body)
    for prefix, base in (("0x", 16), ("0o", 8), ("0b", 2)):
        if body.startswith(prefix):
            return sign * int(body[2:], base)
    if any(c in body for c in ".eE"):
        return float(clean)
    return int(clean)
