import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { ErrorNote } from "@/components/page";
import { PreparingAnalyzers } from "@/components/preparing-analyzers";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import type { DiffTarget } from "@/features/review/flow/flow-facts";
import { anyOf } from "@/features/review/flow/FlowView";
import { OutputBlock, ReviewPage, ToolProblems } from "@/features/review/ReviewPage";
import { FailureCard } from "@/features/review/tests/FailureCard";
import { MutationSection } from "@/features/review/tests/MutationSection";
import { allOf, clock, Spin, TestsContext, type TestsContextValue, useTestsContext } from "@/features/review/tests/parts";
import {
  countSelection,
  indexSelection,
  keptIn,
  selectionPayload,
  toggled,
  withFailed,
} from "@/features/review/tests/selection";
import { buildSuite, type Failure, type Suite, testCommands } from "@/features/review/tests/suite";
import { SuiteBrowser } from "@/features/review/tests/SuiteBrowser";
import type { MutationHandle } from "@/features/review/tests/use-mutation";
import type { RunChoice, Running, TestsPanel } from "@/features/review/tests/use-tests";
import { RunBox, RunChoicePanel, Verdict } from "@/features/review/tests/Verdict";
import { useNow } from "@/hooks/use-now";
import { i18n } from "@/i18n";
import type { ChangedFile, RunSelection, TestRun } from "@/lib/api";
import { coreText } from "@/lib/core-text";
import { formatDuration, languageName } from "@/lib/format";
import { cn } from "@/lib/utils";

interface TestsViewProps {
  panel: TestsPanel;
  /** Files of the review, to open a test or a covered file in the Diff tab. */
  files: readonly ChangedFile[];
  /** Runs every test (null) or a selection, as the panel's choice says. */
  onRun: (selection: RunSelection | null) => void;
  onCancel: () => void;
  onChoose: (change: Partial<RunChoice>) => void;
  onRetry: () => void;
  onViewDiff: (target: DiffTarget) => void;
  /**
   * What explains a failure in its card: `action` first in its row of actions, `block` under the output; nothing
   * when not given.
   */
  renderExplanation?: (failure: Failure) => FailureExplanation;
  /** Shown over the failures, such as the language of the explanations; nothing when not given. */
  renderFailuresBanner?: (failures: readonly Failure[]) => ReactNode;
  /** Mutation testing of the review, kept above the tab so a run goes on elsewhere; its section is under the run. */
  mutation: MutationHandle;
}

export interface FailureExplanation {
  action: ReactNode;
  block: ReactNode;
}

/** Content of the Tests tab. Nothing runs until the user asks: opening it only reads the last run. */
export function TestsView({
  panel,
  files,
  onRun,
  onCancel,
  onChoose,
  onRetry,
  onViewDiff,
  renderExplanation,
  renderFailuresBanner,
  mutation,
}: TestsViewProps) {
  const { t } = useTranslation();
  if (panel.status === "loading") {
    return (
      <ReviewPage>
        <PreparingAnalyzers
          className="col-span-full"
          fallback={
            <div role="status" aria-label={t("tests.readingLastRun")} className="col-span-full grid max-w-[480px] gap-3">
              <Skeleton className="h-3 w-1/2" />
              <Skeleton className="h-3.5 w-full" />
              <Skeleton className="h-3.5 w-4/5" />
            </div>
          }
        />
      </ReviewPage>
    );
  }
  if (panel.status === "error") {
    return (
      <ReviewPage>
        <div className="col-span-full">
          <ErrorNote>{panel.message}</ErrorNote>
          <Button variant="secondary" size="sm" className="mt-2.5" onClick={onRetry}>
            {t("common.actions.tryAgain")}
          </Button>
        </div>
      </ReviewPage>
    );
  }
  if (panel.tests.unavailable) {
    return (
      <ReviewPage>
        <p className="col-span-full">{noRunnerMessage(panel.tests.unavailable.languages)}</p>
      </ReviewPage>
    );
  }
  return (
    <ReadyView
      panel={panel}
      files={files}
      onRun={onRun}
      onCancel={onCancel}
      onChoose={onChoose}
      onViewDiff={onViewDiff}
      renderExplanation={renderExplanation}
      renderFailuresBanner={renderFailuresBanner}
      mutation={mutation}
    />
  );
}

export function noRunnerMessage(languages: readonly string[]): string {
  const names = languages.length > 0 ? anyOf(languages.map(languageName)) : i18n.t("tests.projectLanguages");
  return i18n.t("tests.noRunner", { languages: names });
}

