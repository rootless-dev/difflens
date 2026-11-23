# difflens-analyzer-jsts

The DiffLens analyzer for JavaScript and TypeScript projects (language ids
`javascript` and `typescript`: a `package.json` at the repository root or one
folder down is JavaScript, unless a `tsconfig.json` is there too, which makes
it TypeScript). It is written in plain ESM JavaScript for Node, runnable as
it is (no build step), on the TypeScript compiler API, and shipped as source:
DiffLens builds its environment the first time such a project needs it.
Its only dependency is `typescript`.

This file is the contract. It is derived from the Go analyzer
(`analyzers/go/internal/app/`), which is the reference implementation, and
from the JSON schemas in `analyzers/protocol/`, which are authoritative for
every document. What each command produces, and how, is in
[What the commands produce](#what-the-commands-produce).

## Layout

```text
package.json         "type": "module"; runtime dependencies only in "dependencies"
package-lock.json    committed; what npm ci installs
src/
    main.js          node src/main.js
    cli.js           command line, exit codes, one document on stdout
    protocol.js      the documents, and languageOf(dir)
    log.js           JSON line logs on stderr
    files.js         source files, test files, generated files, containers
    process.js       running the project's tools (process groups, deadlines)
    sandbox.js       the read-only sandbox of the checkers (macOS)
    analyze/         analyze: functions.js (ids, complexity), programs.js
                     (TypeScript programs), resolve.js, calls.js,
                     entrypoints.js, normalize.js, index.js
    check.js         check: tsc and eslint
    dupes/           dupes: tokens.js (normalized tokens), search.js (windows,
                     hashes, maximal pairs), groups.js (sides, blocks,
                     groups), index.js
    prepare.js       prepare: the project's package manager
    testrun/         test: units.js (runners), index.js (invocations),
                     collect.js (results, progress), commands.js (name
                     patterns, the commands that run a test again),
                     coverage.js, reporters/ (loaded by the project's runner)
    mutate/          mutate: index.js (the run), plan.js (candidates, ids,
                     sample, ranges), points.js (mutants of a file),
                     operators.js (families, statuses), names.js (killedBy),
                     stryker.js (Stryker's configuration and process),
                     tools.js (prepare --tools), reporter.mjs and
                     preload.mjs (loaded into Stryker's processes),
                     engine/ (StrykerJS's pinned package.json and lockfile)
test/                node --test unit tests (npm test)
testdata/            fixture projects; their node_modules are not committed
```

Only `package.json`, `package-lock.json` and `src/**` are shipped
(`bundle.resources` in `src-tauri/tauri.conf.json`). Anything the analyzer
needs at run time must live under `src/` or come from `dependencies`.

`typescript` is pinned to **6.0.3**, the last release whose npm package has
the JavaScript compiler API (`ts.createSourceFile`, `ts.createProgram`, the
type checker). TypeScript 7's package ships only a native compiler and an
unstable API. Change the version with `npm install --package-lock-only
--ignore-scripts typescript@<version>` so the lockfile follows; keep exact
versions.

## Language reported

Every document's `language` is `typescript` when `--dir` has a
`tsconfig.json` at its root or one folder down (hidden folders and
`node_modules`, `vendor`, `target`, `dist`, `build` aside), and `javascript`
otherwise, as the core detects it: `protocol.languageOf(dir)`. The core
accepts either from this analyzer. A TypeScript project's `.js` files are
analysed too.

## Commands

The core runs one command per process, with stdin closed, the current
directory set to `--dir`, `PATH` set to the user's login-shell PATH, and the
rest of the environment inherited from the app.

| Command | Flags the core passes | Prints (stdout) |
| --- | --- | --- |
| `analyze` | `--dir <tree> --timeout 120s` | `analysis.schema.json` |
| `check` | `--dir <copy> --timeout 120s --cache-dir <folder>`, after `prepare` on that copy | `checks.schema.json` |
| `dupes` | `--dir <tree> --timeout 120s --changed <file>` and, as asked, `--min-tokens <n>`, `--min-lines <n>` | `dupes.schema.json` |
| `prepare` | `--dir <copy> --cache-dir <folder>` (before `test`, and before `check`) | `tests.schema.json` `$defs/prepare` |
| `test` | `--dir <copy> --timeout 600s --cache-dir <folder> --progress <file>` and, as asked, `--no-race`, `--no-cover`, `--selection <file>`, `--offline` (always under the sandbox) | `tests.schema.json` |
| `prepare --tools` | `--dir <copy> --cache-dir <folder> --tools <folder>` (before `mutate`) | `tests.schema.json` `$defs/prepare` |
| `mutate` | `--dir <copy> --timeout <d> --changed <file> --coverage <file> --cache-dir <folder> --tools <folder> --progress <file>` and, as asked, `--max-mutants`, `--jobs`, `--min-mutant-timeout`, `--offline` | `mutation.schema.json` |
| `--version` (also `-version`, `version`) | none | `difflens-analyzer-jsts 0.0.0` and a newline |

Usage, as the Go analyzer prints it:

```text
difflens-analyzer-jsts analyze --dir <source tree> [--timeout 120s]
difflens-analyzer-jsts prepare --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--tools <folder>]
difflens-analyzer-jsts test --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--offline] [--no-race]
    [--no-cover] [--selection <file>] [--progress <file>]
difflens-analyzer-jsts check --dir <source tree> [--timeout 5m] [--cache-dir <folder>]
difflens-analyzer-jsts dupes --dir <source tree> --changed <file> [--timeout 120s]
    [--min-tokens 50] [--min-lines 6]
difflens-analyzer-jsts mutate --dir <source tree> --changed <file> --coverage <file> [--timeout 10m]
    [--cache-dir <folder>] [--tools <folder>] [--max-mutants 200] [--jobs <n>]
    [--min-mutant-timeout 10s] [--progress <file>] [--offline]
difflens-analyzer-jsts --version
```

Flags:

- Accept `--flag value` (what the core sends) and `--flag=value`.
- `--dir` is required; `--timeout` is a Go duration (`120s`, `10m`, `1m30s`)
  and must be positive; defaults when absent: analyze `120s`, prepare `10m`,
  test `10m`, check `5m`, dupes `120s`, mutate `10m`. No positional
  arguments. Unknown flags are errors. `cli.parseDuration` reads durations.
- `dupes` requires `--changed`; `--min-tokens` (default `50`) and
  `--min-lines` (default `6`) are whole numbers of at least 1.
- `mutate` requires `--changed` and `--coverage`; `--max-mutants` (default
  `200`) and `--jobs` (default half the CPUs, at least 1) are whole numbers
  of at least 1; `--min-mutant-timeout` (default `10s`) is a positive
  duration; `--tools` (also on `prepare`) is the folder of the mutation
  engine.
- `--cache-dir` (prepare, test, check and mutate, optional) is a folder of
  the core outside the tree, one per project and analyzer, kept between
  runs, that the core opens for writes in the test sandbox. `mutate` keeps
  StrykerJS's incremental results there ([mutate](#mutate)); prepare, test
  and check accept it and keep nothing there: each run's tree is a fresh
  copy at another path, and every
  cache of this ecosystem worth keeping is keyed by absolute paths. Jest's
  haste map and transform cache live in folders named after
  `sha1(rootDir + config path)`, so a copy elsewhere starts new ones (checked
  with Jest 30: a second copy wrote a second set of folders and reused
  nothing); tsc's build info and ESLint's cache name files by absolute path;
  Vitest, Mocha and `node --test` keep nothing worth reusing. Without the
  flag, everything is the same. `dupes` does not take it.
- `--timeout` bounds the whole command; finish, and report what you have,
  before it. The core kills the process group a little after it: analyze and
  check at 150 s, test at 630 s, and prepare (given no `--timeout`, so 10m by
  default) at 600 s.

Exit codes and streams:

- `0`: exactly one JSON document on stdout, followed by a newline. Nothing
  else on stdout, ever (no `console.log`). `main.js` sets `process.exitCode`
  instead of calling `process.exit`, so a large document is flushed whole.
- `1`: the command itself failed (the tree is not a directory, an internal
  error). stderr says why; the core shows its last lines to the user
  ("The JavaScript/TypeScript analyzer failed (exit status: 1): …").
- `2`: bad command line, or a `--selection`, `--changed` or `--coverage`
  file that cannot be read (`--coverage` must match `mutation.schema.json`
  `$defs/coverage` exactly; `--changed` must match `dupes.schema.json` `$defs/changed`
  exactly: unknown fields are rejected, `protocol` must be 1).
- Logs go to stderr as JSON lines (`log.js`: `time`, `level`, `service`,
  `msg`, fields); the level comes from `DIFFLENS_LOG_LEVEL`, an unknown name
  means info. Keep stdout under 200 MB.

What each command must and must not do:

- `analyze` reads the tree and prints its functions, calls, externals and
  diagnostics. It must not write inside `--dir` (the tree is shared and
  cached per commit) and must not execute project code (no loading the
  project's config files with `import()`). The `id` of a function must be the
  same on both sides of a review even when the function moved or its body
  changed (for Go: import path, receiver, name; closures append `$1`,
  `$2`…): the core matches the old and new analyses by id. Paths are relative
  to `--dir` with `/` separators; lines start at 1. Problems that do not stop
  the analysis (a file that does not parse) are `diagnostics`, not a failure.
- `check` runs static checkers on a throwaway copy of the tree that
  `prepare` has installed, read only: it must not write inside `--dir`, and
  what runs project code (ESLint's configuration and plugins) runs in this
  analyzer's own sandbox. One entry per checker the language has, installed
  or not, in a fixed order; a checker that is not installed is `unavailable`
  with a one-line `message`, not a failure. Each finding has the checker's
  `severity` (`error`, `warning`, `info`) and its message as the checker
  words it, without a severity prefix.
- `prepare` fetches what the tests and the checkers will need so `test` can
  run offline (for a project without `node_modules`, its own `npm ci`, with
  `--ignore-scripts` unless you have decided otherwise). It may use the
  network, must not execute project code, and prints the absolute folders
  outside `--dir` the test step must write to, which the core opens in the
  sandbox. It never runs under the sandbox. It may run for two copies of one
  project at the same time (before `test` and before `check`): what it shares
  between them, the package managers' own stores and caches, is safe for
  that.
- `test` runs the project's tests once and prints the run. Failing tests are
  results (exit 0), not a failure. `--dir` is a throwaway copy the tests may
  write in; `TMPDIR` is set to a short folder of the run. Under the sandbox
  (`--offline` is given): no network beyond loopback, and writes only inside
  the run folder (the parent of `--dir`), `TMPDIR`, the folders `prepare`
  reported, `--cache-dir`, and `/dev/null`-like devices. Reading and
  executing anything is allowed. With `--progress`, keep that file up to
  date (`$defs/progress`), replacing it whole each time (write a temporary
  file in the same folder and rename it; `cli.writeProgress` does). With
  `--selection`, run only what the file names (`$defs/selection`) and report
  only what ran. `container`, `package` and `function` of each test must be
  written exactly as `analyze` writes `container` and function ids, so the
  core can place tests in the call graph. Every test has its `hierarchy`,
  and every test and package its `command` (null only when there is none).
  `--no-race` / `--no-cover` turn off the race detector and coverage;
  JavaScript has no race detector, so `race` is `false`.
- `dupes` reads the tree and prints the duplicated code the lines of
  `--changed` introduce. It must not write inside `--dir` and must not
  execute project code; it follows the algorithm of the schema's
  description (steps 1 to 10) so that every language reports alike.
- `mutate` runs the project's tests against mutants of the lines
  `--changed` adds that `--coverage` says ran, on a throwaway copy that
  `prepare` (with `--tools`) installed, under the same sandbox as `test`. It
  may write anything inside `--dir`; outside it, only in `--cache-dir` and
  `TMPDIR`. Surviving mutants are results (exit 0).

## What the commands produce

### analyze

The tree is read through TypeScript programs: one per `tsconfig.json` /
`jsconfig.json` found in it (any depth, `references` followed, `extends`
and `paths` respected, read as data: nothing of the project runs), and one
more with `allowJs` for the files no configuration includes. Nothing is
emitted or built (`noEmit`, no build info), and the options TypeScript 6
deprecates still apply (`ignoreDeprecations: "6.0"`). The analysed tree
usually has no `node_modules`; imports of dependencies then do not resolve,
and the packages of a workspace are resolved to their sources from their
`package.json` files. Folders skipped: `node_modules`, `vendor`, `target`,
`dist`, `build`, `out`, `coverage` and hidden folders; `.d.ts` files are not
listed.

- **Functions**: function declarations, methods, constructors, getters and
  setters, class fields and object properties holding functions, function
  expressions and arrow functions, and the body of a module that runs code
  when it loads, `(module)`. Ids are the module path (the file's path
  without its extension) and the function's path in it, from syntax only:
  `src/shapes.Circle.area`, `src/shapes.Square.get name`,
  `src/util.helpers.double`, `src/index.default`,
  `lib/index.exports.startTicker` (CommonJS), `lib/legacy.Counter.increment`
  (`Counter.prototype.increment = ...`), `src/a.outer.inner` (a named
  function inside another). Anonymous functions are closures numbered among
  their parent's, `src/a.outer$1`, `src/a.handler$1` for
  `const handler = wrap(() => ...)`. Test callbacks are named by their
  titles: `src/a.test.describe("math").it("adds")`, hooks by their kind
  (`...describe("math").beforeEach`). A name repeated in a file gets `#2`.
  `name` is the path (`Circle.area`), `qualifiedName` adds the module's name
  (`shapes.Circle.area`, the folder's for an index file), `container` is the
  file's folder (`src/routes`; the root package's name for root files).
- **test** is true in test files (`*.test.*`, `*.spec.*`, `test-*`, `*-test`,
  `*_test`, `test.js`, and anything under `__tests__`, `__mocks__`,
  `__fixtures__`, `test`, `tests`, `spec`, `e2e`) and in test callbacks of a
  file that imports a test framework (in-source tests). **generated** is true
  for a header comment saying so (`@generated`, `Code generated ... DO NOT
  EDIT`, `auto-generated`), `*.min.js`, `*.generated.*`, `*.gen.*`,
  `__generated__/`, and minified files.
- **Complexity**: 1 plus `if` (an `else if` is an `if`), `for`, `for-in`,
  `for-of`, `while`, `do-while`, each `case` (not `default`), `catch`, the
  conditional operator, and `&&`, `||`, `??`, `&&=`, `||=`, `??=`. Optional
  chaining and default values are not counted: they handle a missing value
  rather than choose a path through the logic (as ESLint's classic
  `complexity` rule does). Nested functions are separate functions.
- **Calls** (`calls.js`), resolved with the type checker: static when the
  callee is one declaration of the tree; dynamic for an interface member or
  an abstract method (each method of the tree's classes implementing or
  extending it, or the member itself as an external when there is none; a
  test fake is left out of production callers), for a method of a union of
  types (each member's), and for a call of a function parameter (each
  function of the tree passed for it at the function's call sites). Getters
  and setters are called by reading and assigning; `f.call` / `f.apply`
  call `f`; `new C()` calls `C.constructor`; a JSX component is called by
  the element that renders it. A callee outside the tree is an external
  named after its import when it has one (`express.Router`,
  `fs.readFileSync`, the same with or without @types), else after its
  declaration (`lib.Array.map` for the JavaScript library,
  `express-serve-static-core.Response.json`). A value of unknown type
  (dependencies not installed) is left out.
- **Kind**: `always` unless the call is in a branch (if/else, a case, the
  conditional operator, the right of `&&` `||` `??`, an optional chain, a
  catch, a default value, a loop body other than `do-while` and
  `while (true)`) or after a statement that may return early (or break out
  of such a loop); `throw` is ignored, as Go ignores panics. `async`: a
  function handed to `setTimeout`, `setInterval`, `setImmediate`,
  `queueMicrotask`, `process.nextTick`, `requestAnimationFrame`,
  `requestIdleCallback` or to a promise's `then` / `catch` / `finally`, and a
  promise-returning call whose result is dropped (a statement of its own,
  `void`, or a dropped then-chain). `await`ed calls are not async.
- **Registrations** (`entrypoints.js`), each `async`/`static` with its
  evidence: routes (`route GET /users/:id`), middleware, plugins, Fastify
  hooks, CLI commands (`command serve`), jobs (`schedule */5 * * * *`),
  event listeners (`event tick`, from `on`, `once`, `addListener`,
  `prependListener`, `addEventListener`), and test callbacks (`it adds`).
  **References**: a function of the tree used as a value without being called
  there (an argument, a property, an assignment, a return, a JSX prop),
  closures included, unless a call or registration of that line covers it.
- **Entry points**: `main` for the modules of `bin` (labelled with the
  command) and of `main` in a private package; `exported_api` for the
  exports of `main`, `module`, `source` and `exports` of a published package
  (`<package>.<name>`, `<package>/<subpath>.<name>`, public methods of
  exported classes), build paths (`dist/index.js`) mapped back to sources;
  `test` for top-level describe/test callbacks; `http_route` for Express,
  Koa, Hono, Fastify and restify style routers
  (`<router>.get(path, ...handlers)` with a constant path, `route(path)`
  chains, Fastify `route({ method, url, handler })`, Hono `on(method, path,
  ...)`), with the prefixes of `use(prefix, router)`, `route(prefix, app)`
  and `register(plugin, { prefix })` put in front, for NestJS controllers
  (`@Controller` + `@Get`...), Next.js route handlers
  (`app/**/route.ts`) and API routes (`pages/api/**`), and Remix / React
  Router route modules (`loader`, `action`) when the package depends on the
  framework; `message_handler` (`@MessagePattern`, `@EventPattern`,
  `@SubscribeMessage`, `@OnEvent`); `scheduled_job` (`@Cron`, `@Interval`,
  `@Timeout`, node-cron, cron); `rpc_method` (`@GrpcMethod`, GraphQL
  resolvers); `cli_command` (commander and cac `.command(...).action(fn)`,
  yargs `.command(...)`). All of it is recognised from the code alone.
- **Diagnostics**: syntax errors (at most 3 per file, with their line), a
  configuration that cannot be read, and a run that ran out of time (the
  calls of the files left are not all analysed, the functions are).

### check

The core runs `prepare` on a throwaway copy of the commit's tree, then
`check` on that copy, so `node_modules` is there and the project's own
tools run: `tsc` then `eslint` (`check.js`), each the project's
(`node_modules` of the folder or one above, up to the tree, which finds
the tools of a workspace package installed in it or hoisted to the root,
and pnpm's symbolic links into `node_modules/.pnpm`), else one on the PATH
(said in a diagnostic), else `unavailable`. `tsc --noEmit -p <tsconfig>
--pretty false` runs for every `tsconfig.json` with files of its own (an
incremental project keeps its build info in a temporary folder); findings
are `TS<code>`, errors about unbuilt project references are left out, and
the severity is tsc's category (`error`; `warning`; `suggestion` and
`message` are `info`). `eslint --format json .` runs where a configuration
is (the root, else each package of the workspace, from `workspaces` or
`pnpm-workspace.yaml`, and each folder one down; flat, `eslint.config.ts`
included, or legacy, `ESLINT_USE_FLAT_CONFIG` set to match the version);
severity 2 is `error`, 1 `warning`, and a fatal (parse) error `error`, and
messages are ESLint's own, with no prefix. ESLint loads the project's
configuration and plugins, which are code, so on macOS both run under
`sandbox-exec` with the network off and writes allowed only in a temporary
folder (`DIFFLENS_JSTS_NO_SANDBOX=1` turns that off). Plugins that cache by
convention write there too: `CACHE_DIR` (find-cache-dir, the usual
`node_modules/.cache` users) and `XDG_CACHE_HOME` point at it, and the
caches of jiti (TypeScript configurations) and `@babel/register`, which
would be written next to the code and refused, are off
(`JITI_FS_CACHE=false`, `BABEL_DISABLE_CACHE=1`). Nothing goes to
`--cache-dir` (see [Commands](#commands)). Checked on a pnpm workspace
whose package has `eslint.config.ts` with typescript-eslint
(`testdata/pnpm-workspace`).

### prepare

Installs the project's dependencies into the run's copy with its package
manager and install scripts off (`prepare.js`): `pnpm install
--frozen-lockfile --ignore-scripts`, `yarn install --immutable
--mode=skip-build` (Berry) or `--frozen-lockfile --ignore-scripts`
(classic), `bun install --frozen-lockfile --ignore-scripts`, `npm ci
--ignore-scripts`, or `npm install --ignore-scripts --no-package-lock`
without a lockfile; corepack provides pnpm and yarn when they are not on
the PATH. The root package (a workspace root, from `workspaces` or
`pnpm-workspace.yaml`, installs every package even when it depends on
nothing itself), else each folder one down with a package.json; a package
with `node_modules` already, or without dependencies, is left as it is.
The caches stay where the package managers keep them, outside the tree
(they are safe for two installs at once); `writable` is empty, and nothing
goes to `--cache-dir`.

With `--tools <folder>`, prepare also installs the mutation engine `mutate`
drives, StrykerJS, into `<folder>/stryker` when it is not there
(`mutate/tools.js`). StrykerJS is not a dependency of the analyzer (its
environment keeps only typescript): its pinned manifest is
`src/mutate/engine/package.json` and `package-lock.json` (under `src/`, so
it ships with the app), exact versions:

| Package | Version |
| --- | --- |
| `@stryker-mutator/core` (with `@stryker-mutator/instrumenter`, the command runner) | 10.1.1 |
| `@stryker-mutator/vitest-runner` | 10.1.1 |
| `@stryker-mutator/jest-runner` | 10.1.1 |
| `@stryker-mutator/mocha-runner` | 10.1.1 |

The install copies the manifest into a temporary folder inside `<folder>`,
runs `npm ci --omit=dev --ignore-scripts --legacy-peer-deps --no-audit
--no-fund` there (no install scripts; `--legacy-peer-deps` leaves out the
plugins' peer dependencies, vitest and mocha, so the project's own are
used), writes `.difflens-engine` (a SHA-256 of the manifest) and renames the
folder to `<folder>/stryker`. An install whose marker matches is left as it
is (no network); one of another manifest is replaced. Two prepares at once
each install into their own temporary folder; the one that finishes second
finds the first one's install in place and drops its own. It runs no
project code. Update the versions with `npm install --package-lock-only
--ignore-scripts --legacy-peer-deps` in `src/mutate/engine/` after editing
its `package.json` (exact versions only).

### test

Runners (`testrun/units.js`), per package folder: the one its `test` script
names (vitest, jest, mocha, `node --test`, also through tsx, ts-node, c8 or
nyc), else a runner's configuration file (`vitest.config.*`,
`jest.config.*` or a `jest` key, `.mocharc.*` or a `mocha` key), else a
runner among its dependencies (vitest, jest, mocha), else `node --test` when
a test file imports `node:test`. The root is the one unit unless it is a
workspace root delegating to its packages. Each runs with the project's own
binary and a reporter of `src/testrun/reporters/` that streams results to
the progress file; test files no runner takes are said in a diagnostic.

- Names: titles joined by `/` (a `/` in a title becomes `∕`, U+2215):
  `math/divide/refuses zero`; `hierarchy` is those titles as the name writes
  them, from the top-level describe/test down (`["math", "divide", "refuses
  zero"]`, so joining it with `/` gives the name); every describe level is
  listed with the outcome of what it holds, its output the failures below
  it. `function`,
  `file`, `line` are those of the top-level describe/test callback, as
  analyze names it; `output` of a failed test is the runner's own failure
  text (message, stack, diff). A file that does not load (a syntax error,
  an import that fails) makes its package `build_failed`.
- Packages are folders; a whole run lists every folder of the project
  (`no_tests` without test files). A selection runs the test files of the
  packages it names, and for a package with some tests only, those by name
  pattern (titles joined by spaces, as the runners match them); a subtest
  runs its first-level ancestor, and with `node --test` its top-level test.
- Commands (`testrun/commands.js`): each test and package has the command
  that runs it again on its own from the root of `--dir`, with the runner
  of its unit, `cd <folder> && ` in front for a unit that is not the root,
  files relative to that folder, and every argument in single quotes (`'`
  becomes `'\''`). The pattern is the titles joined by spaces,
  regexp-escaped (a title's real `/`, not `∕`), anchored at the start, and
  at the end too for a test with nothing below it:

  | Runner | Test | Package |
  | --- | --- | --- |
  | vitest | `npx vitest run 'src/math.test.ts' -t '^(?:math divide refuses zero)$'` | `npx vitest run 'src/math.test.ts'` |
  | jest | `npx jest 'src/__tests__/sum.test.js' -t '^(?:sum adds)$'` | `npx jest 'src/__tests__/sum.test.js'` |
  | mocha | `npx mocha 'test/stack.spec.js' --grep '^(?:Stack when empty)(?: \|$)'` | `npx mocha 'test/stack.spec.js'` |
  | node | `cd api && node --import tsx --test --test-name-pattern='^(?:routes)$' 'src/a.test.ts'` | `cd api && node --import tsx --test 'src/a.test.ts'` |

  `node --test` is given the top-level test, as a selection is: a subtest
  made with `t.test` exists only while its parent runs, and a parent that
  matches runs every test below it. Mocha's command leaves c8 / nyc out; the
  loaders of a `node --test` script (`--import tsx`, `--require
  ts-node/register`) are kept. A package without test files, or whose files
  no runner takes, has `command: null`.
- Coverage, unless `--no-cover`, with the runner's provider: Vitest's
  `@vitest/coverage-v8` or `-istanbul` (unavailable when neither is
  installed; written even when tests fail), Jest's, node's
  `--experimental-test-coverage` (lines holding code), c8 or nyc for Mocha
  (unavailable without them). Blocks are the project's own source files:
  not tests, not generated, not dependencies. A run stopped by the time
  limit reports what finished and no coverage.
- `race` is always false: `--no-race` is accepted and has nothing to turn
  off.

### dupes

The algorithm is the schema's (`dupes.schema.json`, its description); what
is particular to JavaScript and TypeScript:

- **Files**: the source files `analyze` lists (same extensions, `.d.ts`
  out, same skipped folders), test files included, without the files
  `analyze` marks generated (header comment, `*.min.js`, `*.generated.*`,
  `*.gen.*`, `__generated__/`, minified), files over 1 MiB, and the files
  the `exclude` of `--changed` names (translation catalogs): a folder of the
  path (not the file name) equal to one of `exclude.dirs`, or a file name
  ending with one of `exclude.extensions`, both ignoring ASCII case, so
  `exclude.dirs: ["locales"]` leaves out `src/i18n/locales/en.ts` and
  `src/i18n/Locales/en.ts` but not `src/locales.ts`. No `exclude`, nothing
  left out. `mutate` reads the same file and ignores `exclude`.
- **Scopes** (`dupes/index.js`): a file is test code when `analyze` calls it
  a test file (`isTestFile` in `files.js`: `*.test.*`, `*.spec.*`,
  `test.js`, `test-*.js`, `*-test.js`, `*_test.js`, and anything under
  `__tests__`, `__mocks__`, `__fixtures__`, `test`, `tests`, `spec` or `e2e`),
  production code otherwise. The search runs once on the production files
  and once on the test files, so a clone never pairs test code with
  production code, and every group carries its `scope`.
- **Tokens** (`dupes/tokens.js`): the leaves of the TypeScript parser's
  tree, and the punctuation the scanner finds between them, so regular
  expressions, template pieces and JSX text are read as the parser reads
  them (`.jsx`, `.tsx` and JavaScript files with JSX). Identifiers, private
  names and JSX tag names are `ID`; string literals, template pieces
  (`` `a${ ``, `}b${`, `` }c` ``, and templates without substitutions),
  regular expressions and JSX text are `STR`; numbers and bigints are
  `NUM`; keywords, operators and punctuation are compared by their kind.
  Comments and white space (JSX white space included) are dropped. Import
  declarations, `import x = ...`, `export ... from`, and top-level
  statements that only bind or run `require(...)` (`const x = require("y")`,
  `const { a } = require("y").z`, `require("./setup")`) are left out and
  split the file.
- **Search** (`dupes/search.js`): two 32-bit lanes of a polynomial rolling
  hash over the token ids, candidate groups in two passes (hashes of the
  windows touching the change, then every window with one of them), groups
  of one or of more than 64 windows dropped, every pair verified token by
  token. A pair is extended backward, then forward; in one file the two
  sides never overlap (a run longer than the distance between the sides
  stops at that distance). A window lying inside a maximal pair already
  found on the same diagonal (the same two files at the same offset) is not
  extended again, so a long clone costs one extension.
- **Groups** (`dupes/groups.js`): a maximal pair is kept only when both of
  its sides span at least `--min-lines` lines. A block whose occurrences are all
  blocks copying existing code (so it has no other block repeated in the
  change to list) is not reported on its own. Production groups come
  first, then test groups; at most 500 groups per scope.
- **Diagnostics**: a file that cannot be read or tokenized, groups past 500
  (one diagnostic per scope that had more), and a search that ran out of
  time (the duplicates of the pairs found by then are reported; a scope not
  searched yet is skipped).

### mutate

Mutation testing with StrykerJS (`mutate/`), installed by `prepare --tools`.
Without `--tools`, or without an install of the pinned manifest there,
`mutate` exits 1 and says on stderr to run `prepare --tools <folder>`.

1. **Candidate lines** (rule 1): lines `--changed` adds that a
   `--coverage` block with `count > 0` spans, in the source files `analyze`
   lists, not test files, not generated, not over 1 MiB.
2. **Mutants**: StrykerJS's own instrumenter (`@stryker-mutator/instrumenter`
   from the tools folder, in this process: nothing of the project runs)
   lists the mutants of each candidate file; a mutant is kept when its first
   character is on a candidate line (`mutate/points.js`). Left out (rule 2):
   mutants inside test callbacks (in-source tests), inside a logging or
   assertion call (`console`, `log`, `logger`, `debug`, `assert`, `expect`,
   `invariant`, also through `this.`), and in `const` declarations and enums
   outside any function. Mutators left out of every run, to keep runs short
   and close to the schema's families: `StringLiteral`, `Regex`,
   `ObjectLiteral`, `ArrayDeclaration` (data, not logic) and
   `BlockStatement` (it empties whole bodies). Static mutants (code that
   runs only when a module loads) are skipped by StrykerJS (`ignoreStatic`)
   and said in a diagnostic.
3. **Positions**: StrykerJS's lines are 1-based and columns 0-based in
   UTF-16 units; the report's `column` / `endColumn` are 1-based code points
   (`endColumn` just past the last character), `original` is the source text
   of that span, `mutated` StrykerJS's replacement. `function` is the id
   `analyze` writes for the innermost function holding the mutant (a mutant
   that replaces a whole arrow function belongs to the function around it),
   null at the top level. `description` is
   `<Mutator>: replace <original> with <mutated> in <function>` (first line,
   60 characters at most).
4. **Operators** (`mutate/operators.js`): `EqualityOperator` is
   `relational_boundary` or `relational_negation` by the operators before
   and after; `ArithmeticOperator` `arithmetic`; `LogicalOperator`
   `logical`; `UpdateOperator` `increment`; `AssignmentOperator` `increment`
   (`+=` `-=`), `arithmetic` (`*=` `/=` `%=`) or `logical` (`&&=` `||=`
   `??=`); `BooleanLiteral` `negation_removal` for `!x` into `x`;
   `ConditionalExpression` `negation_removal` for the condition of an
   `if`, a loop or `?:` forced to `true` / `false` (StrykerJS forces a
   condition rather than negating it), `return_value` for a returned
   condition into `false`; `ArrowFunction` `return_value`; `CallExpression`
   (StrykerJS's statement removal) `call_removal` for a call statement into
   `;`; everything else (`OptionalChaining`, `MethodExpression`,
   `UnaryOperator`, a `throw` removed, `true`/`false` swaps) `other`.
5. **Ids**: a SHA-256 (20 hex digits) of the file, function id, mutator,
   original, mutated and the position inside the function (lines from the
   function's first line, and the column); a repeat gets `-2`, `-3`.
6. **Cap** (rule 4, `mutate/plan.js`): above `--max-mutants`, round-robin
   across functions (by file then first line; the mutants outside any
   function one group per file, placed by their first line), each group in
   source order. Deterministic.
7. **Runs** (`mutate/stryker.js`): one StrykerJS run per unit (a package
   folder and its runner, as `test` finds them) holding chosen mutants,
   Stryker's command line in a child process of its own process group,
   working in the unit's folder, `mutate` given exactly the span of each
   chosen mutant (`file:line:column-endLine:endColumn`; a span can hold
   other mutants, which are tested and not reported). Runners: `vitest`,
   `jest` and `mocha` through their StrykerJS plugins with `perTest`
   coverage (each mutant runs only the tests that reach it; Mocha is given
   the unit's test files as `spec`); `node --test` through StrykerJS's
   command runner (`node [the script's flags] --test <the unit's test
   files>`: every test for every mutant, no coverage, so `killedBy` is null
   and static mutants are tested too). Settings: `inPlace` (the tree is a
   throwaway copy: no sandbox copy; Stryker's backups and temporary files in
   `.difflens-stryker/` there), `disableTypeChecks` everywhere,
   `concurrency` `--jobs`, no checker, no dashboard, html or log file, no
   reporter but `mutate/reporter.mjs`, which streams the baseline's tests,
   the plan and each result to a file as they come (so a run stopped at the
   deadline keeps what it tested).
8. **Finding the project's runner**: the plugins live in the tools folder
   and import `vitest/node`, `mocha`, `jest-environment-node` by name, which
   Node would look for there. `mutate/preload.mjs`, loaded into every
   process of the run with `NODE_OPTIONS=--import=...`, resolves a bare
   name that fails from inside the tools folder again from the unit's
   `package.json` (the project's `node_modules`), then from the analyzer's
   own folder (its `typescript`). Checked with Vitest 4.1, Jest 30, Mocha 11
   and `node --test` on copies of the fixtures.
9. **Timeouts**: StrykerJS stops a mutant after `timeoutFactor x net time +
   timeoutMS + overhead`, the net time being its tests' time in the
   baseline (Stryker's dry run). `timeoutFactor` is 3 and `timeoutMS`
   `--min-mutant-timeout`, so the limit is the schema's sum rather than its
   `max(--min-mutant-timeout, 3 x baseline)`: never shorter, at most
   `--min-mutant-timeout` longer. An infinite loop is often caught sooner by
   StrykerJS's hit limit (100 x the baseline's hits), also a `timeout`.
   `--timeout` stops Stryker (its whole process group) at the deadline; the
   mutants not tested are left out, with a diagnostic. When `mutate`
   itself is stopped, `preload.mjs` in Stryker's main process sees its
   parent gone within a second and kills Stryker's process group.
10. **Statuses**: `Killed` killed, `Survived` survived, `Timeout` timeout,
    `NoCoverage` no_coverage, `CompileError` and `RuntimeError` error;
    `Ignored` left out. When the baseline fails (a test fails on the
    unmutated code, a suite does not load), StrykerJS stops: every mutant
    of that unit is `error`, with a diagnostic holding StrykerJS's error
    records (finer than the schema's "each of their mutants" cannot be done:
    StrykerJS does not run with failing tests).
11. **killedBy** (`mutate/names.js`): the first test StrykerJS says killed
    it, named as `test` names it (titles joined by `/`, `/` in a title
    written `∕`): the test call at the position StrykerJS gives (Vitest,
    Jest), checked against StrykerJS's full name, else the one test of the
    file whose titles joined by spaces are that name (Mocha); null when
    neither finds it (a title held in a variable, a test made in a loop,
    `node --test`).
12. **Kept results** (rule 6): with `--cache-dir`, StrykerJS's incremental
    mode, its file `<cache-dir>/stryker-incremental.json`
    (`stryker-incremental-<hash of the folder>.json` for a unit that is not
    the root). StrykerJS reuses a result when the mutant's code and the
    tests that killed it (or, for a survivor, the set of tests) did not
    change; those are the mutants it plans as early results, reported with
    `cached: true`. StrykerJS keeps no duration for them, so
    `<cache-dir>/difflens-mutants.json` keeps each mutant's last measured
    `durationMs` by id. Without `--cache-dir`, nothing is kept.
13. **Progress**: `--progress` is replaced whole on each change: `baseline`
    until StrykerJS plans its mutants (after its dry run), `mutating` after;
    `total` the mutants to report, `done` those with a result (reused ones
    included); `current` is always null (StrykerJS says when a mutant is
    done, not when it starts).
14. **Diagnostics**: a file that cannot be read or parsed, mutants whose
    files no runner tests, a failing baseline or a StrykerJS failure (its
    ERROR records), static mutants skipped, and the time limit.

## Testing it

```sh
cd analyzers/jsts
npm ci
for d in testdata/*/; do [ -f "$d/package-lock.json" ] && (cd "$d" && npm ci --ignore-scripts); done
for d in testdata/*/; do [ -f "$d/pnpm-lock.yaml" ] && (cd "$d" && pnpm install --frozen-lockfile --ignore-scripts); done
npm test
```

The tests that need a fixture's dependencies skip when they are not
installed, and the one that runs `prepare` on a copy of
`testdata/pnpm-workspace` skips without `pnpm` on the PATH. The end-to-end
`mutate` tests (`testdata/mutation-vitest`) also need StrykerJS: they run
`prepare --tools` into `$TMPDIR/difflens-jsts-test-tools`, kept between runs
(the network is needed once), and skip when that install fails.

## How it is launched and provisioned

On first need (a JS/TS project's Flow, Quality, Tests or full analysis), the
core (`src-tauri/src/analyzer_envs.rs`) builds, in
`~/Library/Application Support/dev.difflens.app/analyzers/jsts/<fingerprint>/`:

1. `source/`: a copy of this folder, without `node_modules/` and other build
   output. The fingerprint is a SHA-256 of exactly these files (sorted
   relative paths and contents), so any change gets a fresh environment, and
   older ones are deleted once it is ready.
2. `npm ci --omit=dev --ignore-scripts --no-audit --no-fund` in `source/`
   (the one step that downloads, once per version). So: runtime dependencies
   in `dependencies`, test-only ones in `devDependencies`, and nothing that
   needs an install script.
3. A `ready` marker. A folder without it is rebuilt from scratch.

Every command then runs as `node <env>/source/src/main.js <command> …`, with
the user's `node` found on the PATH each time. Without `node` or `npm` the
user sees "The JavaScript/TypeScript analyzer needs node, …" with how to
install it. Write for Node 20 and later. To run the *project's* tests, use
the project's own toolchain from `--dir` (its `node_modules`, its test
runner), never the analyzer's, and never install anything into the user's
project.

## Running it by hand

```sh
cd analyzers/jsts
npm ci     # first, or node resolves typescript@7 from the app's node_modules above
node src/main.js analyze --dir /path/to/a/ts/tree --timeout 120s
node src/main.js test --dir /tmp/copy --timeout 10m --progress /tmp/progress.json
node src/main.js prepare --dir /tmp/copy --tools /tmp/tools
node src/main.js mutate --dir /tmp/copy --changed /tmp/changed.json --coverage /tmp/coverage.json \
    --tools /tmp/tools --cache-dir /tmp/cache --progress /tmp/progress.json
```

(`node_modules/` here is ignored by git and left out of the fingerprint.)

Through the app:

- `pnpm tauri dev` and open a JS/TS project: debug builds provision from
  this folder directly, so every edit is a new environment (`npm ci`, a few
  seconds).
- Or point the app at a ready program, which is run as it is:
  `DIFFLENS_ANALYZER_JSTS=/path/to/wrapper pnpm tauri dev`, with a wrapper
  such as `#!/bin/sh` / `exec node /abs/path/to/analyzers/jsts/src/main.js "$@"`.
  Cached results follow both the wrapper and this folder's fingerprint.
- The provisioning end to end, with this machine's toolchain:
  `cd src-tauri && cargo test analyzer_envs::tests::the_jsts_stub -- --ignored --nocapture`.
  It builds a real environment in a temporary folder and runs every command,
  `test` under the real sandbox, checking each document against the core's
  parser. Keep it passing.

## Working on this analyzer alongside the others

Work only inside `analyzers/jsts/` (`src/`, its tests, `package.json`,
`package-lock.json`, this README). Leave the shared core alone, so the Python
and Rust analyzers can be written at the same time without conflicts:
`src-tauri/**` (in particular `analyzers.rs`, `analyzer_envs.rs`,
`languages.rs`, `flow.rs`, `test_runs.rs`, `quality.rs`, `sandbox.rs`,
`tauri.conf.json`), `analyzers/protocol/*.schema.json`, `analyzers/go/`, the
other `analyzers/*/` folders, `src/` at the repository root (frontend and
i18n), the root `package.json`, `pnpm-lock.yaml`, `vite.config.ts` and
`.gitignore`. Tests of the analyzer belong in this folder (`node --test`),
never under the root `src/`, which vitest owns. If the analyzer needs
something from the core (a file shipped outside `src/`, a schema change,
another environment step), say so instead of changing it.
