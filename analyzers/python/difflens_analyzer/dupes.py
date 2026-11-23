"""The dupes command: duplicated code that a review introduces.

The algorithm is the one in the description of
analyzers/protocol/dupes.schema.json, shared by every analyzer so that the
languages agree. Each step is a function of its own:

- find_duplicates: the files (the tree's Python files without those the
  exclude of --changed names, see Exclude), split into the production and
  the test scope by project.is_test_path, the rule analyze uses. The search
  below runs once per scope on that scope's files only (search_scope), so a
  clone never pairs test code with production code; production groups come
  first, and each scope is capped at MAX_GROUPS groups.
- tokens_of: a file's normalized tokens (identifiers, strings and numbers
  become placeholders), in splits: import statements are left out and end a
  split.
- Search.add_candidates and Search.collect: every run of --min-tokens tokens
  of a split is a window; the windows that touch the change are hashed first,
  then every window of every file, keeping only those whose hash is one of
  the first ones (the candidates) in buckets.
- Search.pairs: the windows of a bucket, two by two, extended into maximal
  pairs. A window pair lying inside a maximal pair already found on the same
  diagonal (same two files, same offset) is skipped, so a long clone is
  extended once and not once per window.
- group: the new sides merged into blocks, and the blocks grouped into
  copies of existing code and code repeated within the change.
"""

from __future__ import annotations

import io
import keyword
import os
import platform
import string
import time
import tokenize
import warnings
from array import array
from bisect import bisect_right
from typing import Any, Iterable, List, NamedTuple, Optional, Tuple, Union

from difflens_analyzer import protocol
from difflens_analyzer.log import Logger
from difflens_analyzer.project import SourceFile, discover, is_generated

PYTHON = platform.python_version()

# A bucket of more windows than this is boilerplate, not a copy.
WINDOW_CAP = 64
MAX_GROUPS = 500  # per scope
# The scopes in the order they are reported; a file is test code when
# SourceFile.test (project.is_test_path) says so.
PRODUCTION, TEST = "production", "test"
MAX_FILE_BYTES = 1 << 20
_POSITION = (1 << 32) - 1  # the position part of a packed window

# A normalized token: its text (ID, STR, NUM, NEWLINE, INDENT, DEDENT, a
# keyword or an operator), the line it starts on and the line it ends on.
Token = Tuple[str, int, int]
Ranges = List[Tuple[int, int]]

_SKIPPED = {tokenize.COMMENT, tokenize.NL, tokenize.ENCODING, tokenize.ENDMARKER}
_STRUCTURE = {tokenize.NEWLINE: "NEWLINE", tokenize.INDENT: "INDENT", tokenize.DEDENT: "DEDENT"}
# Python 3.12 splits f-strings (3.14 t-strings too) into parts; older
# versions read one STRING. Either way the whole literal is one STR.
_STRING_STARTS = {getattr(tokenize, name) for name in ("FSTRING_START", "TSTRING_START") if hasattr(tokenize, name)}
_STRING_ENDS = {getattr(tokenize, name) for name in ("FSTRING_END", "TSTRING_END") if hasattr(tokenize, name)}
# What comes right before a statement. `from` there starts an import, not
# `yield from` or `raise ... from`.
_STATEMENT_START = {"", "NEWLINE", "INDENT", "DEDENT", ";", ":"}


class TokenizeProblem(Exception):
    """A file the tokenizer rejects: a diagnostic, not a failure."""


