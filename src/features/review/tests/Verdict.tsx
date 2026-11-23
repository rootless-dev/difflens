import { Check, ChevronDown, ChevronRight, X } from "lucide-react";
import { type ReactNode, useId, useState } from "react";
import { useTranslation } from "react-i18next";

import { Hint } from "@/components/page";
import { PreparingAnalyzers } from "@/components/preparing-analyzers";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { DiffTarget } from "@/features/review/flow/flow-facts";
import {
  CoverageBar,
  coverageRatio,
  coverageTitle,
  coverageTone,
  fileDiffTarget,
  TONE_TEXT,
} from "@/features/review/ReviewPage";
import { ISOLATIONS, isolationLook } from "@/features/review/tests/isolation";
import { allOf, clock, Spin } from "@/features/review/tests/parts";
import { failedSelection, type Suite } from "@/features/review/tests/suite";
import type { RunChoice, Running } from "@/features/review/tests/use-tests";
import { useNow } from "@/hooks/use-now";
import { i18n } from "@/i18n";
import type { ChangedFile, Isolation, RunSelection, TestProgress, TestRun } from "@/lib/api";
import { formatDuration, formatNumber, formatPercent, formatRelativeTime } from "@/lib/format";
import { cn } from "@/lib/utils";

/** Packages named in the line under a failing verdict, before "and N more". */
const NAMED_PACKAGES = 3;

/** "2 tests failed and 1 package did not build", or "All 8,974 tests passed". */
function verdictTitle(suite: Suite): string {
  if (suite.failures.length === 0) {
    return suite.total === 0 ? i18n.t("tests.verdict.noTests") : i18n.t("tests.verdict.allPassed", { count: suite.total });
  }
  const parts: string[] = [];
  if (suite.failed > 0) parts.push(i18n.t("tests.verdict.testsFailed", { count: suite.failed }));
  if (suite.didNotBuild > 0) parts.push(i18n.t("tests.verdict.notBuilt", { count: suite.didNotBuild }));
  if (suite.failedOutside > 0) parts.push(i18n.t("tests.verdict.failedOutside", { count: suite.failedOutside }));
  return allOf(parts);
}

function failingPackages(suite: Suite): string[] {
  const names = suite.failures.map((failure) => (failure.kind === "test" ? failure.test.package : failure.pkg.name));
  return [...new Set(names)];
}

interface VerdictProps {
  run: TestRun;
  suite: Suite;
  choice: RunChoice;
  busy: boolean;
  files: readonly ChangedFile[];
  onRun: (selection: RunSelection | null) => void;
  onChoose: (change: Partial<RunChoice>) => void;
  onViewDiff: (target: DiffTarget) => void;
}

