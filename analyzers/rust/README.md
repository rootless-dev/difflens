# difflens-analyzer-rust

The DiffLens analyzer for Rust projects (language id `rust`, detected by a
`Cargo.toml` at the repository root or one folder down). It is written in
Rust on `syn` and shipped as source: DiffLens compiles it the first time a
Rust project needs it. It implements every command with the semantics of
the Go analyzer; "What the analyzer reports" below says how each maps to
Rust.

This file is the contract. It is derived from the Go analyzer
(`analyzers/go/internal/app/`), which is the reference implementation, and
from the JSON schemas in `analyzers/protocol/`, which are authoritative for
every document.

## Layout

```text
Cargo.toml     its own workspace ([workspace]); serde, serde_json, syn 2 (full, visit),
               proc-macro2 (span-locations: line numbers of the syntax)
Cargo.lock     committed; the build is --locked
src/
    main.rs    the binary difflens-analyzer-rust
    cli.rs     command line, exit codes, one document on stdout
    protocol.rs  the documents, as serde types (camelCase, exactly the schema's fields)
    log.rs     JSON line logs on stderr
    workspace.rs  the Cargo projects of a tree (cargo metadata --no-deps --offline,
               or Cargo's conventions when cargo cannot read them)
    manifest.rs   the few Cargo.toml keys read directly (harness, members, dependencies)
    source.rs  parsing, lines, generated files, code inside macro invocations
    index.rs   crates, module trees, items, function ids, name resolution
    walk.rs    function bodies: calls, kinds, closures, references, registrations
    complexity.rs  cyclomatic complexity
    entrypoints.rs main, tests, public API, HTTP routes, CLI commands
    analyze.rs the analyze command
    checks.rs  the check command (clippy), sandbox.rs its macOS sandbox
    testrun.rs the prepare and test commands, coverage.rs cargo-llvm-cov
    cache.rs   the build output kept in --cache-dir: its locks, and what keeps it correct
    dupes.rs   the dupes command: tokens, windows and hashes, pairs, groups
    mutation.rs  the mutate command: candidate lines, cargo-mutants' listing and run,
               sampling, operators, results kept, progress
    engine.rs  cargo-mutants: the pinned version, `prepare --tools` installing it
    process.rs child processes with deadlines and process groups
testdata/      fixture projects of the unit tests (analysed in place, built and run
               only in temporary copies; their dependencies are never fetched)
```

Only `Cargo.toml`, `Cargo.lock` and `src/**` are shipped (`bundle.resources`
in `src-tauri/tauri.conf.json`): no `build.rs`, `benches/` or `tests/` folder
reaches the user's build, so the binary must build from those alone (and
there must be no `build.rs`). After
changing dependencies (`cargo add`, an edit of `Cargo.toml`), update and
commit `Cargo.lock`. syn 2 is used on purpose (syn 3 exists); stay on stable
Rust features, edition 2021: the user's `cargo` may be any recent stable or
nightly toolchain (Rust 1.89 or later: `cache.rs` uses `File::try_lock`).

## Commands

The core runs one command per process, with stdin closed, the current
directory set to `--dir`, `PATH` set to the user's login-shell PATH, and the
rest of the environment inherited from the app.

| Command | Flags the core passes | Prints (stdout) |
| --- | --- | --- |
| `analyze` | `--dir <tree> --timeout 120s` | `analysis.schema.json` |
| `check` | `--dir <tree> --timeout 120s --cache-dir <folder>` | `checks.schema.json` |
| `prepare` | `--dir <tree> --cache-dir <folder>` and, before `mutate`, `--tools <folder>` | `tests.schema.json` `$defs/prepare` |
| `test` | `--dir <tree> --timeout 600s --cache-dir <folder> --progress <file>` and, as asked, `--no-race`, `--no-cover`, `--selection <file>`, `--offline` (always under the sandbox) | `tests.schema.json` |
| `dupes` | `--dir <tree> --timeout 120s --changed <file>` and, as asked, `--min-tokens <n>`, `--min-lines <n>` | `dupes.schema.json` |
| `mutate` | `--dir <tree> --timeout <d> --changed <file> --coverage <file> --cache-dir <folder> --tools <folder> --progress <file> --offline` and, as asked, `--max-mutants <n>`, `--jobs <n>`, `--min-mutant-timeout <d>` | `mutation.schema.json` |
| `--version` (also `-version`, `version`) | none | `difflens-analyzer-rust 0.0.0` and a newline |

