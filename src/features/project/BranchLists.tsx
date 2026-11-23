import { LoaderCircle } from "lucide-react";
import { type ReactNode, useState } from "react";
import { Trans, useTranslation } from "react-i18next";

import { DiffStat } from "@/components/diff-stat";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { cardUrl, ExternalLink } from "@/features/jira/jira-links";
import { BaseSelector } from "@/features/project/BaseSelector";
import { BranchActions } from "@/features/project/BranchActions";
import { remoteName } from "@/features/project/branches";
import type { CompareState } from "@/features/project/compare-queue";
import type { ReviewOpening } from "@/features/review/views";
import type { AsyncState } from "@/hooks/use-async-data";
import { i18n } from "@/i18n";
import type { Branch, FullAnalysisJob, ReviewTarget, TerminalKind, WorkingTreeSummary, Worktree } from "@/lib/api";
import { errorMessage, formatDate } from "@/lib/format";
import { cn } from "@/lib/utils";

// The 60px column is the full analysis's status, right before the date; the ⋮ of BranchActions sits in the right
// padding, attached to the row's end. Both keep their room when empty, so the columns line up from row to row.
const ROW =
  "relative grid grid-cols-[minmax(0,1.5fr)_84px_150px_78px_170px_60px_62px] items-center gap-x-3.5 py-[11px] pr-[46px] pl-4 hover:bg-secondary";

const NO_WORKTREES: Worktree[] = [];

interface BranchRowProps {
  projectId: string;
  branch: Branch;
  bases: string[];
  /** Comparison with the branch's base; undefined until it is queued. */
  compare: CompareState | undefined;
  older?: boolean;
  /** Resolves once the branch was updated with the new base. */
  onChangeBase: (base: string | null) => Promise<void>;
  /**
   * Opens the review of the branch against its current base: on its Overview, or on `opening`'s tab; with a terminal
   * session of `terminal`'s kind when given.
   */
  onReview: (target: ReviewTarget, opening?: ReviewOpening, terminal?: TerminalKind) => void;
  /** The repository's worktrees: the ⋮ menu says where the branch's terminal opens. */
  worktrees?: Worktree[];
  /** The Jira site when Jira is connected: the card key then links to the card. Nothing is fetched per row. */
  jiraSite?: string | null;
  /** The latest full analysis of the branch's review in this run of the app: waiting, running or ended. */
  analysis?: FullAnalysisJob | null;
  /** Offered by a full analysis's confirmation when a model is not set up. */
  onOpenPreferences?: () => void;
}

/**
 * A branch to review: clicking the row opens its review; the ⋮ at its end opens a menu of actions instead (a full
 * analysis, shortcuts to a tab), and the column before the date says how its full analysis is going.
 */
