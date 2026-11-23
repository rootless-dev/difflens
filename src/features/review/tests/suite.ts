// The tests of a run as the Tests tab shows them: packages, their top-level tests and those tests' subtests, the
// failures (one per failing top-level test, one per package that failed outside its tests), and the counts of the
// verdict. Subtests count inside their test: a test with failing subtests is one failure.

import type { PackageOutcome, RunSelection, TestCase, TestOutcome, TestRun } from "@/lib/api";

export interface SuiteSubtest {
  /** Relative to its test, its levels below it joined by "/": "gateway_declines", or "a/b" when nested. */
  name: string;
  /** "gateway declines" in a Go run, where "_" reads as a space; the relative name as it is in any other. */
  title: string;
  case: TestCase;
}

export interface SuiteTest {
  package: string;
  /** Top-level name. */
  name: string;
  /** The test itself, when the run reported it (it does, unless only its subtests were kept). */
  case: TestCase | null;
  /** Failed first, then by name. */
  subtests: SuiteSubtest[];
  outcome: TestOutcome;
  durationMs: number;
  state: TestCase["state"];
  file: string | null;
  line: number | null;
  failedSubtests: number;
}

export interface SuitePackage {
  name: string;
  outcome: PackageOutcome | null;
  /** Failed first, then by name. */
  tests: SuiteTest[];
  failed: number;
  passed: number;
  skipped: number;
  added: number;
  modified: number;
  didNotBuild: boolean;
  /** It failed in a way no single test explains (a panic in setup, a timeout outside tests). */
  failedOutside: boolean;
  durationMs: number;
}

export type Failure = { kind: "test"; test: SuiteTest } | { kind: "package"; pkg: SuitePackage };

export interface Suite {
  /** Failing first, then by name. */
  packages: SuitePackage[];
  failures: Failure[];
  /** Top-level tests. */
  total: number;
  passed: number;
  failed: number;
  skipped: number;
  /** Top-level tests whose function this branch added, and changed. */
  added: number;
  modified: number;
  didNotBuild: number;
  failedOutside: number;
}

const byFailureThenName = <T>(failed: (item: T) => boolean, name: (item: T) => string) => (a: T, b: T) =>
  Number(failed(b)) - Number(failed(a)) || name(a).localeCompare(name(b));

/** The top-level test a case belongs to: its first level, whatever its name holds. */
const topOf = (test: TestCase) => test.hierarchy[0] ?? test.name;

/** A subtest's name relative to its top-level test: its levels below it, joined as the framework joins them. */
const relativeOf = (test: TestCase) => test.hierarchy.slice(1).join("/");

/** The level of a subtest right under its top-level test. */
const firstLevel = (sub: SuiteSubtest) => sub.case.hierarchy[1] ?? sub.name;

/** Go prints a subtest's spaces as "_": "gateway_declines" reads "gateway declines". */
function subtestTitle(relative: string, language: string): string {
  return language === "go" ? relative.replaceAll("_", " ") : relative;
}

function testOf(pkg: string, name: string, own: TestCase | null, subs: TestCase[], language: string): SuiteTest {
  const subtests = subs
    .map((sub) => {
      const relative = relativeOf(sub);
      return { name: relative, title: subtestTitle(relative, language), case: sub };
    })
    .sort(byFailureThenName((sub) => sub.case.outcome === "fail", (sub) => sub.name));
  const failedSubtests = subs.filter((sub) => sub.outcome === "fail").length;
  const outcome: TestOutcome =
    own?.outcome ??
    (failedSubtests > 0 ? "fail" : subs.length > 0 && subs.every((sub) => sub.outcome === "skip") ? "skip" : "pass");
  const first = own ?? subs[0];
  return {
    package: pkg,
    name,
    case: own,
    subtests,
    outcome,
    durationMs: own?.durationMs ?? subs.reduce((sum, sub) => sum + sub.durationMs, 0),
    state: first?.state ?? "untouched",
    file: first?.file ?? null,
    line: first?.line ?? null,
    failedSubtests,
  };
}

const suites = new WeakMap<TestRun, Suite>();

