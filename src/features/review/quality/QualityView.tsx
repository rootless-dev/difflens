import { Fragment, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight } from "lucide-react";

import { CodeLines } from "@/components/code-lines";
import { CopyPromptButton } from "@/components/copy-prompt-button";
import { ErrorNote, Hint } from "@/components/page";
import { PreparingAnalyzers } from "@/components/preparing-analyzers";
import { Segment, SegmentGroup } from "@/components/segment";
import { Button } from "@/components/ui/button";
import { WithCode } from "@/components/with-code";
import type { DiffTarget } from "@/features/review/flow/flow-facts";
import { noAnalyzerMessage, Spinner, unavailableHeading } from "@/features/review/flow/FlowView";
import {
  attention,
  checkStatus,
  checksCount,
  crossingText,
  formatScore,
  functionDraftBody,
  functionSignal,
  functionsPrompt,
  limitsText,
  type OverLimit,
  raceStatus,
  reviewCoverageCrossing,
  reviewCoverageText,
  type Tone,
} from "@/features/review/quality/quality-text";
import type { QualityState } from "@/features/review/quality/use-quality";
import {
  coverageRatio,
  coverageTitle,
  coverageTone,
  fileDiffTarget,
  notInReview,
  OutputBlock,
  PageSection,
  ReviewPage,
  TONE_TEXT,
  ToolProblems,
} from "@/features/review/ReviewPage";
import { CheckSeverityTag, DuplicateChip } from "@/features/review/signals/signal-marks";
import { duplicateKindText, duplicateSizeText, rangeText } from "@/features/review/signals/signals-text";
import { useNow } from "@/hooks/use-now";
import type {
  ChangedFile,
  CheckSeverity,
  Duplicate,
  DuplicateBlock,
  QualityFunction,
  QualityReport,
  SignalDraft,
  SignalDraftRequest,
  StaticCheck,
} from "@/lib/api";
import { coreText } from "@/lib/core-text";
import { formatNumber, formatPercent, formatRelativeTime } from "@/lib/format";
import { cn } from "@/lib/utils";

/** How the Quality tab writes and shows the review comment drafts of the functions over a limit (no model). */
export interface QualityDrafts {
  /** The draft written for this signal, while it is there. */
  draftOf: (signal: string) => SignalDraft | null;
  create: (request: SignalDraftRequest) => Promise<SignalDraft | null>;
  /** Brings a draft into view in the Diff tab. */
  view: (draft: SignalDraft) => void;
  busy: boolean;
}

interface QualityViewProps {
  state: QualityState;
  /** Files of the review, to open a function or a finding in the Diff tab. */
  files: readonly ChangedFile[];
  /** Null until the review state is read: no draft can be written yet. */
  drafts: QualityDrafts | null;
  onRetry: () => void;
  onViewDiff: (target: DiffTarget) => void;
  onOpenTests: () => void;
  onOpenDiff: () => void;
}

/**
 * Content of the Quality tab, in three views: the changed functions that cross a limit (the others folded away), the
 * static checks, and the duplicated code the branch introduces.
 */
export function QualityView({ state, files, drafts, onRetry, onViewDiff, onOpenTests, onOpenDiff }: QualityViewProps) {
  const { t } = useTranslation();
  if (state.status === "error") {
    return (
      <ReviewPage>
        <div className="col-span-full">
          <ErrorNote>{state.message}</ErrorNote>
          <Button variant="secondary" size="sm" className="mt-2.5" onClick={onRetry}>
            {t("common.actions.tryAgain")}
          </Button>
        </div>
      </ReviewPage>
    );
  }
  if (state.status !== "ready") {
    return (
      <ReviewPage>
        <PreparingAnalyzers
          className="col-span-full"
          fallback={<Measuring startedAt={state.status === "loading" ? state.startedAt : null} />}
        />
      </ReviewPage>
    );
  }
  const { result, pending } = state;
  if (result.kind === "unavailable") {
    return (
      <ReviewPage>
        <div className="col-span-full max-w-[620px]">
          {result.reason === "no_analyzer" ? (
            <>
              <p>{noAnalyzerMessage(result.languages)}</p>
              <Button variant="secondary" size="sm" className="mt-2.5" onClick={onOpenDiff}>
                {t("flow.view.openDiff")}
              </Button>
            </>
          ) : (
            <>
              <ErrorNote>{unavailableHeading(result)}</ErrorNote>
              {/* The analyzer's own output, as it wrote it. */}
              <OutputBlock text={coreText(result.message)} label={t("quality.analyzerOutput")} />
              <Button variant="secondary" size="sm" onClick={onRetry}>
                {t("common.actions.tryAgain")}
              </Button>
            </>
          )}
        </div>
      </ReviewPage>
    );
  }
  return (
    <Report
      report={result.report}
      pending={pending}
      files={files}
      drafts={drafts}
      onViewDiff={onViewDiff}
      onOpenTests={onOpenTests}
    />
  );
}