export function BranchRow({
  projectId,
  branch,
  bases,
  compare,
  older = false,
  onChangeBase,
  onReview,
  worktrees = NO_WORKTREES,
  jiraSite = null,
  analysis = null,
  onOpenPreferences,
}: BranchRowProps) {
  const { t } = useTranslation();
  const target: ReviewTarget | null = branch.base
    ? { kind: "branch", branch: branch.name, base: branch.base.name }
    : null;
  const [baseChange, setBaseChange] = useState<
    { status: "idle" } | { status: "pending" } | { status: "error"; message: string }
  >({ status: "idle" });

  async function changeBase(base: string | null) {
    setBaseChange({ status: "pending" });
    try {
      await onChangeBase(base);
      setBaseChange({ status: "idle" });
    } catch (error) {
      setBaseChange({ status: "error", message: errorMessage(error) });
    }
  }

  return (
    <li className={cn(ROW, older && "opacity-70")}>
      <span className="min-w-0">
        <span className="flex min-w-0 items-center gap-2">
          {/* Covers the whole row (::after), so the row reads and clicks as one button. */}
          <button
            type="button"
            aria-disabled={branch.base ? undefined : true}
            title={branch.base ? undefined : t("project.row.noBaseToReview")}
            onClick={() => {
              if (target) onReview(target);
            }}
            className="min-w-0 cursor-pointer truncate text-left font-mono text-xs font-medium outline-none after:absolute after:inset-0 focus-visible:after:outline-2 focus-visible:after:-outline-offset-2 focus-visible:after:outline-ring"
          >
            {branch.name}
          </button>
          {branch.location === "remote_only" && (
            <Badge title={t("project.row.remoteOnlyTitle")}>{t("project.row.remoteOnly")}</Badge>
          )}
        </span>
        {branch.upstream && branch.upstream.behind > 0 && (
          <span
            className="block truncate text-xs text-mod"
            title={t("project.row.behindTitle", { upstream: branch.upstream.name })}
          >
            {t("project.row.newCommits", {
              count: branch.upstream.behind,
              remote: remoteName(branch.upstream.name),
            })}
          </span>
        )}
      </span>
      {jiraSite && branch.card ? (
        // Above the row's button overlay, like the base selector, so it opens the card rather than the review.
        <span className="relative z-10 flex min-w-0">
          <ExternalLink
            href={cardUrl(jiraSite, branch.card)}
            title={t("project.row.openCard", { card: branch.card })}
            className="min-w-0 px-[7px] py-px font-mono text-primary"
          >
            <span className="truncate">{branch.card}</span>
          </ExternalLink>
        </span>
      ) : (
        <span className="truncate font-mono text-xs text-primary">{branch.card}</span>
      )}
      <span className="relative z-10 flex min-w-0">
        {branch.base ? (
          <BaseSelector
            branch={branch.name}
            base={branch.base}
            bases={bases}
            pending={baseChange.status === "pending"}
            onChoose={changeBase}
          />
        ) : (
          <span className="text-xs text-faint" title={t("project.row.noBaseTitle")}>
            {t("project.row.noBase")}
          </span>
        )}
      </span>
      <span className="text-xs whitespace-nowrap text-muted-foreground">
        {t("project.row.commits", { count: branch.ahead })}
      </span>
      <CompareCell branch={branch} state={compare} />
      {analysis ? <AnalysisStatus job={analysis} /> : <span />}
      <time dateTime={branch.head.authoredAt} className="text-right text-xs whitespace-nowrap text-muted-foreground">
        {formatDate(branch.head.authoredAt)}
      </time>
      <BranchActions
        projectId={projectId}
        branch={branch.name}
        target={target}
        analysis={analysis}
        worktrees={worktrees}
        onReview={onReview}
        onOpenPreferences={onOpenPreferences}
      />
      {baseChange.status === "error" && (
        <p role="alert" className="col-span-full mt-1 text-xs text-destructive">
          {t("project.row.baseChangeFailed", { message: baseChange.message })}
        </p>
      )}
    </li>
  );
}

/**
 * How a branch's full analysis is going, right before the date, with the words of the top bar's menu: "Queued" or
 * its percent while it waits or runs, then a discreet "Analyzed", "Failed" or "Cancelled" until it is dismissed (its
 * review was opened).
 */
function AnalysisStatus({ job }: { job: FullAnalysisJob }) {
  const { t } = useTranslation();
  const [text, tone] = statusOf(job);
  return (
    <span
      role="status"
      title={t("fullAnalysis.branch.statusTitle", { status: text })}
      className={cn("truncate text-right text-xs whitespace-nowrap", tone)}
    >
      {text}
    </span>
  );
}

/** The words of a full analysis's status on its row, in the language shown, and their color. */
function statusOf(job: FullAnalysisJob): [string, string] {
  if (job.status === "queued") return [i18n.t("fullAnalysis.branch.queued"), "text-primary"];
  if (job.status === "running") {
    return [i18n.t("fullAnalysis.indicator.progress", { progress: job.progress }), "font-mono text-primary"];
  }
  if (job.outcome === "failed") return [i18n.t("fullAnalysis.branch.failed"), "text-del"];
  if (job.outcome === "cancelled") return [i18n.t("fullAnalysis.branch.cancelled"), "text-faint"];
  return [i18n.t("fullAnalysis.branch.analyzed"), "text-muted-foreground"];
}