Usage, as the Go analyzer prints it:

```text
difflens-analyzer-rust analyze --dir <source tree> [--timeout 120s]
difflens-analyzer-rust prepare --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--tools <folder>]
difflens-analyzer-rust test --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--offline] [--no-race]
    [--no-cover] [--selection <file>] [--progress <file>]
difflens-analyzer-rust check --dir <source tree> [--timeout 5m] [--cache-dir <folder>]
difflens-analyzer-rust dupes --dir <source tree> --changed <file> [--timeout 120s]
    [--min-tokens 50] [--min-lines 6]
difflens-analyzer-rust mutate --dir <source tree> --changed <file> --coverage <file> [--timeout 30m]
    [--cache-dir <folder>] [--tools <folder>] [--max-mutants 200] [--jobs <n>]
    [--min-mutant-timeout 10s] [--progress <file>] [--offline]
difflens-analyzer-rust --version
```

Flags:

- Accept `--flag value` (what the core sends) and `--flag=value`, with one
  or two dashes, as Go's `flag` package does.
- `--dir` is required; `--timeout` is a Go duration (`120s`, `10m`, `1m30s`)
  and must be positive; defaults when absent: analyze `120s`, prepare `10m`,
  test `10m`, check `5m`, dupes `120s`, mutate `30m`. No positional
  arguments. Unknown flags are errors. `cli::parse_duration` reads durations.
- `dupes` requires `--changed` (a file matching `dupes.schema.json`
  `$defs/changed`: protocol 1, unknown fields rejected, lines from 1);
  `--min-tokens` (default 50) and `--min-lines` (default 6) are whole
  numbers of at least 1.
- `mutate` requires `--changed` (as dupes) and `--coverage` (a file
  matching `mutation.schema.json` `$defs/coverage`, read the same way);
  `--max-mutants` (default 200) and `--jobs` (default half the CPUs, at
  least 1) are whole numbers of at least 1, `--min-mutant-timeout` (default
  `10s`) a positive duration. `--tools` (prepare and mutate; optional,
  absolute or taken from the current directory, empty is none) is the
  folder of the mutation engine (see "mutate" below); `mutate` without it,
  or without the engine installed there, fails (exit 1) and says to run
  `prepare --tools <folder>`.
- `--cache-dir` (prepare, test, check and mutate; optional) is a folder of the
  core outside the tree, one per project and analyzer, kept between runs
  (the core deletes it when it grows too large or the project is removed)
  and opened by the core in its test sandbox. A relative path is taken from
  the current directory; an empty one is the same as none. Without it,
  every command behaves as it did before the flag existed.
- `--timeout` bounds the whole command; finish, and report what you have,
  before it. The core kills the process group a little after it: analyze and
  check at 150 s, test at 630 s, and prepare (given no `--timeout`, so 10m by
  default) at 600 s.

Exit codes and streams:

- `0`: exactly one JSON document on stdout, followed by a newline. Nothing
  else on stdout, ever (no `println!`).
- `1`: the command itself failed (the tree is not a directory, an internal
  error). stderr says why; the core shows its last lines to the user
  ("The Rust analyzer failed (exit status: 1): …").
- `2`: bad command line, or a `--selection` or `--changed` file that cannot
  be read or does not match its schema.
- Logs go to stderr as JSON lines (`log.rs`: `time`, `level`, `service`,
  `msg`, fields); the level comes from `DIFFLENS_LOG_LEVEL`, an unknown name
  means info. Keep stdout under 200 MB.

