import { type ReactNode, useId, useMemo, useState } from "react";
import { Trans, useTranslation } from "react-i18next";

import { ErrorNote, Hint } from "@/components/page";
import { PreparingAnalyzers } from "@/components/preparing-analyzers";
import { Button } from "@/components/ui/button";
import { WithCode } from "@/components/with-code";
import { ExternalLink } from "@/features/jira/jira-links";
import { TranslationBanner } from "@/features/review/ai-text/TranslationBanner";
import { type ShownSummary, shownSummary } from "@/features/review/ai-text/versions";
import type { CardStatus } from "@/features/review/card/use-card";
import type { DiffTarget } from "@/features/review/flow/flow-facts";
import { Spinner } from "@/features/review/flow/FlowView";
import type { FlowState } from "@/features/review/flow/use-flow";
import { baseName } from "@/features/review/file-tree";
import { formatTokens, modelName, severityName } from "@/features/review/insights/insights-text";
import {
  changedFilesText,
  type Jump,
  leftOutLines,
  type LookItem,
  lookFirst,
  type MeasuredRow,
  measuredRows,
  noEvidenceNote,
  type Since,
  shapeRows,
  sinceOf,
  type Suggestions,
  suggestions,
  tallyText,
  verdictLabel,
} from "@/features/review/overview/overview-text";
import {
  CheckSeverityTag,
  DuplicateChip,
  LimitChip,
  MeasuredTag,
  ModelTag,
  SeverityChip,
  ToolChip,
  UntestedChip,
} from "@/features/review/signals/signal-marks";
import { findingLine, type ReviewSignals, reviewSignals } from "@/features/review/signals/signals";
import type { OverviewHandle, OverviewPlanState } from "@/features/review/overview/use-overview";
import type { PullRequestStatus } from "@/features/review/pull-request/pull-request-status";
import type { QualityState } from "@/features/review/quality/use-quality";
import { shortSha } from "@/features/review/review-target";
import { fileDiffTarget, PageSection, ReviewPage } from "@/features/review/ReviewPage";
import type { TestsPanel } from "@/features/review/tests/use-tests";
import { useNow } from "@/hooks/use-now";
import { useAppLanguage } from "@/i18n/language";
import type {
  Card,
  ChangedFile,
  Criterion,
  CriterionVerdict,
  Insights,
  OverviewSummary,
  ReviewState,
} from "@/lib/api";
import { modelProblem } from "@/lib/core-text";
import { formatDate, formatRelativeTime, intlLocale } from "@/lib/format";
import { cn } from "@/lib/utils";

interface OverviewViewProps {
  handle: OverviewHandle;
  card: { status: CardStatus; retry: () => void };
  pullRequest: PullRequestStatus;
  files: readonly ChangedFile[];
  review: ReviewState | null;
  insights: Insights | null;
  tests: TestsPanel;
  quality: QualityState;
  flow: FlowState;
  onViewDiff: (target: DiffTarget) => void;
  onOpenFlow: (functionId: string) => void;
  onOpenTests: () => void;
  /** Opens the Insights tab, where every finding is. */
  onOpenInsights?: () => void;
  /** The review's signals, worked out once for the Overview, the Flow and the Diff; from the data above when absent. */
  signals?: ReviewSignals;
  /** Opens the Diff tab showing only what changed since the last review. */
  onShowSince: () => void;
  onOpenPreferences?: () => void;
  /** The Full analysis button and its confirmation, under the lead. */
  fullAnalysis?: ReactNode;
}

/**
 * Content of the Overview tab (the prototype's `.overview`). The left column is what the change does and what its card
 * asks (written by the model on Summarize), then where to look and what a model would not see; the right column is
 * measured or derived and needs no model at all.
 */
