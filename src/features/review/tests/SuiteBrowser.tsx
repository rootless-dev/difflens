import { ChevronRight } from "lucide-react";
import { useId, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { OutcomeMark, RowRun, SelectBox, Spin, StateTag, useTestsContext } from "@/features/review/tests/parts";
import {
  packageCheck,
  packageLeaves,
  subtestKey,
  testCheck,
  testLeaves,
} from "@/features/review/tests/selection";
import {
  selectionRuns,
  selectionTouches,
  type Suite,
  type SuitePackage,
  type SuiteSubtest,
  type SuiteTest,
} from "@/features/review/tests/suite";
import { formatDuration, formatNumber } from "@/lib/format";
import { cn } from "@/lib/utils";

/** Rows rendered per package, and packages, before a "Show the other …" button. */
export const TEST_PAGE = 50;
export const PACKAGE_PAGE = 60;
/** A search matching at most this many packages opens them. */
const SEARCH_OPENS = 12;

type Filter = "failing" | "passing" | "skipped" | "branch";
const FILTERS: Filter[] = ["failing", "passing", "skipped", "branch"];

const ROW =
  "group/row grid grid-cols-[16px_18px_16px_minmax(0,1fr)_auto_150px_64px_84px] items-center gap-2 min-h-8 rounded-[6px] px-2 py-[3px] hover:bg-card focus-within:bg-card has-[>input:checked]:bg-primary-soft max-[900px]:grid-cols-[16px_18px_16px_minmax(0,1fr)_auto_64px]";

/** Every test of the run: one line until opened, then packages, their tests and those tests' subtests. */
export function SuiteBrowser({ suite }: { suite: Suite }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [filters, setFilters] = useState<ReadonlySet<Filter>>(new Set());
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [packageLimit, setPackageLimit] = useState(PACKAGE_PAGE);
  const bodyId = useId();

  const visible = useMemo(() => visiblePackages(suite, query.trim().toLowerCase(), filters), [suite, query, filters]);
  const opensBySearch = query.trim() !== "" && visible.length <= SEARCH_OPENS;
  const failures = suite.failures.length;
  const counts: Record<Filter, number> = {
    failing: failures,
    passing: suite.passed,
    skipped: suite.skipped,
    branch: suite.added + suite.modified,
  };
  const toggleFilter = (filter: Filter) =>
    setFilters((current) => {
      const next = new Set(current);
      if (next.has(filter)) next.delete(filter);
      else next.add(filter);
      return next;
    });
  const togglePackage = (name: string) =>
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });

  return (
    <section aria-label={t("tests.browser.title")} className="col-span-full mt-[30px] border-t">
      <button
        type="button"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => setOpen((value) => !value)}
        className="flex w-full cursor-pointer items-center gap-2.5 rounded-[6px] px-2 py-3 text-left outline-none hover:bg-card focus-visible:outline-2 focus-visible:outline-ring"
      >
        <ChevronRight aria-hidden="true" className={cn("size-3.5 text-faint transition-transform", open && "rotate-90")} />
        <strong className="font-medium">{t("tests.browser.title")}</strong>
        <span className="text-muted-foreground">
          {t("tests.browser.summary", {
            count: suite.total,
            packages: t("tests.browser.packages", { count: suite.packages.length }),
          })}
          ,{" "}
          {failures > 0 ? (
            <span className="text-del">{t("tests.browser.failures", { count: failures })}</span>
          ) : (
            <span className="text-add">{t("tests.browser.noneFailing")}</span>
          )}
        </span>
      </button>
      <div id={bodyId} hidden={!open} className="pt-1">
        {open && (
          <>
            <div className="mb-2.5 flex flex-wrap items-center gap-2.5 px-2">
              <input
                type="search"
                value={query}
                onChange={(event) => setQuery(event.currentTarget.value)}
                placeholder={t("tests.browser.search")}
                aria-label={t("tests.browser.search")}
                className="w-[300px] rounded-[6px] border bg-background px-2.5 py-[5px] text-sm outline-none placeholder:text-faint focus-visible:outline-2 focus-visible:outline-ring"
              />
              <div role="group" aria-label={t("tests.browser.filters")} className="flex gap-1.5">
                {FILTERS.map((filter) => (
                  <button
                    key={filter}
                    type="button"
                    aria-pressed={filters.has(filter)}
                    onClick={() => toggleFilter(filter)}
                    className="cursor-pointer rounded-xl border px-2.5 py-[3px] text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring aria-pressed:border-primary aria-pressed:bg-primary-soft aria-pressed:text-foreground"
                  >
                    {t(`tests.browser.${filter}`)}
                    <b className="ml-1 font-mono font-medium">{formatNumber(counts[filter])}</b>
                  </button>
                ))}
              </div>
              <p className="ml-auto text-xs text-faint">{t("tests.browser.hint")}</p>
            </div>
            {visible.length === 0 ? (
              <p className="px-2 text-muted-foreground">{t("tests.browser.nothingFound")}</p>
            ) : (
              <ul aria-label={t("tests.browser.title")} className="[&>li+li]:border-t">
                {visible.slice(0, packageLimit).map(({ pkg, tests }) => (
                  <PackageRow
                    key={pkg.name}
                    pkg={pkg}
                    tests={tests}
                    open={expanded.has(pkg.name) || opensBySearch}
                    onToggle={() => togglePackage(pkg.name)}
                  />
                ))}
                {visible.length > packageLimit && (
                  <li>
                    <MoreButton onClick={() => setPackageLimit(Infinity)}>
                      {t("tests.browser.morePackages", { count: visible.length - packageLimit })}
                    </MoreButton>
                  </li>
                )}
              </ul>
            )}
          </>
        )}
      </div>
    </section>
  );
}

