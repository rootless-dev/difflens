import type { ReactNode } from "react";
import { Trans, useTranslation } from "react-i18next";

import { Segment, SegmentGroup } from "@/components/segment";
import type { DiffLayout } from "@/features/review/DiffEditor";
import { ToolChip } from "@/features/review/signals/signal-marks";
import { cn } from "@/lib/utils";

export interface DiffOptions {
  /** Collapse unchanged stretches ("Changes only") or show the whole file ("Full file"). */
  changesOnly: boolean;
  layout: DiffLayout;
  /** The model's findings under their lines, and the bars of the lines they cite. */
  showFindings: boolean;
  /** The static check findings under their lines, and their marks. */
  showChecks: boolean;
}

export const DEFAULT_DIFF_OPTIONS: DiffOptions = {
  changesOnly: true,
  layout: "unified",
  showFindings: true,
  showChecks: true,
};

/**
 * What the two signal toggles count in the file shown; each toggle is only there when the review has something of its
 * kind to show.
 */
export interface SignalToggles {
  findings: { review: boolean; inFile: number };
  checks: { review: boolean; inFile: number };
}

/** What the Diff tab compares: the whole review, or only what changed since the last review action. */
export type DiffScope = "branch" | "since";

/** The scope switch: whether "Since last review" can be chosen, and the title saying why or why not. */
export interface ScopeControl {
  scope: DiffScope;
  onChange: (scope: DiffScope) => void;
  sinceAvailable: boolean;
  sinceTitle: string;
}

/** What the Diff tab shows: the files' diffs, or the review's comments (on the pull request and on lines). */
export type DiffMode = "files" | "comments";

/** The Files | Comments switch, with the number of open conversations. */
export interface ModeControl {
  mode: DiffMode;
  onChange: (mode: DiffMode) => void;
  /** Open conversations (`isOpen`), general and on lines; null while the comments are not read yet. */
  conversations: number | null;
}

interface DiffToolbarProps {
  options: DiffOptions;
  onChange: (options: DiffOptions) => void;
  scope?: ScopeControl;
  mode?: ModeControl;
  /** Lines take comments: say so. */
  commentable?: boolean;
  /** "Publish comments", when the review's comments can go to a pull request. */
  publish?: ReactNode;
  signals?: SignalToggles;
}

/** The Diff tab's controls. In the Comments view only the switch and Publish apply: the rest is about the files. */
export function DiffToolbar({
  options,
  onChange,
  scope,
  mode,
  commentable = false,
  publish,
  signals,
}: DiffToolbarProps) {
  const { t } = useTranslation();
  const set = (change: Partial<DiffOptions>) => onChange({ ...options, ...change });
  const files = mode === undefined || mode.mode === "files";
  return (
    <div className="flex flex-none flex-wrap items-center gap-x-3.5 gap-y-2 border-b px-4 py-2.5">
      {mode && (
        <SegmentGroup label={t("review.toolbar.shown")}>
          <Segment className="py-[3px]" pressed={mode.mode === "files"} onClick={() => mode.onChange("files")}>
            {t("review.toolbar.files")}
          </Segment>
          <Segment
            className="py-[3px]"
            pressed={mode.mode === "comments"}
            title={
              mode.conversations === null
                ? undefined
                : t("review.toolbar.commentsTitle", { count: mode.conversations })
            }
            onClick={() => mode.onChange("comments")}
          >
            {mode.conversations === null
              ? t("review.toolbar.comments")
              : t("review.toolbar.commentsCount", { count: mode.conversations })}
          </Segment>
        </SegmentGroup>
      )}
      {files && <FileControls options={options} set={set} scope={scope} />}
      {files && signals?.findings.review && (
        <Toggle
          tone="ai"
          checked={options.showFindings}
          onChange={(showFindings) => set({ showFindings })}
          title={t("signals.diff.showFindingsTitle")}
          label={t("signals.diff.showFindings")}
          count={
            <em className="rounded-[9px] bg-ai-bg px-[7px] text-[11px] whitespace-nowrap text-ai not-italic">
              {t("signals.diff.inFile", { count: signals.findings.inFile })}
            </em>
          }
        />
      )}
      {files && signals?.checks.review && (
        <Toggle
          tone="ink"
          checked={options.showChecks}
          onChange={(showChecks) => set({ showChecks })}
          title={t("signals.diff.showChecksTitle")}
          label={t("signals.diff.showChecks")}
          count={<ToolChip>{t("signals.diff.inFile", { count: signals.checks.inFile })}</ToolChip>}
        />
      )}
      {publish}
      {files && (
        <div className="ml-auto flex flex-wrap items-center gap-x-3.5 gap-y-1 text-xs text-faint">
          {commentable && <span>{t("review.toolbar.clickToComment")}</span>}
          <p>
            <Trans i18nKey="review.toolbar.fileKeys" components={{ kbd: <Key /> }} />
          </p>
        </div>
      )}
    </div>
  );
}

