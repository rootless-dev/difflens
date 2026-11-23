# difflens-analyzer-python

The DiffLens analyzer for Python projects (language id `python`, detected by a
`pyproject.toml` or `requirements.txt` at the repository root or one folder
down). It is written in Python on the standard library only (`ast`, and a
small TOML reader for Python 3.9 and 3.10, which lack `tomllib`) and shipped
as source: DiffLens builds its environment the first time a Python project
needs it. `requirements.txt` stays empty.

This file is the contract. It is derived from the Go analyzer
(`analyzers/go/internal/app/`), which is the reference implementation, and
from the JSON schemas in `analyzers/protocol/`, which are authoritative for
every document. What each command produces, and how, is in
[What the commands produce](#what-the-commands-produce).

## Layout

```text
requirements.txt          runtime dependencies, pinned (name==version); none
difflens_analyzer/
    __init__.py           NAME, VERSION, LANGUAGE
    __main__.py           python -m difflens_analyzer
    cli.py                command line, exit codes, one document on stdout
    protocol.py           the documents, as dicts
    log.py                JSON line logs on stderr
    project.py            the tree's Python files and their module names
    model.py              functions, classes, ids, and what names denote
    complexity.py         cyclomatic complexity
    calls.py              call sites, their kind and resolution, references
    entrypoints.py        entry points and registrations
    analysis.py           analyze
    dupes.py              dupes (duplicated code a change adds)
    mutator.py            the mutants of a file (mutation operators, exact spans)
    mutation.py           mutate (which mutants, which tests, running them, kept results)
    environment.py        the project's own interpreter and prepared environments
    prepare.py            prepare
    testrun.py            test
    checks.py             check (ruff, mypy)
    processes.py          subprocesses with deadlines, in their own process group
    toml.py               TOML, with a fallback reader for Python < 3.11
    inproject/            run by the *project's* interpreter, never imported here:
        _difflens_pytest.py     pytest plugin streaming collection and results
        _difflens_unittest.py   unittest runner for projects without pytest
        _difflens_mutant.py     runs pytest or that runner with one file served as a mutant
tests/                    the analyzer's unit tests (python3 -m unittest)
testdata/                 fixture projects the tests and the manual checks use
```

Only `requirements.txt` and `difflens_analyzer/**` are shipped
(`bundle.resources` in `src-tauri/tauri.conf.json`). Anything the analyzer
needs at run time lives inside the package.

## Commands

The core runs one command per process, with stdin closed, the current
directory set to `--dir`, `PATH` set to the user's login-shell PATH, and the
rest of the environment inherited from the app.

| Command | Flags the core passes | Prints (stdout) |
| --- | --- | --- |
| `analyze` | `--dir <tree> --timeout 120s` | `analysis.schema.json` |
| `check` | `--dir <tree> --timeout 120s --cache-dir <folder>` | `checks.schema.json` |
| `dupes` | `--dir <tree> --timeout 120s --changed <file>` and, as asked, `--min-tokens <n>`, `--min-lines <n>` | `dupes.schema.json` |
| `prepare` | `--dir <tree> --cache-dir <folder>`, and `--tools <folder>` before a mutation run | `tests.schema.json` `$defs/prepare` |
| `test` | `--dir <tree> --timeout 600s --cache-dir <folder> --progress <file>` and, as asked, `--no-race`, `--no-cover`, `--selection <file>`, `--offline` (always under the sandbox) | `tests.schema.json` |
| `mutate` | `--dir <tree> --timeout <d> --changed <file> --coverage <file> --cache-dir <folder> --tools <folder> --progress <file>` and, as asked, `--max-mutants <n>`, `--jobs <n>`, `--min-mutant-timeout <d>`, `--offline` (always under the sandbox) | `mutation.schema.json` |
| `--version` (also `-version`, `version`) | none | `difflens-analyzer-python 0.0.0` and a newline |

Usage, as the Go analyzer prints it:

```text
difflens-analyzer-python analyze --dir <source tree> [--timeout 120s]
difflens-analyzer-python prepare --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--tools <folder>]
difflens-analyzer-python test --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--offline] [--no-race]
    [--no-cover] [--selection <file>] [--progress <file>]
difflens-analyzer-python check --dir <source tree> [--timeout 5m] [--cache-dir <folder>]
difflens-analyzer-python dupes --dir <source tree> --changed <file> [--timeout 120s]
    [--min-tokens 50] [--min-lines 6]
difflens-analyzer-python mutate --dir <source tree> --changed <file> --coverage <file> [--timeout 10m]
    [--cache-dir <folder>] [--tools <folder>] [--max-mutants 200] [--jobs <n>]
    [--min-mutant-timeout 10s] [--progress <file>] [--offline]
difflens-analyzer-python --version
```

Flags:

- Accept `--flag value` (what the core sends) and `--flag=value`.
- `--dir` is required; `--timeout` is a Go duration (`120s`, `10m`, `1m30s`)
  and must be positive; defaults when absent: analyze `120s`, prepare `10m`,
  test `10m`, check `5m`, dupes `120s`, mutate `10m`. No positional
  arguments. Unknown flags are errors. `cli.parse_duration` reads durations.
- `dupes` requires `--changed` (`dupes.schema.json` `$defs/changed`, unknown
  fields rejected, `protocol` 1, an optional `exclude`); `--min-tokens`
  (default 50) and `--min-lines` (default 6) are integers of at least 1.
- `mutate` requires `--changed` (as dupes; its `exclude` is checked and
  ignored) and `--coverage`
  (`mutation.schema.json` `$defs/coverage`, unknown fields rejected,
  `protocol` 1); `--max-mutants` (default 200) and `--jobs` (default half
  the CPUs, at least 1) are integers of at least 1; `--min-mutant-timeout`
  (default `10s`) is a positive duration.
- `--tools` (prepare, mutate) is the folder for a mutation engine. Python
  needs none (the mutator is the analyzer's own), so both accept the flag
  and ignore it: nothing is installed or read there.
- `--cache-dir` (prepare, test, check, mutate; optional, `cli.cache_dir` reads it)
  is a folder of the core outside the tree, one per project and analyzer,
  kept between runs (deleted when it grows too large or the project is
  removed) and writable under the sandbox. Every run's tree is a fresh copy
  at another path, so only what stays correct when the same relative files
  hold other code is kept there: mypy's incremental cache (`<folder>/mypy`),
  the test and mutation runs' bytecode (`<folder>/pycache`) and the mutation
  results (`<folder>/mutation`, by content), see check, test and mutate below.
  prepare accepts it and does not use it. Without it, both go to temporary
  folders, as before. ruff's cache (keyed by absolute paths) and pytest's
  (`--lf` data of another tree) are not kept. dupes does not take it.
- `--timeout` bounds the whole command; finish, and report what you have,
  before it. The core kills the process group a little after it: analyze and
  check at 150 s, test at 630 s, and prepare (given no `--timeout`, so 10m by
  default) at 600 s.

Exit codes and streams:

- `0`: exactly one JSON document on stdout, followed by a newline. Nothing
  else on stdout, ever (no prints from libraries).
- `1`: the command itself failed (the tree is not a directory, an internal
  error). stderr says why; the core shows its last lines to the user
  ("The Python analyzer failed (exit status: 1): …").
- `2`: bad command line, or a `--selection`, `--changed` or `--coverage`
  file that cannot be read or is not valid.
- Logs go to stderr as JSON lines (`log.py`: `time`, `level`, `service`,
  `msg`, fields); the level comes from `DIFFLENS_LOG_LEVEL`, an unknown name
  means info. Keep stdout under 200 MB.

What each command must and must not do:

- `analyze` reads the tree and prints its functions, calls, externals and
  diagnostics. It must not write inside `--dir` (the tree is shared and
  cached per commit) and must not execute project code. The `id` of a
  function must be the same on both sides of a review even when the function
  moved or its body changed (for Go: import path, receiver, name; closures
  append `$1`, `$2`…): the core matches the old and new analyses by id. Paths
  are relative to `--dir` with `/` separators; lines start at 1. Problems that
  do not stop the analysis (a file that does not parse) are `diagnostics`,
  not a failure. `language` must be `python`.
- `dupes` reads the tree and prints the duplicated code the change adds,
  following the algorithm in the description of `dupes.schema.json`. Read
  only, no project code; problems that do not stop the search (a file that
  does not tokenize, the time limit, the 500-group cap of a scope) are
  `diagnostics`.
- `check` runs static checkers on the tree, read only. The core runs
  `prepare` on a throwaway copy of the commit first and checks that copy, so
  the project's environment is there: mypy resolves the project's imports,
  and the project's own ruff and mypy versions are used. One entry per
  checker the language has, installed or not, in a fixed order; a checker
  that is not installed is `unavailable` with a one-line `message`, not a
  failure. The checkers do not run the project's code, but mypy loads the
  plugins the project's configuration names (`plugins = pydantic.mypy`,
  `mypy_django_plugin.main`), which is code of the project's dependencies,
  installed by prepare, run with the user's rights and no sandbox.
- `prepare` fetches what the tests will need so `test` can run offline. It
  may use the network, must not execute project code, and prints the absolute
  folders outside `--dir` the test step must write to (a build or bytecode
  cache), which the core opens in the sandbox. It never runs under the
  sandbox. It may run for two copies of one project at once (one for a
  check, one for a test run), building the same environment: that is safe
  (see [Project environments](#project-environments-environmentpy-preparepy)).
- `test` runs the project's tests once and prints the run. Failing tests are
  results (exit 0), not a failure. `--dir` is a throwaway copy the tests may
  write in; `TMPDIR` is set to a short folder of the run. Under the sandbox
  (`--offline` is given): no network beyond loopback, and writes only inside
  the run folder (the parent of `--dir`), `TMPDIR`, `--cache-dir`, the
  folders `prepare` reported, and `/dev/null`-like devices. Reading and executing anything is
  allowed. With `--progress`, keep that file up to date
  (`$defs/progress`), replacing it whole each time (write a temporary file in
  the same folder and rename it; `cli.write_progress` does). With
  `--selection`, run only what the file names (`$defs/selection`) and report
  only what ran. `container`, `package` and `function` of each test must be
  written exactly as `analyze` writes `container` and function ids, so the
  core can place tests in the call graph. `--no-race` / `--no-cover` turn off
  the race detector and coverage; Python has no race detector, so `race` is
  `false` with a diagnostic only if that is worth saying.
- `mutate` changes the covered lines a review adds one small piece at a
  time and runs the tests that can see each change, following the rules in
  the description of `mutation.schema.json`. It runs under the same
  isolation as `test` (a throwaway copy `prepare` ran on, `--offline` under
  the sandbox, writes only in the copy, `TMPDIR` and `--cache-dir`).
  Surviving mutants are results (exit 0); the core asks for it on a click,
  never as part of an analysis.

## What the commands produce

### Files and modules (`project.py`)

Every `*.py` file of the tree, except in version control and cache folders,
virtual environments (any folder with a `pyvenv.cfg`, `.venv/`, `venv/`),
`node_modules/`, `.tox/`, `.nox/`, `*.egg-info/`, and `build/`, `dist/`,
`env/` unless they hold an `__init__.py`. Symlinked folders are not followed.

A file's **container** is its module name, as `import` would write it:
the path below the deepest *source root* that contains it, `pkg/__init__.py`
being `pkg`. Source roots are the tree, every folder with a
`pyproject.toml`, `setup.cfg` or `setup.py`, the `src/` folder next to any of
them (src layout), and the folders their configuration names (setuptools
`package-dir` and `packages.find.where`, Poetry `packages[].from`). Folders
without `__init__.py` are namespace packages. When two files would get the
same name (two projects of a monorepo, both with `app/main.py`), both are
named from the tree root instead (`services.api.app.main`).

A file is **test** code by pytest's and unittest's conventions:
`test_*.py`, `*_test.py`, `conftest.py`, or any file under a `tests/` or
`test/` folder (helpers included). It is **generated** when one of its first
comment lines says `Code generated`, `Generated by`, `auto-generated`,
`@generated` or `DO NOT EDIT` (protobuf, Django migrations, Go-style
headers), or it is a `*_pb2.py` / `*_pb2_grpc.py` module.

### analyze (`model.py`, `calls.py`, `entrypoints.py`)

**Functions.** Every `def` and `async def` (module functions, methods,
static and class methods, properties, functions nested in functions,
methods of classes nested anywhere), every lambda (Go reports its function
literals), and one function per `if __name__ == "__main__":` block (or for
the whole of a `__main__.py`). Ids are built from the syntax only:

| What | id | name |
| --- | --- | --- |
| function | `shop.billing.charge` | `charge` |
| method | `shop.billing.Invoice.total` | `Invoice.total` |
| nested function | `shop.billing.charge.retry` | `charge.retry` |
| lambda | `shop.billing.charge$1` (k-th lambda written directly in it) | `charge$1` |
| module or class level `name = lambda` | `shop.billing.key$1` | `key$1` |
| main block | `shop.cli.__main__` | `__main__` |
| redefinition in the same scope | `shop.billing.charge#2` | `charge#2` |

`qualifiedName` is the module's last part and the name (`billing.Invoice.total`).
`startLine` is the first decorator's line. Syntax errors are `error`
diagnostics with the file, the line and the Python version that parsed the
file (`ast` uses the grammar of the interpreter it runs on).

**Complexity** is 1 plus one per decision point of the function's own code
(nested functions and lambdas are their own): `if` and each `elif`; `for`,
`async for` and `while`; each `except`; each `case` of a `match` except an
irrefutable last one (Go's `default`); each `and` / `or`; each conditional
expression; each `for` and each `if` of a comprehension. `else`, `finally`,
`with`, `assert`, `return`, `raise` do not count.

**Calls.** Names are resolved with Python's scoping (local, enclosing
functions, module, builtins) over what the code binds: assignments, imports
(absolute, relative and `*`, followed across the modules of the tree),
parameters with annotations (`Optional`, `Union`, `X | None`, `Annotated`,
strings), `self` and `cls`, instance attributes set in methods
(`self.repo = repo`), class attributes, and return annotations. Nothing of
the project is imported or run.

- `kind`: **always** when the call is on every path from the start of the
  function to a normal return. Paths ending in an exception are ignored, as
  Go ignores panics: after `if not ok: raise ...` calls are still always, and
  so is everything in a `try` body, its `else` and `finally`; a function that
  never returns (a `while True:` loop) counts every iteration instead.
  **conditional** in a branch (`if`/`elif`/`else`, `except`, `case`, the
  right of `and`/`or`, either side of a conditional expression), in a loop
  body, in a comprehension past its first iterable, or after a statement
  that may `return`, `break` or `continue` early. **async** when started
  without waiting: the coroutine given to `asyncio.create_task`,
  `ensure_future`, `run_coroutine_threadsafe` or a task group's
  `create_task`; the function given to `threading.Thread`, `Timer`,
  `multiprocessing.Process`, an executor's `submit`, an event loop's
  `call_soon`/`call_later`/`call_at`/`run_in_executor`, AnyIO's `start_soon`,
  Starlette/FastAPI `add_task`; a Celery task sent with `.delay()` or
  `.apply_async()`. `await` waits, so an awaited call is a call.
- `resolution`: **static** when the callee is known: a function, an
  imported one, `Class.method`, a method of `self`, `cls`, `super()` or of a
  value of a known class that no subclass in the tree overrides, a
  constructor (`Class(...)` calls `Class.__init__` by the MRO).
  **dynamic** when several targets are possible, each with its own call: a
  method some subclass overrides (abstract methods are replaced by their
  implementations), a `Protocol` method (every class of the tree with a
  method of that name and a compatible arity), or a method of an object of
  unknown type (an unannotated parameter), resolved by name to the methods
  of the tree that have it, when at most four classes do and the name is not
  one of the common method names of standard types (`get`, `append`,
  `close`...). As in Go, production code never lists targets declared in
  test files.
- **externals**: callees outside the tree, by dotted path (`requests.get`,
  `requests.Session.get` for a method of an instance, `os.path.join`), with
  the module as container. Builtins are left out (like Go's), except
  `print`, `open`, `input`, `exec`, `eval`, `compile`, `__import__`,
  `breakpoint`, `exit`; so is `typing`.
- `reference`: a function used as a value without being called (passed,
  stored, returned; a lambda not called on the spot; a nested function given
  to a decorator of the tree).
- `registration`: a registration (below) written inside a function of the
  tree, with `evidence` such as `route GET /health`, `command list`,
  `signal SIGTERM`, `task receipts.send`, `middleware`, `atexit`. Written at
  module level (decorators, `urlpatterns`), a registration has no caller and
  only marks the entry point, as Go does for package-level commands.

**Entry points**, from the source only (frameworks are recognised by what
an object is an instance of, by the imports, or, for an object of unknown
type such as a factory's `app` parameter, by the module importing the
framework):

| kind | What |
| --- | --- |
| `main` | `if __name__ == "__main__":` blocks and `__main__.py` (`python -m pkg.cli`); console and GUI scripts of `[project.scripts]`, `[project.gui-scripts]`, `[tool.poetry.scripts]`, setup.cfg `console_scripts` (the script's name, `file`/`line` of the declaration) |
| `http_route` | FastAPI/Starlette (`@app.get`, `@router.post`, `api_route`, `websocket`, `add_api_route`, `Route`), Flask/Quart (`@app.route` with `methods`, `@bp.get`, `add_url_rule`, `MethodView.as_view`), aiohttp, Litestar and Bottle style decorators, Django `path`/`re_path`/`url` (function views; `View.as_view()` lists the HTTP methods the class defines), Django REST framework `router.register` view sets. A router's constant `prefix=`/`url_prefix=` is prepended. Labels are `GET /orders/{order_id}`, or the path alone when the method is not known (Django function views) |
| `cli_command` | click and rich-click commands and groups (subcommands labelled with their group chain: `cli draw`; names derived as click 8.2 does), Typer commands, `typer.run`, `fire.Fire`, Flask's `@app.cli.command`, argparse `set_defaults(func=...)` labelled with the subparser's name |
| `message_handler` | Celery tasks (`@app.task`, `@shared_task`; the task name), Dramatiq actors, RQ jobs, Django signal receivers (`@receiver`, `signal.connect`), `signal.signal` handlers (`SIGTERM`) |
| `scheduled_job` | Celery beat schedule entries (matched to the task by name), `add_periodic_task`, APScheduler (`@scheduled_job`, `add_job`), `schedule.every(10).minutes.do(job)` |
| `test` | in test files: pytest tests (`test*` functions, `test*` methods of `Test*` classes without `__init__`, labelled `TestClass::test_x`), unittest `TestCase` methods, xunit setup/teardown, fixtures (`fixture db`), pytest hooks in conftest.py |
| `exported_api` | what a top-level package's `__init__.py` exports: its `__all__`, or without one its public re-exports (`pkg.area_of`; the public methods of an exported class) |

Middlewares, request hooks and error handlers are registrations without an
entry point, like Go's middlewares.

### Project environments (`environment.py`, `prepare.py`)

The analyzer's own virtual environment is never the project's. `test` and
`check` use the project's interpreter, found in this order:

1. the environment `prepare` built for the tree's dependency files, in
   `~/Library/Caches/dev.difflens.app/python-envs/<key>/` (macOS; elsewhere
   `$XDG_CACHE_HOME/difflens/python-envs/`; `DIFFLENS_PYTHON_ENVS` overrides
   it). The key is a hash of the dependency files (`pyproject.toml`,
   `uv.lock`, `poetry.lock`, `Pipfile`, `Pipfile.lock`, `setup.cfg`,
   `.python-version`, `requirements*.txt`, `requirements/*.txt`) and of how
   they are installed, so every commit with the same dependencies shares one
   environment. Environments not prepared again for 30 days are deleted;
2. a virtual environment inside the tree: `.venv/`, `venv/`, `env/`,
   `.env/` (a folder with a `pyvenv.cfg`), at the root or in the project
   folder;
3. `python3` on PATH, then `python`.

The project folder is the tree's root when it has dependency files, else the
first folder one level down that has them. **prepare** installs into the
cached environment, using the network, without running project code (the
project itself is never built or installed, so no `setup.py` runs):

- `uv.lock` and `uv` on PATH: `uv sync --frozen --no-install-workspace
  --compile-bytecode` with `UV_PROJECT_ENVIRONMENT` set to the environment,
  plus `--group`/`--extra` for the test, dev, lint and typing groups and
  extras the project declares (uv chooses and may download the Python of
  `.python-version`/`requires-python`);
- `poetry.lock` and `poetry` on PATH: a virtual environment, then
  `poetry install --no-root` into it (`VIRTUAL_ENV`), with those groups;
- otherwise a virtual environment (`uv venv --python <.python-version or
  requires-python>`, else `python3 -m venv`) and `uv pip install` (else `pip
  install`) of the `requirements*.txt` files, `[project]` dependencies and
  those extras, `[dependency-groups]` (with `include-group`), Poetry's tables
  (`^`/`~` turned into ranges), a Pipfile's packages, setup.cfg's
  `install_requires`. Editable and local-path requirements (`-e .`) are
  dropped, `-r`/`-c` includes followed.

Nothing is installed for a project that declares no dependencies. `writable`
is always empty: the environment is only read by the test step. prepare
fails (exit 1, the installer's last lines on stderr) only when installing
fails, or when another prepare is still building the same environment at its
deadline.

**Concurrency.** Two prepares of copies with the same dependency files (a
check's copy and a test run's) build one environment. While it validates or
builds an environment, prepare holds an exclusive `flock` on
`python-envs/.locks/<key>.lock`; the second one waits for it (until its own
deadline), then finds the environment ready and reuses it. An environment is
used (by `test`, `check`, or a prepare skipping the build) only when its
ready marker `difflens-ready.json` exists, and the marker is written last,
whole (a complete file renamed into place), so a half-built environment is
never used; one without the marker (a build that failed or was killed) is
deleted and built again under the lock. Pruning skips an environment whose
lock another prepare holds.

### test (`testrun.py`, `inproject/`)

The tests run once with the project's interpreter in `--dir`, never with the
analyzer's. The project is put on `PYTHONPATH` (its source roots, deepest
first), with distribution metadata written to a temporary folder so that
`importlib.metadata.version("<project>")` and the project's entry points
work without installing it. Offline, `PIP_NO_INDEX` and `UV_OFFLINE` are
set. Everything written goes to a temporary folder in `TMPDIR` (coverage
data, the event stream), apart from what the tests themselves write in the
copy, and the bytecode: `PYTHONPYCACHEPREFIX` is `<--cache-dir>/pycache`
(a temporary folder without `--cache-dir`). With that variable Python
ignores the bytecode next to the sources, so without a kept folder the
standard library and every installed package would be compiled again on
every run. Their paths do not change between runs, so their bytecode is
reused; the tree's paths do, so its bytecode (`<pycache>/<tree path>`) is
removed after the run.

- **pytest** when the environment has it:
  `python -m pytest -p _difflens_pytest -o cache_dir=<temporary>
  --continue-on-collection-errors --tb=short -q` (unittest suites included;
  the project's own pytest configuration and addopts apply),
  plus `--no-cov` with pytest-cov and `-n 0` with xdist when coverage is
  measured. The plugin (`inproject/`, on `PYTHONPATH`) streams the collection
  and each test's end as JSON lines; the progress file follows them.
  Without pytest, unittest suites run through `inproject/_difflens_unittest.py`
  and a diagnostic says so.
- **Packages** are test modules, by container (`tests.test_orders`): `pass`,
  `fail` (`output` null when failing tests explain it), `build_failed` for a
  module that does not import or collect, or for every module under a
  `conftest.py` that does not import (pytest's report as output), `fail`
  with "not every test of the module ran" for one the timeout (or `-x`)
  stopped. A full run also lists
  every other module of the tree as `no_tests`.
- **Tests** are pytest nodes: `name` is the node id without the file
  (`test_total[2-3]`, `TestShipping::test_light`), `function` the id analyze
  gives the test function, found by where it is defined (inherited test
  methods included), `container` that function's module, `package` the module
  it ran in. A failed test's `output` is what it printed (pytest's captured
  sections), then pytest's short traceback, so it ends with the failing
  `E ...` lines; at most 64 KiB, the end kept. xfail is `skip`, a strict
  xpass `fail`. `hierarchy` is always `[name]`: pytest and unittest have no
  subtests the core can select (a parametrised case is its own node), so
  every test is one level, even when its id holds a `/` (`test_url[a/b]`).
- **Commands** (`command` on each test and package), run from the root of
  `--dir`, single-quoted for the shell (`'` written `'\''`):
  - pytest runs from the root, so the file is relative to it:
    `python -m pytest 'tests/test_ship.py::TestShipping::test_light'`
    (the node name as in `name`, parameters included) and
    `python -m pytest 'tests/test_ship.py'` for the module;
  - the unittest fallback: `python -m unittest tests.test_ship.TestShipping.test_light`
    and `python -m unittest tests.test_ship`, with the module's name from
    analyze, run from the folder that name starts at (its source root, with
    `cd svc && ` when that is not the root; `cd 'my tools' && ` when it
    needs quoting); null when the name is not importable (a folder such as
    `bad-name/`);
  - null for a module listed only as `no_tests` in a full run (nothing of
    it ran).
- **Selection**: packages by container, tests by name; a test with subtests
  is `name` + `/` + subtest put back together into the node id, so a
  parametrised case whose id holds a `/` still runs alone. An unknown package
  is a diagnostic.
- **Coverage**, unless `--no-cover`, when coverage.py is in the environment:
  `python -m coverage run --source=<dir> -m pytest ...`, `coverage combine`
  when the project's configuration runs it in parallel mode, then `coverage
  json`. One block per statement line (`statements` 1, `count` 1 or 0) of the
  project's own code: no test files, no generated files, and not the lines
  of `def` statements and their decorators, which run at import time and
  would make every function look covered. Without coverage.py, `coverage` is
  null with a diagnostic. `prepare` adds coverage.py to every environment it
  builds when the project does not declare it (`prepare.add_coverage`;
  environment `FORMAT` 2 rebuilds the ones built before), so the project's own
  dependencies need not list it; a virtual environment in the tree or a
  `python3` on PATH is never changed, and without coverage.py there the
  diagnostic says so.
- `race` is always false (Python has no race detector); `--no-race` is
  accepted. The timeout stops the run (the process group is terminated) a
  little before `--timeout`, and the results so far are reported with a
  diagnostic.
- The command fails (exit 1) only when the tests could not run at all: no
  interpreter, or pytest refusing its command line before collecting.

### check (`checks.py`)

`ruff`, then `mypy`, each when the project uses it: configured (`[tool.ruff]`,
`ruff.toml`, `.ruff.toml`; `[tool.mypy]`, `mypy.ini`, `.mypy.ini`, setup.cfg
`[mypy]`) or installed in the project's environment. Each is looked for in
that environment, then on PATH; otherwise it is `unavailable` with why. The
tree is a copy the core prepared (`prepare` ran on it), so the environment
is the one the tests use.

- `ruff check --output-format json --no-cache --exit-zero .` (the project's
  configuration applies; `--format json` for old versions). Rule: ruff's code.
  Severity: `warning`, since ruff has no levels, except a syntax error (no
  rule, `invalid-syntax` or `E999`), which is `error`.
- `mypy --show-error-codes --no-error-summary --show-absolute-path
  --hide-error-context --explicit-package-bases --cache-dir <cache>` on
  the top-level packages and modules of the source roots (or the `files` the
  configuration at the root names, expanded as mypy expands them), given as
  absolute paths, with the source roots on `MYPYPATH`. It runs as
  `python -m mypy` in the project's environment, or as the `mypy` on PATH
  with `--python-executable` pointing at the environment; with no project
  environment it ignores missing imports and a diagnostic says so. Errors
  and warnings are findings (rule: the error code; severity `error` or
  `warning` as mypy says), notes are dropped; exit status 2 is `failed`.
  mypy loads the plugins the project's configuration names: that is code of
  the project's dependencies, run without a sandbox.
- mypy's cache is `<--cache-dir>/mypy`, kept between runs (a temporary
  folder without `--cache-dir`). It stays correct across copies: the paths
  given are absolute, so a module of the tree is recorded where it was, and
  mypy rehashes a module whose path differs and rechecks it when its contents
  changed (with relative paths, a same-size change in a file with the same
  mtime would pass for unchanged). The standard library and the
  environment's packages keep their paths and are reused. mypy validates its
  version and options too, and writes each cache file by renaming a complete
  one, so two checks of a project may share the folder.
- Neither writes in the tree. A checker that runs out of time is `failed`.

### dupes (`dupes.py`)

The steps are those of `dupes.schema.json`; what is Python's:

- **Files**: the `*.py` files `project.py` lists (the same skipped folders),
  test files included, without generated files (the rule above), files
  over 1 MiB and the files the `exclude` of `--changed` names (a folder of
  the path named one of `dirs`, or a file name ending with one of
  `extensions`, ASCII case ignored). Python has no minified files. A file of
  `--changed` that is not one of them is ignored.
- **Scopes**: a file is test code when `project.is_test_path` says so (the
  rule analyze uses, above: `test_*.py`, `*_test.py`, `conftest.py`, files
  under `tests/` or `test/`), production code otherwise. The files are split
  into the two scopes and the search below runs once per scope, so a clone
  never pairs a test with production code. Production groups come first, then
  test groups; each carries its `scope`, and each scope is capped at 500
  groups on its own. Both scopes share the time limit.
- **Tokens**: the standard `tokenize` module. Comments, blank lines (`NL`),
  `ENCODING` and `ENDMARKER` are dropped (type comments are comments). Names
  are `ID` unless `keyword.iskeyword` (so `True`, `False`, `None`, `async`,
  `await` stay themselves; the soft keywords `match`, `case`, `_`, `type`
  are `ID`); strings and bytes are `STR`; numbers `NUM`; operators
  themselves. An f-string (and a 3.14 t-string) is **one** `STR`, replacement
  fields included, as Python before 3.12 reads it, so every version
  tokenizes it alike. `NEWLINE`, `INDENT` and `DEDENT` are kept as tokens
  (Python's braces) but have no line: they are compared, yet never make a
  clone span or touch a line. Otherwise nearly every clone would begin with
  the `NEWLINE`/`DEDENT` ending the statement before it and span that
  statement and the blank lines after it. A file `tokenize` rejects is a
  diagnostic (with the Python version: `tokenize` reads the interpreter's
  grammar).
- **Imports**: a logical line starting with `import`, or with `from` at the
  start of a statement (not `yield from` nor `raise ... from`), up to its
  `NEWLINE` or `;`, wherever it is (inside `try:`, `if TYPE_CHECKING:`, a
  function), parenthesized and backslash-continued ones included, and
  `from __future__ import`. They are left out and split the file. Docstrings
  are ordinary `STR` tokens.
- **Windows and buckets**: each token is one character of a string (its
  interned kind), so a window is a substring hashed by Python's own string
  hash. The windows touching the change are hashed first; then every window
  of every file is hashed and kept only when its hash is one of those, so
  memory follows the candidates, not the tree. Tokens of a bucket's windows
  are compared before pairing (hashes can collide).
- **Maximal pairs**: extended backward, then forward. A window pair inside a
  maximal pair already found on the same diagonal (the same two files and
  offset) is skipped, so a clone of n windows is extended once, not n times.
- `--timeout`: the search stops a little before it (as analyze does) and
  reports the pairs of the windows collected so far, with a diagnostic.

### mutate (`mutator.py`, `mutation.py`, `inproject/_difflens_mutant.py`)

The rules are those of `mutation.schema.json`; what is Python's:

- **Which lines**: those `--changed` adds that a `--coverage` block with a
  count above 0 spans, in the tree's own files: not test files, not
  generated files (the rules above), not files that do not parse (a
  diagnostic). A mutant is on such a line when its first character is.
- **Mutants** are made on the text, from `ast` positions (UTF-8 byte
  columns, turned into code points for the report) and, for operators, the
  `tokenize` token between the operands, so `original` is the operator
  alone (`>=`, `not in`, `+=`). The operators:
  - `relational_boundary`: `<`/`<=`, `>`/`>=` into each other;
  - `relational_negation`: `==`/`!=`, `<`/`>=`, `>`/`<=`, `in`/`not in`,
    `is`/`is not` into each other;
  - `arithmetic`: `+`/`-` into each other, `*` into `/`, `/`, `//` and `%`
    into `*`; not when an operand is a string, bytes, f-string, list or
    tuple literal (concatenation, repetition, `%` formatting);
  - `logical`: `and`/`or` into each other;
  - `negation_removal`: `not x` into `x` (the `not` and the blanks after
    it removed); an `if`/`elif`/`while` condition `c` into `not (c)`, except
    a single comparison (its `relational_negation` is the same mutant), a
    `not` (its removal is), a constant (`while True:`) and `TYPE_CHECKING`;
  - `return_value`: the returned value into a zero value: a literal into
    its type's (`0`, `""`, `b""`, `[]`, `{}`, `set()`, `()`; `True` and
    `False` into each other), else by the return annotation (`int`,
    `float`, `complex` 0; `str` ""; `bytes` b""; `bool` False; `list`,
    `dict`, `set`, `frozenset`, `tuple` and their typing names empty; any
    other annotation None), else `False` for a comparison, `not`, or
    `and`/`or` of those, else `None`. `return None`, a bare `return` and a
    value that already is the zero are not mutated;
  - `call_removal`: a statement that is only a call, removed (`mutated` is
    empty), or replaced by `pass` when it is alone in its block or shares
    its line with another statement. Kept: `print`, `pprint`,
    `breakpoint`, logging (a callee whose dotted name has a part containing
    `log`, and the methods `debug`, `info`, `warning`, `warn`, `error`,
    `exception`, `critical`, `fatal`, `trace` on anything), assertions (a
    callee name starting with `assert`), and `warnings.*`, `pytest.*`,
    `unittest.*`, `pdb.*`;
  - `increment`: `+=`/`-=` into each other.

  Never mutated: annotations, `if __name__ == "__main__":` tests, and
  assignments outside functions (module and class constants), except
  inside the lambdas they hold. Each mutant is parsed before it is kept; one
  that would not parse is dropped. The `function` is the id `analyze` gives
  the innermost function, lambda or main block holding it (default values
  and decorators belong to the enclosing scope), null at top level.
  `description` reads "replace >= with > in Account.withdraw", "remove the
  call to notify in charge", "return 0 instead in total"…
- **Cap**: rule 4, with a file's top-level group placed at its first
  mutant's line.
- **Tests**: the test modules (`test_*.py`, `*_test.py`) that import the
  mutated module, directly or through other modules of the tree (`import
  a.b`, `from a.b import x`, `from a import b`, relative imports, anywhere in
  the file), the `conftest.py` files above a test module counting as its
  imports; when none does, every test. With pytest:
  `python -m _difflens_mutant pytest -p _difflens_pytest -o
  cache_dir=<run folder> -x -q --tb=short --color=no` (plus `--no-cov` with
  pytest-cov and `-n 0` with xdist) and the test files, or none for every
  test, so the project's own configuration applies; without pytest, the
  unittest runner with `failfast`. Each selection runs once unmutated first;
  if a test fails or a module does not import, every mutant it would test is
  `error` and a diagnostic names the first failure. `killedBy` is the first
  failing test's `name`, as `test` writes it (`TestShipping::test_light`,
  `test_total[2-3]`).
- **Running a mutant**: the tree is never changed. `_difflens_mutant`
  (on `PYTHONPATH` with the other `inproject/` modules) puts a finder first
  on `sys.meta_path` before the runner imports anything; for the module
  name whose file is the mutated one (by real path, so `src/` layouts and
  packages' `__init__.py` work) it compiles the mutant's source, read from
  a file of the run's own folder, in memory: no bytecode of it is read or
  written. Up to `--jobs` runs share the tree, each with its own `TMPDIR`,
  event file and pytest cache folder, in their own process group; a run
  over its time limit (max(`--min-mutant-timeout`, 3 × its baseline's wall
  time)) is terminated, its mutant `timeout`. A mutant whose test modules
  then fail to import, or whose runner exits with an error and no failing
  test, is `error`; `no_coverage` is never reported (the lines are covered
  by construction). The environment is `test`'s (interpreter, `PYTHONPATH`,
  bytecode in `<cache-dir>/pycache`, `--offline`).
- **Kept results**: `<cache-dir>/mutation/<xx>/<fingerprint>.json`, written
  whole (renamed into place), for killed, survived and timeout results. The
  fingerprint hashes a format number, the engine and its version, the
  function's source lines (the whole file for top-level mutants), the
  files the mutant's selection runs (test modules and `conftest.py` files,
  path and contents), and the mutant (file, function id, operator,
  original, mutated, and its position relative to the function's first
  line). The `id` hashes the same mutant identity, so it is stable while the
  function does not move inside itself. A selection whose mutants are all
  kept does not run at all, baseline included.
- `--timeout`: the run stops a little before it (as test does); mutants
  not tested by then (or stopped by it) are left out, with a diagnostic
  saying how many. `--progress` is replaced whole on every change, from the
  moment the mutants are listed.

### Approximations

- Resolution follows what the syntax says. Values that flow through
  containers, `getattr`, dependency injection frameworks or decorators that
  replace a function by another object are not followed; a call through
  such a value has no edge (or a dynamic one, by method name).
- Duck-typed calls are resolved by method name only, within the limits
  above; an arity check is done for `Protocol` methods only.
- Router prefixes are taken from the router's constructor, not from
  `include_router(..., prefix=...)` or `register_blueprint(url_prefix=...)`;
  Django `include()` prefixes are not followed.
- unittest `subTest`s are not reported one by one; test classes whose
  `setUpClass` fails report no tests in the unittest runner.
- Coverage counts are 0 or 1 (coverage.py records whether a line ran).
- Mutation runs share the tree's files: the sources are never changed, but
  tests that write fixed paths inside the tree may meet another run's
  files. A process a test starts with a fresh interpreter (multiprocessing's
  spawn, a subprocess) imports the unmutated module, and so does a module
  pytest's assertion rewriting claims (`register_assert_rewrite`).
  A mutant's time limit comes from its baseline's wall time, which on a
  first run includes compiling the bytecode, so limits are looser then.
  Equivalent mutants (a change that cannot alter behaviour) survive.
- A unittest command (`python -m unittest tests.test_x...`) imports the
  test folder by name; a `tests/` without `__init__.py` is then a namespace
  package, which loses to a regular package called `tests` installed in the
  interpreter's site-packages (some distributions ship one). The run itself
  imports test modules as pytest does and is not affected.

## How it is launched and provisioned

On first need (a Python project's Flow, Quality, Tests or full analysis),
the core (`src-tauri/src/analyzer_envs.rs`) builds, in
`~/Library/Application Support/dev.difflens.app/analyzers/python/<fingerprint>/`:

1. `source/`: a copy of this folder, without `__pycache__/`, `.venv/`, `*.pyc`
   and other build output. The fingerprint is a SHA-256 of exactly these files
   (sorted relative paths and contents), so any change gets a fresh
   environment, and older ones are deleted once it is ready.
2. `venv/`: `uv venv --no-config --python <python3>` when `uv` is on the
   PATH, else `python3 -m venv`, using the user's `python3` (whatever version
   it is: write for Python 3.9 and later, and remember `ast` parses with the
   grammar of the interpreter it runs on). A `difflens_analyzer.pth` in its
   site-packages points at `source/`.
3. When `requirements.txt` lists anything: `uv pip install -r` or
   `<venv>/bin/python -m pip install -r` into the venv. Pin versions.
4. A `ready` marker. A folder without it is rebuilt from scratch.

Every command then runs as
`<venv>/bin/python -I -B -X utf8 -m difflens_analyzer <command> …`: isolated
(`-I`: the project folder and `PYTHON*` variables never reach the analyzer's
imports; the user's site-packages are ignored), no bytecode written (`-B`,
the environment is read-only under the sandbox), UTF-8 I/O. Without
`python3` on the PATH the user sees "The Python analyzer needs python3, …"
with how to install it. The *project's* tests and checkers run with the
project's own interpreter ([Project environments](#project-environments-environmentpy-preparepy)),
never the analyzer's, and nothing is ever installed into the user's project.

## Running it by hand

```sh
cd analyzers/python
python3 -B -m difflens_analyzer analyze --dir /path/to/a/python/tree --timeout 120s
DIFFLENS_PYTHON_ENVS=/tmp/envs python3 -B -m difflens_analyzer prepare --dir /tmp/copy
DIFFLENS_PYTHON_ENVS=/tmp/envs python3 -B -m difflens_analyzer test --dir /tmp/copy --timeout 10m --cache-dir /tmp/cache --progress /tmp/progress.json
DIFFLENS_PYTHON_ENVS=/tmp/envs python3 -B -m difflens_analyzer check --dir /tmp/copy --cache-dir /tmp/cache
python3 -B -m difflens_analyzer dupes --dir testdata/dupes --changed testdata/dupes/changed.json
DIFFLENS_PYTHON_ENVS=/tmp/envs python3 -B -m difflens_analyzer mutate --dir /tmp/copy --changed /tmp/copy/changed.json --coverage /tmp/coverage.json --cache-dir /tmp/cache --progress /tmp/progress.json
```

(`--coverage` is `{"protocol": 1, "blocks": [...]}` with the `coverage.blocks`
of a `test` run of the same copy.)

(`-B` keeps `__pycache__/` out of the repository. Run `test` on a copy: the
tests write in the tree.)

The analyzer's own tests use the standard library only and run with any
Python 3.9 or later; the ones that run tests use the `python3` on PATH (with
pytest when it has it, else unittest):

```sh
cd analyzers/python && python3 -B -m unittest
```

`testdata/` holds fixture projects: `shapes` (a src-layout package with
classes, inheritance, a Protocol, decorators, async code, click and argparse
commands, console scripts), `webapp` (FastAPI, Flask, Django, Celery, Typer,
found from the source; its requirements are not needed for analyze), `uvproject`
(a uv project with pytest and coverage in `uv.lock`), `failing` (failing and
parametrised tests, a unittest case, an editable requirement that prepare
drops), `importerror` (a test module that does not import, a file that does
not parse), `lint` (ruff and mypy configured, with findings), `dupes` (a
branch, described by its `changed.json`, that copies an existing helper,
repeats a block three times, and adds near misses, shared imports and a
generated copy that are not reported), `mutation` (a branch, described by
its `changed.json`, whose tests miss a `>=` boundary, kill every mutant of
a well-tested function, loop forever on one mutant, and never run one
changed function; pytest and coverage in `requirements-dev.txt`, unittest
tests so it also runs without them). To run them
end to end, copy one out of the repository, `prepare` it with
`DIFFLENS_PYTHON_ENVS` pointing at a scratch folder, then `test` and `check`
it.

Through the app:

- `pnpm tauri dev` and open a Python project: debug builds provision from
  this folder directly, so every edit is a new environment (seconds for
  Python).
- Or point the app at a ready program, which is run as it is:
  `DIFFLENS_ANALYZER_PYTHON=/path/to/wrapper pnpm tauri dev`, with a wrapper
  such as
  `#!/bin/sh` / `PYTHONPATH=/abs/path/to/analyzers/python exec python3 -B -m difflens_analyzer "$@"`.
  Cached results follow both the wrapper and this folder's fingerprint.
- The provisioning end to end, with this machine's toolchain:
  `cd src-tauri && cargo test analyzer_envs::tests::the_python -- --ignored --nocapture`.
  It builds a real environment in a temporary folder and runs every command,
  `test` under the real sandbox, checking each document against the core's
  parser. Keep it passing.

## Working on this analyzer alongside the others

Work only inside `analyzers/python/` (the package, its tests, `requirements.txt`,
this README). Leave the shared core alone, so the JS/TS and Rust analyzers can
be written at the same time without conflicts: `src-tauri/**` (in particular
`analyzers.rs`, `analyzer_envs.rs`, `languages.rs`, `flow.rs`, `test_runs.rs`,
`quality.rs`, `sandbox.rs`, `tauri.conf.json`), `analyzers/protocol/*.schema.json`,
`analyzers/go/`, the other `analyzers/*/` folders, `src/` (frontend and i18n),
the root `package.json`, `pnpm-lock.yaml` and `.gitignore`. If the analyzer
needs something from the core (a new file shipped outside the package, a
schema change, another environment step), say so instead of changing it.
