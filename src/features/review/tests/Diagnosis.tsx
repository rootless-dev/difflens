import { type ComponentProps, type ReactNode, useId } from "react";
import { useTranslation } from "react-i18next";

import { CopyPromptButton } from "@/components/copy-prompt-button";
import { ErrorNote } from "@/components/page";
import { Button } from "@/components/ui/button";
import { WithCode } from "@/components/with-code";
import type { ShownDiagnosis } from "@/features/review/ai-text/versions";
import { TranslationBanner } from "@/features/review/ai-text/TranslationBanner";
import { modelName } from "@/features/review/insights/insights-text";
import { diagnosisPrompt, failureKey, faultName } from "@/features/review/tests/diagnosis-text";
import { Location, useTestsContext } from "@/features/review/tests/parts";
import type { Failure } from "@/features/review/tests/suite";
import type { FailureExplanation } from "@/features/review/tests/TestsView";
import type { DiagnosesHandle } from "@/features/review/tests/use-diagnoses";
import { useNow } from "@/hooks/use-now";
import { useAppLanguage } from "@/i18n/language";
import { i18n } from "@/i18n";
import type { Language } from "@/lib/api";
import { formatRelativeTime } from "@/lib/format";
import { cn } from "@/lib/utils";

/** Who Explain the failure asks, as its tooltip names it. */
function diagnosticianName(handle: DiagnosesHandle): string {
  return handle.model ? modelName(handle.model) : i18n.t("tests.diagnosis.modelUnknown");
}

/** What a failure's card shows of its explanation: the button that asks for it, and the block it is shown in. */
export function failureExplanation(failure: Failure, handle: DiagnosesHandle): FailureExplanation {
  return {
    action: <ExplainAction failure={failure} handle={handle} />,
    block: <DiagnosisBlock failure={failure} handle={handle} />,
  };
}

const AI_BUTTON =
  "inline-flex h-[27px] cursor-pointer items-center rounded-[6px] border border-ai/45 bg-ai-bg px-2.5 text-xs font-medium whitespace-nowrap text-ai outline-none hover:border-ai focus-visible:outline-2 focus-visible:outline-ring";

const LINKISH =
  "cursor-pointer rounded-sm text-xs text-primary outline-none hover:underline focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:text-muted-foreground disabled:no-underline";

/**
 * Explain the failure: a model call, named in its tooltip, only on click. Shown once the explanations already written
 * were read, while the failure has none, none is being written and the last one did not fail (the block then offers to
 * try again).
 */
function ExplainAction({ failure, handle }: { failure: Failure; handle: DiagnosesHandle }) {
  const { t } = useTranslation();
  const key = failureKey(failure);
  if (handle.saved.status !== "ready" || handle.shown.has(key) || handle.runOf(key) || handle.errorOf(key)) return null;
  return (
    <button
      type="button"
      title={t("tests.diagnosis.explainTitle", { model: diagnosticianName(handle) })}
      onClick={() => handle.explain(failure)}
      className={AI_BUTTON}
    >
      {t("tests.diagnosis.explain")}
    </button>
  );
}

function Frame({ children, ...props }: ComponentProps<"section">) {
  return (
    <section {...props} className={cn("max-w-[920px] rounded-[8px] border-l-2 border-l-ai bg-ai-bg px-3.5 py-3", props.className)}>
      {children}
    </section>
  );
}

/** Under the output: the explanation while it is written (with Stop), once written, or why it could not be. */
function DiagnosisBlock({ failure, handle }: { failure: Failure; handle: DiagnosesHandle }) {
  const { t } = useTranslation();
  const key = failureKey(failure);
  const run = handle.runOf(key);
  const shown = handle.shown.get(key);
  const error = handle.errorOf(key);
  const labelId = useId();
  if (run) {
    const model = diagnosticianName(handle);
    const label = run.stopping
      ? t("tests.diagnosis.stopping")
      : run.queued
        ? t("tests.diagnosis.waiting", { model })
        : t("tests.diagnosis.explaining", { model });
    return (
      <Frame aria-labelledby={labelId} aria-busy="true">
        <div role="status" className="grid gap-2">
          <p className="flex items-center gap-2.5 text-muted-foreground">
            <span
              aria-hidden="true"
              className="inline-block size-3 flex-none animate-spin rounded-full border-[1.6px] border-ai/25 border-t-ai motion-reduce:[animation-duration:3s]"
            />
            <span id={labelId}>{label}</span>
            <button type="button" disabled={run.stopping} onClick={() => handle.cancel(key)} className={cn(LINKISH, "ml-auto")}>
              {t("tests.diagnosis.stop")}
            </button>
          </p>
          <span aria-hidden="true" className="h-2.5 w-[88%] rounded-[4px] bg-ai/15" />
          <span aria-hidden="true" className="h-2.5 w-[64%] rounded-[4px] bg-ai/15" />
        </div>
      </Frame>
    );
  }
  if (shown) return <Answer failure={failure} diagnosis={shown} handle={handle} error={error} />;
  if (error) {
    return (
      <Frame aria-labelledby={labelId}>
        <p id={labelId} className="mb-1.5 font-medium">
          {t("tests.diagnosis.title")}
        </p>
        <ErrorNote>{t("tests.diagnosis.failed", { message: error })}</ErrorNote>
        <Button
          variant="secondary"
          size="xs"
          className="mt-2.5"
          title={t("tests.diagnosis.explainTitle", { model: diagnosticianName(handle) })}
          onClick={() => handle.explain(failure)}
        >
          {t("tests.diagnosis.tryAgain")}
        </Button>
      </Frame>
    );
  }
  return null;
}