interface VisiblePackage {
  pkg: SuitePackage;
  /** Its tests that pass the search and the filters. */
  tests: SuiteTest[];
}

/** The packages and tests the search and the filters keep; all of them when neither is set. */
function visiblePackages(suite: Suite, query: string, filters: ReadonlySet<Filter>): VisiblePackage[] {
  if (query === "" && filters.size === 0) return suite.packages.map((pkg) => ({ pkg, tests: pkg.tests }));
  const outcomes = (["failing", "passing", "skipped"] as const).filter((filter) => filters.has(filter));
  const outcomeOf = { failing: "fail", passing: "pass", skipped: "skip" } as const;
  const keeps = (test: SuiteTest, pkgMatches: boolean) =>
    (outcomes.length === 0 || outcomes.some((filter) => test.outcome === outcomeOf[filter])) &&
    (!filters.has("branch") || test.state !== "untouched") &&
    (query === "" ||
      pkgMatches ||
      test.name.toLowerCase().includes(query) ||
      test.subtests.some((sub) => sub.name.toLowerCase().includes(query) || sub.title.toLowerCase().includes(query)));
  const result: VisiblePackage[] = [];
  for (const pkg of suite.packages) {
    const pkgMatches = query !== "" && pkg.name.toLowerCase().includes(query);
    if (pkg.tests.length === 0) {
      const failing = pkg.didNotBuild || pkg.failedOutside;
      const passesFilters =
        !filters.has("branch") && (outcomes.length === 0 || (failing && outcomes.includes("failing")));
      if (passesFilters && (query === "" || pkgMatches)) result.push({ pkg, tests: [] });
      continue;
    }
    const tests = pkg.tests.filter((test) => keeps(test, pkgMatches));
    if (tests.length > 0) result.push({ pkg, tests });
  }
  return result;
}

function MoreButton({ onClick, children }: { onClick: () => void; children: string }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="block w-full cursor-pointer rounded-[6px] py-[7px] pr-2 pl-[74px] text-left text-xs text-muted-foreground outline-none hover:bg-card hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
    >
      {children}
    </button>
  );
}