/** The head of a completed run: did it pass, the numbers behind it, and how to run again. */
export function Verdict({ run, suite, choice, busy, files, onRun, onChoose, onViewDiff }: VerdictProps) {
  const { t } = useTranslation();
  const now = useNow(30_000);
  const [coverageOpen, setCoverageOpen] = useState(false);
  const titleId = useId();
  const coverageId = useId();
  const failed = suite.failures.length > 0;
  const packages = failingPackages(suite);
  const named = packages.slice(0, NAMED_PACKAGES);
  if (packages.length > NAMED_PACKAGES) named.push(t("tests.verdict.morePackages", { count: packages.length - NAMED_PACKAGES }));
  const changed = run.coverage ? coverageRatio(run.coverage.changedLines) : null;
  const busyTitle = busy ? t("tests.busy") : undefined;

  return (
    <section
      aria-labelledby={titleId}
      className="col-span-full grid grid-cols-[auto_1fr_auto] items-start gap-x-3.5 gap-y-1 border-b pb-4 max-[900px]:grid-cols-[auto_1fr]"
    >
      <span
        aria-hidden="true"
        className={cn(
          "mt-px grid size-[30px] place-items-center rounded-full",
          failed ? "bg-del-bg text-del" : "bg-add-bg text-add",
        )}
      >
        {failed ? <X className="size-4" /> : <Check className="size-4" />}
      </span>
      <div>
        <h1 id={titleId} className={cn("text-lg leading-[1.3] font-semibold tracking-[-0.01em]", failed && "text-del")}>
          {verdictTitle(suite)}
        </h1>
        <p className="mt-0.5 text-muted-foreground">
          {failed
            ? `${t("tests.verdict.inPackages", { packages: allOf(named) })} ${t("tests.verdict.rest")}`
            : t("tests.verdict.passedIn", {
                count: suite.packages.length,
                race: run.raceDetector ? t("tests.verdict.noRace") : t("tests.verdict.raceOff"),
              })}
        </p>
      </div>
      <div className="flex items-center gap-2 max-[900px]:col-span-full">
        {failed && (
          <Button variant="secondary" disabled={busy} title={busyTitle} onClick={() => onRun(failedSelection(suite))}>
            {t("tests.verdict.runFailed")}
          </Button>
        )}
        <span className="inline-flex">
          <Button className="rounded-r-none" disabled={busy} title={busyTitle} onClick={() => onRun(null)}>
            {t("tests.verdict.runAll")}
          </Button>
          <RunOptionsMenu choice={choice} lastUsed={run.isolation} disabled={busy} onChoose={onChoose} />
        </span>
      </div>
      <ul
        aria-label={t("tests.facts.label")}
        className="col-span-2 col-start-2 mt-2.5 flex flex-wrap gap-x-[18px] gap-y-1 text-xs text-muted-foreground max-[900px]:col-span-full"
      >
        <Fact value={formatNumber(suite.passed)} label={t("tests.facts.passing")} />
        {failed && <Fact value={formatNumber(suite.failed)} label={t("tests.facts.failing")} bad />}
        {suite.didNotBuild > 0 && (
          <Fact value={formatNumber(suite.didNotBuild)} label={t("tests.facts.notBuilt", { count: suite.didNotBuild })} bad />
        )}
        {suite.failedOutside > 0 && (
          <Fact
            value={formatNumber(suite.failedOutside)}
            label={t("tests.facts.failedOutside", { count: suite.failedOutside })}
            bad
          />
        )}
        {suite.skipped > 0 && <Fact value={formatNumber(suite.skipped)} label={t("tests.facts.skipped")} />}
        <Fact value={formatNumber(suite.added)} label={t("tests.facts.added")} />
        <li>
          {changed === null ? (
            <>
              <b className="font-mono font-medium text-faint">—</b>{" "}
              {run.coverage ? t("tests.facts.nothingToCover") : t("tests.facts.notMeasured")}
            </>
          ) : (
            <button
              type="button"
              aria-expanded={coverageOpen}
              aria-controls={coverageId}
              onClick={() => setCoverageOpen((open) => !open)}
              className="inline-flex cursor-pointer items-center gap-1 rounded-sm text-muted-foreground outline-none hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
            >
              <b className={cn("font-mono font-medium text-foreground", TONE_TEXT[coverageTone(changed)])}>
                {formatPercent(changed)}
              </b>{" "}
              {t("tests.facts.covered")}
              <ChevronRight aria-hidden="true" className={cn("size-3 transition-transform", coverageOpen && "rotate-90")} />
            </button>
          )}
        </li>
        <Fact value={formatDuration(run.durationMs)} label={t("tests.facts.duration")} />
        <li>
          {run.newSha ? (
            <>
              {t("tests.facts.atCommit", { when: formatRelativeTime(run.startedAt, now) })} <code>{run.newSha.slice(0, 7)}</code>
            </>
          ) : (
            t("tests.facts.workingTree", { when: formatRelativeTime(run.startedAt, now) })
          )}
        </li>
        <li className="text-faint">
          {isolationLook(run.isolation).ranIn},{" "}
          {run.raceDetector ? t("tests.facts.withRace") : t("tests.facts.withoutRace")}
        </li>
      </ul>
      {run.partial && <PartialNote run={run} now={now} />}
      {run.coverage && changed !== null && (
        <div
          id={coverageId}
          hidden={!coverageOpen}
          className="col-span-2 col-start-2 mt-3 max-w-[560px] rounded-[8px] border bg-card px-3.5 py-3 max-[900px]:col-span-full"
        >
          {coverageOpen && <CoverageByFile coverage={run.coverage} files={files} onViewDiff={onViewDiff} />}
        </div>
      )}
    </section>
  );
}

