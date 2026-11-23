"""Runs a module as `python -m <module> <args>` would, in the project's own
interpreter, with one source file of the tree replaced by a mutant:
`python -m _difflens_mutant pytest -p _difflens_pytest ...`.

DIFFLENS_MUTANT_TARGET names the file (absolute) and DIFFLENS_MUTANT_SOURCE
the file holding its mutated source. A finder first on sys.meta_path, put
there before the runner imports anything of the project, serves the mutated
source for whatever module name resolves to the target, compiled in memory:
no bytecode of it is read or written, and the tree is never changed, so
several mutants of one tree can run at the same time. Without the variables
it only runs the module (the baseline). Python 3.7 or later.
"""

import importlib.machinery
import os
import runpy
import sys

_TARGET = os.environ.get("DIFFLENS_MUTANT_TARGET", "")
_SOURCE = os.environ.get("DIFFLENS_MUTANT_SOURCE", "")


class _MutantLoader(importlib.machinery.SourceFileLoader):
    """The target's loader, compiling the mutant instead of the file."""

    def get_code(self, fullname):
        with open(_SOURCE, "rb") as stream:
            data = stream.read()
        return self.source_to_code(data, self.path)


class _MutantFinder:
    def __init__(self, target):
        self.target = os.path.realpath(target)
        name = os.path.basename(self.target)[:-3]
        # The last part of the module names that can be the target.
        self.leaf = os.path.basename(os.path.dirname(self.target)) if name == "__init__" else name

    def find_spec(self, fullname, path=None, target=None):
        if fullname.rpartition(".")[2] != self.leaf:
            return None
        spec = importlib.machinery.PathFinder.find_spec(fullname, path)
        if spec is None or not spec.origin or os.path.realpath(spec.origin) != self.target:
            return None
        spec.loader = _MutantLoader(fullname, spec.origin)
        return spec

    def invalidate_caches(self):
        pass


def main():
    if len(sys.argv) < 2:
        sys.stderr.write("usage: python -m _difflens_mutant <module> [args]\n")
        return 2
    module = sys.argv[1]
    del sys.argv[1]
    if _TARGET and _SOURCE:
        sys.meta_path.insert(0, _MutantFinder(_TARGET))
    runpy.run_module(module, run_name="__main__", alter_sys=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