What each command must and must not do:

- `analyze` reads the tree and prints its functions, calls, externals and
  diagnostics. It must not write inside `--dir` (the tree is shared and
  cached per commit) and must not execute project code: no `cargo build`, no
  build scripts or proc macros (`cargo metadata --no-deps --offline` reads
  the workspace without running any). The `id` of a function must be the same
  on both sides of a review even when the function moved or its body changed
  (for Go: import path, receiver, name; closures append `$1`, `$2`…): the
  core matches the old and new analyses by id. Paths are relative to `--dir`
  with `/` separators; lines start at 1. Problems that do not stop the
  analysis (a file that does not parse) are `diagnostics`, not a failure.
  `language` must be `rust`.
- `check` runs static checkers on the tree, read only, no project code. One
  entry per checker the language has, installed or not, in a fixed order; a
  checker that is not installed is `unavailable` with a one-line `message`,
  not a failure. (`cargo clippy` compiles and runs build scripts and proc
  macros: that is project code.)
- `prepare` fetches what the tests will need so `test` can run offline (for
  Cargo: `cargo fetch`). It may use the network, must not execute project
  code, and prints the absolute folders outside `--dir` the test step must
  write to (for example a `CARGO_TARGET_DIR` you choose), which the core opens
  in the sandbox; the cargo registry cache is only read by the test step. It
  never runs under the sandbox.
- `test` runs the project's tests once and prints the run. Failing tests are
  results (exit 0), not a failure. `--dir` is a throwaway copy the tests may
  write in (its `target/` included); `TMPDIR` is set to a short folder of the
  run. Under the sandbox (`--offline` is given, so `cargo --offline`): no
  network beyond loopback, and writes only inside the run folder (the parent
  of `--dir`), `TMPDIR`, the folders `prepare` reported, `--cache-dir`, and
  `/dev/null`-like devices. Reading and executing anything is allowed. With `--progress`, keep
  that file up to date (`$defs/progress`), replacing it whole each time
  (write a temporary file in the same folder and rename it;
  `cli::write_progress` does). With `--selection`, run only what the file
  names (`$defs/selection`, `protocol::Selection`) and report only what ran.
  `container`, `package` and `function` of each test must be written exactly
  as `analyze` writes `container` and function ids, so the core can place
  tests in the call graph. `--no-race` / `--no-cover` turn off the race
  detector and coverage; Rust has no race detector in stable cargo test, so
  `race` is `false`.
- `dupes` reads the tree and prints the duplicated code the lines of
  `--changed` introduce, found exactly as the schema's description says
  (steps 1 to 10), so that every language reports the same kind of groups.
  Like `analyze`, it must not write inside `--dir` nor execute project code.
  A file that cannot be read or tokenized, and a search that runs out of
  time (the groups found so far are kept), are `diagnostics`.
- `mutate` changes one small piece of each added line the tests run (a
  mutant), runs the tests that can see it, and reports which mutants no
  test noticed, as the schema's rules 1 to 7 say. It runs under the same
  isolation as `test`: `--dir` is a throwaway copy `prepare` ran on, and
  it writes only there, in `--cache-dir` and in TMPDIR. Surviving mutants
  are results (exit 0). The core asks for it on a click only.

## What the analyzer reports

`analyze` (syntax only, never builds):