function Fact({ value, label, bad = false }: { value: string; label: string; bad?: boolean }) {
  return (
    <li>
      <b className={cn("font-mono font-medium text-foreground", bad && "text-del")}>{value}</b> {label}
    </li>
  );
}

function PartialNote({ run, now }: { run: TestRun; now: Date }) {
  const { t } = useTranslation();
  const partial = run.partial;
  if (!partial) return null;
  const values = {
    when: formatRelativeTime(partial.startedAt, now),
    tests: t("tests.partial.tests", { count: partial.tests }),
    packages: t("tests.partial.packages", { count: partial.packages }),
  };
  const text =
    partial.status === "completed"
      ? t("tests.partial.merged", values)
      : partial.status === "cancelled"
        ? t("tests.partial.cancelled", values)
        : t("tests.partial.stopped", values);
  return <p className="col-span-2 col-start-2 mt-1.5 text-xs text-muted-foreground max-[900px]:col-span-full">{text}</p>;
}

function CoverageByFile({
  coverage,
  files,
  onViewDiff,
}: {
  coverage: NonNullable<TestRun["coverage"]>;
  files: readonly ChangedFile[];
  onViewDiff: (target: DiffTarget) => void;
}) {
  const { t } = useTranslation();
  if (coverage.files.length === 0) return <p className="text-muted-foreground">{t("tests.coverage.nothing")}</p>;
  return (
    <>
      <ul aria-label={t("tests.coverage.byFile")} className="grid grid-cols-[auto_1fr_40px] items-center gap-x-3 gap-y-2">
        {coverage.files.map((file) => {
          const ratio = coverageRatio(file) ?? 0;
          const target = fileDiffTarget(file.path, 1, files);
          return (
            <li key={file.path} title={coverageTitle(file)} className="col-span-3 grid grid-cols-subgrid items-center">
              {target ? (
                <button
                  type="button"
                  onClick={() => onViewDiff(target)}
                  className="max-w-[320px] cursor-pointer truncate rounded-sm text-left font-mono text-xs text-primary outline-none hover:underline focus-visible:outline-2 focus-visible:outline-ring"
                >
                  {file.path}
                </button>
              ) : (
                <code className="max-w-[320px] truncate">{file.path}</code>
              )}
              <CoverageBar ratio={ratio} />
              <span className="text-right font-mono text-xs text-muted-foreground">{formatPercent(ratio)}</span>
            </li>
          );
        })}
      </ul>
      <Hint className="mt-2">{t("tests.coverage.hint")}</Hint>
    </>
  );
}