def tokens_of(source: bytes) -> list[list[Token]]:
    """The file's normalized tokens, split at import statements (which are left out).

    Comments, blank lines and type comments are dropped; NEWLINE, INDENT and
    DEDENT are kept, as they are Python's braces, but have no line (0): they
    are compared like any token, yet a clone does not span, nor touch, a
    line because of them. Otherwise nearly every clone would start with the
    NEWLINE or DEDENT that ends the statement before it, and span that
    statement's line and the blank lines after it.

    Imports are the logical lines that start with `import` or `from`,
    wherever they are (inside `try:` or `if TYPE_CHECKING:` too),
    parenthesized ones included; one ends at its NEWLINE or `;`.
    """
    splits: list[list[Token]] = [[]]
    in_import = False
    previous = ""
    nesting = 0  # f-strings open
    string_start = 0
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")  # invalid escape sequences and the like
        try:
            for token in tokenize.tokenize(io.BytesIO(source).readline):
                kind = token.type
                if kind in _STRING_STARTS:
                    if nesting == 0:
                        string_start = token.start[0]
                    nesting += 1
                    continue
                if nesting:
                    if kind not in _STRING_ENDS:
                        continue
                    nesting -= 1
                    if nesting:
                        continue
                    text, start, end = "STR", string_start, token.end[0]
                elif kind in _SKIPPED:
                    continue
                else:
                    text = _normal(token)
                    if not text:
                        continue
                    if kind in _STRUCTURE:
                        start = end = 0
                    else:
                        start, end = token.start[0], token.end[0]
                if in_import:
                    if text in ("NEWLINE", ";"):
                        in_import = False
                        previous = text
                        if splits[-1]:
                            splits.append([])
                    continue
                if text == "import" or (text == "from" and previous in _STATEMENT_START):
                    in_import = True
                    continue
                splits[-1].append((text, start, end))
                previous = text
        except tokenize.TokenError as error:
            message, where = (error.args + ("", None))[:2]
            line = f" (line {where[0]})" if isinstance(where, tuple) and where else ""
            raise TokenizeProblem(f"{message}{line}") from error
        except SyntaxError as error:
            line = f" (line {error.lineno})" if error.lineno else ""
            raise TokenizeProblem(f"{error.msg}{line}") from error
        except (UnicodeDecodeError, LookupError, ValueError) as error:
            raise TokenizeProblem(str(error)) from error
    return [split for split in splits if split]


def _normal(token: tokenize.TokenInfo) -> str:
    """The normalized text of a token, "" for one to drop."""
    kind = token.type
    if kind == tokenize.NAME:
        # Soft keywords (match, case, _, type) are identifiers.
        return token.string if keyword.iskeyword(token.string) else "ID"
    if kind == tokenize.STRING:
        return "STR"
    if kind == tokenize.NUMBER:
        return "NUM"
    if kind == tokenize.OP:
        return token.string
    structure = _STRUCTURE.get(kind)
    if structure is not None:
        return structure
    if token.string.strip() == "":
        return ""
    raise TokenizeProblem(f"unexpected {token.string!r} (line {token.start[0]})")


def merge_ranges(ranges: Iterable[tuple[int, int]], adjacent: bool = False) -> Ranges:
    """Sorted ranges with the overlapping ones (and, with adjacent, the touching ones) merged."""
    merged: Ranges = []
    gap = 1 if adjacent else 0
    for start, end in sorted(ranges):
        if end < start:
            continue
        if merged and start <= merged[-1][1] + gap:
            if end > merged[-1][1]:
                merged[-1] = (merged[-1][0], end)
        else:
            merged.append((start, end))
    return merged


def count_added(added: Ranges, start: int, end: int) -> int:
    """How many of the lines start..end are in added (merged ranges)."""
    count = 0
    for low, high in added[max(bisect_right(added, (start, start)) - 1, 0):]:
        if low > end:
            break
        count += max(0, min(high, end) - max(low, start) + 1)
    return count


def is_new(added: Ranges, start: int, end: int) -> bool:
    """A side is new when at least half of the lines it spans are added."""
    return 2 * count_added(added, start, end) >= end - start + 1


class Side(NamedTuple):
    file: str
    start: int  # line
    end: int  # line
    new: bool

    @property
    def lines(self) -> int:
        return self.end - self.start + 1


# Two sides with the same tokens, and how many tokens they share.
Pair = Tuple[Side, Side, int]