/** The suite of a completed run, worked out once per run object. */
export function buildSuite(run: TestRun): Suite {
  let suite = suites.get(run);
  if (!suite) {
    suite = computeSuite(run);
    suites.set(run, suite);
  }
  return suite;
}

function computeSuite(run: TestRun): Suite {
  const grouped = new Map<string, Map<string, { own: TestCase | null; subs: TestCase[] }>>();
  for (const test of run.tests) {
    const pkg = test.package || test.container;
    let tests = grouped.get(pkg);
    if (!tests) {
      tests = new Map();
      grouped.set(pkg, tests);
    }
    const top = topOf(test);
    let entry = tests.get(top);
    if (!entry) {
      entry = { own: null, subs: [] };
      tests.set(top, entry);
    }
    if (test.hierarchy.length <= 1) entry.own = test;
    else entry.subs.push(test);
  }
  const outcomes = new Map(run.packages.map((pkg) => [pkg.container, pkg]));
  const names = new Set([...grouped.keys(), ...run.packages.filter((pkg) => pkg.outcome !== "no_tests").map((pkg) => pkg.container)]);
  const packages: SuitePackage[] = [...names].map((name) => {
    const tests = [...(grouped.get(name) ?? new Map()).entries()]
      .map(([top, entry]) => testOf(name, top, entry.own, entry.subs, run.language))
      .sort(byFailureThenName((test) => test.outcome === "fail", (test) => test.name));
    const outcome = outcomes.get(name) ?? null;
    const count = (outcome: TestOutcome) => tests.filter((test) => test.outcome === outcome).length;
    return {
      name,
      outcome,
      tests,
      failed: count("fail"),
      passed: count("pass"),
      skipped: count("skip"),
      added: tests.filter((test) => test.state === "new").length,
      modified: tests.filter((test) => test.state === "modified").length,
      didNotBuild: outcome?.outcome === "build_failed",
      failedOutside: outcome?.outcome === "fail" && outcome.output !== null,
      durationMs: outcome?.durationMs ?? tests.reduce((sum, test) => sum + test.durationMs, 0),
    };
  });
  const failing = (pkg: SuitePackage) => pkg.didNotBuild || pkg.failedOutside || pkg.failed > 0;
  packages.sort(byFailureThenName(failing, (pkg) => pkg.name));

  const failures: Failure[] = [];
  for (const pkg of packages) {
    if (pkg.didNotBuild || pkg.failedOutside) failures.push({ kind: "package", pkg });
    for (const test of pkg.tests) if (test.outcome === "fail") failures.push({ kind: "test", test });
  }
  const sum = (pick: (pkg: SuitePackage) => number) => packages.reduce((total, pkg) => total + pick(pkg), 0);
  return {
    packages,
    failures,
    total: sum((pkg) => pkg.tests.length),
    passed: sum((pkg) => pkg.passed),
    failed: sum((pkg) => pkg.failed),
    skipped: sum((pkg) => pkg.skipped),
    added: sum((pkg) => pkg.added),
    modified: sum((pkg) => pkg.modified),
    didNotBuild: packages.filter((pkg) => pkg.didNotBuild).length,
    failedOutside: packages.filter((pkg) => pkg.failedOutside).length,
  };
}

/** Failing tests and packages, as the tab counts them. */
export function failureCount(run: TestRun): number {
  return buildSuite(run).failures.length;
}

// ---------- what a selection runs ----------

/** The level right under `test` of its subtest named `relative`, which the runner runs as a whole. */
function firstLevelOf(test: SuiteTest, relative: string): string {
  const sub = test.subtests.find((candidate) => candidate.name === relative);
  return sub ? firstLevel(sub) : relative;
}

/**
 * Whether `selection` runs `test`, or its subtest `sub`: a subtest runs when its test does, or a subtest selected
 * under the same first level (the runner runs a first level as a whole).
 */