export function OverviewView(props: OverviewViewProps) {
  const { t } = useTranslation();
  const { handle, files, review, insights, tests, quality, flow } = props;
  const graph = flow.status === "ready" && flow.result.kind === "graph" ? flow.result.graph : null;
  const run = tests.status === "ready" && tests.tests.run?.status === "completed" ? tests.tests.run : null;
  const report = quality.status === "ready" && quality.result.kind === "report" ? quality.result.report : null;
  const since = sinceOf(review, insights?.findings ?? null);
  // The summary in the language shown when it has a version in it; otherwise the original, under a banner.
  const language = useAppLanguage();
  const stored = handle.overview?.summary ?? null;
  const summary = useMemo(() => (stored === null ? null : shownSummary(stored, language)), [stored, language]);
  const findings = insights?.findings ?? null;
  const signals = useMemo(() => props.signals ?? reviewSignals(report, findings), [props.signals, report, findings]);
  const look = lookFirst({ findings, run, quality: report, graph, files, signals });
  const suggested = suggestions(signals, look);
  return (
    <ReviewPage className="grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)] gap-x-14 px-[max(28px,calc(50%-540px))] pt-9">
      <div className="min-w-0">
        <Lead {...props} summary={summary} />
        {props.fullAnalysis && <PageSection title={t("fullAnalysis.title")}>{props.fullAnalysis}</PageSection>}
        <CardSection {...props} summary={summary} />
        {props.card.status.kind === "found" && <CriteriaSection summary={summary} files={files} onViewDiff={props.onViewDiff} />}
        <PageSection title={t("overview.look.title")}>
          <LookList
            items={look}
            analysed={insights?.analysis !== null && insights !== null}
            measuring={quality.status === "loading" || tests.status === "loading"}
            onJump={(jump) => jumpTo(jump, props)}
          />
        </PageSection>
        {suggested && (
          <SuggestionList
            suggestions={suggested}
            files={files}
            onViewDiff={props.onViewDiff}
            onOpenInsights={props.onOpenInsights}
          />
        )}
        <PageSection title={t("overview.leftOut.title")}>
          {handle.overview === null ? (
            <Reading error={handle.error} text={t("overview.readingChange")} />
          ) : (
            <LeftOut lines={leftOutLines(handle.overview.leftOut, summary?.condensed ?? false)} />
          )}
        </PageSection>
      </div>
      <div className="min-w-0">
        {since && <SinceSection since={since} onShowSince={props.onShowSince} />}
        <PageSection title={t("overview.measured.title")} first={since === null}>
          <Measured
            tests={tests}
            quality={quality}
            rows={measuredRows(run, report, signals)}
            onOpenTests={props.onOpenTests}
          />
        </PageSection>
        <PageSection title={t("overview.shape.title")}>
          {handle.overview === null ? (
            <Reading error={handle.error} text={t("overview.readingChange")} />
          ) : (
            <dl className="grid grid-cols-[1fr_auto] gap-x-4 gap-y-[9px] border-t pt-3">
              {flow.status === "loading" && (
                <ShapeRow label={t("overview.shape.entryPoints")} value={t("overview.shape.analysing")} />
              )}
              {shapeRows(handle.overview.shape, graph).map((row) => (
                <ShapeRow key={row.label} label={row.label} value={row.value} title={row.title} />
              ))}
            </dl>
          )}
          {/* A review of a language seen for the first time waits for its analyzer to be prepared. */}
          {flow.status === "loading" && <PreparingAnalyzers className="mt-3" />}
        </PageSection>
      </div>
    </ReviewPage>
  );
}

function jumpTo(jump: Jump, { onViewDiff, onOpenFlow, onOpenTests }: OverviewViewProps) {
  if (jump.kind === "flow") onOpenFlow(jump.functionId);
  else if (jump.kind === "diff") onViewDiff(jump.target);
  else onOpenTests();
}

function Reading({ error, text }: { error: string | null; text: string }) {
  if (error) return <ErrorNote>{error}</ErrorNote>;
  return (
    <p role="status" className="flex items-center gap-2 text-muted-foreground">
      <Spinner />
      {text}
    </p>
  );
}

interface LeadProps extends OverviewViewProps {
  summary: ShownSummary | null;
}