function Measuring({ startedAt }: { startedAt: number | null }) {
  const { t } = useTranslation();
  const now = useNow(1000);
  const seconds = startedAt === null ? null : Math.max(0, Math.floor((now.getTime() - startedAt) / 1000));
  return (
    <div role="status" aria-label={t("quality.measuring.label")} className="col-span-full max-w-[620px]">
      <p className="flex items-center gap-2">
        <Spinner />
        {t("quality.measuring.text")}
        {seconds !== null && <span className="font-mono text-xs text-muted-foreground">{seconds} s</span>}
      </p>
      <p className="mt-1.5 text-xs text-faint">{t("quality.measuring.hint")}</p>
    </div>
  );
}

interface ReportProps {
  report: QualityReport;
  pending: boolean;
  files: readonly ChangedFile[];
  drafts: QualityDrafts | null;
  onViewDiff: (target: DiffTarget) => void;
  onOpenTests: () => void;
}

const TONE: Record<Tone, string> = { ok: "text-add", bad: "text-del", faint: "text-faint" };

type QualityPane = "functions" | "checks" | "duplication";

function Report({ report, pending, files, drafts, onViewDiff, onOpenTests }: ReportProps) {
  const { t } = useTranslation();
  // The view shown stays while the tab is kept mounted, across reports.
  const [pane, setPane] = useState<QualityPane>("functions");
  const parts = attention(report);
  const counts: Record<QualityPane, number> = {
    functions: parts.over.length,
    checks: checksCount(report),
    duplication: report.duplicates.length,
  };
  const panes: { pane: QualityPane; label: string; title: string }[] = [
    {
      pane: "functions",
      label: t("quality.views.functions", { count: counts.functions }),
      title: t("quality.views.functionsTitle", { count: counts.functions }),
    },
    {
      pane: "checks",
      label: t("quality.views.checks", { count: counts.checks }),
      title: t("quality.views.checksTitle", { count: counts.checks }),
    },
    {
      pane: "duplication",
      label: t("quality.views.duplication", { count: counts.duplication }),
      title: t("quality.views.duplicationTitle", { count: counts.duplication }),
    },
  ];
  return (
    <ReviewPage aria-busy={pending || undefined}>
      <div className="col-span-full flex flex-wrap items-center gap-3">
        <SegmentGroup label={t("quality.views.label")}>
          {panes.map((item) => (
            <Segment
              key={item.pane}
              className="py-[3px]"
              pressed={pane === item.pane}
              title={item.title}
              onClick={() => setPane(item.pane)}
            >
              {item.label}
            </Segment>
          ))}
        </SegmentGroup>
        {pending && (
          <p role="status" className="flex items-center gap-2 text-xs text-muted-foreground">
            <Spinner />
            {t("quality.updating")}
          </p>
        )}
      </div>
      <div className={cn("col-span-full mt-3", pending && "opacity-60")}>
        {pane === "functions" && (
          <FunctionsPane
            report={report}
            over={parts.over}
            within={parts.within}
            files={files}
            drafts={drafts}
            onViewDiff={onViewDiff}
            onOpenTests={onOpenTests}
          />
        )}
        {pane === "checks" && (
          <PageSection first title={t("quality.checks.title")} className="max-w-[760px]">
            <OtherChecks report={report} files={files} onViewDiff={onViewDiff} onOpenTests={onOpenTests} />
          </PageSection>
        )}
        {pane === "duplication" && <DuplicationPane report={report} files={files} onViewDiff={onViewDiff} />}
      </div>
      <ToolProblems diagnostics={report.diagnostics} what={t("quality.toolProblems")} />
    </ReviewPage>
  );
}