function Chip({ children, title, className }: { children: ReactNode; title?: string; className?: string }) {
  return (
    <em title={title} className={cn("rounded-[9px] border bg-card px-2 text-[11px] text-foreground not-italic", className)}>
      {children}
    </em>
  );
}

function Answer({
  failure,
  diagnosis,
  handle,
  error,
}: {
  failure: Failure;
  diagnosis: ShownDiagnosis;
  handle: DiagnosesHandle;
  error: string | null;
}) {
  const { t } = useTranslation();
  const titleId = useId();
  const now = useNow();
  const { commands } = useTestsContext();
  return (
    <Frame aria-labelledby={titleId} className={cn(diagnosis.outdated && "text-muted-foreground")}>
      <header className="mb-2.5 flex flex-wrap items-center gap-2">
        <strong id={titleId} className="font-medium text-foreground">
          {t("tests.diagnosis.title")}
        </strong>
        <em className="rounded-[9px] border border-ai/30 px-[7px] text-[11px] text-ai not-italic">{t("overview.tag.model")}</em>
        <Chip title={t("tests.diagnosis.originTitle")}>
          <b className="font-normal text-muted-foreground">{t("tests.diagnosis.origin")}</b> {faultName(diagnosis.fault)}
        </Chip>
        {diagnosis.confidence === "low" && <Chip title={t("tests.diagnosis.unsureTitle")}>{t("tests.diagnosis.unsure")}</Chip>}
        {diagnosis.outdated && (
          <em className="rounded-[9px] bg-primary-soft px-[7px] text-[11px] text-primary not-italic">
            {t("tests.diagnosis.outdatedTag")}
          </em>
        )}
        <span className="ml-auto text-xs text-faint">
          {t("tests.diagnosis.byline", {
            provider: diagnosis.providerName,
            model: diagnosis.model,
            when: formatRelativeTime(diagnosis.createdAt, now),
          })}
        </span>
      </header>
      <dl className="grid grid-cols-[120px_minmax(0,1fr)] gap-x-4 gap-y-2.5 text-[13px] leading-[1.55]">
        <dt className="text-muted-foreground">{t("tests.diagnosis.cause")}</dt>
        <dd className="max-w-[76ch]">
          <WithCode text={diagnosis.cause} />
        </dd>
        <dt className="text-muted-foreground">{t("tests.diagnosis.where")}</dt>
        <dd>
          {diagnosis.location ? (
            <Location file={diagnosis.location.file} line={diagnosis.location.line} full />
          ) : (
            <span className="text-xs text-muted-foreground">{t("tests.diagnosis.noWhere")}</span>
          )}
        </dd>
        {(diagnosis.fix || diagnosis.fixCode) && (
          <>
            <dt className="text-muted-foreground">{t("tests.diagnosis.fix")}</dt>
            <dd className="max-w-[76ch]">
              {diagnosis.fix && <WithCode text={diagnosis.fix} />}
              {diagnosis.fixCode && (
                <pre
                  aria-label={t("tests.diagnosis.snippet")}
                  className="mt-1.5 overflow-auto rounded-[7px] border bg-background px-3 py-2 font-mono text-xs leading-[1.65] text-foreground"
                >
                  {diagnosis.fixCode}
                </pre>
              )}
            </dd>
          </>
        )}
      </dl>
      {diagnosis.outdated && <p className="mt-2.5 text-xs text-primary">{t("tests.diagnosis.outdated")}</p>}
      {error && <ErrorNote className="mt-2.5">{t("tests.diagnosis.failed", { message: error })}</ErrorNote>}
      {handle.commentErrorOf(diagnosis.key) && (
        <ErrorNote className="mt-2.5">
          {t("tests.diagnosis.comment.failed", { message: handle.commentErrorOf(diagnosis.key) })}
        </ErrorNote>
      )}
      <div className="mt-3 flex flex-wrap items-center gap-3">
        <CopyPromptButton
          variant="secondary"
          size="sm"
          text={() => diagnosisPrompt(failure, diagnosis, commands)}
          label={t("tests.diagnosis.copy")}
          failedLabel={t("tests.diagnosis.copyFailed")}
        />
        <ReviewCommentAction diagnosisKey={diagnosis.key} handle={handle} />
        <button
          type="button"
          title={t("tests.diagnosis.againTitle", { model: diagnosticianName(handle) })}
          onClick={() => handle.explain(failure)}
          className={LINKISH}
        >
          {t("tests.diagnosis.again")}
        </button>
      </div>
    </Frame>
  );
}

