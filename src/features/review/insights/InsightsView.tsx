import { useId, useMemo, useState } from "react";
import { Trans, useTranslation } from "react-i18next";

import { CodeLines } from "@/components/code-lines";
import { CopyPromptButton } from "@/components/copy-prompt-button";
import { ErrorNote, Hint } from "@/components/page";
import { Segment, SegmentGroup } from "@/components/segment";
import { Button } from "@/components/ui/button";
import { TranslationBanner } from "@/features/review/ai-text/TranslationBanner";
import { otherLanguages, type ShownFinding, shownFinding } from "@/features/review/ai-text/versions";
import type { DiffTarget } from "@/features/review/flow/flow-facts";
import { Spinner } from "@/features/review/flow/FlowView";
import {
  agentPrompt,
  callsText,
  categoryName,
  isFixed,
  locationNote,
  modelName,
  planShape,
  recheckNote,
  resolutionByline,
  SEVERITIES,
  severityName,
  skippedText,
  startText,
  verdictName,
  VERDICTS,
  whereText,
  writtenBy,
} from "@/features/review/insights/insights-text";
import type { InsightsHandle, PlanState, Running } from "@/features/review/insights/use-insights";
import { fileDiffTarget, notInReview, PageSection, ReviewPage } from "@/features/review/ReviewPage";
import { useNow } from "@/hooks/use-now";
import { useAppLanguage } from "@/i18n/language";
import type { ChangedFile, Finding, FindingVerdict, InsightsAnalysis, Severity } from "@/lib/api";
import { modelProblem } from "@/lib/core-text";
import { errorMessage, formatDate, formatRelativeTime } from "@/lib/format";
import { cn } from "@/lib/utils";

interface InsightsViewProps {
  handle: InsightsHandle;
  /** Files of the review, to open a finding in the Diff tab. */
  files: readonly ChangedFile[];
  onViewDiff: (target: DiffTarget) => void;
  onOpenPreferences?: () => void;
}

/** Content of the Insights tab: defects the model read in the change, for the reviewer to confirm or dismiss. */
export function InsightsView({ handle, files, onViewDiff, onOpenPreferences }: InsightsViewProps) {
  const { t } = useTranslation();
  const { insights, error } = handle;
  const [offer, setOffer] = useState(false);
  // Each finding in the language shown when it has a version in it; the banner names what is not.
  const language = useAppLanguage();
  const findings = useMemo(
    () => (insights?.findings ?? []).map((finding) => shownFinding(finding, language)),
    [insights, language],
  );
  const writtenIn = otherLanguages(findings, language);
  const untranslated = findings.filter((finding) => finding.shownLanguage !== language).length;
  if (insights === null) {
    return (
      <ReviewPage>
        <div className="col-span-full">
          {error ? (
            <ErrorNote>{error}</ErrorNote>
          ) : (
            <p role="status" className="flex items-center gap-2 text-muted-foreground">
              <Spinner />
              {t("insights.loading")}
            </p>
          )}
        </div>
      </ReviewPage>
    );
  }
  if (insights.analysis === null && insights.findings.length === 0) {
    return (
      <ReviewPage>
        <Intro handle={handle} onOpenPreferences={onOpenPreferences} />
      </ReviewPage>
    );
  }
  return (
    <ReviewPage>
      <AnalysisBar
        handle={handle}
        analysis={insights.analysis}
        onOpenPreferences={onOpenPreferences}
        offer={offer}
        setOffer={setOffer}
      />
      {insights.findings.length === 0 ? (
        <p className="col-span-full text-muted-foreground">
          {handle.running ? t("insights.noFindingYet") : t("insights.nothingFound")}
        </p>
      ) : (
        <>
          {writtenIn.length > 0 && !handle.running && (
            <div className="col-span-full">
              <TranslationBanner
                subject={{ kind: "findings", count: untranslated }}
                writtenIn={writtenIn[0]}
                plan={() => handle.planTranslation(language)}
                runCall={(call, runId) => handle.translateCall(language, call.ids, runId)}
                redo={{ label: t("aiText.analyzeAgain"), onClick: () => setOffer(true), disabled: offer }}
              />
            </div>
          )}
          <Findings handle={handle} findings={findings} files={files} onViewDiff={onViewDiff} />
        </>
      )}
    </ReviewPage>
  );
}