const ROW = "grid grid-cols-[minmax(0,1fr)_88px_104px_72px] items-center gap-3 px-2 text-right";

interface FunctionsPaneProps {
  report: QualityReport;
  over: OverLimit[];
  within: QualityFunction[];
  files: readonly ChangedFile[];
  drafts: QualityDrafts | null;
  onViewDiff: (target: DiffTarget) => void;
  onOpenTests: () => void;
}

/** The functions over a limit, each with what it crosses and its actions; the others in one folded row. */
function FunctionsPane({ report, over, within, files, drafts, onViewDiff, onOpenTests }: FunctionsPaneProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const withinId = useId();
  const review = reviewCoverageCrossing(report);
  const row = (fn: QualityFunction) => (
    <FunctionRow fn={fn} report={report} target={fileDiffTarget(fn.file, fn.startLine, files)} onViewDiff={onViewDiff} />
  );
  return (
    <PageSection first title={t("quality.functions.title")}>
      {report.functions.length === 0 ? (
        <p className="text-muted-foreground">{t("quality.functions.none")}</p>
      ) : (
        <div>
          <FunctionHeader report={report} />
          {over.length === 0 ? (
            <p className="border-b px-2 py-[11px] text-add">{t("quality.functions.allWithin")}</p>
          ) : (
            <ul aria-label={t("quality.functions.listLabel")}>
              {over.map((item) => (
                <li key={item.fn.id} className="border-b border-l-2 border-l-del">
                  {row(item.fn)}
                  <OverLimitDetail item={item} drafts={drafts} />
                </li>
              ))}
            </ul>
          )}
          {within.length > 0 && (
            <>
              <button
                type="button"
                aria-expanded={open}
                aria-controls={withinId}
                onClick={() => setOpen((value) => !value)}
                className="flex w-full cursor-pointer items-center gap-1.5 border-b px-2 py-[9px] text-left text-muted-foreground outline-none hover:bg-card hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
              >
                <ChevronRight aria-hidden="true" className={cn("size-3.5 transition-transform", open && "rotate-90")} />
                {t("quality.functions.within", { count: within.length })}
              </button>
              <ul id={withinId} hidden={!open} aria-label={t("quality.functions.withinLabel")}>
                {open && within.map((fn) => <li key={fn.id}>{row(fn)}</li>)}
              </ul>
            </>
          )}
          {over.length > 1 && <ListActions over={over} drafts={drafts} />}
        </div>
      )}
      {review && <p className="mt-3 border-l-2 border-l-del pl-3">{reviewCoverageText(review)}</p>}
      <CoverageNote report={report} onOpenTests={onOpenTests} />
      <Hint>{limitsText(report)}</Hint>
    </PageSection>
  );
}

function FunctionHeader({ report }: { report: QualityReport }) {
  const { t } = useTranslation();
  return (
    <div aria-hidden="true" className={cn(ROW, "items-end border-b pb-[9px] text-muted-foreground")}>
      <span className="text-left">{t("quality.functions.function")}</span>
      <span>
        {t("quality.functions.complexity")}
        <small className="block text-[11px] text-faint">
          {t("quality.functions.limit", { value: formatScore(report.limits.complexity) })}
        </small>
      </span>
      <span>
        {t("quality.functions.coverage")}
        <small className="block text-[11px] text-faint">{t("quality.functions.changedLines")}</small>
      </span>
      <span>
        {t("quality.functions.crap")}
        <small className="block text-[11px] text-faint">
          {t("quality.functions.limit", { value: formatScore(report.limits.crap) })}
        </small>
      </span>
    </div>
  );
}

/**
 * Why Coverage and CRAP read as they do: from which run, or why they are empty (no run, a run without coverage, a
 * run whose analyzer could not measure it, in its own words), with the way to the Tests tab.
 */