class FileTokens:
    """One file's tokens: one character per token (its interned kind), their lines, the splits."""

    def __init__(self, index: int, rel: str, splits: list[list[Token]], kinds: dict[str, str], added: Ranges) -> None:
        self.index = index  # the order of the file in the search
        self.rel = rel
        self.added = added
        characters: list[str] = []
        self.starts = array("i")
        self.ends = array("i")
        self.splits: list[tuple[int, int]] = []  # [begin, end) token positions
        for split in splits:
            begin = len(characters)
            for text, start, end in split:
                kind = kinds.get(text)
                if kind is None:
                    kind = kinds[text] = chr(len(kinds) + 1)
                characters.append(kind)
                self.starts.append(start)
                self.ends.append(end)
            self.splits.append((begin, len(characters)))
        self.text = "".join(characters)
        self._split_begins = [begin for begin, _ in self.splits]
        # touching[i]: how many of the first i tokens start on an added line.
        self.touching: Optional[array] = None
        if added:
            low = [start for start, _ in added]
            touching = array("i", [0])
            count = 0
            for line in self.starts:
                at = bisect_right(low, line) - 1
                if at >= 0 and line <= added[at][1]:
                    count += 1
                touching.append(count)
            self.touching = touching

    def touches(self, position: int, size: int) -> bool:
        """Whether the window at position starts a token on an added line."""
        touching = self.touching
        return touching is not None and touching[position + size] > touching[position]

    def split_of(self, position: int) -> tuple[int, int]:
        return self.splits[bisect_right(self._split_begins, position) - 1]

    def side(self, first: int, last: int) -> Optional[Side]:
        """The lines of tokens first..last, from the first to the last that has a line."""
        starts, ends = self.starts, self.ends
        while first <= last and starts[first] == 0:
            first += 1
        while last >= first and ends[last] == 0:
            last -= 1
        if first > last:
            return None  # structure only
        start, end = starts[first], ends[last]
        return Side(self.rel, start, end, bool(self.added) and is_new(self.added, start, end))


class Search:
    """The windows of the files, their buckets and the maximal pairs, for one window size."""

    def __init__(self, size: int) -> None:
        self.size = size
        self.kinds: dict[str, str] = {}
        self.candidates: set[int] = set()
        # Window hash -> the windows with it, each one packed as
        # file index << 32 | position: a bare int while there is one, then a
        # list of up to WINDOW_CAP + 1 (past that, the bucket is dropped).
        # Most buckets hold one window, and ints keep them small.
        self.buckets: dict[int, Union[int, list[int]]] = {}
        # The files with a window in a bucket, by index.
        self.files: dict[int, FileTokens] = {}
        self.stopped = False

    def add_candidates(self, tokens: FileTokens) -> None:
        """Hashes the windows of a changed file that touch the change."""
        size, text, touching, candidates = self.size, tokens.text, tokens.touching, self.candidates
        if touching is None:
            return
        for begin, end in tokens.splits:
            for position in range(begin, end - size + 1):
                if touching[position + size] > touching[position]:
                    candidates.add(hash(text[position : position + size]))

    def collect(self, tokens: FileTokens) -> None:
        """Puts the windows of a file whose hash is a candidate in their buckets."""
        size, text, candidates, buckets = self.size, tokens.text, self.candidates, self.buckets
        base = tokens.index << 32
        found = False
        for begin, end in tokens.splits:
            for position in range(begin, end - size + 1):
                key = hash(text[position : position + size])
                if key in candidates:
                    found = True
                    bucket = buckets.get(key)
                    if bucket is None:
                        buckets[key] = base | position
                    elif isinstance(bucket, int):
                        buckets[key] = [bucket, base | position]
                    elif len(bucket) <= WINDOW_CAP:
                        bucket.append(base | position)
        if found:
            self.files[tokens.index] = tokens

    def pairs(self, deadline: float) -> list[tuple[FileTokens, int, int, FileTokens, int, int]]:
        """The maximal pairs, (file, first, last token) of both sides, found before deadline.

        Windows are in their buckets in the order of the files, then of their
        positions, so the first of two windows of one file is the earlier.
        """
        size = self.size
        found: list[tuple[FileTokens, int, int, FileTokens, int, int]] = []
        # (file, file, offset) -> the token ranges of the first side of the
        # maximal pairs found on that diagonal. A window pair inside one of
        # them extends into the same maximal pair.
        covered: dict[tuple[int, int, int], list[tuple[int, int]]] = {}
        for count, bucket in enumerate(self.buckets.values()):
            if count % 256 == 0 and time.monotonic() > deadline:
                self.stopped = True
                break
            if isinstance(bucket, int) or len(bucket) > WINDOW_CAP:
                continue
            windows = [(self.files[entry >> 32], entry & _POSITION) for entry in bucket]
            for at, (a, p) in enumerate(windows):
                a_touches = a.touches(p, size)
                for b, q in windows[at + 1 :]:
                    if not a_touches and not b.touches(q, size):
                        continue
                    if a is b and q - p < size:
                        continue  # overlapping windows
                    key = (a.index, b.index, q - p)
                    spans = covered.get(key)
                    if spans is not None and any(low <= p and p + size - 1 <= high for low, high in spans):
                        continue
                    if a.text[p : p + size] != b.text[q : q + size]:
                        continue  # the hashes collide
                    # Same file: extend never lets the two ranges overlap, so
                    # back-to-back repeats are capped at their distance.
                    first_a, last_a, first_b, last_b = self.extend(a, p, b, q)
                    covered.setdefault(key, []).append((first_a, last_a))
                    found.append((a, first_a, last_a, b, first_b, last_b))
        return found

    def extend(self, a: FileTokens, p: int, b: FileTokens, q: int) -> tuple[int, int, int, int]:
        """Extends the equal windows at p and q backward, then forward, while the tokens are equal,
        within both splits and without the two ranges overlapping (same file, p < q)."""
        same = a is b
        text_a, text_b = a.text, b.text
        low_a, high_a = a.split_of(p)
        low_b, high_b = b.split_of(q)
        first_a, first_b = p, q
        last_a, last_b = p + self.size - 1, q + self.size - 1
        while first_a > low_a and first_b > low_b and text_a[first_a - 1] == text_b[first_b - 1]:
            if same and last_a >= first_b - 1:
                break
            first_a -= 1
            first_b -= 1
        while last_a + 1 < high_a and last_b + 1 < high_b and text_a[last_a + 1] == text_b[last_b + 1]:
            if same and last_a + 1 >= first_b:
                break
            last_a += 1
            last_b += 1
        return first_a, last_a, first_b, last_b