/** Before the first analysis: what Analyze does, what it would send and to whom. */
function Intro({ handle, onOpenPreferences }: { handle: InsightsHandle; onOpenPreferences?: () => void }) {
  const { t } = useTranslation();
  const titleId = useId();
  return (
    <section aria-labelledby={titleId} className="col-span-full max-w-[680px]">
      <h1 id={titleId} className="text-lg font-semibold tracking-[-0.01em]">
        {t("insights.intro.title")}
      </h1>
      <p className="mt-2">{t("insights.intro.body")}</p>
      <PlanBlock
        plan={handle.plan}
        onAnalyze={handle.analyze}
        onRetryPlan={handle.retryPlan}
        onOpenPreferences={onOpenPreferences}
        label={t("insights.analyze")}
      />
      {handle.error && <ErrorNote className="mt-2.5">{handle.error}</ErrorNote>}
    </section>
  );
}

interface PlanBlockProps {
  plan: PlanState;
  onAnalyze: () => void;
  onRetryPlan: () => void;
  onOpenPreferences?: () => void;
  label: string;
}

/** The cost shape of an analysis and its button, or why there can be none. */
function PlanBlock({ plan, onAnalyze, onRetryPlan, onOpenPreferences, label }: PlanBlockProps) {
  const { t } = useTranslation();
  if (plan.status === "idle" || plan.status === "loading") {
    return (
      <p role="status" className="mt-4 flex items-center gap-2 text-muted-foreground">
        <Spinner />
        {t("insights.plan.working")}
      </p>
    );
  }
  if (plan.status === "error") {
    return (
      <div className="mt-4">
        <ErrorNote>{plan.message}</ErrorNote>
        <Button variant="secondary" size="sm" className="mt-2.5" onClick={onRetryPlan}>
          {t("common.actions.tryAgain")}
        </Button>
      </div>
    );
  }
  const { plan: shape } = plan;
  if (!shape.model.ready) {
    return (
      <div className="mt-4">
        <p>
          {t("insights.plan.noModel", { problem: modelProblem(shape.model) ?? t("insights.plan.chooseModel") })}
        </p>
        {onOpenPreferences && (
          <Button variant="secondary" size="sm" className="mt-2.5" onClick={onOpenPreferences}>
            {t("insights.plan.openPreferences")}
          </Button>
        )}
      </div>
    );
  }
  if (shape.calls.length === 0) {
    return <p className="mt-4 text-muted-foreground">{t("insights.plan.nothingToSend")}</p>;
  }
  const skipped = skippedText(shape);
  const truncated = shape.calls.flatMap((call) => call.truncated);
  return (
    <div className="mt-4">
      <p aria-label={t("insights.plan.sendsLabel")}>
        <Trans
          i18nKey="insights.plan.sends"
          values={{ shape: planShape(shape), model: modelName(shape.model) }}
          components={{ b: <b className="font-medium" /> }}
        />
      </p>
      {skipped && <Hint className="mt-1">{skipped}</Hint>}
      {truncated.length > 0 && (
        <Hint className="mt-1">{t("insights.plan.truncated", { count: truncated.length })}</Hint>
      )}
      <Button className="mt-3.5" onClick={onAnalyze}>
        {label}
      </Button>
      <Hint>{t("insights.plan.sentTo", { provider: shape.model.providerName })}</Hint>
    </div>
  );
}

interface AnalysisBarProps {
  handle: InsightsHandle;
  analysis: InsightsAnalysis | null;
  onOpenPreferences?: () => void;
  /** Analyze again was clicked, here or on the language banner: what it would send is shown. */
  offer: boolean;
  setOffer: (offer: boolean) => void;
}