function CoverageNote({ report, onOpenTests }: { report: QualityReport; onOpenTests: () => void }) {
  const { t } = useTranslation();
  const { coverage } = report;
  const runTests = (
    <Button variant="link" size="inline" className="text-xs text-primary" onClick={onOpenTests}>
      {t("quality.functions.runTests")}
    </Button>
  );
  if (coverage.state === "measured" && report.testRunAt !== null) {
    return <Hint>{t("quality.functions.coverageNote.measured", { when: formatRelativeTime(report.testRunAt) })}</Hint>;
  }
  if (coverage.state === "no_run" || report.testRunAt === null) {
    return (
      <Hint>
        {t("quality.functions.coverageNote.noRun")} {runTests}
      </Hint>
    );
  }
  if (coverage.state === "off") {
    return (
      <p role="note" className="mt-3 text-xs text-muted-foreground">
        {t("quality.functions.coverageNote.off")} {runTests}
      </p>
    );
  }
  return (
    <div role="note" className="mt-3 text-xs text-muted-foreground">
      <p>
        {coverage.reasons.length > 0
          ? t("quality.functions.coverageNote.notMeasured")
          : t("quality.functions.coverageNote.notMeasuredNoReason")}{" "}
        {runTests}
      </p>
      {coverage.reasons.length > 0 && (
        // The analyzer's own words, as it wrote them.
        <OutputBlock
          text={coverage.reasons.join("\n")}
          label={t("quality.functions.coverageNote.reasons")}
          className="my-1.5"
        />
      )}
    </div>
  );
}

function FunctionRow({
  fn,
  report,
  target,
  onViewDiff,
}: {
  fn: QualityFunction;
  report: QualityReport;
  target: DiffTarget | null;
  onViewDiff: (target: DiffTarget) => void;
}) {
  const { t } = useTranslation();
  const over = (kind: "complexity" | "crap" | "changed_lines_coverage") =>
    report.crossed.some((limit) => limit.kind === kind && limit.function?.id === fn.id);
  const overComplexity = over("complexity");
  const overCrap = over("crap");
  const underCoverage = over("changed_lines_coverage");
  const ratio = fn.changedLines ? coverageRatio(fn.changedLines) : null;
  const complexity = fn.complexity === null ? "—" : formatScore(fn.complexity);
  const coverage = ratio === null ? "—" : formatPercent(ratio);
  const crap = fn.crap === null ? "—" : formatScore(fn.crap);
  const label = [
    fn.qualifiedName,
    overComplexity
      ? t("quality.functions.row.complexityOver", { value: complexity })
      : t("quality.functions.row.complexity", { value: complexity }),
    underCoverage
      ? t("quality.functions.row.coverageUnder", { value: coverage })
      : t("quality.functions.row.coverage", { value: coverage }),
    overCrap ? t("quality.functions.row.crapOver", { value: crap }) : t("quality.functions.row.crap", { value: crap }),
  ].join(", ");
  const complexityTitle =
    fn.previousComplexity !== null && fn.previousComplexity !== fn.complexity
      ? t("quality.functions.before", { value: formatScore(fn.previousComplexity) })
      : fn.state === "new"
        ? t("quality.functions.newInBranch")
        : undefined;
  return (
    <button
      type="button"
      aria-label={label}
      title={
        target ? t("quality.functions.openInDiff", { location: `${fn.file}:${fn.startLine}` }) : notInReview()
      }
      disabled={target === null}
      onClick={() => target && onViewDiff(target)}
      className={cn(
        ROW,
        "w-full cursor-pointer py-[9px] font-mono text-xs outline-none hover:bg-card focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring disabled:cursor-default",
      )}
    >
      <code className="truncate text-left">{fn.qualifiedName}</code>
      <span title={complexityTitle} className={cn(overComplexity && "text-del")}>
        {complexity}
      </span>
      <span
        title={fn.changedLines ? coverageTitle(fn.changedLines) : undefined}
        className={cn(
          ratio === null
            ? "text-faint"
            : underCoverage
              ? "text-del"
              : TONE_TEXT[coverageTone(ratio, report.limits.changedLinesCoverage)],
        )}
      >
        {coverage}
      </span>
      <span className={cn(fn.crap === null && "text-faint", overCrap && "text-del")}>{crap}</span>
    </button>
  );
}

