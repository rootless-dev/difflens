// The Mutation section of the Tests tab: what mutation testing does and the button that runs it (never on its own),
// the run going on with its progress, and the last completed run: its score, the mutants that survived or ran out of
// time, each with its line as it is and as mutated, and, folded, the killed ones and those that could not be tested.

import { type ReactNode, useId } from "react";
import { useTranslation } from "react-i18next";

import { ErrorNote, Hint } from "@/components/page";
import { PreparingAnalyzers } from "@/components/preparing-analyzers";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { WithCode } from "@/components/with-code";
import { OutputBlock, ToolProblems } from "@/features/review/ReviewPage";
import { mutantChangeText, operatorName } from "@/features/review/signals/signals-text";
import { CopyMutantPrompt, MutantCode, MutantStatusTag } from "@/features/review/tests/mutant-parts";
import { mutantPlace, mutantShortChange, mutationScore } from "@/features/review/tests/mutation-text";
import { clock, Location, Spin } from "@/features/review/tests/parts";
import type { MutationHandle, MutationRunning } from "@/features/review/tests/use-mutation";
import { useNow } from "@/hooks/use-now";
import type { Mutant, MutationProgress, MutationRun, MutationState } from "@/lib/api";
import { coreText } from "@/lib/core-text";
import { formatDuration, formatNumber, formatPercent, formatRelativeTime } from "@/lib/format";
import { cn } from "@/lib/utils";

/**
 * The Mutation section, under the test run. Hidden when no analyzer of the project's languages runs tests (the tab
 * already says so). `testsBusy`: a test run goes on, so none can start.
 */
export function MutationSection({ handle, testsBusy }: { handle: MutationHandle; testsBusy: boolean }) {
  const { t } = useTranslation();
  const titleId = useId();
  const { panel } = handle;
  if (panel.status === "ready" && panel.state.unavailable?.reason === "no_runner") return null;
  return (
    <section aria-labelledby={titleId} className="col-span-full mt-[30px] border-t pt-6">
      <h2 id={titleId} className="mb-3 text-base font-semibold tracking-[-0.01em]">
        {t("tests.mutation.title")}
      </h2>
      {panel.status === "loading" && (
        <div role="status" aria-label={t("tests.mutation.reading")} className="grid max-w-[480px] gap-3">
          <Skeleton className="h-3 w-1/2" />
          <Skeleton className="h-3.5 w-full" />
        </div>
      )}
      {panel.status === "error" && (
        <div>
          <ErrorNote>{panel.message}</ErrorNote>
          <Button variant="secondary" size="sm" className="mt-2.5" onClick={handle.retry}>
            {t("common.actions.tryAgain")}
          </Button>
        </div>
      )}
      {panel.status === "ready" && (
        <>
          {panel.runError && (
            <div className="mb-3">
              <ErrorNote>{panel.runError}</ErrorNote>
            </div>
          )}
          {panel.running ? (
            <RunningBox running={panel.running} progress={panel.progress} onCancel={handle.cancel} />
          ) : (
            <Head state={panel.state} ended={panel.ended} testsBusy={testsBusy} onRun={handle.run} />
          )}
          {panel.state.run && (
            <div
              data-testid="mutation-run"
              data-faded={panel.running !== null || undefined}
              className={cn(panel.running !== null && "opacity-50 saturate-[.6]")}
            >
              <RunDetails run={panel.state.run} />
            </div>
          )}
        </>
      )}
    </section>
  );
}

/** Before a run, or after one: what it does or how it went, why it cannot run when it cannot, and the button. */
function Head({
  state,
  ended,
  testsBusy,
  onRun,
}: {
  state: MutationState;
  ended: MutationRun | null;
  testsBusy: boolean;
  onRun: () => void;
}) {
  const { t } = useTranslation();
  const blockedId = useId();
  const needsCoverage = state.unavailable?.reason === "needs_coverage";
  const { run } = state;
  const button = (
    <Button
      variant={run ? "secondary" : "default"}
      disabled={needsCoverage || testsBusy}
      aria-describedby={needsCoverage ? blockedId : undefined}
      title={testsBusy ? t("tests.busy") : undefined}
      onClick={onRun}
    >
      {run ? t("tests.mutation.runAgain") : t("tests.mutation.run")}
    </Button>
  );
  return (
    <div className="max-w-[760px]">
      {ended && <EndedNote run={ended} previous={run !== null} />}
      {run ? (
        <div className="flex flex-wrap items-start gap-x-3.5 gap-y-2">
          <Score run={run} />
          <span className="ml-auto">{button}</span>
        </div>
      ) : (
        <>
          <p className="max-w-[68ch]">
            <WithCode text={t("tests.mutation.lead")} />
          </p>
          <p className="mt-1.5 max-w-[68ch] text-muted-foreground">
            {t("tests.mutation.limits", { count: state.maxMutants })}
          </p>
          <div className="mt-3.5">{button}</div>
        </>
      )}
      {needsCoverage && (
        <p id={blockedId} className="mt-2.5 max-w-[68ch] text-mod">
          {t("tests.mutation.needsCoverage")}
        </p>
      )}
    </div>
  );
}