function FileControls({
  options,
  set,
  scope,
}: {
  options: DiffOptions;
  set: (change: Partial<DiffOptions>) => void;
  scope?: ScopeControl;
}) {
  const { t } = useTranslation();
  return (
    <>
      <SegmentGroup label={t("review.toolbar.linesShown")}>
        <Segment className="py-[3px]" pressed={options.changesOnly} onClick={() => set({ changesOnly: true })}>
          {t("review.toolbar.changesOnly")}
        </Segment>
        <Segment className="py-[3px]" pressed={!options.changesOnly} onClick={() => set({ changesOnly: false })}>
          {t("review.toolbar.fullFile")}
        </Segment>
      </SegmentGroup>
      {scope && (
        <SegmentGroup label={t("review.toolbar.compared")}>
          <Segment className="py-[3px]" pressed={scope.scope === "branch"} onClick={() => scope.onChange("branch")}>
            {t("review.toolbar.wholeBranch")}
          </Segment>
          <Segment
            className="py-[3px]"
            pressed={scope.scope === "since"}
            disabled={!scope.sinceAvailable}
            title={scope.sinceTitle}
            onClick={() => scope.onChange("since")}
          >
            {t("review.toolbar.sinceLastReview")}
          </Segment>
        </SegmentGroup>
      )}
      <SegmentGroup label={t("review.toolbar.layout")}>
        <Segment className="py-[3px]" pressed={options.layout === "unified"} onClick={() => set({ layout: "unified" })}>
          {t("review.toolbar.unified")}
        </Segment>
        <Segment className="py-[3px]" pressed={options.layout === "split"} onClick={() => set({ layout: "split" })}>
          {t("review.toolbar.split")}
        </Segment>
      </SegmentGroup>
    </>
  );
}

/**
 * A checkbox that shows or hides a kind of block: in the model's colour for its findings, in ink for what a tool
 * measured.
 */
function Toggle({
  tone,
  checked,
  onChange,
  title,
  label,
  count,
}: {
  tone: "ai" | "ink";
  checked: boolean;
  onChange: (checked: boolean) => void;
  title: string;
  label: string;
  count: ReactNode;
}) {
  return (
    <label
      title={title}
      className={cn(
        "inline-flex cursor-pointer items-center gap-[7px] rounded-[7px] border bg-card py-[3px] pr-2.5 pl-2 text-[12.5px] has-focus-visible:outline-2 has-focus-visible:outline-offset-2 has-focus-visible:outline-ring",
        checked && tone === "ai" && "border-ai/45 bg-ai-bg",
        checked && tone === "ink" && "border-foreground/30 bg-secondary",
      )}
    >
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
        className={cn("m-0 outline-none", tone === "ai" ? "accent-ai" : "accent-foreground")}
      />
      {label} {count}
    </label>
  );
}

/** A key to press; Trans fills in its letter. */
function Key({ children }: { children?: ReactNode }) {
  return <kbd className="rounded-[4px] border bg-card px-1 font-mono text-[11px] text-muted-foreground">{children}</kbd>;
}