def _range(file: str, start: int, end: int) -> dict[str, Any]:
    return {"file": file, "startLine": start, "endLine": end}


Block = Tuple[str, int, int]  # file, start line, end line


def group(pairs: list[Pair], scope: str) -> list[dict[str, Any]]:
    """The duplicates of the maximal pairs of one scope (steps 7 to 9 of the schema), sorted, not capped."""
    new_sides: dict[str, list[tuple[int, int]]] = {}
    for a, b, _ in pairs:
        for side in (a, b):
            if side.new:
                new_sides.setdefault(side.file, []).append((side.start, side.end))
    blocks = {file: merge_ranges(ranges) for file, ranges in new_sides.items()}

    starts = {file: [start for start, _ in ranges] for file, ranges in blocks.items()}

    def block_of(side: Side) -> Block:
        start, end = blocks[side.file][bisect_right(starts[side.file], side.start) - 1]
        return side.file, start, end

    existing: dict[Block, list[tuple[str, int, int]]] = {}
    existing_tokens: dict[Block, int] = {}
    links: list[tuple[Block, Block, int]] = []
    for a, b, tokens in pairs:
        for side, other in ((a, b), (b, a)):
            if not side.new:
                continue
            block = block_of(side)
            if other.new:
                other_block = block_of(other)
                if other_block != block:
                    links.append((block, other_block, tokens))
            else:
                existing.setdefault(block, []).append((other.file, other.start, other.end))
                existing_tokens[block] = max(existing_tokens.get(block, 0), tokens)

    groups: list[dict[str, Any]] = []
    for block, occurrences in existing.items():
        by_file: dict[str, list[tuple[int, int]]] = {}
        for file, start, end in occurrences:
            by_file.setdefault(file, []).append((start, end))
        others = [(file, start, end) for file, ranges in by_file.items() for start, end in merge_ranges(ranges)]
        groups.append(_duplicate("copies_existing", scope, block, others, existing_tokens[block]))

    # The blocks without an existing occurrence, joined through the pairs
    # between them. Only links between two such blocks make a component, so
    # a block whose only new occurrences copy existing code is in none (and
    # no group ever has empty others).
    parent: dict[Block, Block] = {}

    def root(block: Block) -> Block:
        while parent[block] != block:
            parent[block] = parent[parent[block]]
            block = parent[block]
        return block

    for one, other, _ in links:
        if one in existing or other in existing:
            continue
        parent.setdefault(one, one)
        parent.setdefault(other, other)
        parent[root(one)] = root(other)
    members: dict[Block, list[Block]] = {}
    for block in parent:
        members.setdefault(root(block), []).append(block)
    tokens_of_root: dict[Block, int] = {}
    for one, other, tokens in links:
        if one in parent and other in parent:
            top = root(one)
            tokens_of_root[top] = max(tokens_of_root.get(top, 0), tokens)
    for top, blocks_of_group in members.items():
        ordered = sorted(blocks_of_group)
        groups.append(_duplicate("repeated_in_change", scope, ordered[0], ordered[1:], tokens_of_root[top]))

    groups.sort(key=lambda g: (g["block"]["file"], g["block"]["startLine"], g["block"]["endLine"]))
    return groups