/** Under a function over a limit: the limits it crosses against their values, and what can be done about it. */
function OverLimitDetail({ item, drafts }: { item: OverLimit; drafts: QualityDrafts | null }) {
  const { t } = useTranslation();
  const name = item.fn.qualifiedName;
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 px-2 pb-2.5 text-xs">
      <p className="mr-auto text-muted-foreground">
        <span className="text-faint">{t("quality.functions.crosses")}:</span>{" "}
        {item.crossings.map((limit, index) => (
          <Fragment key={limit.kind}>
            {index > 0 && <span className="text-faint"> · </span>}
            <span className="text-foreground">{crossingText(limit)}</span>
          </Fragment>
        ))}
      </p>
      <CommentButton item={item} drafts={drafts} />
      <CopyPromptButton
        variant="secondary"
        size="xs"
        text={() => functionsPrompt([item])}
        label={t("quality.functions.actions.copyPrompt")}
        title={t("quality.functions.actions.copyPromptTitle", { name })}
        failedLabel={t("quality.functions.actions.copyFailed")}
      />
    </div>
  );
}

function draftRequest(item: OverLimit): SignalDraftRequest {
  return {
    signal: functionSignal(item.fn),
    path: item.fn.file,
    line: item.fn.startLine,
    endLine: item.fn.endLine,
    body: functionDraftBody(item),
  };
}

/** Writes the function's review comment draft, or once there is one, shows it: never a second one. */
function CommentButton({ item, drafts }: { item: OverLimit; drafts: QualityDrafts | null }) {
  const { t } = useTranslation();
  const name = item.fn.qualifiedName;
  const draft = drafts?.draftOf(functionSignal(item.fn)) ?? null;
  if (draft && drafts) {
    return (
      <Button
        variant="secondary"
        size="xs"
        title={t("quality.functions.actions.viewDraftTitle", { name })}
        onClick={() => drafts.view(draft)}
      >
        {t("quality.functions.actions.viewDraft")}
      </Button>
    );
  }
  return (
    <Button
      variant="secondary"
      size="xs"
      disabled={drafts === null || drafts.busy}
      title={t("quality.functions.actions.commentTitle", { name })}
      onClick={() => void drafts?.create(draftRequest(item))}
    >
      {t("quality.functions.actions.comment")}
    </Button>
  );
}

/** For every function over a limit at once: a draft on each that has none, and one prompt for them all. */
function ListActions({ over, drafts }: { over: OverLimit[]; drafts: QualityDrafts | null }) {
  const { t } = useTranslation();
  const [writing, setWriting] = useState(false);
  const missing = drafts ? over.filter((item) => drafts.draftOf(functionSignal(item.fn)) === null) : over;
  const commentAll = async () => {
    if (!drafts) return;
    setWriting(true);
    try {
      // One after the other: each save reads the review state again before the next.
      for (const item of missing) await drafts.create(draftRequest(item));
    } finally {
      setWriting(false);
    }
  };
  return (
    <div className="mt-3 flex flex-wrap items-center gap-2.5">
      <Button
        variant="secondary"
        size="sm"
        disabled={drafts === null || drafts.busy || writing || missing.length === 0}
        title={t("quality.functions.actions.commentAllTitle")}
        onClick={() => void commentAll()}
      >
        {missing.length === 0 ? t("quality.functions.actions.allCommented") : t("quality.functions.actions.commentAll")}
      </Button>
      <CopyPromptButton
        variant="secondary"
        size="sm"
        text={() => functionsPrompt(over)}
        label={t("quality.functions.actions.copyAll")}
        title={t("quality.functions.actions.copyAllTitle")}
        failedLabel={t("quality.functions.actions.copyFailed")}
      />
    </div>
  );
}