type ReadyProps = Omit<TestsViewProps, "panel" | "onRetry"> & { panel: Extract<TestsPanel, { status: "ready" }> };

function ReadyView({
  panel,
  files,
  onRun,
  onCancel,
  onChoose,
  onViewDiff,
  renderExplanation,
  renderFailuresBanner,
  mutation,
}: ReadyProps) {
  const { t } = useTranslation();
  const { tests, running, runError, progress, choice } = panel;
  const run = tests.run;
  const completed = run?.status === "completed" ? run : null;
  const suite = useMemo(() => (completed ? buildSuite(completed) : null), [completed]);
  const [chosen, setChosen] = useState<ReadonlySet<string>>(new Set());
  const selected = useMemo(() => (suite ? keptIn(suite, chosen) : new Set<string>()), [suite, chosen]);
  const index = useMemo(() => indexSelection(selected), [selected]);
  const lastBox = useRef<HTMLInputElement | null>(null);
  const selectFailedButton = useRef<HTMLButtonElement>(null);

  const onToggle = useCallback((leaves: readonly string[], on: boolean, box: HTMLInputElement) => {
    lastBox.current = box;
    setChosen((current) => toggled(current, leaves, on));
  }, []);
  const context: TestsContextValue = {
    files,
    onViewDiff,
    busy: running !== null,
    running,
    // The race detector as the next run will have it, for Go's commands.
    commands: (selection) => (completed ? testCommands(completed, selection, choice.race) : []),
    index,
    onToggle,
    onRun,
    onCancel,
  };
  const showRunBox = running !== null && (running.selection === null || completed === null);

  return (
    <TestsContext.Provider value={context}>
      <ReviewPage aria-busy={running !== null || undefined} className="pb-0">
        {runError && (
          <div className="col-span-full mb-4">
            <ErrorNote>{runError}</ErrorNote>
          </div>
        )}
        {showRunBox ? (
          <RunBox running={running} progress={progress} previous={completed} onCancel={onCancel} />
        ) : completed && suite ? (
          <Verdict
            run={completed}
            suite={suite}
            choice={choice}
            busy={running !== null}
            files={files}
            onRun={onRun}
            onChoose={onChoose}
            onViewDiff={onViewDiff}
          />
        ) : run === null ? (
          <RunChoicePanel title={t("tests.notRunYet")} choice={choice} onChoose={onChoose} onRun={() => onRun(null)} />
        ) : (
          <RunChoicePanel
            title={t("tests.runAgainTitle")}
            lead={<RunEnded run={run} />}
            choice={choice}
            onChoose={onChoose}
            onRun={() => onRun(null)}
          />
        )}
        {completed && suite && (
          <div
            data-testid="test-run"
            data-faded={(showRunBox && running !== null) || undefined}
            className={cn("col-span-full", showRunBox && "opacity-50 saturate-[.6]")}
          >
            {suite.failures.length > 0 && (
              <section aria-labelledby="tests-failures">
                <div className="mt-[30px] mb-2.5 flex items-baseline gap-3.5">
                  <h2 id="tests-failures" className="flex items-center gap-2 text-sm font-medium text-muted-foreground">
                    {t("tests.failures.title")} <span className="font-normal text-faint">{suite.failures.length}</span>
                  </h2>
                  <button
                    ref={selectFailedButton}
                    type="button"
                    disabled={running !== null}
                    onClick={() => setChosen((current) => withFailed(suite, current))}
                    className={cn(
                      "cursor-pointer rounded-sm text-xs text-primary outline-none hover:underline focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:no-underline disabled:opacity-45",
                      showRunBox && "invisible",
                    )}
                  >
                    {t("tests.failures.selectFailed")}
                  </button>
                </div>
                {renderFailuresBanner?.(suite.failures)}
                <div className="grid gap-3">
                  {suite.failures.map((failure) => {
                    const explanation = renderExplanation?.(failure);
                    return (
                      <FailureCard
                        key={failure.kind === "test" ? `t:${failure.test.package}:${failure.test.name}` : `p:${failure.pkg.name}`}
                        failure={failure}
                        action={explanation?.action}
                        explanation={explanation?.block}
                      />
                    );
                  })}
                </div>
              </section>
            )}
            <SuiteBrowser suite={suite} />
          </div>
        )}
        {run && <ToolProblems diagnostics={run.diagnostics} what={t("tests.result.toolProblems")} />}
        <MutationSection handle={mutation} testsBusy={running !== null} />
        {suite && selected.size > 0 && (
          <SelectionBar
            suite={suite}
            selected={selected}
            running={running}
            onRun={onRun}
            onCancel={onCancel}
            onClear={() => {
              setChosen(new Set());
              const box = lastBox.current;
              (box?.isConnected && !box.disabled ? box : selectFailedButton.current)?.focus();
            }}
          />
        )}
        <div aria-hidden="true" className="col-span-full h-12" />
      </ReviewPage>
    </TestsContext.Provider>
  );
}

