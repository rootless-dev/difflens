// Picking tests to run: checkboxes on packages, tests and subtests (in the tree and on the failure cards, which share
// the same keys and so stay in sync). Only leaves are kept: a subtest, a test without subtests, a package without
// tests. A parent is checked when all the leaves under it are, and mixed when only some are.

import type { RunSelection } from "@/lib/api";
import type { Suite, SuitePackage, SuiteTest } from "@/features/review/tests/suite";

export type Check = "none" | "some" | "all";

export const packageKey = (pkg: string) => JSON.stringify([pkg]);
export const testKey = (pkg: string, test: string) => JSON.stringify([pkg, test]);
export const subtestKey = (pkg: string, test: string, subtest: string) => JSON.stringify([pkg, test, subtest]);

export function testLeaves(test: SuiteTest): string[] {
  return test.subtests.length === 0
    ? [testKey(test.package, test.name)]
    : test.subtests.map((sub) => subtestKey(test.package, test.name, sub.name));
}

export function packageLeaves(pkg: SuitePackage): string[] {
  return pkg.tests.length === 0 ? [packageKey(pkg.name)] : pkg.tests.flatMap(testLeaves);
}

/** How many leaves are selected under each package and each test. */
export interface SelectionIndex {
  leaves: ReadonlySet<string>;
  packages: Map<string, number>;
  tests: Map<string, number>;
}

export function indexSelection(selected: ReadonlySet<string>): SelectionIndex {
  const packages = new Map<string, number>();
  const tests = new Map<string, number>();
  for (const key of selected) {
    const [pkg, test] = JSON.parse(key) as [string, string?, string?];
    packages.set(pkg, (packages.get(pkg) ?? 0) + 1);
    if (test !== undefined) {
      const k = testKey(pkg, test);
      tests.set(k, (tests.get(k) ?? 0) + 1);
    }
  }
  return { leaves: selected, packages, tests };
}

const check = (on: number, total: number): Check => (on === 0 ? "none" : on >= total ? "all" : "some");

export function packageCheck(pkg: SuitePackage, index: SelectionIndex): Check {
  const total = pkg.tests.length === 0 ? 1 : pkg.tests.reduce((sum, test) => sum + Math.max(1, test.subtests.length), 0);
  return check(index.packages.get(pkg.name) ?? 0, total);
}

export function testCheck(test: SuiteTest, index: SelectionIndex): Check {
  return check(index.tests.get(testKey(test.package, test.name)) ?? 0, Math.max(1, test.subtests.length));
}

/** `selected` with `leaves` all on, or all off. */
export function toggled(selected: ReadonlySet<string>, leaves: readonly string[], on: boolean): Set<string> {
  const next = new Set(selected);
  for (const leaf of leaves) {
    if (on) next.add(leaf);
    else next.delete(leaf);
  }
  return next;
}

/**
 * Adds what failed: the failed subtests of a test (not their passing siblings; all of a test that failed with none
 * of its subtests failing), failed tests without subtests, and the packages that did not build or failed outside.
 */
export function withFailed(suite: Suite, selected: ReadonlySet<string>): Set<string> {
  const next = new Set(selected);
  for (const failure of suite.failures) {
    if (failure.kind === "package") {
      for (const leaf of packageLeaves(failure.pkg)) next.add(leaf);
      continue;
    }
    const { test } = failure;
    const failed = test.subtests.filter((sub) => sub.case.outcome === "fail");
    const leaves =
      failed.length > 0 ? failed.map((sub) => subtestKey(test.package, test.name, sub.name)) : testLeaves(test);
    for (const leaf of leaves) next.add(leaf);
  }
  return next;
}

export interface SelectionCount {
  tests: number;
  subtests: number;
  /** Packages without tests (one that did not build), selected whole. */
  packages: number;
}

function touched(suite: Suite, index: SelectionIndex): SuitePackage[] {
  return suite.packages.filter((pkg) => (index.packages.get(pkg.name) ?? 0) > 0);
}

/**
 * The count rule: a test selected whole counts once, whatever its subtests; subtests count on their own only when
 * their test is partly selected, since then they are what runs; a package counts all its tests.
 */
export function countSelection(suite: Suite, index: SelectionIndex): SelectionCount {
  const count: SelectionCount = { tests: 0, subtests: 0, packages: 0 };
  for (const pkg of touched(suite, index)) {
    if (pkg.tests.length === 0) {
      count.packages += 1;
      continue;
    }
    for (const test of pkg.tests) {
      const state = testCheck(test, index);
      if (state === "all") count.tests += 1;
      if (state === "some") count.subtests += index.tests.get(testKey(test.package, test.name)) ?? 0;
    }
  }
  return count;
}

/** What to ask the runner: whole packages where all is selected, else whole tests, else some subtests. */
export function selectionPayload(suite: Suite, selected: ReadonlySet<string>): RunSelection {
  const index = indexSelection(selected);
  const packages: RunSelection["packages"] = [];
  for (const pkg of touched(suite, index)) {
    if (packageCheck(pkg, index) === "all") {
      packages.push({ package: pkg.name, tests: null });
      continue;
    }
    const tests: { name: string; subtests: string[] | null }[] = [];
    for (const test of pkg.tests) {
      const state = testCheck(test, index);
      if (state === "all") tests.push({ name: test.name, subtests: null });
      if (state === "some") {
        tests.push({
          name: test.name,
          subtests: test.subtests
            .filter((sub) => selected.has(subtestKey(test.package, test.name, sub.name)))
            .map((sub) => sub.name),
        });
      }
    }
    packages.push({ package: pkg.name, tests });
  }
  return { packages };
}

/** Keys that no longer name anything in `suite`, dropped (after a run changed what is listed). */
export function keptIn(suite: Suite, selected: ReadonlySet<string>): Set<string> {
  if (selected.size === 0) return new Set();
  const leaves = new Set(suite.packages.flatMap(packageLeaves));
  return new Set([...selected].filter((key) => leaves.has(key)));
}