/** Production code's copies, counted; the tests' own copies apart, folded away and not counted. */
function DuplicationPane({
  report,
  files,
  onViewDiff,
}: {
  report: QualityReport;
  files: readonly ChangedFile[];
  onViewDiff: (target: DiffTarget) => void;
}) {
  const { t } = useTranslation();
  const [testsOpen, setTestsOpen] = useState(false);
  const testsId = useId();
  const tests = report.testDuplicates;
  return (
    <>
      <PageSection first title={t("quality.duplication.title")}>
        {report.duplicates.length === 0 ? (
          <p className="text-add">{t("quality.duplication.none")}</p>
        ) : (
          <Duplicates
            duplicates={report.duplicates}
            label={t("quality.duplication.listLabel")}
            files={files}
            onViewDiff={onViewDiff}
          />
        )}
        <Hint>
          {t("quality.duplication.summary", {
            tokens: formatNumber(report.limits.duplicationTokens),
            lines: formatNumber(report.limits.duplicationLines),
          })}
        </Hint>
      </PageSection>
      <section className="mt-6">
        {tests.length === 0 ? (
          <p className="text-xs text-faint">{t("quality.duplication.testsNone")}</p>
        ) : (
          <>
            <button
              type="button"
              aria-expanded={testsOpen}
              aria-controls={testsId}
              onClick={() => setTestsOpen((value) => !value)}
              className="flex cursor-pointer items-center gap-1.5 rounded-sm text-sm font-medium text-muted-foreground outline-none hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
            >
              <ChevronRight aria-hidden="true" className={cn("size-3.5 transition-transform", testsOpen && "rotate-90")} />
              {t("quality.duplication.tests", { count: tests.length })}
            </button>
            <div id={testsId} hidden={!testsOpen} className="mt-2.5">
              <Hint className="mt-0 mb-2.5">{t("quality.duplication.testsHint")}</Hint>
              {testsOpen && (
                <Duplicates
                  duplicates={tests}
                  label={t("quality.duplication.testsListLabel")}
                  files={files}
                  onViewDiff={onViewDiff}
                />
              )}
            </div>
          </>
        )}
      </section>
    </>
  );
}

/** Each duplicate with its new code beside what it copies, or beside its other times in the change. */
function Duplicates({
  duplicates,
  label,
  files,
  onViewDiff,
}: {
  duplicates: readonly Duplicate[];
  label: string;
  files: readonly ChangedFile[];
  onViewDiff: (target: DiffTarget) => void;
}) {
  const { t } = useTranslation();
  return (
    <ul aria-label={label} className="grid gap-6 border-t pt-4">
      {duplicates.map((duplicate) => {
        const { block } = duplicate;
        const caption =
          duplicate.kind === "copies_existing" ? t("quality.duplication.existing") : t("quality.duplication.repeated");
        return (
          <li
            key={`${duplicate.kind}:${block.file}:${block.startLine}:${block.endLine}`}
            data-duplicate={duplicate.kind}
            className="grid gap-2.5"
          >
            <p className="flex flex-wrap items-center gap-x-2.5 gap-y-1">
              <DuplicateChip />
              <strong className="font-medium">
                <WithCode text={duplicateKindText(duplicate)} />
              </strong>
              <span className="text-xs text-faint">{duplicateSizeText(duplicate)}</span>
            </p>
            <div className="grid grid-cols-2 items-start gap-4">
              <DuplicateSide
                block={block}
                caption={t("quality.duplication.newCode")}
                files={files}
                onViewDiff={onViewDiff}
              />
              <div className="grid gap-3">
                {duplicate.others.map((other) => (
                  <DuplicateSide
                    key={`${other.file}:${other.startLine}`}
                    block={other}
                    caption={caption}
                    files={files}
                    onViewDiff={onViewDiff}
                  />
                ))}
              </div>
            </div>
          </li>
        );
      })}
    </ul>
  );
}

/** One occurrence: what it is, where (opening the Diff when the file is in the review), and its code. */
function DuplicateSide({
  block,
  caption,
  files,
  onViewDiff,
}: {
  block: DuplicateBlock;
  caption: string;
  files: readonly ChangedFile[];
  onViewDiff: (target: DiffTarget) => void;
}) {
  const { t } = useTranslation();
  const where = rangeText(block);
  const target = fileDiffTarget(block.file, block.startLine, files);
  const left = block.endLine - block.startLine + 1 - block.code.length;
  return (
    <figure className="m-0 grid min-w-0 gap-1">
      <figcaption className="flex min-w-0 items-baseline gap-2 text-xs">
        <span className="text-muted-foreground">{caption}</span>
        {target ? (
          <button
            type="button"
            onClick={() => onViewDiff(target)}
            title={t("quality.duplication.openInDiff", { location: where })}
            className="min-w-0 cursor-pointer truncate rounded-sm font-mono text-primary outline-none hover:underline focus-visible:outline-2 focus-visible:outline-ring"
          >
            {where}
          </button>
        ) : (
          <code title={notInReview()} className="min-w-0 truncate text-muted-foreground">
            {where}
          </code>
        )}
      </figcaption>
      {block.code.length > 0 ? (
        <CodeLines
          label={t("quality.duplication.codeAt", { where })}
          lines={block.code.map((text, index) => ({ line: block.startLine + index, text }))}
          className="max-h-[360px]"
        />
      ) : (
        <p className="text-xs text-faint">{t("quality.duplication.unreadable")}</p>
      )}
      {block.code.length > 0 && left > 0 && (
        <p className="text-xs text-faint">{t("quality.duplication.more", { count: left })}</p>
      )}
    </figure>
  );
}