- Functions: free functions, inherent and trait impl methods, trait
  default methods, functions nested in bodies, and closures (as Go lists
  function literals). Ids, from syntax: `<module path>::<name>`,
  `<module path>::<Type>::<name>`, `<module path>::<<Type> as <Trait>>::<name>`,
  `<module path>::<Trait>::<name>`, closures `<parent id>$<k>`. The module
  path starts with the crate root: the library's crate name (`shapes`), or
  `<package>/<bin|tests|benches|examples>/<target>` for other targets (a
  package without a library has its main binary at the crate name), so a
  test's libtest name prefixed with its root is its function id. `name` is
  `Type::method` (Rust's own separator), `qualifiedName` adds the last
  module segment, `container` is the module path. `test`: `#[cfg(test)]`
  code, test attributes, and integration test and bench targets.
  `generated`: an `@generated` (or "generated … do not edit") header.
- Complexity: 1 plus each `if` (`else if` and `if let` included), `while`,
  `for`, `loop`, match arm other than a catch-all, match guard, `&&`, `||`,
  `?` and `let … else`; closures count their own.
- Calls: resolved through modules, `use` imports (globs and re-exports),
  `Self`, `crate`, `super`, other workspace crates, and the type of a
  method's receiver when syntax tells it (parameters, `let` types,
  constructors, struct fields, return types, `?`/`unwrap`). Static for a
  known function or a method of a type of the tree; dynamic for a trait
  object, `impl Trait` or a generic bound (every implementation of the
  tree, or the trait method as an external) and, by method name, for a
  receiver of unknown type (at most 4 candidates, never common standard
  library names). Production callers do not list test-only targets.
  Calls inside macro invocations whose arguments are expressions count.
  Kinds: `conditional` inside branches, loop bodies, the right side of
  `&&`/`||` and after a `?` or a conditional `return`; `async` for what is
  handed to `spawn`/`spawn_blocking`/`spawn_local` (a closure gets an async
  call; an async block's calls are async). Functions used as values are
  `reference`s. Calls of code outside the tree are externals named by
  path (`std::collections::HashMap::new`).
- Entry points: `main` of binaries and build scripts, tests, the public API
  of libraries (`exported_api`, the shortest public path), axum, actix-web
  and rocket routes (`http_route`, with `.nest`/`web::scope`/`mount`
  prefixes, as `registration` calls with "route GET /x" evidence;
  middlewares and fallbacks as registrations), clap subcommands
  (`cli_command` on what a match on a `#[derive(Subcommand)]` enum or on
  `matches.subcommand()` dispatches to). Frameworks are recognised only in
  packages that depend on them.
- Diagnostics: files that do not parse, module files not found, manifests
  cargo could not read.

`check`: one checker, clippy (`--workspace --all-targets`), whose
findings include rustc's lints; `rule` is the lint name
(`clippy::needless_return`, `unused_variables`), and `severity` the level
the compiler gave the message: `error` for `error` (a lint set to deny) and
`error: internal compiler error`, `warning` for `warning`, `info` for
`note`, `help` and `failure-note`. Clippy runs build scripts and proc
macros, so it runs on a copy of each workspace in a scratch folder, after
`cargo fetch`, offline, under the macOS sandbox (no network, writes only in
the scratch folder and, with `--cache-dir`, in `<cache-dir>/clippy`, its
target folder: see "The cache folder"); it is `unavailable` without cargo,
clippy or the sandbox, and `failed` when the code does not compile.

`prepare`: `cargo fetch` in every workspace; no folder to open (the build
goes to `target/` inside the run's copy, or to `--cache-dir`, which the core
opens itself, and the cargo home is only read). It accepts `--cache-dir` and
has nothing to keep there. With `--tools <folder>` it then installs the
mutation engine there when it is missing (see "mutate").

`test`: `cargo build --tests --keep-going --message-format=json` (with
`--offline`, into `target/` of each workspace, or `<cache-dir>/target`
with `--cache-dir`), then each test binary this build reported (never
another one of the target folder) runs as `cargo test` would run it, with
libtest's JSON events (`-Z unstable-options --format json --report-time`,
accepted by a stable libtest through `RUSTC_BOOTSTRAP=1` set on that
process only). A protocol package is a test target, named by its crate
root; targets that do not build are `build_failed` with the compiler's
errors while the others run; `harness = false` targets are judged by their
exit status; doc tests are not run. Failing tests' `output` is what
libtest captured (the raw panic message and the test's own output).
Selections run the named targets with `--exact` filters (Rust tests have
no subtests: a test selected with an empty list does not run, and every
test's `hierarchy` is its name alone, `[name]`, whatever the name holds).
Every test and package has its `command`, the `cargo test` a developer
runs from the root of `--dir` to run it alone: `cargo test -p <package>
--lib` for a package (`--bin <name>`, `--test <name>`, `--example <name>`,
`--bench <name>` by target), and for a test the same followed by
`-- <name> --exact` (a `harness = false` target takes no name filter, so
its command runs it whole); `--manifest-path <folder>/Cargo.toml` comes
first when the workspace is not at the root of `--dir`; words a shell would
read are put in single quotes (`'` as `'\''`). Coverage uses
cargo-llvm-cov (`show-env`, then `report --lcov`, both with the target
folder of the build), with the toolchain's llvm-tools found first so that
cargo-llvm-cov never installs them; the profiles an earlier run left in
the target folder (`*.profraw`, `*.profdata`) are removed before each
build, as `report` merges every one it finds; blocks are runs of lines
with the same count, without test code and generated files.

### The cache folder

With `--cache-dir`, `test` builds in `<cache-dir>/target` and clippy in
`<cache-dir>/clippy` (apart, so that a check never waits for the lock of a
test build), so that dependencies are compiled once, not on every run.
`mutate` builds in `<cache-dir>/mutation/target` (lock
`<cache-dir>/mutation/target.lock`, held for that build and the copy to
the workspace's `target/` only; without coverage instrumentation, so apart
from `test`'s) and keeps its results in `<cache-dir>/mutation/results.json`.
Every run is a fresh copy of the tree at another path, and Cargo shares
artifacts between such copies: the metadata hash of a path package uses
its path relative to the workspace root, and its freshness compares the
mtimes of its files with the earlier build's. A copy that keeps older
mtimes (the core's copy may; clippy's own copy does, as `std::fs::copy`
clones on macOS) would then be taken as fresh against artifacts of other
code. So (`cache.rs`):

- right before each build, every file of the tree being built (for clippy,
  its copy) is set modified now (`File::set_times`; `.git`, `node_modules`
  and Cargo target folders, marked by `CACHEDIR.TAG`, are left alone): the
  tree's own crates, build scripts and path dependencies are always built
  again, while registry dependencies, which Cargo fingerprints by version,
  stay built. (`cargo clean -p` of the members is the alternative: one more
  cargo run per package, and it misses path dependencies that are not
  members.)
- a run holds the folder's lock (`<cache-dir>/target.lock`,
  `<cache-dir>/clippy.lock`, an advisory file lock) from before that touch
  until its last test binary and coverage report are done, so that two runs
  of one project never interleave (a build that starts after another
  copy's touch would make that copy look fresh, and test binaries and
  profiles are used outside Cargo's own lock); a run waits for it until its
  deadline.
- when the folder cannot be used (it cannot be created or locked, a file
  cannot be touched), the run builds where it would without the flag and a
  diagnostic says why.

### dupes

- Files: every `.rs` file under `--dir`, test files included, without
  hidden folders, `target/`, `vendor/` and `node_modules/` (links are not
  followed), generated files (the `@generated` header `analyze` reports),
  files over 1 MiB, and the files the `exclude` of `--changed` names: a
  folder of the path (not the file name) equal to one of `dirs`, or a file
  name ending with one of `extensions`, both ignoring ASCII case. `mutate`
  reads the same file and ignores `exclude`.
- Scopes: every token is production or test code, and the search runs once
  per scope on that scope's tokens, so a clone never pairs test with
  production code. Every file under a folder named `tests` is test code. In
  the other files, syn parses the token stream and every item an outer
  `#[cfg(test)]` or `#[cfg(all(test, …))]` applies to (`index::is_cfg_test`;
  items at any depth, impl and trait items included) is test code, from that
  attribute's `#` through the end of the item, by line and column. Leaving
  or entering such an item splits the file: the production stream has a
  split where the item was, and each test item is a split of its own. A file
  syn cannot parse is all production code, with a diagnostic. Production
  groups come first, then test groups; each carries its `scope`, and each
  scope keeps at most 500 groups (a diagnostic says when one had more).
- Tokens: proc-macro2's lexer (`TokenStream::from_str`), groups flattened
  into their delimiters and contents. Comments are dropped, doc comments
  included (proc-macro2 makes them `#[doc = "…"]` / `#![doc = "…"]`
  attributes, which are dropped as the comments they are). Identifiers are
  `ID`, except keywords (strict and reserved ones, `self`, `Self`, `crate`,
  `super`, `true`, `false`, `_`), which stay as they are; `r#name` is an
  identifier. Lifetimes and labels are one token, `'ID` (`'static` and `'_`
  as they are). Literals are `NUM` when they start with a digit, `STR`
  otherwise (strings, characters, bytes, byte and C strings, raw or not).
  Punctuation joins into the multi-character operators (`::`, `->`, `=>`,
  `==`, `+=`, `..=`, `>>=`…) when proc-macro2 marks it joint; otherwise one
  character is one token. A file that does not lex is a diagnostic.
- Imports: `use`, `extern crate` and `mod name;` items, at any depth, with
  their outer attributes and visibility (`pub use`, `pub(crate) use`), are
  left out and split the file. `use<'a>` capture bounds are not imports.
- Search: a polynomial rolling hash (wrapping u64) over interned token
  ids; equal hashes are compared token by token. Pairs of windows are
  visited sorted, and a pair inside a run already extended on its diagonal
  is skipped, so a long clone is extended once, not once per window. A run
  of one file longer than the distance between its sides (code repeated
  back to back) gives the consecutive pieces as long as that distance, from
  the run's start, so the sides never overlap.

### mutate

The engine is [cargo-mutants](https://mutants.rs/), pinned at **27.1.0**
(`engine::VERSION`; it needs Rust 1.88 or later), reported as
`engine: {"name": "cargo-mutants", "version": "27.1.0"}`. It is never
installed globally: `prepare --tools <folder>` runs, when
`<folder>/cargo-mutants/VERSION` does not say the pinned version,
`cargo install --locked --root <folder>/.cargo-mutants-install-<unique>/root
--target-dir <that folder>/target cargo-mutants@27.1.0` (network, no
project code; about 40 s on an 11-core Mac), writes the `VERSION` marker,
and renames the root to `<folder>/cargo-mutants/` (binary in `bin/`); the
temporary folder, build output included, is deleted. A complete install is
never replaced, a racing prepare that loses finds the winner's, and another
version is set aside and replaced. `mutate --tools <folder>` runs
`<folder>/cargo-mutants/bin/cargo-mutants mutants …`.

The run (`mutation.rs`):

1. Candidate lines: added in `--changed` and inside a `--coverage` block
   with a count above 0 (rule 1).
2. Per workspace with candidate lines in `.rs` files: a unified diff in the
   run's scratch folder (in TMPDIR, removed at the end) whose `+` lines are
   exactly those lines with their text (`@@ -0,0 +<line>,<n> @@` per run of
   lines; cargo-mutants checks the text against the source), then
   `cargo mutants --list --json --workspace --dir <workspace> --in-diff
   <diff>`. Kept: mutants whose first character is on a candidate line (it
   also lists a whole-body replacement when any line of the body changed),
   outside generated files and test functions. `function` is the innermost
   function `analyze` lists around the line (closures included, by lines).
3. Cap (rule 4): groups per function, ordered by file then start line (the
   mutants outside any function one group per file), each group in source
   order; taken round-robin until `--max-mutants`.
4. Results kept (rule 6) in `<cache-dir>/mutation/results.json` (written
   whole under `<cache-dir>/mutation.lock`, the 50 000 most recently used),
   by a fingerprint (128-bit FNV-1a) of: the function's lines, the test code
   of its package (the `#[cfg(test)]` modules and test functions of its
   crates and its whole test and bench targets), cargo-mutants' name and
   version, and the mutant (file, function id, operator, original, mutated,
   line inside the function, column, cargo-mutants' description). Results
   of a failing baseline or a crash of cargo-mutants are not kept.
5. The mutants left, per workspace: the packages' tests are built first
   (`cargo test --no-run --package=<name>@<version>…`), with --cache-dir
   into `<cache-dir>/mutation/target` (its lock held and the tree set
   modified now first, as for `test`; see "The cache folder"), then copied
   to `<workspace>/target`; without it into `<workspace>/target` directly.
   Then `cargo mutants --workspace --dir <workspace> --in-diff <diff>
   --re '^<name>$'… --output <scratch> --jobs <n> --timeout-multiplier 3
   --minimum-test-timeout <--min-mutant-timeout in seconds>
   --copy-target=true --no-shuffle --caught --unviable --colors never`
   (and `--cargo-arg=--offline` and `CARGO_NET_OFFLINE=true` with
   `--offline`), with `TMPDIR` set to the scratch folder and
   `CARGO_TARGET_DIR` removed. `--re` takes exactly the sampled mutants by
   the names `--list` gave (escaped as `regex::escape` does). cargo-mutants
   copies the workspace (its `target/` included, so that dependencies are
   not compiled again per job) into a build folder per job, runs the
   baseline (the tests of the mutated packages, unmutated), then each
   mutant with the tests of its own package only, `--jobs` at a time; it
   stops a mutant's tests after `max(--min-mutant-timeout, ceil(3 x the
   baseline's test time))` (whole seconds: it measures the baseline
   itself). Lints are not capped, so a mutant that trips a `deny` lint does
   not build.
6. Progress: `mutants.out/outcomes.json`, which cargo-mutants rewrites
   after each mutant, is read every 300 ms: `baseline` until its baseline
   outcome is there, then `mutating`; `current` is a mutant whose log
   exists without an outcome.
7. Outcomes: `CaughtMutant` is `killed` (`killedBy`: the first `test <name>
   ... FAILED` line of its log, libtest's name, or null), `MissedMutant`
   `survived`, `Timeout` `timeout`, `Unviable` (does not build) and
   anything else `error`; `durationMs` is its test phase (its build when it
   never ran tests). A baseline that fails makes every mutant of the
   workspace `error`, with a diagnostic holding the end of its log. Two
   seconds before the deadline cargo-mutants gets SIGTERM (it stops the
   tests it runs in process groups of their own and cleans up), its
   process group is killed at the deadline, and whatever still runs from
   the scratch folder is killed (`pkill -f`); mutants without a result are
   left out, with a diagnostic counting them.

Operators: `FnValue` (the body replaced by a value: `0`, `1`, `true`,
`Default::default()`, `Ok(0)`, `()`…) is `return_value`, `original` being
the body's text and `mutated` the replacement; `delete !` is
`negation_removal`; a binary operator is `relational_boundary` (`<`/`<=`,
`>`/`>=`), `relational_negation` (`==`/`!=`, `<`/`>=`, `>`/`<=`),
`logical` (`&&`/`||`), `increment` (`+=`/`-=`), `arithmetic` (any of `+ -
* / %` into another, and their compound assignments), else `other` (`<`
into `==` or `>`, bitwise and shift operators); `delete -`, match arms and
guards and struct fields are `other`. cargo-mutants never removes calls,
so `call_removal` does not occur, nor `no_coverage` (uncovered lines are
not candidates). Spans are cargo-mutants' own: 1-based lines and columns
in characters, the end exclusive, as the protocol wants. `description` is
cargo-mutants' name without its `file:line:column: ` prefix
("replace > with >= in Account::withdraw").

## How it is launched and provisioned

On first need (a Rust project's Flow, Quality, Tests or full analysis), the
core (`src-tauri/src/analyzer_envs.rs`) builds, in
`~/Library/Application Support/dev.difflens.app/analyzers/rust/<fingerprint>/`:

1. `source/`: a copy of this folder, without `target/` and other build
   output. The fingerprint is a SHA-256 of exactly these files (sorted
   relative paths and contents), so any change gets a fresh environment, and
   older ones are deleted once it is ready.
2. `cargo build --release --locked --manifest-path source/Cargo.toml` with
   `CARGO_TARGET_DIR=<env>/target` (downloads crates once, then compiles; up
   to 30 minutes allowed). The binary is copied to `<env>/bin/` and the
   build folder deleted.
3. A `ready` marker. A folder without it is rebuilt from scratch.

Every command then runs as `<env>/bin/difflens-analyzer-rust <command> …`.
Without `cargo` on the PATH the user sees "The Rust analyzer needs cargo, …"
with how to install it. Keep the dependency tree small: every user compiles
it once. To run the *project's* tests, use the user's `cargo` on the project
in `--dir`, never install anything into the user's project.

## Running it by hand

```sh
cd analyzers/rust
cargo run -- analyze --dir /path/to/a/rust/tree --timeout 120s
cargo run -- dupes --dir testdata/dupes --changed testdata/dupes/changed.json
cargo run -- prepare --dir testdata/mutation --tools target/mutation-tools
cp -R testdata/mutation /tmp/mutation-copy   # mutate writes in --dir
cargo run -- mutate --dir /tmp/mutation-copy --changed testdata/mutation/changed.json \
    --coverage testdata/mutation/coverage.json --tools target/mutation-tools
cargo test
cargo test mutation:: -- --ignored   # runs cargo-mutants (installs it the first time)
```

The tests that run cargo-mutants are `#[ignore]`d: the first one installs
it with `prepare --tools` into `target/mutation-tools` (network, about
40 s), and they take about 15 s more.

(`target/` here is ignored by git and left out of the fingerprint and the
bundle.)

Through the app:

- `pnpm tauri dev` and open a Rust project: debug builds provision from this
  folder directly, so every edit is a new environment and a full release
  build (a minute or so). To iterate faster, point the app at your own build,
  which is run as it is:
  `cargo build --release && DIFFLENS_ANALYZER_RUST=$PWD/target/release/difflens-analyzer-rust pnpm tauri dev`
  (from the repository root, give the absolute path). Cached results follow
  both the binary and this folder's fingerprint.
- The provisioning end to end, with this machine's toolchain:
  `cd src-tauri && cargo test analyzer_envs::tests::the_rust_stub -- --ignored --nocapture`.
  It builds a real environment in a temporary folder and runs every command,
  `test` under the real sandbox, checking each document against the core's
  parser. Keep it passing.

## Working on this analyzer alongside the others

Work only inside `analyzers/rust/` (`src/`, `Cargo.toml`, `Cargo.lock`, this
README). Leave the shared core alone, so the Python and JS/TS analyzers can
be written at the same time without conflicts: `src-tauri/**` (in particular
`analyzers.rs`, `analyzer_envs.rs`, `languages.rs`, `flow.rs`,
`test_runs.rs`, `quality.rs`, `sandbox.rs`, `tauri.conf.json`, and
`src-tauri/Cargo.toml`, which is another crate), `analyzers/protocol/*.schema.json`,
`analyzers/go/`, the other `analyzers/*/` folders, `src/` at the repository
root (frontend and i18n), the root `package.json`, `pnpm-lock.yaml` and
`.gitignore`. Tests may live in `src/` (`#[cfg(test)]`) or `tests/` (not
shipped, which is fine: the release build does not need them); a `build.rs`
would not be shipped and would break the user's build. If the analyzer needs something from the core
(a file shipped outside `src/`, a schema change, another environment step),
say so instead of changing it.