/** The title and lead the model wrote, who wrote them and when, or what Summarize would send. */
function Lead({ handle, summary, pullRequest, onOpenPreferences }: LeadProps) {
  const { t } = useTranslation();
  const language = useAppLanguage();
  const now = useNow();
  const [offer, setOffer] = useState(false);
  const titleId = useId();
  const sending = (
    <PlanBlock
      plan={handle.plan}
      pullRequest={pullRequest.kind === "linked"}
      label={summary ? t("overview.lead.summarizeAgain") : t("overview.lead.summarize")}
      onSummarize={() => {
        setOffer(false);
        handle.summarize();
      }}
      onRetry={handle.retry}
      onOpenPreferences={onOpenPreferences}
    />
  );
  const running = handle.running && (
    <div className="mt-4 flex items-center gap-4">
      <p role="status" className="flex flex-1 items-center gap-2">
        <Spinner />
        {handle.running.stopping
          ? t("overview.lead.stopping")
          : handle.plan.status === "ready"
            ? t("overview.lead.asking", { model: modelName(handle.plan.plan.model) })
            : t("overview.lead.askingModel")}
      </p>
      <Button variant="secondary" disabled={handle.running.stopping} onClick={handle.cancel}>
        {handle.running.stopping ? t("overview.lead.stopping") : t("common.actions.cancel")}
      </Button>
    </div>
  );
  if (summary === null) {
    return (
      <section aria-labelledby={titleId}>
        <h1 id={titleId} className="text-[22px] font-semibold tracking-[-0.02em]">
          {t("overview.lead.title")}
        </h1>
        <p className="mt-2.5 max-w-[64ch] text-muted-foreground">{t("overview.lead.intro")}</p>
        {running || sending}
        {handle.error && <ErrorNote className="mt-2.5">{handle.error}</ErrorNote>}
      </section>
    );
  }
  return (
    <section aria-labelledby={titleId}>
      {summary.shownLanguage !== language && !handle.running && (
        <TranslationBanner
          subject={{ kind: "summary" }}
          writtenIn={summary.shownLanguage}
          plan={() => handle.planTranslation(language)}
          runCall={(_call, runId) => handle.translate(language, runId)}
          redo={{ label: t("aiText.summarizeAgain"), onClick: () => setOffer(true), disabled: offer }}
        />
      )}
      <h1 id={titleId} className="text-[22px] font-semibold tracking-[-0.02em]">
        {summary.title}
      </h1>
      <SummaryText summary={summary} />
      <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1.5 text-xs text-faint">
        <ModelTag />
        <span>
          {t(summary.condensed ? "overview.lead.bylineCondensed" : "overview.lead.byline", {
            model: modelName(summary),
            when: formatRelativeTime(summary.createdAt, now),
          })}
        </span>
        {!handle.running && !offer && !summary.outdated && (
          <Button variant="link" size="inline" className="text-xs text-primary" onClick={() => setOffer(true)}>
            {t("overview.lead.summarizeAgain")}
          </Button>
        )}
      </div>
      {summary.outdated && !handle.running && (
        <div className="mt-3 flex flex-wrap items-center gap-3 rounded-[6px] bg-primary-soft px-2.5 py-2 text-primary">
          <span className="flex-1">
            <Trans
              i18nKey="overview.lead.outdated"
              values={{ sha: shortSha(summary.headSha ?? "") }}
              components={{ code: <code /> }}
            />
          </span>
          {!offer && (
            <Button variant="secondary" size="sm" onClick={() => setOffer(true)}>
              {t("overview.lead.summarizeAgain")}
            </Button>
          )}
        </div>
      )}
      {running}
      {!handle.running && offer && <div className="max-w-[680px]">{sending}</div>}
      {handle.error && <ErrorNote className="mt-2.5">{handle.error}</ErrorNote>}
    </section>
  );
}

/**
 * What the model wrote about the change: the lead as a paragraph, how the change works as a list, and what needs the
 * reviewer's attention set apart, quieter. A summary written before points existed is its lead alone: a paragraph.
 */