/** Who wrote the findings and when, the calls being asked or that did not answer, and Analyze again. */
function AnalysisBar({ handle, analysis, onOpenPreferences, offer, setOffer }: AnalysisBarProps) {
  const { t } = useTranslation();
  const now = useNow();
  const { running, plan } = handle;
  return (
    <div className="col-span-full mb-3.5 border-b pb-[18px]">
      <div className="flex items-center gap-4">
        <div className="grid flex-1 gap-0.5">
          {running ? (
            <RunningState running={running} analysis={analysis} />
          ) : (
            <>
              <strong className="text-[15px] font-medium">
                {t("insights.findingCount", {
                  count: (handle.insights?.findings ?? []).filter((finding) => !isFixed(finding)).length,
                })}
              </strong>
              {analysis && (
                <span className="text-muted-foreground">
                  {writtenBy(analysis)}, {formatRelativeTime(analysis.createdAt, now)}
                </span>
              )}
            </>
          )}
        </div>
        {running ? (
          <Button variant="secondary" disabled={running.stopping} onClick={handle.cancel}>
            {running.stopping ? t("insights.stopping") : t("common.actions.cancel")}
          </Button>
        ) : (
          !offer && (
            <Button variant="secondary" onClick={() => setOffer(true)}>
              {t("insights.analyzeAgain")}
            </Button>
          )
        )}
      </div>
      {!running && offer && (
        <div className="max-w-[680px]">
          <PlanBlock
            plan={plan}
            onAnalyze={() => {
              setOffer(false);
              handle.analyze();
            }}
            onRetryPlan={handle.retryPlan}
            onOpenPreferences={onOpenPreferences}
            label={t("insights.analyze")}
          />
          <Hint>{t("insights.replaceNote")}</Hint>
        </div>
      )}
      {analysis && <Calls analysis={analysis} running={running} onRetry={handle.retry} />}
      {handle.error && <ErrorNote className="mt-2.5">{handle.error}</ErrorNote>}
    </div>
  );
}

function RunningState({ running, analysis }: { running: Running; analysis: InsightsAnalysis | null }) {
  const { t } = useTranslation();
  return (
    <>
      <strong role="status" className="flex items-center gap-2 text-[15px] font-medium">
        <Spinner />
        {running.stopping
          ? t("insights.stopping")
          : running.active.length > 0
            ? callsText(running.active, running.total)
            : t("insights.starting")}
      </strong>
      {analysis && (
        <span className="text-muted-foreground">{t("insights.asking", { model: modelName(analysis) })}</span>
      )}
    </>
  );
}

/** Calls of the latest analysis that failed, or did not run (stopped, or the screen closed), each with Retry. */
function Calls({
  analysis,
  running,
  onRetry,
}: {
  analysis: InsightsAnalysis;
  running: Running | null;
  onRetry: (index: number) => void;
}) {
  const { t } = useTranslation();
  const total = analysis.batches.length;
  const shown = analysis.batches.filter(
    (batch) => batch.status === "failed" || (batch.status === "pending" && running === null),
  );
  if (shown.length === 0) return null;
  return (
    <ul aria-label={t("insights.calls.listLabel")} className="mt-3 grid gap-2">
      {shown.map((batch) => (
        <li key={batch.index} className="grid grid-cols-[1fr_auto] items-start gap-x-4 border-l-2 border-l-del pl-3">
          <span>
            <strong className="font-medium">
              {batch.status === "failed"
                ? t("insights.calls.failed", { number: batch.index + 1, total })
                : t("insights.calls.didNotRun", { number: batch.index + 1, total })}
            </strong>{" "}
            {batch.error && errorMessage(batch.error)}
            <small className="block truncate text-xs text-faint" title={batch.paths.join("\n")}>
              {batch.paths.join(", ")}
            </small>
          </span>
          <Button variant="secondary" size="sm" disabled={running !== null} onClick={() => onRetry(batch.index)}>
            {t("common.actions.retry")}
          </Button>
        </li>
      ))}
    </ul>
  );
}

type Filter = "all" | Severity;

