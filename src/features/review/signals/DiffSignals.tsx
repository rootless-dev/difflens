// The review's signals inside the Diff: the summary in a file's header, the marker of a function over a limit and the
// note on one no test runs, the panel of its limits, and the blocks of the model's findings, of static checks, of
// duplicated code and of mutants that survived under their lines, each able to start a comment draft.

import { use, useState } from "react";
import { useTranslation } from "react-i18next";

import { ErrorNote } from "@/components/page";
import { Button } from "@/components/ui/button";
import { WithCode } from "@/components/with-code";
import { shownFinding } from "@/features/review/ai-text/versions";
import {
  CheckSeverityTag,
  DuplicateChip,
  GaugeMark,
  LimitChip,
  MeasuredTag,
  ModelTag,
  MutantChip,
  NoTestMark,
  SeverityChip,
  ToolChip,
  UntestedChip,
} from "@/features/review/signals/signal-marks";
import {
  checkSignal,
  duplicateSignal,
  type FileSignals,
  findingSignal,
  type FunctionSignals,
  mutantSignal,
  SignalsContext,
} from "@/features/review/signals/signals";
import {
  checkDraftBody,
  duplicateDraftBody,
  duplicateKindText,
  duplicateSizeText,
  findingPlace,
  limitPill,
  limitRows,
  mutantChangeText,
  mutantDraftBody,
  operatorName,
} from "@/features/review/signals/signals-text";
import type { FindingComments } from "@/features/review/signals/use-finding-comments";
import { CopyMutantPrompt } from "@/features/review/tests/mutant-parts";
import { useAppLanguage } from "@/i18n/language";
import type {
  Duplicate,
  Finding,
  LocatedCheck,
  Mutant,
  QualityLimits,
  SignalDraft,
  SignalDraftRequest,
} from "@/lib/api";
import { formatDate, formatPercent } from "@/lib/format";
import { cn } from "@/lib/utils";

/** "▮▮▮ 4 findings", "◠ 3 limits", "🔧 1 check", "⚲ 1 function without a test": what the file holds, in its header. */
export function FileSignalSummary({ file }: { file: FileSignals | undefined }) {
  const { t } = useTranslation();
  if (!file) return null;
  const severity = file.findings[0]?.severity ?? null;
  const limits = file.functions.reduce((sum, fn) => sum + fn.limits.length, 0);
  const untested = file.functions.filter((fn) => fn.untested).length;
  const duplicates = file.duplicates.length;
  const mutants = file.mutants.length;
  if (
    severity === null &&
    limits === 0 &&
    file.checks.length === 0 &&
    duplicates === 0 &&
    mutants === 0 &&
    untested === 0
  ) {
    return null;
  }
  return (
    <span data-file-signals="" className="flex flex-none items-center gap-2">
      {severity && <SeverityChip severity={severity}>{t("signals.chip.findings", { count: file.findings.length })}</SeverityChip>}
      {limits > 0 && <LimitChip>{t("signals.chip.limits", { count: limits })}</LimitChip>}
      {file.checks.length > 0 && <ToolChip>{t("signals.chip.checks", { count: file.checks.length })}</ToolChip>}
      {duplicates > 0 && <DuplicateChip>{t("signals.chip.duplicates", { count: duplicates })}</DuplicateChip>}
      {mutants > 0 && <MutantChip>{t("signals.chip.mutants", { count: mutants })}</MutantChip>}
      {untested > 0 && <UntestedChip>{t("signals.chip.untestedFunctions", { count: untested })}</UntestedChip>}
    </span>
  );
}

/**
 * At the end of a function's first line: "◠ CC 14 · CRAP 42 · coverage 48%" with only the limits crossed, which opens
 * the panel of its limits; and, dashed, "No test runs this function", whose detail is its tooltip.
 */
