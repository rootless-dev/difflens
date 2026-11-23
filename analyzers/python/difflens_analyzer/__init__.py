"""The DiffLens analyzer for Python source trees.

It implements the analyzer command line every DiffLens analyzer shares (see
README.md next to this package): one command per run, one JSON document on
stdout, logs on stderr.
"""

# Identity reported by --version and in every document.
NAME = "difflens-analyzer-python"
VERSION = "0.0.0"
# The language id the core detects Python projects as.
LANGUAGE = "python"