function Findings({
  handle,
  findings,
  files,
  onViewDiff,
}: Omit<InsightsViewProps, "onOpenPreferences"> & { findings: ShownFinding[] }) {
  const { t } = useTranslation();
  const [filter, setFilter] = useState<Filter>("all");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [fixedShown, setFixedShown] = useState(false);
  // Open findings are filtered and counted by severity; fixed ones wait in their own group, folded at first.
  const open = findings.filter((finding) => !isFixed(finding));
  const fixed = findings.filter(isFixed);
  const shown = filter === "all" ? open : open.filter((finding) => finding.severity === filter);
  const selected =
    shown.find((finding) => finding.id === selectedId) ??
    fixed.find((finding) => finding.id === selectedId) ??
    shown[0] ??
    null;
  const count = (severity: Severity) => open.filter((finding) => finding.severity === severity).length;
  const noneText =
    open.length === 0 && fixed.length > 0 ? t("insights.fixed.allFixed") : t(`insights.findings.none.${filter}`);
  return (
    <>
      <div>
        <div className="mb-3 flex flex-wrap items-center gap-3.5">
          <SegmentGroup label={t("insights.findings.severityLabel")}>
            <Segment pressed={filter === "all"} onClick={() => setFilter("all")}>
              {t("insights.findings.all", { total: open.length })}
            </Segment>
            {SEVERITIES.map((severity) => (
              <Segment key={severity} pressed={filter === severity} onClick={() => setFilter(severity)}>
                {severityName(severity)} {count(severity)}
              </Segment>
            ))}
          </SegmentGroup>
          <span className="text-xs text-faint">{t("insights.findings.caveat")}</span>
        </div>
        {shown.length === 0 ? (
          <p className="px-2 py-3 text-muted-foreground">{noneText}</p>
        ) : (
          <ul aria-label={t("insights.findings.listLabel")}>
            {shown.map((finding) => (
              <li key={finding.id}>
                <FindingRow
                  finding={finding}
                  selected={finding.id === selected?.id}
                  onSelect={() => setSelectedId(finding.id)}
                />
              </li>
            ))}
          </ul>
        )}
        {fixed.length > 0 && (
          <FixedGroup
            fixed={fixed}
            shown={fixedShown}
            onToggle={() => setFixedShown((current) => !current)}
            selectedId={selected?.id ?? null}
            onSelect={setSelectedId}
          />
        )}
      </div>
      {selected && (
        <FindingDetail
          key={selected.id}
          finding={selected}
          files={files}
          saving={handle.saving}
          onVerdict={(verdict) => handle.setVerdict(selected.id, verdict)}
          onFixed={(value) => {
            // It moves between the lists; it stays the one shown.
            setSelectedId(selected.id);
            handle.setFixed(selected.id, value);
          }}
          onViewDiff={onViewDiff}
        />
      )}
    </>
  );
}

interface FixedGroupProps {
  fixed: Finding[];
  /** Unfolded: folded until clicked. */
  shown: boolean;
  onToggle: () => void;
  selectedId: string | null;
  onSelect: (id: string) => void;
}