export function selectionRuns(selection: RunSelection, test: SuiteTest, sub: SuiteSubtest | null = null): boolean {
  const selected = selection.packages.find((entry) => entry.package === test.package);
  if (!selected) return false;
  if (selected.tests === null) return true;
  const entry = selected.tests.find((candidate) => candidate.name === test.name);
  if (!entry) return false;
  if (entry.subtests === null || sub === null) return true;
  const first = firstLevel(sub);
  return entry.subtests.some((relative) => firstLevelOf(test, relative) === first);
}

/** Whether `selection` runs the package as a whole or some of it. */
export function selectionTouches(selection: RunSelection, pkg: string): boolean {
  return selection.packages.some((entry) => entry.package === pkg);
}

/** "Run only the failed ones": the failed top-level tests, and the packages that did not build or failed outside. */
export function failedSelection(suite: Suite): RunSelection {
  const packages: RunSelection["packages"] = [];
  for (const pkg of suite.packages) {
    if (pkg.didNotBuild || pkg.failedOutside) {
      packages.push({ package: pkg.name, tests: null });
    } else if (pkg.failed > 0) {
      packages.push({
        package: pkg.name,
        tests: pkg.tests.filter((test) => test.outcome === "fail").map((test) => ({ name: test.name, subtests: null })),
      });
    }
  }
  return { packages };
}

const quoteRegexp = (name: string) => name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const alternation = (names: readonly string[]) =>
  names.length === 1 ? `^${quoteRegexp(names[0])}$` : `^(${names.map(quoteRegexp).join("|")})$`;

/** A Go subtest's first level: Go joins levels with "/" and its names cannot hold one otherwise. */
function goFirstLevel(relative: string): string {
  const slash = relative.indexOf("/");
  return slash === -1 ? relative : relative.slice(0, slash);
}

/**
 * The `go test` lines a selection takes, as the runner runs them: one for the packages run whole, then per package
 * one for its whole tests and one per test with only some subtests (Go cannot give each test its own subtest
 * pattern in one -run).
 */
function goTestCommands(selection: RunSelection, race: boolean): string[] {
  const flag = race ? "-race " : "";
  const path = (pkg: string) => `./${pkg}`;
  const whole = selection.packages.filter((entry) => entry.tests === null).map((entry) => path(entry.package));
  const lines = whole.length > 0 ? [`go test ${flag}${whole.join(" ")}`] : [];
  for (const entry of selection.packages) {
    if (entry.tests === null) continue;
    const full = entry.tests.filter((test) => test.subtests === null).map((test) => test.name);
    if (full.length > 0) lines.push(`go test ${flag}-run '${alternation(full)}' ${path(entry.package)}`);
    for (const test of entry.tests) {
      if (test.subtests === null || full.includes(test.name)) continue;
      const firsts = [...new Set(test.subtests.map(goFirstLevel))];
      if (firsts.length === 0) continue;
      lines.push(`go test ${flag}-run '${alternation([test.name])}/${alternation(firsts)}' ${path(entry.package)}`);
    }
  }
  return lines;
}

/** One shell word: in single quotes, a quote inside it written as '\''. */
function shellQuote(text: string): string {
  return `'${text.replaceAll("'", "'\\''")}'`;
}

const CARGO_TARGETS: Record<string, string> = { bin: "--bin", tests: "--test", benches: "--bench", examples: "--example" };

/**
 * What a Rust container says of where its tests are: a crate ("shapes") is `-p shapes`; a target of a package
 * ("shapes/tests/area") is `-p shapes --test area`. Nothing for any other container.
 */
function cargoScope(container: string): string {
  // Crate and target names are plain words; anything else is quoted all the same.
  const word = (name: string) => (/^[\w.-]+$/.test(name) ? name : shellQuote(name));
  const parts = container.split("/");
  if (parts.length === 1 && parts[0] !== "") return ` -p ${word(parts[0])}`;
  const flag = parts.length === 3 ? CARGO_TARGETS[parts[1]] : undefined;
  return flag && parts[0] !== "" && parts[2] !== "" ? ` -p ${word(parts[0])} ${flag} ${word(parts[2])}` : "";
}

/** The test a fallback command names: its full name, its levels, and its file when the run reported one. */
type Named = Pick<TestCase, "name" | "hierarchy" | "file">;