function Toggle({ open, controls, label, onClick }: { open: boolean; controls: string; label: string; onClick: () => void }) {
  return (
    <button
      type="button"
      aria-expanded={open}
      aria-controls={controls}
      aria-label={label}
      onClick={onClick}
      className="grid size-[18px] cursor-pointer place-items-center rounded-[4px] outline-none focus-visible:outline-2 focus-visible:outline-ring"
    >
      <ChevronRight aria-hidden="true" className={cn("size-3.5 text-faint transition-transform", open && "rotate-90")} />
    </button>
  );
}

function PackageCount({ pkg }: { pkg: SuitePackage }) {
  const { t } = useTranslation();
  if (pkg.didNotBuild) return <span className="text-del">{t("tests.browser.didNotBuild")}</span>;
  if (pkg.failedOutside) return <span className="text-del">{t("tests.browser.failedOutside")}</span>;
  const passing = t("tests.browser.passingCount", { number: formatNumber(pkg.passed) });
  if (pkg.failed === 0) return <>{passing}</>;
  return (
    <>
      <span className="text-del">{t("tests.browser.failingCount", { number: formatNumber(pkg.failed) })}</span>, {passing}
    </>
  );
}

function PackageRow({
  pkg,
  tests,
  open,
  onToggle,
}: {
  pkg: SuitePackage;
  tests: SuiteTest[];
  open: boolean;
  onToggle: () => void;
}) {
  const { t } = useTranslation();
  const { index, running } = useTestsContext();
  const [limit, setLimit] = useState(TEST_PAGE);
  const listId = useId();
  const failing = pkg.didNotBuild || pkg.failedOutside || pkg.failed > 0;
  const runningHere = running?.selection != null && selectionTouches(running.selection, pkg.name);
  const rest = tests.slice(limit);
  return (
    <li>
      <div className={ROW}>
        <SelectBox
          state={packageCheck(pkg, index)}
          leaves={packageLeaves(pkg)}
          label={t("tests.failures.selectPackage", { name: pkg.name })}
        />
        {pkg.tests.length > 0 || pkg.didNotBuild ? (
          <Toggle
            open={open}
            controls={listId}
            label={open ? t("tests.browser.close", { name: pkg.name }) : t("tests.browser.open", { name: pkg.name })}
            onClick={onToggle}
          />
        ) : (
          <span />
        )}
        {runningHere ? (
          <Spin />
        ) : (
          <OutcomeMark
            outcome={failing ? "fail" : pkg.tests.length > 0 && pkg.passed === 0 ? "skip" : "pass"}
            label={pkg.didNotBuild ? t("tests.browser.didNotBuild") : undefined}
          />
        )}
        <span className="min-w-0 truncate">
          <code className="text-[12.5px] font-medium text-foreground">{pkg.name}</code>
        </span>
        <span className="flex gap-1.5">
          {pkg.added > 0 && <StateTag state="new">{t("tests.browser.newCount", { count: pkg.added })}</StateTag>}
          {pkg.modified > 0 && (
            <StateTag state="modified">{t("tests.browser.modifiedCount", { count: pkg.modified })}</StateTag>
          )}
        </span>
        <span className="text-right text-xs whitespace-nowrap text-faint max-[900px]:hidden">
          <PackageCount pkg={pkg} />
        </span>
        <span className="text-right font-mono text-xs text-faint">{pkg.didNotBuild ? "—" : formatDuration(pkg.durationMs)}</span>
        <RowRun selection={{ packages: [{ package: pkg.name, tests: null }] }} className="max-[900px]:hidden" />
      </div>
      {open && (
        <ul id={listId} className="ml-[22px] border-l pl-1">
          {pkg.tests.length === 0 && (
            <li>
              <p className="py-[7px] pr-2 pl-[50px] text-xs text-muted-foreground">{t("tests.browser.noneBuilt")}</p>
            </li>
          )}
          {tests.slice(0, limit).map((test) => (
            <TestRow key={test.name} test={test} />
          ))}
          {rest.length > 0 && (
            <li>
              <MoreButton onClick={() => setLimit((value) => value + TEST_PAGE * 4)}>
                {rest.every((test) => test.outcome === "pass")
                  ? t("tests.browser.morePassing", { count: rest.length })
                  : t("tests.browser.more", { count: rest.length })}
              </MoreButton>
            </li>
          )}
        </ul>
      )}
    </li>
  );
}