/**
 * Write review comment (a model call, named in its tooltip, only on click), which keeps a draft in the Diff without
 * leaving the tab; once the draft is there, View comment in Diff, until the draft is deleted.
 */
function ReviewCommentAction({ diagnosisKey, handle }: { diagnosisKey: string; handle: DiagnosesHandle }) {
  const { t } = useTranslation();
  const comment = handle.commentOf(diagnosisKey);
  const writing = handle.commentRunOf(diagnosisKey);
  const model = handle.commentModel ? modelName(handle.commentModel) : i18n.t("tests.diagnosis.modelUnknown");
  if (writing) {
    return (
      <span role="status" className="flex items-center gap-2 text-xs text-muted-foreground">
        <span
          aria-hidden="true"
          className="inline-block size-3 flex-none animate-spin rounded-full border-[1.6px] border-ai/25 border-t-ai motion-reduce:[animation-duration:3s]"
        />
        {writing.stopping ? t("tests.diagnosis.stopping") : t("tests.diagnosis.comment.writing", { model })}
        <button
          type="button"
          disabled={writing.stopping}
          onClick={() => handle.cancelComment(diagnosisKey)}
          className={LINKISH}
        >
          {t("tests.diagnosis.stop")}
        </button>
      </span>
    );
  }
  if (comment) {
    const title =
      comment.kind === "line"
        ? t("tests.diagnosis.comment.viewTitleLine", {
            place: `${comment.path}:${comment.currentLine ?? comment.line}`,
          })
        : t("tests.diagnosis.comment.viewTitleGeneral");
    return (
      <Button variant="secondary" size="sm" title={title} onClick={() => handle.viewComment(diagnosisKey)}>
        {t("tests.diagnosis.comment.view")}
      </Button>
    );
  }
  return (
    <button
      type="button"
      title={t("tests.diagnosis.comment.writeTitle", { model })}
      onClick={() => handle.writeComment(diagnosisKey)}
      className={AI_BUTTON}
    >
      {t("tests.diagnosis.comment.write")}
    </button>
  );
}

/**
 * Over the failures: while the explanations already written are read, or why they could not be (with Try again);
 * then, when some explanation shown reads in another language than the one shown, says so and offers Translate (a
 * model call, only on click). Changing the language never asks a model.
 */
export function DiagnosesBanner({ failures, handle }: { failures: readonly Failure[]; handle: DiagnosesHandle }) {
  return (
    <>
      <SavedNote handle={handle} />
      <LanguageBanner failures={failures} handle={handle} />
    </>
  );
}

function SavedNote({ handle }: { handle: DiagnosesHandle }) {
  const { t } = useTranslation();
  const { saved } = handle;
  if (saved.status === "reading") {
    return (
      <p role="status" className="flex items-center gap-2.5 text-xs text-muted-foreground">
        <span
          aria-hidden="true"
          className="inline-block size-3 flex-none animate-spin rounded-full border-[1.6px] border-ai/25 border-t-ai motion-reduce:[animation-duration:3s]"
        />
        {t("tests.diagnosis.reading")}
      </p>
    );
  }
  if (saved.status === "failed") {
    return (
      <div className="flex flex-wrap items-center gap-3 text-xs">
        <ErrorNote>{t("tests.diagnosis.readFailed", { message: saved.message })}</ErrorNote>
        <Button variant="secondary" size="xs" onClick={handle.retryRead}>
          {t("tests.diagnosis.tryAgain")}
        </Button>
      </div>
    );
  }
  return null;
}

function LanguageBanner({ failures, handle }: { failures: readonly Failure[]; handle: DiagnosesHandle }) {
  const language = useAppLanguage();
  const other = failures
    .map((failure) => handle.shown.get(failureKey(failure)))
    .filter((diagnosis): diagnosis is ShownDiagnosis => diagnosis !== undefined && diagnosis.shownLanguage !== language);
  if (other.length === 0) return null;
  const keys = other.map((diagnosis) => diagnosis.key);
  const writtenIn: Language = other[0].shownLanguage;
  return (
    <TranslationBanner
      subject={{ kind: "diagnoses", count: other.length }}
      writtenIn={writtenIn}
      plan={() => handle.planTranslation(language, keys)}
      runCall={(call, runId) => handle.translateCall(language, call.ids, runId)}
    />
  );
}