/** A command for one test of `pkg` when the runner reported none; null for a language without one. */
function fallbackTestCommand(language: string, pkg: string, test: Named): string | null {
  switch (language) {
    case "rust":
      return `cargo test${cargoScope(pkg)} ${shellQuote(test.name)} -- --exact`;
    case "python":
      return `python -m pytest ${shellQuote(`${test.file ?? pkg}::${test.name}`)}`;
    case "javascript":
    case "typescript":
      return `npm test -- -t ${shellQuote(test.hierarchy.join(" "))}`;
    default:
      return null;
  }
}

/** A command for the whole package `pkg` when the runner reported none; null for a language without one. */
function fallbackPackageCommand(language: string, pkg: string, suitePkg: SuitePackage | undefined): string | null {
  switch (language) {
    case "rust":
      return `cargo test${cargoScope(pkg)}`;
    case "python":
      return `python -m pytest ${shellQuote(suitePkg?.tests.find((test) => test.file !== null)?.file ?? pkg)}`;
    case "javascript":
    case "typescript":
      return "npm test";
    default:
      return null;
  }
}

/** One thing a selection runs: the command the run reported for it, and the language's usual one otherwise. */
interface SelectedItem {
  command: string | null;
  fallback: () => string | null;
}

/**
 * Each thing `selection` runs, in selection order: a package run whole, a whole test, and for some subtests each
 * first level they are under (the runner runs a first level as a whole), or the subtest itself when the run has no
 * case for that level.
 */
function selectedItems(run: TestRun, selection: RunSelection): SelectedItem[] {
  const { language } = run;
  const packages = new Map(buildSuite(run).packages.map((pkg) => [pkg.name, pkg]));
  const items: SelectedItem[] = [];
  const test = (pkg: string, named: Named, command: string | null) =>
    items.push({ command, fallback: () => fallbackTestCommand(language, pkg, named) });
  for (const entry of selection.packages) {
    const pkg = packages.get(entry.package);
    if (entry.tests === null) {
      items.push({
        command: pkg?.outcome?.command ?? null,
        fallback: () => fallbackPackageCommand(language, entry.package, pkg),
      });
      continue;
    }
    for (const chosen of entry.tests) {
      const found = pkg?.tests.find((candidate) => candidate.name === chosen.name);
      if (chosen.subtests === null || !found) {
        // Without its own case (only its subtests were kept), a test is still at its subtests' file.
        const subject = found?.case ?? { name: chosen.name, hierarchy: [chosen.name], file: found?.file ?? null };
        test(entry.package, subject, found?.case?.command ?? null);
        continue;
      }
      for (const relative of chosen.subtests) {
        const sub = found.subtests.find((candidate) => candidate.name === relative);
        if (!sub) {
          test(entry.package, { name: `${found.name}/${relative}`, hierarchy: [found.name, relative], file: found.file }, null);
          continue;
        }
        const first = firstLevel(sub);
        const level =
          found.subtests.find((candidate) => candidate.case.hierarchy.length === 2 && candidate.case.hierarchy[1] === first) ??
          sub;
        test(entry.package, level.case, level.case.command);
      }
    }
  }
  return items;
}

const distinct = (lines: readonly (string | null)[]) =>
  lines.filter((line, index): line is string => line !== null && lines.indexOf(line) === index);

/**
 * The shell lines, from the project root, that run `selection` again: the commands the run reported for what it
 * selects (see selectedItems), de-duplicated, in selection order. In any language but Go, a command the run did not
 * report is written the language's usual way. Go takes the reported ones only when every selected item has one (they
 * know the module and the root package); otherwise, for a run kept from before runners reported them, it builds them
 * as the runner runs them, grouping a package's tests into one -run, with -race when `race`.
 */
export function testCommands(run: TestRun, selection: RunSelection, race: boolean): string[] {
  const items = selectedItems(run, selection);
  if (run.language === "go") {
    return items.every((item) => item.command !== null)
      ? distinct(items.map((item) => item.command))
      : goTestCommands(selection, race);
  }
  return distinct(items.map((item) => item.command ?? item.fallback()));
}