def _duplicate(kind: str, scope: str, block: Block, others: list[tuple[str, int, int]], tokens: int) -> dict[str, Any]:
    file, start, end = block
    return {
        "kind": kind,
        "scope": scope,
        "block": _range(file, start, end),
        "others": [_range(*other) for other in sorted(others)],
        "tokens": tokens,
        "lines": end - start + 1,
    }


_ASCII_LOWER = str.maketrans(string.ascii_uppercase, string.ascii_lowercase)


class Exclude:
    """The files the exclude of --changed leaves out (step 1 of the schema): those with a folder
    (a path segment before the file name) named one of dirs, or a file name ending with one of
    extensions, both compared ignoring ASCII case. None excludes nothing."""

    def __init__(self, exclude: Optional[dict[str, list[str]]]) -> None:
        exclude = exclude or {"dirs": [], "extensions": []}
        self.dirs = {name.translate(_ASCII_LOWER) for name in exclude["dirs"]}
        self.extensions = tuple(extension.translate(_ASCII_LOWER) for extension in exclude["extensions"])

    def __call__(self, rel: str) -> bool:
        parts = rel.translate(_ASCII_LOWER).split("/")
        return parts[-1].endswith(self.extensions) or any(part in self.dirs for part in parts[:-1])


class Clock:
    """The search's time limit, shared by the scopes: it stops a little before --timeout, and
    out of time the buckets filled so far get one short moment, for all scopes, to be paired."""

    def __init__(self, timeout: float) -> None:
        self.started = time.monotonic()
        self.deadline = self.started + max(timeout - min(timeout / 10, 5.0), 1.0)
        self._grace = min(timeout / 20, 2.5)
        self._grace_end: Optional[float] = None

    def expired(self) -> bool:
        return time.monotonic() > self.deadline

    def pairing_deadline(self, stopped: bool) -> float:
        if not stopped:
            return self.deadline
        if self._grace_end is None:
            self._grace_end = time.monotonic() + self._grace
        return self._grace_end


class ScopeResult(NamedTuple):
    duplicates: list[dict[str, Any]]  # sorted, not capped
    stopped: bool  # out of time
    files: int  # read and collected
    changed: int
    buckets: int
    pairs: int


def _load(
    index: int, source: SourceFile, kinds: dict[str, str], added: Ranges, diagnostics: list[str]
) -> Optional[FileTokens]:
    """A file's tokens; None for a file left out (too large, generated) or that cannot be read."""
    rel = source.rel
    try:
        if os.path.getsize(source.path) > MAX_FILE_BYTES:
            return None
        with open(source.path, "rb") as stream:
            text = stream.read()
    except OSError as error:
        diagnostics.append(f"{rel}: cannot read the file: {error.strerror or error}")
        return None
    if is_generated(text, rel):
        return None
    try:
        splits = tokens_of(text)
    except TokenizeProblem as problem:
        diagnostics.append(f"{rel}: cannot tokenize the file: {problem} (Python {PYTHON})")
        return None
    except RecursionError:
        diagnostics.append(f"{rel}: cannot tokenize the file: it nests too deeply")
        return None
    return FileTokens(index, rel, splits, kinds, added)