/** How the last run started here ended when it did not complete; the completed one before it stays below. */
function EndedNote({ run, previous }: { run: MutationRun; previous: boolean }) {
  const { t } = useTranslation();
  const duration = formatDuration(run.durationMs);
  return (
    <div className="mb-3.5">
      {run.status === "failed" && (
        <>
          <ErrorNote>{t("tests.mutation.failed")}</ErrorNote>
          {run.message && <OutputBlock text={coreText(run.message)} label={t("tests.mutation.output")} />}
        </>
      )}
      {run.status === "timed_out" && <ErrorNote>{t("tests.mutation.timedOut", { duration })}</ErrorNote>}
      {run.status === "cancelled" && <p>{t("tests.mutation.cancelled", { duration })}</p>}
      {previous && <p className="mt-1 text-xs text-muted-foreground">{t("tests.mutation.previous")}</p>}
    </div>
  );
}

/** "80% 12 of 15 killed · 1 timeout", and under it what else the run says: errors, reused results, by what, when. */
function Score({ run }: { run: MutationRun }) {
  const { t } = useTranslation();
  const now = useNow(30_000);
  const { summary } = run;
  const { killed, total } = mutationScore(summary);
  return (
    <div>
      {total > 0 ? (
        <p className="text-lg leading-[1.3] font-semibold tracking-[-0.01em]" title={t("tests.mutation.scoreTitle")}>
          <span className={cn("font-mono", summary.survived > 0 ? "text-del" : "text-add")}>
            {formatPercent(killed / total)}
          </span>{" "}
          {t("tests.mutation.score", { killed: formatNumber(killed), total: formatNumber(total) })}
          {summary.timeout > 0 && ` · ${t("tests.mutation.timeoutCount", { count: summary.timeout })}`}
        </p>
      ) : (
        <p className="text-lg font-semibold">
          {run.mutants.length === 0 ? t("tests.mutation.nothing") : t("tests.mutation.noneTested")}
        </p>
      )}
      <ul className="mt-1.5 flex flex-wrap gap-x-[18px] gap-y-1 text-xs text-muted-foreground">
        {summary.error > 0 && <li>{t("tests.mutation.errors", { count: summary.error })}</li>}
        {summary.noCoverage > 0 && <li>{t("tests.mutation.noCoverage", { count: summary.noCoverage })}</li>}
        {summary.cached > 0 && <li>{t("tests.mutation.cached", { count: summary.cached })}</li>}
        {run.engine && <li>{t("tests.mutation.by", { engine: `${run.engine.name} ${run.engine.version}` })}</li>}
        <li>
          {t("tests.mutation.ranAt", {
            when: formatRelativeTime(run.startedAt, now),
            duration: formatDuration(run.durationMs),
          })}
        </li>
      </ul>
      {summary.sampled && (
        <p className="mt-1.5 text-xs text-muted-foreground">
          {t("tests.mutation.sampled", {
            tested: formatNumber(summary.maxMutants),
            candidates: formatNumber(summary.candidates),
          })}
        </p>
      )}
      {run.coverageRunAt && (
        <p className="mt-1 text-xs text-faint">
          {t("tests.mutation.coverageFrom", { when: formatRelativeTime(run.coverageRunAt, now) })}
        </p>
      )}
    </div>
  );
}