function RunEnded({ run }: { run: TestRun }) {
  const { t } = useTranslation();
  return (
    <div className="mt-2">
      {run.status === "failed" && (
        <>
          <ErrorNote>{t("tests.result.runnerStopped")}</ErrorNote>
          {/* The runner's own output, as it wrote it. */}
          {run.message && <OutputBlock text={coreText(run.message)} label={t("tests.result.runnerOutput")} />}
        </>
      )}
      {run.status === "timed_out" && (
        <ErrorNote>{t("tests.result.timedOut", { duration: formatDuration(run.durationMs) })}</ErrorNote>
      )}
      {run.status === "cancelled" && <p>{t("tests.result.cancelled", { duration: formatDuration(run.durationMs) })}</p>}
    </div>
  );
}

function selectionText(suite: Suite, selected: ReadonlySet<string>): { what: string; total: number } {
  const count = countSelection(suite, indexSelection(selected));
  const parts: string[] = [];
  if (count.tests > 0) parts.push(i18n.t("tests.selection.tests", { count: count.tests }));
  if (count.subtests > 0) parts.push(i18n.t("tests.selection.subtests", { count: count.subtests }));
  if (count.packages > 0) parts.push(i18n.t("tests.selection.packages", { count: count.packages }));
  return { what: allOf(parts), total: count.tests + count.subtests + count.packages };
}

/** At the bottom while something is selected: how much, and running it. */
function SelectionBar({
  suite,
  selected,
  running,
  onRun,
  onCancel,
  onClear,
}: {
  suite: Suite;
  selected: ReadonlySet<string>;
  running: Running | null;
  onRun: (selection: RunSelection) => void;
  onCancel: () => void;
  onClear: () => void;
}) {
  const { t } = useTranslation();
  const { commands } = useTestsContext();
  const now = useNow(1000);
  const payload = useMemo(() => selectionPayload(suite, selected), [suite, selected]);
  const { what, total } = selectionText(suite, selected);
  const mine = running?.selection != null && JSON.stringify(running.selection) === JSON.stringify(payload);
  const bar = useRef<HTMLDivElement>(null);
  const runButton = useRef<HTMLButtonElement>(null);
  const cancelButton = useRef<HTMLButtonElement>(null);
  const wasMine = useRef(false);
  useEffect(() => {
    if (mine) {
      cancelButton.current?.focus();
    } else if (wasMine.current) {
      const focus = document.activeElement;
      if (focus === null || focus === document.body || bar.current?.contains(focus)) runButton.current?.focus();
    }
    wasMine.current = mine;
  }, [mine]);
  return (
    <div
      ref={bar}
      role="region"
      aria-label={t("tests.selection.label")}
      className="sticky bottom-0 z-10 col-span-full -mx-[max(28px,calc(50%-600px))] mt-6 flex items-center gap-2.5 border-t bg-card px-[max(28px,calc(50%-600px))] py-2.5 shadow-[0_-12px_32px_rgba(0,0,0,.18)]"
    >
      {mine && <Spin />}
      <p aria-live="polite" className="font-medium">
        {mine ? t("tests.selection.running", { what }) : t("tests.selection.selected", { count: total, what })}
      </p>
      {mine && running && <span className="font-mono text-xs text-muted-foreground">{clock(running.since, now.getTime())}</span>}
      <span className="flex-1" />
      {mine ? (
        <Button ref={cancelButton} variant="secondary" disabled={running?.cancelling} onClick={onCancel}>
          {t("common.actions.cancel")}
        </Button>
      ) : (
        <>
          <Button variant="secondary" onClick={onClear}>
            {t("tests.selection.clear")}
          </Button>
          <Button
            ref={runButton}
            disabled={running !== null}
            title={
              running !== null
                ? t("tests.busy")
                : `${t("tests.selection.commands")}\n${commands(payload).join("\n")}`
            }
            onClick={() => onRun(payload)}
          >
            {t("tests.selection.run")}
          </Button>
        </>
      )}
    </div>
  );
}