function OtherChecks({
  report,
  files,
  onViewDiff,
  onOpenTests,
}: {
  report: QualityReport;
  files: readonly ChangedFile[];
  onViewDiff: (target: DiffTarget) => void;
  onOpenTests: () => void;
}) {
  const { t } = useTranslation();
  const races = raceStatus(report);
  return (
    <dl className="grid grid-cols-[1fr_auto] gap-x-4 gap-y-[9px] border-t pt-3">
      {report.checks.map((check) => (
        <Check key={check.tool} check={check} files={files} onViewDiff={onViewDiff} />
      ))}
      <dt className="text-muted-foreground">{t("quality.checks.raceDetector")}</dt>
      <dd className={cn("text-right", TONE[races.tone])}>
        {races.text}
        {report.raceDetector === "races" && (
          <Button variant="link" size="inline" className="ml-2 text-primary" onClick={onOpenTests}>
            {t("quality.checks.seeTests")}
          </Button>
        )}
      </dd>
      {races.note && <dd className="col-span-full -mt-1.5 text-xs text-faint">{races.note}</dd>}
    </dl>
  );
}

/** The rule beside a finding: red for an error, as before severities; quieter as the tool says it matters less. */
const SEVERITY_BORDER: Record<CheckSeverity, string> = {
  error: "border-l-del",
  warning: "border-l-mod",
  info: "border-l-border",
};

function Check({
  check,
  files,
  onViewDiff,
}: {
  check: StaticCheck;
  files: readonly ChangedFile[];
  onViewDiff: (target: DiffTarget) => void;
}) {
  const { t } = useTranslation();
  const status = checkStatus(check);
  return (
    <Fragment>
      <dt className="text-muted-foreground">{check.tool}</dt>
      <dd className={cn("text-right", TONE[status.tone])}>{status.text}</dd>
      {check.status === "findings" && (
        <dd className="col-span-full mb-1.5">
          {/* Each finding's message and rule are the checker's own words. */}
          <ul aria-label={t("quality.checks.findingsOf", { tool: check.tool })} className="grid gap-1.5">
            {check.findings.map((finding, index) => {
              const target = fileDiffTarget(finding.file, finding.line, files);
              const where = `${finding.file}:${finding.line}`;
              return (
                <li key={index} className={cn("grid gap-0.5 border-l-2 pl-3", SEVERITY_BORDER[finding.severity])}>
                  <span className="flex flex-wrap items-center gap-2">
                    {target ? (
                      <button
                        type="button"
                        onClick={() => onViewDiff(target)}
                        title={t("quality.checks.openInDiff")}
                        className="cursor-pointer justify-self-start rounded-sm text-left font-mono text-xs text-primary outline-none hover:underline focus-visible:outline-2 focus-visible:outline-ring"
                      >
                        {where}
                      </button>
                    ) : (
                      <code title={notInReview()} className="text-muted-foreground">
                        {where}
                      </code>
                    )}
                    <CheckSeverityTag severity={finding.severity} />
                  </span>
                  <span>
                    {finding.message}
                    {finding.rule && <span className="ml-2 font-mono text-xs text-faint">{finding.rule}</span>}
                  </span>
                </li>
              );
            })}
          </ul>
        </dd>
      )}
      {check.message && check.status !== "findings" && (
        <dd className={cn("col-span-full -mt-1.5 text-xs", check.status === "failed" ? "text-muted-foreground" : "text-faint")}>
          {check.status === "failed" ? (
            <OutputBlock
              text={check.message}
              label={t("quality.checks.outputOf", { tool: check.tool })}
              className="my-1"
            />
          ) : (
            check.message
          )}
        </dd>
      )}
    </Fragment>
  );
}