function SummaryText({ summary }: { summary: ShownSummary }) {
  const { t } = useTranslation();
  return (
    <div className="mt-2.5 grid max-w-[64ch] gap-2.5">
      <p className="text-[15px] leading-[1.6]">
        <WithCode text={summary.lead} />
      </p>
      {summary.points.length > 0 && (
        <ul aria-label={t("overview.lead.points")} className="grid list-disc gap-1 pl-[18px] leading-[1.55]">
          {summary.points.map((point, index) => (
            <li key={index}>
              <WithCode text={point} />
            </li>
          ))}
        </ul>
      )}
      {summary.attention.length > 0 && (
        <ul
          aria-label={t("overview.lead.attention")}
          className="grid gap-1 border-l-2 pl-3 text-[13px] leading-[1.55] text-muted-foreground"
        >
          {summary.attention.map((item, index) => (
            <li key={index}>
              <WithCode text={item} />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

interface PlanBlockProps {
  plan: OverviewPlanState;
  pullRequest: boolean;
  label: string;
  onSummarize: () => void;
  onRetry: () => void;
  onOpenPreferences?: () => void;
}

/** The cost shape of Summarize and its button, or why there can be none. */
function PlanBlock({ plan, pullRequest, label, onSummarize, onRetry, onOpenPreferences }: PlanBlockProps) {
  const { t } = useTranslation();
  if (plan.status === "idle" || plan.status === "loading") {
    return (
      <p role="status" className="mt-4 flex items-center gap-2 text-muted-foreground">
        <Spinner />
        {t("overview.plan.working")}
      </p>
    );
  }
  if (plan.status === "error") {
    return (
      <div className="mt-4">
        <ErrorNote>{plan.message}</ErrorNote>
        <Button variant="secondary" size="sm" className="mt-2.5" onClick={onRetry}>
          {t("common.actions.tryAgain")}
        </Button>
      </div>
    );
  }
  const shape = plan.plan;
  if (!shape.model.ready) {
    return (
      <div className="mt-4">
        <p>{t("overview.plan.noModel", { problem: modelProblem(shape.model) ?? t("overview.plan.chooseModel") })}</p>
        {onOpenPreferences && (
          <Button variant="secondary" size="sm" className="mt-2.5" onClick={onOpenPreferences}>
            {t("overview.plan.openPreferences")}
          </Button>
        )}
        <Hint>{t("overview.plan.worksWithoutModel")}</Hint>
      </div>
    );
  }
  if (shape.files === 0) {
    return <p className="mt-4 text-muted-foreground">{t("overview.plan.nothingToSend")}</p>;
  }
  const files = t("overview.plan.files", { count: shape.files });
  const tokens = formatTokens(shape.tokens);
  const shapeText = shape.cardKey
    ? t("overview.plan.shapeWithCard", {
        files,
        card: shape.cardKey,
        comments: t("overview.plan.comments", { count: shape.cardComments }),
        tokens,
      })
    : t("overview.plan.shape", { files, tokens });
  const sent = [
    t("overview.plan.sentCode"),
    ...(shape.cardKey ? [t("overview.plan.sentCard")] : []),
    ...(pullRequest ? [t("overview.plan.sentPullRequest")] : []),
  ];
  return (
    <div className="mt-4">
      <p aria-label={t("overview.plan.sendsLabel")}>
        <Trans
          i18nKey="overview.plan.sends"
          values={{ shape: shapeText, model: modelName(shape.model) }}
          components={{ b: <b className="font-medium" /> }}
        />
      </p>
      {shape.condensed && <Hint className="mt-1">{t("overview.plan.condensed")}</Hint>}
      <Button className="mt-3.5" onClick={onSummarize}>
        {label}
      </Button>
      <Hint>
        {t("overview.plan.sentTo", {
          count: sent.length,
          items: capitalized(new Intl.ListFormat(intlLocale(), { type: "conjunction" }).format(sent)),
          provider: shape.model.providerName,
        })}
      </Hint>
    </div>
  );
}

function capitalized(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

interface CardSectionProps extends OverviewViewProps {
  summary: OverviewSummary | null;
}

/** "What the card asks": the card with the model's reading of it, or quietly why there is none. */
function CardSection({ card, pullRequest, summary, onOpenPreferences }: CardSectionProps) {
  const { t } = useTranslation();
  const { status } = card;
  return (
    <PageSection title={t("overview.card.title")}>
      {status.kind === "loading" && <Reading error={null} text={t("overview.card.reading")} />}
      {status.kind === "found" && <Ticket card={status.card} pullRequest={pullRequest} summary={summary} />}
      {status.kind === "working_tree" && <p className="text-muted-foreground">{t("overview.card.workingTree")}</p>}
      {status.kind === "no_key" && (
        <p className="text-muted-foreground">
          <Trans
            i18nKey={pullRequest.kind === "linked" ? "overview.card.noKeyOrPullRequest" : "overview.card.noKey"}
            values={{ branch: status.branch }}
            components={{ code: <code /> }}
          />
        </p>
      )}
      {status.kind === "not_connected" && (
        <p className="text-muted-foreground">
          <Trans
            i18nKey="overview.card.notConnected"
            values={{ cardKey: status.key }}
            components={{ code: <code /> }}
          />{" "}
          {onOpenPreferences && (
            <Button variant="link" size="inline" className="text-primary" onClick={onOpenPreferences}>
              {t("overview.card.connect")}
            </Button>
          )}
        </p>
      )}
      {status.kind === "not_found" && (
        <p className="text-muted-foreground">
          <Trans i18nKey="overview.card.notFound" values={{ cardKey: status.key }} components={{ code: <code /> }} />
        </p>
      )}
      {status.kind === "error" && (
        <div>
          <ErrorNote>{t("overview.card.readFailed", { message: status.message })}</ErrorNote>
          <Button variant="secondary" size="sm" className="mt-2.5" onClick={card.retry}>
            {t("common.actions.tryAgain")}
          </Button>
        </div>
      )}
    </PageSection>
  );
}

/** Characters of the card's own description shown before it is summarised. */
const DESCRIPTION_PREVIEW = 600;

function Ticket({
  card,
  pullRequest,
  summary,
}: {
  card: Card;
  pullRequest: PullRequestStatus;
  summary: OverviewSummary | null;
}) {
  const { t } = useTranslation();
  const read = summary !== null && summary.cardKey === card.key && summary.cardSummary !== "" ? summary : null;
  const description =
    card.description.length > DESCRIPTION_PREVIEW
      ? `${card.description.slice(0, DESCRIPTION_PREVIEW).trimEnd()}…`
      : card.description;
  return (
    <div className="grid gap-2.5 rounded-[10px] border bg-card px-[18px] py-4">
      <div className="flex items-center gap-2">
        <h3 className="min-w-0 flex-1 text-[15px] font-medium">{card.summary}</h3>
        <ExternalLink href={card.url} title={t("overview.card.openInJira")}>
          {card.key}
        </ExternalLink>
        {pullRequest.kind === "linked" && (
          <ExternalLink
            href={pullRequest.pullRequest.url}
            title={t("overview.card.openPullRequest", { host: pullRequest.hostName })}
          >
            PR #{pullRequest.pullRequest.number}
          </ExternalLink>
        )}
      </div>
      <p className="-mt-1.5 text-xs text-muted-foreground">
        {[card.issueType, card.status].filter((part) => part !== "").join(" · ")}
      </p>
      {read ? (
        <>
          <p className="max-w-[66ch]">
            <WithCode text={read.cardSummary} />
          </p>
          <p className="flex items-center gap-2 text-xs text-faint">
            <ModelTag />
            {read.cardComments > 0
              ? t("overview.card.summarisedWithComments", { count: read.cardComments })
              : t("overview.card.summarised")}
          </p>
        </>
      ) : (
        <>
          {description !== "" ? (
            // Plain text as Jira has it, lists and headings kept; never HTML.
            <p className="max-w-[66ch] whitespace-pre-wrap text-muted-foreground">{description}</p>
          ) : (
            <p className="text-muted-foreground">{t("overview.card.noDescription")}</p>
          )}
          <p className="text-xs text-faint">{t("overview.card.asWritten", { count: card.comments.length })}</p>
        </>
      )}
    </div>
  );
}

const VERDICT_TONE: Record<CriterionVerdict, string> = {
  met: "bg-add-bg text-add",
  partly_met: "bg-mod-bg text-mod",
  not_met: "bg-del-bg text-del",
  no_evidence: "bg-secondary text-muted-foreground",
};

function CriteriaSection({
  summary,
  files,
  onViewDiff,
}: {
  summary: OverviewSummary | null;
  files: readonly ChangedFile[];
  onViewDiff: (target: DiffTarget) => void;
}) {
  const { t } = useTranslation();
  const criteria = summary?.cardKey ? summary.criteria : null;
  return (
    <PageSection
      title={
        <>
          {t("overview.criteria.title")}
          {criteria && criteria.length > 0 && <span className="font-normal text-faint">{tallyText(criteria)}</span>}
          <ModelTag />
        </>
      }
    >
      {criteria === null ? (
        <p className="text-muted-foreground">{t("overview.criteria.summarizeFirst")}</p>
      ) : criteria.length === 0 ? (
        <p className="text-muted-foreground">{t("overview.criteria.none")}</p>
      ) : (
        <ul aria-label={t("overview.criteria.title")} className="border-t">
          {criteria.map((criterion, index) => (
            <CriterionRow key={index} criterion={criterion} files={files} onViewDiff={onViewDiff} />
          ))}
        </ul>
      )}
    </PageSection>
  );
}

function CriterionRow({
  criterion,
  files,
  onViewDiff,
}: {
  criterion: Criterion;
  files: readonly ChangedFile[];
  onViewDiff: (target: DiffTarget) => void;
}) {
  const { t } = useTranslation();
  const place = criterion.path !== null ? `${criterion.path}${criterion.line ? `:${criterion.line}` : ""}` : null;
  const target =
    criterion.located && criterion.path !== null && criterion.line !== null
      ? fileDiffTarget(criterion.path, criterion.line, files)
      : null;
  return (
    <li className="grid grid-cols-[84px_1fr] gap-3.5 border-b px-2 py-3">
      <em
        className={cn(
          "mt-0.5 self-start rounded-[9px] py-px text-center text-[11px] not-italic",
          VERDICT_TONE[criterion.verdict],
        )}
      >
        {verdictLabel(criterion.verdict)}
      </em>
      <div>
        <strong className="block text-sm font-medium">{criterion.criterion}</strong>
        <span className="text-muted-foreground">
          {criterion.reasoning}
          {criterion.verdict === "no_evidence" && ` ${noEvidenceNote()}`}{" "}
          {place &&
            (target ? (
              <button
                type="button"
                onClick={() => onViewDiff(target)}
                title={t("overview.criteria.openInDiff")}
                className="cursor-pointer font-mono text-xs text-primary hover:underline"
              >
                {place}
              </button>
            ) : (
              <code title={t("overview.criteria.notInChange")}>{place}</code>
            ))}
        </span>
      </div>
    </li>
  );
}

function LookList({
  items,
  analysed,
  measuring,
  onJump,
}: {
  items: LookItem[];
  analysed: boolean;
  measuring: boolean;
  onJump: (jump: Jump) => void;
}) {
  const { t } = useTranslation();
  if (items.length === 0) {
    return (
      <>
        <p className="text-muted-foreground">{measuring ? t("overview.look.measuring") : t("overview.look.nothing")}</p>
        {!analysed && <Hint>{t("overview.look.analyzeHint")}</Hint>}
      </>
    );
  }
  return (
    <>
      <ol aria-label={t("overview.look.title")} className="border-t">
        {items.map((item, index) => (
          <li key={item.id} className="border-b">
            <LookRow item={item} rank={index + 1} onJump={onJump} />
          </li>
        ))}
      </ol>
      {!analysed && <Hint>{t("overview.look.analyzeHint")}</Hint>}
    </>
  );
}

function LookRow({ item, rank, onJump }: { item: LookItem; rank: number; onJump: (jump: Jump) => void }) {
  const { t } = useTranslation();
  const body: ReactNode = (
    <>
      <span className="pt-px font-mono text-[13px] font-medium text-faint">{rank}</span>
      <span className="grid min-w-0 gap-[3px]">
        <strong className="text-sm font-medium">{item.title}</strong>
        {(item.quote || item.detail) && (
          <span className="text-muted-foreground">
            {item.quote && (
              <>
                {t("overview.look.lineQuote", { line: item.quote.line })}{" "}
                {/* The tool's own words, never translated. */}
                <span lang="en">{item.quote.message}</span>
                {item.detail && ". "}
              </>
            )}
            {item.detail}
          </span>
        )}
        <span className="mt-[3px] flex flex-wrap items-center gap-2">
          {item.measured && <MeasuredTag />}
          {item.model && <ModelTag />}
          {item.severity && <SeverityChip severity={item.severity} />}
          {item.tool && <ToolChip tool={item.tool.tool} rule={item.tool.rule} />}
          {item.tool && <CheckSeverityTag severity={item.tool.severity} />}
          {item.limits > 0 && <LimitChip>{t("signals.chip.limits", { count: item.limits })}</LimitChip>}
          {item.duplicate && <DuplicateChip />}
          {item.untested && <UntestedChip />}
          {item.location && <code className="truncate text-xs">{item.location}</code>}
        </span>
      </span>
    </>
  );
  const layout = "grid w-full grid-cols-[28px_1fr_auto] items-start gap-2.5 rounded-[6px] px-2 py-[13px] text-left";
  if (item.jump === null) return <div className={layout}>{body}</div>;
  const jump = item.jump;
  return (
    <button
      type="button"
      onClick={() => onJump(jump)}
      className={cn(
        layout,
        "group cursor-pointer outline-none hover:bg-card focus-visible:outline-2 focus-visible:outline-ring",
      )}
    >
      {body}
      <span className="invisible pt-0.5 text-xs text-primary group-hover:visible group-focus-visible:visible">
        {t(`overview.look.destination.${jump.kind}`)}
      </span>
    </button>
  );
}

/**
 * "The model suggests changing": the findings "Where to look first" had no room for, most severe first, each opening
 * the Diff at its line; and the way to Insights when even more are left. Never drawn empty.
 */
function SuggestionList({
  suggestions: shown,
  files,
  onViewDiff,
  onOpenInsights,
}: {
  suggestions: Suggestions;
  files: readonly ChangedFile[];
  onViewDiff: (target: DiffTarget) => void;
  onOpenInsights?: () => void;
}) {
  const { t } = useTranslation();
  const titleId = useId();
  const hidden = shown.left - shown.items.length;
  return (
    <section aria-labelledby={titleId}>
      <h2 id={titleId} className="mt-[30px] mb-2.5 flex items-center gap-2 text-sm font-medium text-muted-foreground">
        {t("overview.suggestions.title")}
        <ModelTag />
        <span className="font-normal text-faint">
          {shown.above > 0
            ? t("overview.suggestions.beyond", { left: shown.left, count: shown.above })
            : t("overview.suggestions.left", { count: shown.left })}
        </span>
      </h2>
      <ol aria-labelledby={titleId} className="border-t">
        {shown.items.map((finding) => {
          const line = findingLine(finding);
          const location = `${finding.path}:${line}`;
          const target = finding.location.kind === "found" ? fileDiffTarget(finding.path, line, files) : null;
          const body = (
            <>
              <SeverityChip severity={finding.severity} />
              <span className="flex min-w-0 items-baseline gap-2.5">
                <span className="truncate">{finding.title}</span>
                <code title={location} className="flex-none text-xs text-muted-foreground">
                  {`${baseName(finding.path)}:${line}`}
                </code>
              </span>
            </>
          );
          const layout = "grid w-full grid-cols-[70px_minmax(0,1fr)_auto] items-baseline gap-3 rounded-[6px] p-2 text-left";
          return (
            <li key={finding.id} className="border-b">
              {target ? (
                <button
                  type="button"
                  aria-label={t("overview.suggestions.rowLabel", {
                    severity: severityName(finding.severity),
                    title: finding.title,
                    location,
                  })}
                  onClick={() => onViewDiff({ ...target, reveal: "finding" })}
                  className={cn(
                    layout,
                    "group cursor-pointer outline-none hover:bg-card focus-visible:outline-2 focus-visible:outline-ring",
                  )}
                >
                  {body}
                  <span className="invisible text-xs whitespace-nowrap text-primary group-hover:visible group-focus-visible:visible">
                    {t("overview.look.destination.diff")}
                  </span>
                </button>
              ) : (
                <div className={layout}>{body}</div>
              )}
            </li>
          );
        })}
      </ol>
      {hidden > 0 && (
        <p className="mt-2 flex items-center gap-3 text-xs text-faint">
          {onOpenInsights && (
            <Button variant="link" size="inline" className="text-xs text-primary" onClick={onOpenInsights}>
              {t("overview.suggestions.seeAll", { count: shown.total })}
            </Button>
          )}
          <span>{t("overview.suggestions.onlyThere", { count: hidden })}</span>
        </p>
      )}
    </section>
  );
}

function LeftOut({ lines }: { lines: ReturnType<typeof leftOutLines> }) {
  const { t } = useTranslation();
  if (lines.length === 0) {
    return <p className="text-muted-foreground">{t("overview.leftOut.nothing")}</p>;
  }
  return (
    <ul className="grid list-disc gap-1.5 pl-[18px] text-muted-foreground">
      {lines.map((line, index) => (
        <li key={index}>
          {line.path !== null && (
            <>
              <code>{line.path}</code>{" "}
            </>
          )}
          {line.text}
        </li>
      ))}
    </ul>
  );
}

function SinceSection({ since, onShowSince }: { since: Since; onShowSince: () => void }) {
  const { t } = useTranslation();
  const facts = [
    since.files.length > 0 ? changedFilesText(since.files) : null,
    since.reopened > 0 ? t("overview.since.reopened", { count: since.reopened }) : null,
    since.comments > 0 ? t("overview.since.comments", { count: since.comments }) : null,
    since.findings > 0 ? t("overview.since.findings", { count: since.findings }) : null,
  ].filter((fact) => fact !== null);
  return (
    <PageSection title={t("overview.since.title")} first>
      <div className="grid justify-items-start gap-2.5 border-l-2 border-l-primary pl-3.5">
        <p>
          {since.checkpointSha ? (
            <Trans
              i18nKey="overview.since.reviewedOnAt"
              values={{ date: formatDate(since.at), sha: shortSha(since.checkpointSha) }}
              components={{ code: <code /> }}
            />
          ) : (
            t("overview.since.reviewedOn", { date: formatDate(since.at) })
          )}
          {since.commits !== null && ` ${t("overview.since.commitsAdded", { count: since.commits })}`}
          {since.note && ` ${since.note}`}
        </p>
        {facts.length > 0 && (
          <ul className="grid list-disc gap-1.5 pl-[18px] text-muted-foreground">
            {facts.map((fact) => (
              <li key={fact}>{fact}</li>
            ))}
          </ul>
        )}
        {since.commits !== null && (
          <Button variant="secondary" onClick={onShowSince}>
            {t("overview.since.showOnly")}
          </Button>
        )}
      </div>
    </PageSection>
  );
}

function Measured({
  tests,
  quality,
  rows,
  onOpenTests,
}: {
  tests: TestsPanel;
  quality: QualityState;
  rows: MeasuredRow[];
  onOpenTests: () => void;
}) {
  const { t } = useTranslation();
  const noRun = tests.status === "ready" && (tests.tests.run === null || tests.tests.run.status !== "completed");
  return (
    <>
      {tests.status === "loading" && <Reading error={null} text={t("overview.measured.readingRun")} />}
      {tests.status === "error" && <ErrorNote>{tests.message}</ErrorNote>}
      {noRun && (
        <p className="mb-3 text-muted-foreground">
          {tests.tests.unavailable
            ? t("overview.measured.noRunner")
            : tests.tests.run === null
              ? t("overview.measured.notRun")
              : t("overview.measured.incomplete")}{" "}
          {!tests.tests.unavailable && (
            <Button variant="link" size="inline" className="text-primary" onClick={onOpenTests}>
              {t("overview.measured.openTests")}
            </Button>
          )}
        </p>
      )}
      {rows.length > 0 && (
        <dl className="grid border-t">
          {rows.map((row) => (
            <div key={row.label} className="border-b pt-3 pb-3">
              <dt className="text-muted-foreground">{row.label}</dt>
              <dd className="pt-0.5 text-[17px]">
                <b
                  className={cn(
                    "font-medium",
                    row.tone === "warn" && "text-mod",
                    row.tone === "bad" && "text-del",
                  )}
                >
                  {row.value}
                </b>
                {row.bad && (
                  <>
                    , <b className="font-medium text-del">{row.bad}</b>
                  </>
                )}
                {row.note && <small className="mt-px block text-xs text-faint">{row.note}</small>}
              </dd>
            </div>
          ))}
        </dl>
      )}
      {quality.status === "loading" && (
        <p role="status" className="mt-3 flex items-center gap-2 text-xs text-muted-foreground">
          <Spinner />
          {t("overview.measured.measuringComplexity")}
        </p>
      )}
    </>
  );
}

function ShapeRow({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <>
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="text-right" title={title}>
        {value}
      </dd>
    </>
  );
}