/** The caret of "Run all": where the next run goes and what it measures. */
export function RunOptionsMenu({
  choice,
  lastUsed,
  disabled,
  onChoose,
}: {
  choice: RunChoice;
  lastUsed: Isolation | null;
  disabled: boolean;
  onChoose: (change: Partial<RunChoice>) => void;
}) {
  const { t } = useTranslation();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label={t("tests.options.label")}
        title={disabled ? t("tests.busy") : undefined}
        disabled={disabled}
        className="ml-px inline-flex h-[35px] cursor-pointer items-center rounded-r-[7px] bg-primary px-2.5 text-primary-foreground outline-none not-disabled:hover:brightness-110 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-45"
      >
        <ChevronDown aria-hidden="true" className="size-3.5" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-[320px] p-1.5">
        <DropdownMenuGroup>
          <DropdownMenuLabel className="font-normal text-faint">{t("tests.options.where")}</DropdownMenuLabel>
          <DropdownMenuRadioGroup value={choice.isolation} onValueChange={(value) => onChoose({ isolation: value as Isolation })}>
            {ISOLATIONS.map(isolationLook).map((look) => (
              <DropdownMenuRadioItem key={look.value} value={look.value} closeOnClick={false} className="grid items-start gap-0.5 py-1.5">
                <span className="font-medium">
                  {look.again}
                  {look.value === lastUsed && <span className="font-normal text-faint"> · {t("tests.options.lastUsed")}</span>}
                </span>
                <span className="text-xs text-muted-foreground">{look.description}</span>
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </DropdownMenuGroup>
        <DropdownMenuSeparator />
        <DropdownMenuGroup>
          <DropdownMenuLabel className="sr-only">{t("tests.options.measures")}</DropdownMenuLabel>
          <DropdownMenuCheckboxItem
            checked={choice.race}
            closeOnClick={false}
            onCheckedChange={(race) => onChoose({ race })}
            className="grid items-start gap-0.5 py-1.5"
          >
            <span className="font-medium">{t("tests.options.race")}</span>
            <span className="text-xs text-muted-foreground">{t("tests.options.raceHint")}</span>
          </DropdownMenuCheckboxItem>
          <DropdownMenuCheckboxItem
            checked={choice.coverage}
            closeOnClick={false}
            onCheckedChange={(coverage) => onChoose({ coverage })}
            className="grid items-start gap-0.5 py-1.5"
          >
            <span className="font-medium">{t("tests.options.coverage")}</span>
            <span className="text-xs text-muted-foreground">{t("tests.options.coverageHint")}</span>
          </DropdownMenuCheckboxItem>
        </DropdownMenuGroup>
        <DropdownMenuSeparator />
        <p className="px-2 py-1 text-xs text-faint">{t("tests.options.applies")}</p>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** A run of the whole suite going on: how far it got, how long it takes, and a way to stop it. */
export function RunBox({
  running,
  progress,
  previous,
  onCancel,
}: {
  running: Running;
  progress: TestProgress | null;
  previous: TestRun | null;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const now = useNow(1000);
  const ratio = progress && progress.packagesTotal > 0 ? progress.packagesDone / progress.packagesTotal : null;
  return (
    <section role="status" aria-label={t("tests.running.label")} className="col-span-full border-b pb-4">
      <div className="flex items-center gap-3">
        <Spin className="size-4" />
        <h1 className="text-lg font-semibold tracking-[-0.01em]">
          {running.cancelling ? t("tests.running.cancelling") : isolationLook(running.isolation).running}
        </h1>
        <span className="font-mono text-xs text-muted-foreground">{clock(running.since, now.getTime())}</span>
        <Button variant="secondary" className="ml-auto" disabled={running.cancelling} onClick={onCancel}>
          {t("common.actions.cancel")}
        </Button>
      </div>
      {/* Before the runner reports progress, its analyzer may still be being prepared. */}
      {!progress && <PreparingAnalyzers className="mt-3.5" />}
      {progress && (
        <>
          <div
            role="progressbar"
            aria-label={t("tests.running.progress")}
            aria-valuemin={0}
            aria-valuemax={progress.packagesTotal}
            aria-valuenow={progress.packagesDone}
            className="mt-3.5 mb-2 h-1.5 max-w-[720px] overflow-hidden rounded-[3px] bg-border"
          >
            <i className="block h-full bg-primary" style={{ width: `${(ratio ?? 0) * 100}%` }} />
          </div>
          <p className="flex flex-wrap gap-x-[18px] gap-y-1 text-xs text-muted-foreground">
            <span>{t("tests.running.packages", { done: formatNumber(progress.packagesDone), total: formatNumber(progress.packagesTotal) })}</span>
            <span>
              {t("tests.running.testsSoFar", { count: progress.tests })},{" "}
              {progress.failed > 0 ? (
                <span className="text-del">{t("tests.running.failingSoFar", { count: progress.failed })}</span>
              ) : (
                t("tests.running.noneFailing")
              )}
            </span>
            {previous && <span>{t("tests.running.lastTook", { duration: formatDuration(previous.durationMs) })}</span>}
            {progress.current && (
              <span>
                {t("tests.running.now")} <code>{progress.current}</code>
              </span>
            )}
          </p>
        </>
      )}
      <Hint className="mt-1.5">{running.ours ? t("tests.running.oursHint") : t("tests.running.elsewhereHint")}</Hint>
      {previous && (
        <p className="mt-5 flex items-center gap-2.5 text-xs text-muted-foreground">
          <em className="rounded-[9px] border border-dashed border-faint px-2 text-[11px] not-italic">{t("tests.running.previous")}</em>
          {t("tests.running.previousNote", { when: formatRelativeTime(previous.startedAt, now) })}
        </p>
      )}
    </section>
  );
}

/** Before the first run, or after one that did not report: what running does, where, with what, and the button. */
export function RunChoicePanel({
  title,
  lead,
  choice,
  onChoose,
  onRun,
}: {
  title: string;
  lead?: ReactNode;
  choice: RunChoice;
  onChoose: (change: Partial<RunChoice>) => void;
  onRun: () => void;
}) {
  const { t } = useTranslation();
  const titleId = useId();
  return (
    <section aria-labelledby={titleId} className="col-span-full max-w-[700px]">
      <h1 id={titleId} className="text-lg font-semibold tracking-[-0.01em]">
        {title}
      </h1>
      {lead}
      <p className="mt-2 max-w-[68ch]">{t("tests.never.lead")}</p>
      <ul className="mt-4 mb-1 grid gap-2">
        {(["all", "offline", "time", "meanwhile"] as const).map((item) => (
          <li key={item} className="grid grid-cols-[14px_1fr] gap-2 text-muted-foreground">
            <span aria-hidden="true" className="mt-[7px] size-1.5 rounded-[1.5px] bg-primary" />
            <span>{t(`tests.never.${item}`)}</span>
          </li>
        ))}
      </ul>
      <fieldset className="mt-[18px]">
        <legend className="mb-2 font-medium text-muted-foreground">{t("tests.isolation.legend")}</legend>
        <div className="grid grid-cols-2 gap-3 max-[900px]:grid-cols-1">
          {ISOLATIONS.map(isolationLook).map((look) => (
            <label
              key={look.value}
              className="grid cursor-pointer grid-cols-[auto_1fr] items-start gap-x-2.5 gap-y-1 rounded-[10px] border bg-card px-3.5 py-3 has-checked:border-primary has-focus-visible:outline-2 has-focus-visible:outline-offset-2 has-focus-visible:outline-ring"
            >
              <input
                type="radio"
                name="isolation"
                value={look.value}
                checked={choice.isolation === look.value}
                onChange={() => onChoose({ isolation: look.value })}
                className="mt-[3px] accent-primary outline-none"
              />
              <strong className="font-medium">{look.label}</strong>
              <span className="col-start-2 text-muted-foreground">{look.description}</span>
            </label>
          ))}
        </div>
      </fieldset>
      <div className="mt-3 flex gap-[22px]">
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={choice.race}
            onChange={(event) => onChoose({ race: event.currentTarget.checked })}
            className="accent-primary"
          />
          {t("tests.options.race")}
        </label>
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={choice.coverage}
            onChange={(event) => onChoose({ coverage: event.currentTarget.checked })}
            className="accent-primary"
          />
          {t("tests.options.coverage")}
        </label>
      </div>
      <Button className="mt-[18px]" onClick={onRun}>
        {t("tests.isolation.run")}
      </Button>
    </section>
  );
}