export function FunctionMarkers({
  signals,
  limits,
  expanded,
  onToggle,
}: {
  signals: FunctionSignals;
  limits: QualityLimits | null;
  expanded: boolean;
  onToggle: () => void;
}) {
  const { t } = useTranslation();
  const pill = signals.limits.length > 0 ? limitPill(signals) : null;
  const marker =
    "inline-flex items-center gap-1.5 rounded-[9px] border bg-card px-2 font-sans text-[11px] leading-[18px] whitespace-nowrap text-muted-foreground";
  return (
    <>
      {pill && (
        <button
          type="button"
          aria-expanded={expanded}
          aria-label={pill.label}
          title={pill.title}
          onClick={onToggle}
          className={cn(marker, "cursor-pointer outline-none hover:border-faint focus-visible:outline-2 focus-visible:outline-ring")}
        >
          <GaugeMark />
          {pill.parts.map((part, index) => (
            <span key={part.name}>
              {index > 0 && "· "}
              {part.name} <b className="font-mono font-semibold text-foreground">{part.value}</b>
            </span>
          ))}
        </button>
      )}
      {signals.untested && (
        <span
          title={t("signals.diff.untestedTitle", {
            count: signals.fn.changedLines?.coverable ?? 0,
            minimum: formatPercent(limits?.changedLinesCoverage ?? 0.8),
          })}
          className={cn(marker, "cursor-default border-dashed")}
        >
          <NoTestMark />
          {t("signals.diff.untested")}
        </span>
      )}
    </>
  );
}

const BLOCK = "my-1.5 mr-4 ml-4 max-w-[720px] rounded-[8px] border font-sans text-sm whitespace-normal text-foreground";

/** A function's limits under its first line, with the way to the Quality tab. */
export function LimitPanel({
  signals,
  limits,
  onOpenQuality,
}: {
  signals: FunctionSignals;
  limits: QualityLimits;
  onOpenQuality: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div role="group" aria-label={t("signals.diff.panelTitle", { name: signals.fn.name, count: signals.limits.length })} className={cn(BLOCK, "max-w-[560px] bg-card px-3 py-2.5")}>
      <header className="mb-2 flex items-center gap-2">
        <MeasuredTag />
        <strong className="font-medium">
          {t("signals.diff.panelTitle", { name: signals.fn.name, count: signals.limits.length })}
        </strong>
      </header>
      <LimitList signals={signals} limits={limits} />
      <footer className="mt-2">
        <Button variant="link" size="inline" className="text-xs text-primary" onClick={onOpenQuality}>
          {t("signals.diff.openQuality")}
        </Button>
      </footer>
    </div>
  );
}

/** The rows of a function's limits: the crossed ones with the gauge (or the flask) and the value in red. */
export function LimitList({ signals, limits }: { signals: FunctionSignals; limits: QualityLimits }) {
  return (
    <dl className="grid grid-cols-[14px_minmax(0,1fr)_auto] items-baseline gap-x-2 gap-y-1.5">
      {limitRows(signals, limits).map((row) => (
        <div key={row.kind} data-limit={row.kind} data-over={row.over || undefined} className="contents">
          <dt className="contents">
            <span className="self-center">
              {row.over && (row.kind === "untested" ? <NoTestMark /> : <GaugeMark />)}
            </span>
            <span className="text-muted-foreground">
              {row.label}
              {row.note && <small className="block text-[11px] text-faint">{row.note}</small>}
            </span>
          </dt>
          <dd className="text-right font-mono text-xs whitespace-nowrap">
            <b className={cn("font-normal", row.over && "font-semibold text-del")}>{row.value}</b>{" "}
            <span className="font-sans text-faint">{row.bound}</span>
          </dd>
        </div>
      ))}
    </dl>
  );
}

/** How a block starts or shows its comment draft. */
export interface DraftControl {
  /** The draft written from this signal, while it is there. */
  draftOf: (signal: string) => SignalDraft | null;
  create: (request: SignalDraftRequest) => void;
  view: (draft: SignalDraft) => void;
  busy: boolean;
  /** How a model finding's comment is written by the model; without it a finding offers no draft. */
  finding: FindingComments | null;
}