/** The run going on: its phase, how many mutants of how many, what became of them so far, and Cancel. */
function RunningBox({
  running,
  progress,
  onCancel,
}: {
  running: MutationRunning;
  progress: MutationProgress | null;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const now = useNow(1000);
  const label = running.cancelling ? t("tests.mutation.cancelling") : t("tests.mutation.running");
  return (
    <div role="status" aria-label={label} className="max-w-[760px] pb-4">
      <div className="flex items-center gap-3">
        <Spin className="size-4" />
        <p className="font-semibold">{label}</p>
        <span className="font-mono text-xs text-muted-foreground">{clock(running.since, now.getTime())}</span>
        <Button variant="secondary" className="ml-auto" disabled={running.cancelling} onClick={onCancel}>
          {t("common.actions.cancel")}
        </Button>
      </div>
      {/* Before the analyzer writes its progress, it may still be being prepared. */}
      {!progress && (
        <PreparingAnalyzers
          className="mt-3.5"
          fallback={<p className="mt-2 text-muted-foreground">{t("tests.mutation.preparing")}</p>}
        />
      )}
      {progress?.phase === "baseline" && <p className="mt-2 text-muted-foreground">{t("tests.mutation.baseline")}</p>}
      {progress?.phase === "mutating" && (
        <>
          <div
            role="progressbar"
            aria-label={t("tests.mutation.progress")}
            aria-valuemin={0}
            aria-valuemax={progress.total}
            aria-valuenow={progress.done}
            className="mt-3.5 mb-2 h-1.5 overflow-hidden rounded-[3px] bg-border"
          >
            <i
              className="block h-full bg-primary"
              style={{ width: `${progress.total > 0 ? (progress.done / progress.total) * 100 : 0}%` }}
            />
          </div>
          <p className="flex flex-wrap gap-x-[18px] gap-y-1 text-xs text-muted-foreground">
            <span>
              {t("tests.mutation.mutants", { done: formatNumber(progress.done), total: formatNumber(progress.total) })}
            </span>
            <span>{t("tests.mutation.killedCount", { count: progress.killed })}</span>
            <span className={cn(progress.survived > 0 && "text-del")}>
              {t("tests.mutation.survivedCount", { count: progress.survived })}
            </span>
            {progress.timeout > 0 && <span>{t("tests.mutation.timeoutCount", { count: progress.timeout })}</span>}
            {progress.error > 0 && <span>{t("tests.mutation.errorCount", { count: progress.error })}</span>}
            {progress.current && (
              <span>
                {t("tests.mutation.now")} <code>{progress.current}</code>
              </span>
            )}
          </p>
        </>
      )}
      <Hint className="mt-1.5">{t("tests.mutation.meanwhile")}</Hint>
    </div>
  );
}

/** The mutants of a completed run: those that survived or ran out of time in full, the others folded. */
function RunDetails({ run }: { run: MutationRun }) {
  const { t } = useTranslation();
  const listId = useId();
  const shown = run.mutants.filter((mutant) => mutant.status === "survived" || mutant.status === "timeout");
  const killed = run.mutants.filter((mutant) => mutant.status === "killed");
  const untested = run.mutants.filter((mutant) => mutant.status === "error" || mutant.status === "no_coverage");
  return (
    <>
      {shown.length > 0 ? (
        <section aria-labelledby={listId} className="mt-5">
          <h3 id={listId} className="flex items-baseline gap-2 text-sm font-medium text-muted-foreground">
            {t("tests.mutation.survivors")} <span className="font-normal text-faint">{shown.length}</span>
          </h3>
          <p className="mb-2.5 text-xs text-faint">{t("tests.mutation.survivorsHint")}</p>
          <div className="grid gap-3">
            {shown.map((mutant) => (
              <MutantCard key={mutant.id} mutant={mutant} run={run} />
            ))}
          </div>
        </section>
      ) : (
        killed.length > 0 && <p className="mt-4 text-add">{t("tests.mutation.allKilled")}</p>
      )}
      {killed.length > 0 && (
        <Folded title={t("tests.mutation.killed")} mutants={killed}>
          {(mutant) => mutant.killedBy && t("tests.mutation.killedBy", { test: mutant.killedBy })}
        </Folded>
      )}
      {untested.length > 0 && (
        <Folded title={t("tests.mutation.notTested")} mutants={untested}>
          {(mutant) => <MutantStatusTag status={mutant.status} />}
        </Folded>
      )}
      <ToolProblems diagnostics={run.diagnostics} what={t("tests.mutation.toolProblems")} />
    </>
  );
}

/** A mutant that survived or ran out of time: where, what changed and how, its line, and the prompt for an agent. */
function MutantCard({ mutant, run }: { mutant: Mutant; run: MutationRun }) {
  const { t } = useTranslation();
  return (
    <article
      aria-label={t("tests.mutation.cardLabel", { place: mutantPlace(mutant) })}
      data-mutant={mutant.id}
      className="max-w-[860px] rounded-[10px] border bg-card px-4 py-3"
    >
      <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1 text-xs">
        <MutantStatusTag status={mutant.status} />
        <span className="text-muted-foreground">{operatorName(mutant.operator)}</span>
        {mutant.function && <code className="text-muted-foreground">{mutant.function.qualifiedName}</code>}
        <span className="ml-auto">
          <Location file={mutant.file} line={mutant.line} full />
        </span>
      </div>
      <p className="mt-1.5 break-words">
        <WithCode text={mutantChangeText(mutant)} />
      </p>
      <MutantCode mutant={mutant} className="mt-2" />
      {mutant.status === "survived" && (
        <div className="mt-2.5">
          <CopyMutantPrompt mutant={mutant} run={run} />
        </div>
      )}
    </article>
  );
}

/** A folded list of mutants, one line each: where, what changed, and what `aside` adds. */
function Folded({
  title,
  mutants,
  children: aside,
}: {
  title: string;
  mutants: readonly Mutant[];
  children: (mutant: Mutant) => ReactNode;
}) {
  return (
    <details className="mt-4">
      <summary className="cursor-pointer text-sm text-muted-foreground select-none">
        {title} <span className="text-faint">{mutants.length}</span>
      </summary>
      <ul className="mt-2 grid gap-1.5">
        {mutants.map((mutant) => (
          <li
            key={mutant.id}
            data-mutant={mutant.id}
            className="grid grid-cols-[auto_minmax(0,1fr)_auto] items-baseline gap-x-3 text-xs"
          >
            <Location file={mutant.file} line={mutant.line} full />
            <span className="truncate" title={mutant.description}>
              <WithCode text={mutantShortChange(mutant)} />
              <span className="text-faint"> · {operatorName(mutant.operator)}</span>
            </span>
            <span className="text-muted-foreground">{aside(mutant)}</span>
          </li>
        ))}
      </ul>
    </details>
  );
}
