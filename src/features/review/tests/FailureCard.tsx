import { ChevronRight } from "lucide-react";
import { type ReactNode, useId, useState } from "react";
import { useTranslation } from "react-i18next";

import { Hint } from "@/components/page";
import { Button } from "@/components/ui/button";
import { fileDiffTarget, notInReview, OutputBlock } from "@/features/review/ReviewPage";
import {
  Location,
  OutcomeMark,
  RowRun,
  SelectBox,
  Spin,
  StateTag,
  clock,
  useTestsContext,
} from "@/features/review/tests/parts";
import { packageCheck, packageLeaves, subtestKey, testCheck, testLeaves } from "@/features/review/tests/selection";
import {
  type Failure,
  selectionRuns,
  selectionTouches,
  type SuitePackage,
  type SuiteTest,
} from "@/features/review/tests/suite";
import { useNow } from "@/hooks/use-now";
import { i18n } from "@/i18n";
import type { FailureFrame, RaceAccess, RunSelection, TestFailure } from "@/lib/api";
import { formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";

interface FailureCardProps {
  failure: Failure;
  /** What asks for the explanation of the failure, first in the row of its actions. */
  action?: ReactNode;
  /** Where the explanation of the failure goes, under its output. */
  explanation?: ReactNode;
}

/** One failing top-level test, its subtests nested, or one package that failed outside its tests. */
export function FailureCard({ failure, action, explanation }: FailureCardProps) {
  return failure.kind === "test" ? (
    <TestCard test={failure.test} action={action} explanation={explanation} />
  ) : (
    <PackageCard pkg={failure.pkg} action={action} explanation={explanation} />
  );
}

function kindLabel(failure: TestFailure | null): string {
  return i18n.t(`tests.failures.kind.${failure?.kind ?? "other"}`);
}

/** The failure of a test: its own, else its first failed subtest's. */
export function failureOf(test: SuiteTest): TestFailure | null {
  return test.case?.failure ?? test.subtests.find((sub) => sub.case.failure)?.case.failure ?? null;
}

function outputOf(test: SuiteTest): string | null {
  if (test.case?.output) return test.case.output;
  const outputs = test.subtests.map((sub) => sub.case.output).filter((output): output is string => Boolean(output));
  return outputs.length > 0 ? outputs.join("\n") : null;
}

function Card({
  name,
  nameId,
  bodyId,
  open,
  onToggle,
  box,
  kind,
  duration,
  meta,
  children,
}: {
  name: string;
  nameId: string;
  bodyId: string;
  open: boolean;
  onToggle: () => void;
  box: ReactNode;
  kind: string;
  duration: string;
  meta: ReactNode;
  children: ReactNode;
}) {
  return (
    <article aria-labelledby={nameId} className="rounded-[10px] border border-l-2 border-l-del bg-card">
      <header className="grid grid-cols-[auto_minmax(0,1fr)_auto_auto] items-center gap-x-3.5 gap-y-0.5 py-3 pr-4 pl-2.5">
        {box}
        <button
          type="button"
          aria-expanded={open}
          aria-controls={bodyId}
          onClick={onToggle}
          className="-ml-1.5 flex min-w-0 cursor-pointer items-center gap-2 rounded-[6px] px-1 py-0.5 text-left outline-none focus-visible:outline-2 focus-visible:outline-ring"
        >
          <ChevronRight aria-hidden="true" className={cn("size-3.5 flex-none text-faint transition-transform", open && "rotate-90")} />
          <OutcomeMark outcome="fail" />
          <code id={nameId} className="text-[13.5px] font-medium break-all text-foreground">
            {name}
          </code>
        </button>
        <em className="rounded-[9px] bg-del-bg px-[9px] py-px text-[11px] whitespace-nowrap text-del not-italic">
          {kind}
        </em>
        <span className="min-w-[52px] text-right font-mono text-xs text-faint">{duration}</span>
        <p className="col-span-full flex flex-wrap items-center gap-x-3 gap-y-1.5 pl-[58px] text-xs text-muted-foreground">
          {meta}
        </p>
      </header>
      <div id={bodyId} hidden={!open} className="grid gap-3 pr-4 pb-3.5 pl-[58px]">
        {children}
      </div>
    </article>
  );
}

function InlineRun({ label }: { label: string }) {
  const { t } = useTranslation();
  const { running, onCancel } = useTestsContext();
  const now = useNow(1000);
  if (!running) return null;
  return (
    <div role="status" className="flex max-w-[920px] items-center gap-2.5 rounded-[7px] bg-primary-soft px-2.5 py-[7px] text-xs">
      <Spin />
      <span>{running.cancelling ? t("tests.running.cancelling") : label}</span>
      <span className="font-mono text-muted-foreground">{clock(running.since, now.getTime())}</span>
      <Button variant="secondary" size="xs" className="ml-auto" disabled={running.cancelling} onClick={onCancel}>
        {t("common.actions.cancel")}
      </Button>
    </div>
  );
}

function OutputToggle({ output, label }: { output: string | null; label: string }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const id = useId();
  if (!output) return <Hint className="mt-0">{t("tests.failures.noOutput")}</Hint>;
  return (
    <>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={id}
        onClick={() => setOpen((value) => !value)}
        className="inline-flex cursor-pointer items-center gap-1 rounded-[6px] px-1.5 py-1 text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
      >
        <ChevronRight aria-hidden="true" className={cn("size-3 transition-transform", open && "rotate-90")} />
        {open ? t("tests.failures.hideOutput") : t("tests.failures.showOutput")}
      </button>
      <div id={id} hidden={!open} className="col-span-full">
        {open && <OutputBlock text={output} label={label} className="my-0" />}
      </div>
    </>
  );
}

function RunButton({ selection, children }: { selection: RunSelection; children: string }) {
  const { t } = useTranslation();
  const { busy, commands, onRun } = useTestsContext();
  return (
    <Button
      variant="secondary"
      size="xs"
      disabled={busy}
      title={busy ? t("tests.busy") : commands(selection).join("\n")}
      onClick={() => onRun(selection)}
    >
      {children}
    </Button>
  );
}

function OpenInDiff({ file, line }: { file: string | null; line: number | null }) {
  const { t } = useTranslation();
  const { files, onViewDiff } = useTestsContext();
  const why = useId();
  const target = file !== null ? fileDiffTarget(file, line ?? 1, files) : null;
  return (
    <>
      <Button
        variant="secondary"
        size="xs"
        disabled={target === null}
        aria-describedby={target === null ? why : undefined}
        onClick={() => target && onViewDiff(target)}
      >
        {t("tests.failures.openDiff")}
      </Button>
      {target === null && (
        <span id={why} className="text-xs text-faint">
          {file === null ? t("tests.failures.noLocation") : `${notInReview()}.`}
        </span>
      )}
    </>
  );
}

function TestCard({ test, action, explanation }: { test: SuiteTest; action?: ReactNode; explanation?: ReactNode }) {
  const { t } = useTranslation();
  const { index, running } = useTestsContext();
  const [open, setOpen] = useState(true);
  const nameId = useId();
  const bodyId = useId();
  const failure = failureOf(test);
  const runningHere =
    running?.selection != null && selectionRuns(running.selection, test) ? running.selection : null;
  const runningSubtests = runningHere
    ? test.subtests.filter((sub) => selectionRuns(runningHere, test, sub)).length
    : 0;
  const runningLabel =
    test.subtests.length === 0
      ? t("tests.failures.runningTest")
      : runningSubtests === test.subtests.length
        ? t("tests.failures.runningSubtests", { count: runningSubtests })
        : t("tests.failures.runningSome", { count: runningSubtests });
  const whole: RunSelection = { packages: [{ package: test.package, tests: [{ name: test.name, subtests: null }] }] };
  return (
    <Card
      name={test.name}
      nameId={nameId}
      bodyId={bodyId}
      open={open}
      onToggle={() => setOpen((value) => !value)}
      box={
        <SelectBox state={testCheck(test, index)} leaves={testLeaves(test)} label={t("tests.failures.select", { name: test.name })} />
      }
      kind={kindLabel(failure)}
      duration={formatDuration(test.durationMs)}
      meta={
        <>
          <code>{test.package}</code>
          {test.file !== null && test.line !== null && <Location file={test.file} line={test.line} />}
          <StateTag state={test.state} />
        </>
      }
    >
      {runningHere && <InlineRun label={runningLabel} />}
      <div className={cn("grid gap-3", runningHere && "opacity-50")}>
        {test.subtests.length > 0 && (
          <ul aria-label={t("tests.failures.subtests")} className="mt-1 border-l pl-3">
            {test.subtests.map((sub) => {
              const key = subtestKey(test.package, test.name, sub.name);
              const subRunning = runningHere !== null && selectionRuns(runningHere, test, sub);
              return (
                <li
                  key={sub.name}
                  className="group/row grid grid-cols-[14px_14px_minmax(0,1fr)_auto_52px] items-center gap-2 rounded-[6px] px-2 py-[3px] hover:bg-secondary focus-within:bg-secondary has-checked:bg-primary-soft"
                >
                  <SelectBox
                    state={index.leaves.has(key) ? "all" : "none"}
                    leaves={[key]}
                    label={t("tests.failures.selectSubtest", { name: sub.title })}
                  />
                  {subRunning ? <Spin /> : <OutcomeMark outcome={sub.case.outcome} />}
                  <span className={cn("break-words", sub.case.outcome !== "fail" && "text-muted-foreground")}>
                    {sub.title}
                  </span>
                  <RowRun
                    selection={{ packages: [{ package: test.package, tests: [{ name: test.name, subtests: [sub.name] }] }] }}
                  />
                  <small className="text-right font-mono text-xs text-faint">{formatDuration(sub.case.durationMs)}</small>
                </li>
              );
            })}
          </ul>
        )}
        {failure && <FailureExcerpt failure={failure} test={test} />}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        {action}
        <RunButton selection={whole}>{t("tests.failures.runTest")}</RunButton>
        <OpenInDiff file={test.file} line={test.line} />
        <OutputToggle output={outputOf(test)} label={t("tests.failures.output")} />
      </div>
      {explanation}
    </Card>
  );
}

function PackageCard({ pkg, action, explanation }: { pkg: SuitePackage; action?: ReactNode; explanation?: ReactNode }) {
  const { t } = useTranslation();
  const { index, running } = useTestsContext();
  const [open, setOpen] = useState(true);
  const nameId = useId();
  const bodyId = useId();
  const failure = pkg.outcome?.failure ?? null;
  const first = failure?.kind === "build" ? failure.errors.find((error) => error.file !== null) : undefined;
  const runningHere = running?.selection != null && selectionTouches(running.selection, pkg.name);
  return (
    <Card
      name={pkg.name}
      nameId={nameId}
      bodyId={bodyId}
      open={open}
      onToggle={() => setOpen((value) => !value)}
      box={
        <SelectBox
          state={packageCheck(pkg, index)}
          leaves={packageLeaves(pkg)}
          label={t("tests.failures.selectPackage", { name: pkg.name })}
        />
      }
      kind={pkg.didNotBuild ? t("tests.failures.kind.build") : t("tests.failures.kind.outside")}
      duration="—"
      meta={
        <>
          {pkg.didNotBuild && <span>{t("tests.failures.wholePackage")}</span>}
          {first?.file && first.line !== null && <Location file={first.file} line={first.line} />}
        </>
      }
    >
      {runningHere && <InlineRun label={t("tests.failures.runningPackage")} />}
      {failure && (
        <div className={cn(runningHere && "opacity-50")}>
          <FailureExcerpt failure={failure} test={null} />
        </div>
      )}
      <div className="flex flex-wrap items-center gap-2">
        {action}
        <RunButton selection={{ packages: [{ package: pkg.name, tests: null }] }}>{t("tests.failures.runPackage")}</RunButton>
        <OpenInDiff file={first?.file ?? null} line={first?.line ?? null} />
        <OutputToggle output={pkg.outcome?.output ?? null} label={t("tests.failures.packageOutput", { name: pkg.name })} />
      </div>
      {explanation}
    </Card>
  );
}

function Excerpt({ title, aside, children }: { title: ReactNode; aside?: ReactNode; children?: ReactNode }) {
  return (
    <div className="max-w-[920px] rounded-[8px] border bg-background px-3 py-2.5">
      <p className={cn("flex flex-wrap items-baseline gap-x-2.5 text-xs text-muted-foreground", children && "mb-2")}>
        <strong className="text-[13px] font-medium break-words text-foreground">{title}</strong>
        {aside}
      </p>
      {children}
    </div>
  );
}

/** Where a frame is: "in <function>, file:line", or only the place when the output did not name the function (a Rust panic without a backtrace). */
function FrameAt({ frame }: { frame: FailureFrame }) {
  const { t } = useTranslation();
  return (
    <span className="text-muted-foreground">
      {frame.function ? (
        <>
          {t("tests.panic.in")} <code className="text-foreground">{frame.function}</code>,{" "}
        </>
      ) : null}
      <Location file={frame.file} line={frame.line} />
    </span>
  );
}

/** The key part of a failure: the race's two accesses, what was expected and what came, the panic, the errors. */
export function FailureExcerpt({ failure, test }: { failure: TestFailure; test: SuiteTest | null }) {
  const { t } = useTranslation();
  switch (failure.kind) {
    case "race": {
      const [first, ...others] = failure.races;
      if (!first) return null;
      return (
        <Excerpt
          title={t("tests.race.title")}
          aside={others.length > 0 ? <span>{t("tests.race.more", { count: others.length })}</span> : undefined}
        >
          <div role="list" className="grid grid-cols-[72px_auto_minmax(0,1fr)] gap-x-4">
            {first.accesses.map((access, index) => (
              <RaceRow key={index} access={access} other={first.accesses.find((candidate) => candidate !== access)} first={index === 0} />
            ))}
          </div>
        </Excerpt>
      );
    }
    case "assertion": {
      const where =
        failure.test && test && failure.test !== test.name
          ? (test.subtests.find((sub) => sub.case.name === failure.test)?.title ?? null)
          : null;
      return (
        <Excerpt
          title={failure.message}
          aside={
            (where || failure.at) && (
              <span>
                {where && `${t("tests.assertion.in", { test: where })}${failure.at ? ", " : ""}`}
                {failure.at && <Location file={failure.at.file} line={failure.at.line} />}
              </span>
            )
          }
        >
          {(failure.expected !== null || failure.actual !== null) && (
            <dl className="grid grid-cols-[72px_minmax(0,1fr)] gap-x-4 gap-y-[5px] font-mono text-xs">
              {failure.expected !== null && (
                <>
                  <dt className="font-sans text-[13px] text-muted-foreground">{t("tests.assertion.expected")}</dt>
                  <dd className="break-words whitespace-pre-wrap text-add">{failure.expected}</dd>
                </>
              )}
              {failure.actual !== null && (
                <>
                  <dt className="font-sans text-[13px] text-muted-foreground">{t("tests.assertion.actual")}</dt>
                  <dd className="break-words whitespace-pre-wrap text-del">{failure.actual}</dd>
                </>
              )}
            </dl>
          )}
        </Excerpt>
      );
    }
    case "panic":
      return (
        <Excerpt title={`panic: ${failure.message}`} aside={failure.frame && <FrameAt frame={failure.frame} />} />
      );
    case "timeout":
      return (
        <Excerpt
          title={failure.message}
          aside={
            failure.frame && (
              <span>
                {t("tests.timeout.stuck")} <code className="text-foreground">{failure.frame.function}</code>,{" "}
                <Location file={failure.frame.file} line={failure.frame.line} />
              </span>
            )
          }
        >
          {failure.running.length > 0 && (
            <p className="text-xs text-muted-foreground">
              {t("tests.timeout.running")} <code>{failure.running.join(", ")}</code>
            </p>
          )}
        </Excerpt>
      );
    case "build":
      return (
        <div className="max-w-[920px] rounded-[8px] border bg-background px-3 py-2.5 font-mono text-xs leading-[1.7] break-words whitespace-pre-wrap">
          {failure.errors.slice(0, 5).map((error, index) => (
            <div key={index}>
              <span className="-mx-3 block bg-del-bg px-3">
                {error.file !== null && error.line !== null ? (
                  <Location file={error.file} line={error.line} />
                ) : null}
                {error.column !== null && `:${error.column}`}
                {error.file !== null && ": "}
                {error.message}
              </span>
              {error.detail.map((line, n) => (
                <span key={n} className="block pl-4">
                  {line}
                </span>
              ))}
            </div>
          ))}
          {failure.errors.length > 5 && (
            <span className="block font-sans text-muted-foreground">{t("tests.build.more", { count: failure.errors.length - 5 })}</span>
          )}
        </div>
      );
    case "other":
      return failure.lines.length > 0 ? (
        <pre className="max-w-[920px] overflow-auto rounded-[8px] border bg-background px-3 py-2.5 font-mono text-xs leading-[1.7] whitespace-pre-wrap">
          {failure.lines.join("\n")}
        </pre>
      ) : null;
  }
}

function RaceRow({ access, other, first }: { access: RaceAccess; other: RaceAccess | undefined; first: boolean }) {
  const { t } = useTranslation();
  const cell = cn("py-[7px]", !first && "border-t", first && "pt-0.5");
  const who = access.createdAt ? (
    <>
      {t("tests.race.createdAt")} <Location file={access.createdAt.file} line={access.createdAt.line} />
    </>
  ) : access.frame?.test || access.goroutine !== null ? (
    access.frame?.test ? (
      t("tests.race.byTest")
    ) : (
      t("tests.race.byGoroutine", { number: access.goroutine })
    )
  ) : (
    t("tests.race.byMain")
  );
  const before = access.previous && other ? `, ${other.kind === "write" ? t("tests.race.beforeWrite") : t("tests.race.beforeRead")}` : "";
  return (
    <div role="listitem" className="contents">
      <span className={cn(cell, "font-medium", access.kind === "write" ? "text-del" : "text-mod")}>
        {access.kind === "write" ? t("tests.race.write") : t("tests.race.read")}
      </span>
      <span className={cell}>{access.frame ? <Location file={access.frame.file} line={access.frame.line} /> : "—"}</span>
      <span className={cn(cell, "text-muted-foreground")}>
        {access.frame && !access.frame.test && (
          <>
            {t("tests.race.in")} <code className="text-foreground">{access.frame.function}</code>,{" "}
          </>
        )}
        {who}
        {before}
      </span>
    </div>
  );
}