function TestRow({ test }: { test: SuiteTest }) {
  const { t } = useTranslation();
  const { index, running } = useTestsContext();
  const [open, setOpen] = useState(test.outcome === "fail" && test.subtests.length > 0);
  const [limit, setLimit] = useState(TEST_PAGE);
  const listId = useId();
  const runs = (sub: SuiteSubtest | null) => running?.selection != null && selectionRuns(running.selection, test, sub);
  const count =
    test.failedSubtests > 0 ? (
      <span className="text-del">
        {t("tests.browser.ofFailing", { failed: test.failedSubtests, total: test.subtests.length })}
      </span>
    ) : test.outcome === "skip" ? (
      t("tests.browser.skippedTest")
    ) : test.subtests.length > 0 ? (
      t("tests.browser.subtests", { count: test.subtests.length })
    ) : null;
  return (
    <li>
      <div className={ROW}>
        <SelectBox state={testCheck(test, index)} leaves={testLeaves(test)} label={t("tests.failures.select", { name: test.name })} />
        {test.subtests.length > 0 ? (
          <Toggle
            open={open}
            controls={listId}
            label={open ? t("tests.browser.closeSubtests") : t("tests.browser.openSubtests")}
            onClick={() => setOpen((value) => !value)}
          />
        ) : (
          <span />
        )}
        {runs(null) ? <Spin /> : <OutcomeMark outcome={test.outcome} />}
        <span className={cn("min-w-0 truncate", test.outcome !== "fail" && "text-muted-foreground")}>{test.name}</span>
        <span className="flex gap-1.5">
          <StateTag state={test.state} />
        </span>
        <span className="text-right text-xs whitespace-nowrap text-faint max-[900px]:hidden">{count}</span>
        <span className="text-right font-mono text-xs text-faint">{formatDuration(test.durationMs)}</span>
        <RowRun
          selection={{ packages: [{ package: test.package, tests: [{ name: test.name, subtests: null }] }] }}
          className="max-[900px]:hidden"
        />
      </div>
      {open && (
        <ul id={listId} className="ml-[22px] border-l pl-1">
          {test.subtests.slice(0, limit).map((sub) => {
            const key = subtestKey(test.package, test.name, sub.name);
            return (
              <li key={sub.name}>
                <div className={ROW}>
                  <SelectBox
                    state={index.leaves.has(key) ? "all" : "none"}
                    leaves={[key]}
                    label={t("tests.failures.selectSubtest", { name: sub.title })}
                  />
                  <span />
                  {runs(sub) ? <Spin /> : <OutcomeMark outcome={sub.case.outcome} />}
                  <span className={cn("min-w-0 truncate", sub.case.outcome === "fail" ? "text-foreground" : "text-muted-foreground")}>
                    {sub.title}
                  </span>
                  <span />
                  <span className="max-[900px]:hidden" />
                  <span className="text-right font-mono text-xs text-faint">{formatDuration(sub.case.durationMs)}</span>
                  <RowRun
                    selection={{ packages: [{ package: test.package, tests: [{ name: test.name, subtests: [sub.name] }] }] }}
                    className="max-[900px]:hidden"
                  />
                </div>
              </li>
            );
          })}
          {test.subtests.length > limit && (
            <li>
              <MoreButton onClick={() => setLimit(Infinity)}>
                {t("tests.browser.moreSubtests", { count: test.subtests.length - limit })}
              </MoreButton>
            </li>
          )}
        </ul>
      )}
    </li>
  );
}