const DRAFT_BUTTON =
  "cursor-pointer rounded-[6px] border bg-card px-2.5 py-1 text-xs font-medium whitespace-nowrap outline-none focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-45";

function DraftButton({
  signal,
  request,
  control,
  tone,
}: {
  signal: string;
  request: () => SignalDraftRequest;
  control: DraftControl | null;
  tone: "ai" | "ink";
}) {
  const { t } = useTranslation();
  if (!control) return null;
  const draft = control.draftOf(signal);
  return (
    <button
      type="button"
      disabled={control.busy && draft === null}
      onClick={() => (draft ? control.view(draft) : control.create(request()))}
      className={cn(
        DRAFT_BUTTON,
        draft
          ? "text-foreground"
          : tone === "ai"
            ? "border-ai/45 text-ai hover:border-ai"
            : "text-foreground hover:border-faint",
      )}
    >
      {draft ? t("signals.diff.viewDraft") : t("signals.diff.createDraft")}
    </button>
  );
}

/**
 * A model finding's comment: Write comment (a model call, only on click) while the finding has no draft; while it is
 * written, a spinner and Stop; once there, See draft, which asks nothing. Why the last call failed shows under it,
 * the button then trying again.
 */
function FindingDraftAction({ findingId, signal, control }: { findingId: string; signal: string; control: DraftControl | null }) {
  const { t } = useTranslation();
  const comments = control?.finding;
  if (!control || !comments) return null;
  const draft = control.draftOf(signal);
  if (draft) {
    return (
      <button type="button" onClick={() => control.view(draft)} className={cn(DRAFT_BUTTON, "text-foreground")}>
        {t("signals.diff.viewDraft")}
      </button>
    );
  }
  const run = comments.runOf(findingId);
  if (run) {
    return (
      <span role="status" className="flex items-center gap-2 text-xs text-muted-foreground">
        <span
          aria-hidden="true"
          className="inline-block size-3 flex-none animate-spin rounded-full border-[1.6px] border-ai/25 border-t-ai motion-reduce:[animation-duration:3s]"
        />
        {run.stopping ? t("signals.diff.stopping") : t("signals.diff.writingComment")}
        <Button
          variant="link"
          size="inline"
          className="text-xs text-primary"
          disabled={run.stopping}
          onClick={() => comments.cancel(findingId)}
        >
          {t("signals.diff.stop")}
        </Button>
      </span>
    );
  }
  const failure = comments.errorOf(findingId);
  return (
    <>
      <button
        type="button"
        title={t("signals.diff.writeCommentTitle")}
        disabled={control.busy}
        onClick={() => comments.write(findingId)}
        className={cn(DRAFT_BUTTON, "border-ai/45 text-ai hover:border-ai")}
      >
        {failure ? t("common.actions.tryAgain") : t("signals.diff.writeComment")}
      </button>
      {failure && (
        <ErrorNote className="basis-full text-xs">
          {t("signals.diff.draftFailed", { message: failure.message })}
          {failure.needsModel && comments.onOpenPreferences && (
            <>
              {" "}
              <Button variant="link" size="inline" className="text-xs text-primary" onClick={comments.onOpenPreferences}>
                {t("signals.diff.openPreferences")}
              </Button>
            </>
          )}
        </ErrorNote>
      )}
    </>
  );
}

/**
 * A model finding under the last line it cites: the comment card's size, the explanation's tint, the Model tag, its
 * severity, title and lines, folding to one line; open, what it explains, the direction it suggests and a draft.
 */
