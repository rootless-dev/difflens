"""Entry point: `python -m difflens_analyzer <command> [flags]`."""

import sys

from difflens_analyzer.cli import main

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:], sys.stdout, sys.stderr))