/** "Fixed (2)": the findings a later analysis or the user marked fixed, folded until clicked. */
function FixedGroup({ fixed, shown, onToggle, selectedId, onSelect }: FixedGroupProps) {
  const { t } = useTranslation();
  const listId = useId();
  const now = useNow();
  return (
    <div className="mt-4">
      <button
        type="button"
        aria-expanded={shown}
        aria-controls={shown ? listId : undefined}
        onClick={onToggle}
        className="flex cursor-pointer items-center gap-1.5 rounded-[6px] px-2 py-1 text-sm font-medium text-muted-foreground outline-none hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
      >
        <span aria-hidden className={cn("inline-block transition-transform", shown && "rotate-90")}>
          ›
        </span>
        {t("insights.fixed.group", { count: fixed.length })}
      </button>
      {shown && (
        <ul id={listId} aria-label={t("insights.fixed.listLabel")}>
          {fixed.map((finding) => (
            <li key={finding.id}>
              <FindingRow
                finding={finding}
                selected={finding.id === selectedId}
                onSelect={() => onSelect(finding.id)}
                now={now}
              />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

const FIXED_TONE = "bg-add-bg text-add";

/** The "Fixed" tag. */
function FixedTag({ className }: { className?: string }) {
  const { t } = useTranslation();
  return (
    <em className={cn("rounded-[9px] px-[9px] py-px text-[11px] not-italic", FIXED_TONE, className)}>
      {t("insights.fixed.tag")}
    </em>
  );
}

const SEVERITY_TONE: Record<Severity, string> = {
  high: "bg-del-bg text-del",
  medium: "bg-mod-bg text-mod",
  low: "bg-secondary text-muted-foreground",
};

const VERDICT_TONE: Record<FindingVerdict, string> = {
  pending: "border border-dashed border-faint text-muted-foreground",
  confirmed: "bg-primary-soft text-primary",
  dismissed: "bg-secondary text-faint",
};

/** A row of the list: severity, title, category and place, and its verdict, or for a fixed one who said so. */
function FindingRow({
  finding,
  selected,
  onSelect,
  now,
}: {
  finding: Finding;
  selected: boolean;
  onSelect: () => void;
  /** For the relative time of a fixed finding's byline. */
  now?: Date;
}) {
  const { t } = useTranslation();
  const dismissed = finding.verdict === "dismissed";
  const byline = resolutionByline(finding, now);
  return (
    <button
      type="button"
      aria-pressed={selected}
      aria-label={[
        finding.title,
        severityName(finding.severity),
        categoryName(finding.category),
        startText(finding),
        byline ? t("insights.fixed.tag") : verdictName(finding.verdict),
        ...(byline ? [byline] : []),
      ].join(", ")}
      onClick={onSelect}
      className={cn(
        "grid w-full cursor-pointer grid-cols-[62px_1fr_auto] items-start gap-3 rounded-[6px] border-b px-2 py-3 text-left outline-none hover:bg-card focus-visible:outline-2 focus-visible:outline-ring",
        selected && "bg-ai-bg hover:bg-ai-bg",
      )}
    >
      <em className={cn("mt-0.5 rounded-[9px] py-px text-center text-[11px] not-italic", SEVERITY_TONE[finding.severity])}>
        {severityName(finding.severity)}
      </em>
      <span className="min-w-0">
        <strong
          className={cn(
            "block text-sm font-medium",
            (dismissed || byline) && "text-muted-foreground",
            dismissed && "line-through",
          )}
        >
          {finding.title}
        </strong>
        <small className="mt-0.5 flex min-w-0 gap-2 text-xs text-muted-foreground">
          {categoryName(finding.category)}
          <code className="truncate">{startText(finding)}</code>
        </small>
        {byline && <small className="mt-0.5 block text-xs text-faint">{byline}</small>}
      </span>
      {byline ? (
        <FixedTag className="self-center" />
      ) : (
        <em
          className={cn("self-center rounded-[9px] px-[9px] py-px text-[11px] not-italic", VERDICT_TONE[finding.verdict])}
        >
          {verdictName(finding.verdict)}
        </em>
      )}
    </button>
  );
}

interface FindingDetailProps {
  finding: Finding;
  files: readonly ChangedFile[];
  saving: boolean;
  onVerdict: (verdict: FindingVerdict) => void;
  /** Mark it fixed (true), or open it again (false). */
  onFixed: (fixed: boolean) => void;
  onViewDiff: (target: DiffTarget) => void;
}

function FindingDetail({ finding, files, saving, onVerdict, onFixed, onViewDiff }: FindingDetailProps) {
  const { t } = useTranslation();
  const titleId = useId();
  const now = useNow();
  const location = finding.location;
  const target = location.kind === "found" ? fileDiffTarget(finding.path, location.line, files) : null;
  const note = locationNote(finding);
  const recheck = recheckNote(finding);
  const byline = resolutionByline(finding, now);
  const reason = finding.resolution?.reason ?? null;
  return (
    <section aria-labelledby={titleId} className="border-l pl-7">
      <h1 id={titleId} className="text-lg font-semibold tracking-[-0.01em]">
        {finding.title}
      </h1>
      <p
        className="mt-0.5 mb-3.5 font-mono text-muted-foreground"
        title={
          location.kind === "found" && location.moved ? t("insights.detail.moved", { line: finding.line }) : undefined
        }
      >
        {whereText(finding)}
      </p>
      {finding.source.earlier && (
        <p className="mb-3 text-xs text-faint">
          {t("insights.detail.earlier", {
            date: formatDate(finding.source.analyzedAt),
            model: modelName(finding.source),
          })}
        </p>
      )}
      {byline && (
        <div className="mb-3">
          <p className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
            <FixedTag />
            {byline}
          </p>
          {reason && <p className="mt-2 rounded-[6px] bg-add-bg px-2.5 py-2 text-add">{reason}</p>}
        </div>
      )}
      {note && <p className="mb-3 rounded-[6px] bg-mod-bg px-2.5 py-2 text-mod">{note}</p>}
      <p className="max-w-[64ch]">{finding.explanation}</p>
      {finding.excerpt.length > 0 && <Excerpt finding={finding} />}
      {recheck && <p className="mt-3.5 rounded-[6px] bg-primary-soft px-2.5 py-2 text-primary">{recheck}</p>}
      {finding.suggestedDirection && (
        <PageSection title={t("insights.detail.suggestedDirection")}>
          <p className="max-w-[64ch]">{finding.suggestedDirection}</p>
        </PageSection>
      )}
      <PageSection
        title={
          <>
            {t("insights.detail.evidence")}
            <span className="font-normal text-faint">{t("insights.detail.evidenceNote")}</span>
          </>
        }
      >
        <Bullets items={finding.evidence} empty={t("insights.detail.noEvidence")} />
      </PageSection>
      {finding.related.length > 0 && (
        <PageSection title={t("insights.detail.related")}>
          <Bullets items={finding.related} />
        </PageSection>
      )}
      <div className="mt-[26px] flex flex-wrap items-center gap-2.5">
        <CopyPromptButton
          text={() => agentPrompt(finding)}
          label={t("insights.prompt.copy")}
          failedLabel={t("insights.prompt.copyFailed")}
        />
        <Button
          variant="secondary"
          disabled={target === null}
          title={target ? undefined : note ?? notInReview()}
          onClick={() => target && onViewDiff(target)}
        >
          {t("insights.detail.openInDiff")}
        </Button>
        {finding.resolution ? (
          <Button variant="secondary" disabled={saving} onClick={() => onFixed(false)}>
            {t("insights.fixed.reopen")}
          </Button>
        ) : (
          finding.verdict !== "dismissed" && (
            <Button variant="secondary" disabled={saving} onClick={() => onFixed(true)}>
              {t("insights.fixed.markFixed")}
            </Button>
          )
        )}
        <SegmentGroup label={t("insights.detail.verdictLabel")}>
          {VERDICTS.map((verdict) => (
            <Segment
              key={verdict}
              pressed={finding.verdict === verdict}
              disabled={saving}
              onClick={() => finding.verdict !== verdict && onVerdict(verdict)}
            >
              {verdictName(verdict)}
            </Segment>
          ))}
        </SegmentGroup>
      </div>
      <Hint>
        {t("insights.detail.by", {
          model: modelName(finding.source),
          when: formatRelativeTime(finding.source.analyzedAt),
        })}
      </Hint>
    </section>
  );
}

/** The cited lines and two around them, read from the file at the review's new side (the prototype's `.codeblock`). */
function Excerpt({ finding }: { finding: Finding }) {
  const { t } = useTranslation();
  return (
    <CodeLines
      label={t("insights.detail.codeAt", { where: whereText(finding) })}
      lines={finding.excerpt.map((line) => ({ line: line.line, text: line.text, marked: line.cited }))}
      className="my-2.5"
    />
  );
}

function Bullets({ items, empty }: { items: readonly string[]; empty?: string }) {
  if (items.length === 0) return empty ? <p className="text-muted-foreground">{empty}</p> : null;
  return (
    <ul className="grid list-disc gap-1.5 pl-[18px] text-muted-foreground">
      {items.map((item, index) => (
        <li key={index}>{item}</li>
      ))}
    </ul>
  );
}