/** "Analyzing… 45%" or "Queued", on the uncommitted changes' row while a full analysis of them runs or waits. */
function AnalysisNote({ job }: { job: FullAnalysisJob }) {
  const { t } = useTranslation();
  return (
    <span role="status" className="flex items-center gap-1.5 text-xs text-primary">
      {job.status === "running" && <LoaderCircle aria-hidden="true" className="size-3 flex-none animate-spin" />}
      {job.status === "queued"
        ? t("fullAnalysis.branch.queued")
        : t("fullAnalysis.branch.analyzing", { progress: job.progress })}
    </span>
  );
}

function CompareCell({ branch, state }: { branch: Branch; state: CompareState | undefined }) {
  const { t } = useTranslation();
  if (!branch.base) {
    return <span />;
  }
  if (branch.ahead === 0) {
    return (
      <span className="text-xs text-faint" title={t("project.row.nothingNewTitle", { base: branch.base.refName })}>
        {t("project.row.nothingNew")}
      </span>
    );
  }
  if (!state || state.status === "loading") {
    return <Skeleton role="status" aria-label={t("project.row.loadingChanges")} className="h-3.5 w-32" />;
  }
  if (state.status === "error") {
    return (
      <span title={state.message} className="truncate text-xs text-del">
        {state.short}
      </span>
    );
  }
  const { filesChanged, additions, deletions } = state.data;
  return (
    <DiffStat filesChanged={filesChanged} additions={additions} deletions={deletions} className="whitespace-nowrap" />
  );
}

interface WorkingTreeRowProps {
  /** From the project detail, shown until the summary arrives. */
  filesChanged: number;
  summary: AsyncState<WorkingTreeSummary>;
  onReview: (target: ReviewTarget) => void;
  /** A full analysis of the uncommitted changes running or waiting in the core. */
  analysis?: FullAnalysisJob | null;
}

export function WorkingTreeRow({ filesChanged, summary, onReview, analysis = null }: WorkingTreeRowProps) {
  const { t } = useTranslation();
  const files = summary.status === "ready" ? summary.data.filesChanged : filesChanged;
  return (
    <li
      aria-label={t("project.workingTree.label")}
      className="flex items-center gap-2.5 px-4 py-[11px] text-muted-foreground"
    >
      <span>
        <Trans
          i18nKey="project.workingTree.summary"
          count={files}
          components={{ b: <b className="font-medium text-mod" /> }}
        />
      </span>
      {summary.status === "loading" && (
        <Skeleton role="status" aria-label={t("project.workingTree.loading")} className="h-3.5 w-20" />
      )}
      {summary.status === "ready" && (
        <span className="font-mono text-xs">
          <span className="text-add">+{summary.data.additions}</span>{" "}
          <span className="text-del">−{summary.data.deletions}</span>
        </span>
      )}
      {summary.status === "error" && (
        <span title={summary.message} className="truncate text-xs text-del">
          {t("project.workingTree.countFailed")}
        </span>
      )}
      {analysis && <AnalysisNote job={analysis} />}
      <Button
        variant="link"
        size="inline"
        className="ml-auto text-primary"
        onClick={() => onReview({ kind: "working_tree" })}
      >
        {t("project.workingTree.review")}
      </Button>
    </li>
  );
}

/** A line inside a list panel, for an empty group. */
export function ListNote({ children }: { children: ReactNode }) {
  return <li className="px-4 py-[13px] text-muted-foreground">{children}</li>;
}

/** Full-width action at the end of a list, such as "Show 35 older branches". */
export function ListAction({ children, ...props }: { children: ReactNode; onClick: () => void; "aria-expanded"?: boolean }) {
  return (
    <li>
      <button
        type="button"
        className="w-full cursor-pointer px-4 py-[11px] text-left text-muted-foreground outline-none hover:bg-secondary hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
        {...props}
      >
        {children}
      </button>
    </li>
  );
}