export function FindingBlock({
  finding,
  open,
  onToggle,
  draft,
}: {
  finding: Finding;
  open: boolean;
  onToggle: () => void;
  draft: DraftControl | null;
}) {
  const { t } = useTranslation();
  const language = useAppLanguage();
  const { onOpenInsights } = use(SignalsContext);
  const shown = shownFinding(finding, language);
  const signal = findingSignal(finding);
  const titleId = `finding-${finding.id}`;
  return (
    <div
      role="group"
      aria-labelledby={titleId}
      data-finding={finding.id}
      data-open={open}
      className={cn(BLOCK, "border-ai/30 border-l-2 border-l-ai bg-ai-bg px-3", open ? "py-2.5" : "py-1.5")}
    >
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <ModelTag />
        <SeverityChip severity={finding.severity} />
        <strong id={titleId} className="font-medium">
          <WithCode text={shown.title} />
        </strong>
        <span className="text-xs text-faint">{findingPlace(finding)}</span>
        <Button variant="link" size="inline" aria-expanded={open} className="ml-auto text-xs text-primary" onClick={onToggle}>
          {open ? t("signals.diff.collapse") : t("signals.diff.show")}
        </Button>
      </div>
      {open && (
        <div className="mt-1.5">
          <p className="max-w-[78ch]">
            <WithCode text={shown.explanation} />
          </p>
          {shown.suggestedDirection.trim() !== "" && (
            <dl className="mt-2 grid grid-cols-[120px_minmax(0,1fr)] gap-x-3.5 gap-y-1.5">
              <dt className="text-muted-foreground">{t("signals.diff.suggested")}</dt>
              <dd className="max-w-[70ch]">
                <WithCode text={shown.suggestedDirection} />
              </dd>
            </dl>
          )}
          <div className="mt-2.5 flex flex-wrap items-center gap-x-3.5 gap-y-2">
            <FindingDraftAction findingId={finding.id} signal={signal} control={draft} />
            <Button variant="link" size="inline" className="text-xs text-primary" onClick={onOpenInsights}>
              {t("signals.findings.openInsights")}
            </Button>
            <span className="text-xs text-faint">{t("signals.findings.caveat")}</span>
          </div>
        </div>
      )}
    </div>
  );
}