def search_scope(
    sources: list[tuple[int, SourceFile]],
    scope: str,
    added: dict[str, Ranges],
    min_tokens: int,
    min_lines: int,
    clock: Clock,
    diagnostics: list[str],
) -> ScopeResult:
    """Steps 3 to 9 of the schema on the files of one scope (index in the search, file)."""
    search = Search(min_tokens)
    # The changed files first: their touching windows are the candidates.
    changed_files: dict[str, Optional[FileTokens]] = {}
    for index, source in sources:
        if source.rel in added:
            tokens = _load(index, source, search.kinds, added[source.rel], diagnostics)
            changed_files[source.rel] = tokens
            if tokens is not None:
                search.add_candidates(tokens)
    files = 0
    if search.candidates:
        for index, source in sources:
            if clock.expired():
                search.stopped = True
                break
            if source.rel in changed_files:
                tokens = changed_files[source.rel]
            else:
                tokens = _load(index, source, search.kinds, [], diagnostics)
            if tokens is not None:
                files += 1
                search.collect(tokens)
    search.candidates.clear()  # no longer needed: free it before pairing
    raw = search.pairs(clock.pairing_deadline(search.stopped))

    pairs: list[Pair] = []
    for a, first_a, last_a, b, first_b, last_b in raw:
        side_a, side_b = a.side(first_a, last_a), b.side(first_b, last_b)
        if side_a is None or side_b is None:
            continue
        if side_a.lines < min_lines or side_b.lines < min_lines:
            continue  # both sides must reach --min-lines
        if not side_a.new and not side_b.new:
            continue
        pairs.append((side_a, side_b, last_a - first_a + 1))
    return ScopeResult(group(pairs, scope), search.stopped, files, len(changed_files), len(search.buckets), len(pairs))


def find_duplicates(
    root: str,
    changed: dict[str, list[tuple[int, int]]],
    min_tokens: int,
    min_lines: int,
    timeout: float,
    log: Logger,
    exclude: Optional[dict[str, list[str]]] = None,
) -> dict[str, Any]:
    """The duplicates the change introduces in the tree at root (dupes.schema.json): the tree's
    Python files but those exclude names, searched once per scope, production first."""
    clock = Clock(timeout)
    project = discover(root, clock.deadline)
    excluded = Exclude(exclude)
    diagnostics: list[str] = []
    added = {rel: merge_ranges(ranges, adjacent=True) for rel, ranges in changed.items()}
    added = {rel: ranges for rel, ranges in added.items() if ranges}
    # The index of a file stays its place in the tree, whatever its scope.
    kept = [(index, source) for index, source in enumerate(project.files) if not excluded(source.rel)]
    scopes = {
        PRODUCTION: [(index, source) for index, source in kept if not source.test],
        TEST: [(index, source) for index, source in kept if source.test],
    }

    duplicates: list[dict[str, Any]] = []
    stopped = False
    stats = {"files": 0, "changed": 0, "buckets": 0, "pairs": 0}
    for scope, sources in scopes.items():
        if clock.expired():
            stopped = True  # no time left for this scope
            break
        result = search_scope(sources, scope, added, min_tokens, min_lines, clock, diagnostics)
        stopped = stopped or result.stopped
        for name in stats:
            stats[name] += getattr(result, name)
        found = result.duplicates
        if len(found) > MAX_GROUPS:
            diagnostics.append(f"{len(found)} {scope} duplicates found: only the first {MAX_GROUPS} are reported")
            found = found[:MAX_GROUPS]
        duplicates.extend(found)
    if stopped:
        diagnostics.append(f"the search ran out of time after {timeout:g}s: some duplicates may be missing")
    log.info(
        "dupes done",
        excluded=len(project.files) - len(kept),
        duplicates=len(duplicates),
        elapsed_ms=int((time.monotonic() - clock.started) * 1000),
        **stats,
    )
    return protocol.dupes(min_tokens, min_lines, duplicates, diagnostics)