/** A dismissed finding: folded to one line, like a resolved comment, until shown. It counts nowhere else. */
export function DismissedBlock({ finding }: { finding: Finding }) {
  const { t } = useTranslation();
  const language = useAppLanguage();
  const [open, setOpen] = useState(false);
  const shown = shownFinding(finding, language);
  return (
    <div
      role="group"
      aria-label={t("signals.diff.dismissedLabel")}
      data-finding={finding.id}
      data-dismissed=""
      className={cn(BLOCK, "border-l-2 bg-transparent px-3 py-1.5")}
    >
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <ModelTag />
        <span className="text-xs text-faint">{t("signals.diff.dismissed")}</span>
        <strong className="font-medium text-muted-foreground line-through">{shown.title}</strong>
        <span className="text-xs text-faint">{findingPlace(finding)}</span>
        <Button variant="link" size="inline" aria-expanded={open} className="ml-auto text-xs text-primary" onClick={() => setOpen(!open)}>
          {open ? t("signals.diff.collapse") : t("signals.diff.show")}
        </Button>
      </div>
      {open && (
        <div className="mt-1.5">
          <p className="max-w-[78ch]">
            <WithCode text={shown.explanation} />
          </p>
          {finding.verdictAt && (
            <p className="mt-1.5 text-xs text-faint">
              {t("signals.diff.dismissedOn", { date: formatDate(finding.verdictAt) })}
            </p>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * Duplicated code under the last line of its new block, in ink like a static check: what it copies or how many times
 * it is repeated, its size, the way to the Quality tab where both sides are shown, and a draft.
 */
export function DuplicateNote({ duplicate, draft }: { duplicate: Duplicate; draft: DraftControl | null }) {
  const { t } = useTranslation();
  const { onOpenQuality } = use(SignalsContext);
  const signal = duplicateSignal(duplicate);
  const { block } = duplicate;
  return (
    <div
      role="group"
      aria-label={t("signals.diff.duplicateLabel", { start: block.startLine, end: block.endLine })}
      data-duplicate={`${block.file}:${block.startLine}`}
      className={cn(BLOCK, "flex flex-wrap items-center gap-x-2.5 gap-y-1 border-l-2 border-l-foreground bg-card py-1.5 pr-2.5 pl-3")}
    >
      <DuplicateChip />
      <span className="break-words">
        <WithCode text={duplicateKindText(duplicate)} />
      </span>
      <span className="text-xs text-faint">{duplicateSizeText(duplicate)}</span>
      <span className="ml-auto flex items-center gap-3">
        <Button variant="link" size="inline" className="text-xs text-primary" onClick={onOpenQuality}>
          {t("signals.diff.openQuality")}
        </Button>
        <DraftButton
          signal={signal}
          tone="ink"
          control={draft}
          request={() => ({
            signal,
            path: block.file,
            line: block.startLine,
            endLine: block.endLine,
            body: duplicateDraftBody(duplicate),
          })}
        />
      </span>
    </div>
  );
}

/**
 * A mutant that survived, under the last line it changes, in ink like a static check: the change no test caught, its
 * operator, the way to the Tests tab where the run is, the prompt for a coding agent, and a draft.
 */
export function MutantNote({ mutant, draft }: { mutant: Mutant; draft: DraftControl | null }) {
  const { t } = useTranslation();
  const { onOpenTests, signals } = use(SignalsContext);
  const signal = mutantSignal(mutant);
  return (
    <div
      role="group"
      aria-label={t("signals.diff.mutantLabel", { line: mutant.line })}
      data-mutant={mutant.id}
      className={cn(BLOCK, "flex flex-wrap items-center gap-x-2.5 gap-y-1 border-l-2 border-l-foreground bg-card py-1.5 pr-2.5 pl-3")}
    >
      <MutantChip />
      <span className="break-words">
        <WithCode text={mutantChangeText(mutant)} />
      </span>
      <span className="text-xs text-faint">{operatorName(mutant.operator)}</span>
      <span className="ml-auto flex items-center gap-3">
        <Button variant="link" size="inline" className="text-xs text-primary" onClick={onOpenTests}>
          {t("signals.diff.openTests")}
        </Button>
        <CopyMutantPrompt mutant={mutant} run={signals.mutation} look="link" />
        <DraftButton
          signal={signal}
          tone="ink"
          control={draft}
          request={() => ({
            signal,
            path: mutant.file,
            line: mutant.line,
            endLine: Math.max(mutant.endLine, mutant.line),
            body: mutantDraftBody(mutant),
          })}
        />
      </span>
    </div>
  );
}

/**
 * A static check under its line, in ink and without the model's tint: the tool and its rule, its severity, the tool's
 * own message (untranslated), the line, and a draft. One line already: nothing to fold.
 */
export function CheckBlock({ check, draft }: { check: LocatedCheck; draft: DraftControl | null }) {
  const { t } = useTranslation();
  const signal = checkSignal(check);
  return (
    <div
      role="group"
      aria-label={t("signals.diff.checkLabel", { tool: check.rule ? `${check.tool} ${check.rule}` : check.tool, line: check.line })}
      data-check={`${check.tool}:${check.line}`}
      className={cn(BLOCK, "flex flex-wrap items-center gap-x-2.5 gap-y-1 border-l-2 border-l-foreground bg-card py-1.5 pr-2.5 pl-3")}
    >
      <ToolChip tool={check.tool} rule={check.rule} />
      <CheckSeverityTag severity={check.severity} />
      <span lang="en" className="font-mono text-xs break-words">
        {check.message}
      </span>
      <span className="text-xs text-faint">{t("signals.diff.checkLine", { line: check.line })}</span>
      <span className="ml-auto">
        <DraftButton
          signal={signal}
          tone="ink"
          control={draft}
          request={() => ({ signal, path: check.file, line: check.line, endLine: check.line, body: checkDraftBody(check) })}
        />
      </span>
    </div>
  );
}
