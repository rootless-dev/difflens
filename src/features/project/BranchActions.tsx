import { EllipsisVertical, LoaderCircle } from "lucide-react";
import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { isActive } from "@/features/full-analysis/full-analyses";
import { useFullAnalysisConfirmation } from "@/features/full-analysis/FullAnalysisConfirm";
import { FullAnalysisDialog, reviewSubject } from "@/features/full-analysis/FullAnalysisDialog";
import { type ReviewOpening, viewLabel } from "@/features/review/views";
import { SessionIcon } from "@/features/terminal/icons";
import { whereBranchOpens } from "@/features/terminal/terminal-text";
import { i18n } from "@/i18n";
import type { FullAnalysisJob, ReviewTarget, TerminalKind, Worktree } from "@/lib/api";
import { cn } from "@/lib/utils";

/** The shortcuts into a review, in menu order: each opens the review on its tab (Comments: the Diff's comments). */
const SHORTCUTS: { opening: ReviewOpening; label: () => string }[] = [
  { opening: { view: "Overview" }, label: () => viewLabel("Overview") },
  { opening: { view: "Tests" }, label: () => viewLabel("Tests") },
  { opening: { view: "Insights" }, label: () => viewLabel("Insights") },
  { opening: { view: "Diff", diffMode: "comments" }, label: () => i18n.t("review.toolbar.comments") },
];

interface BranchActionsProps {
  projectId: string;
  branch: string;
  /** The review the row opens; null when the branch has no base, so there is nothing to open. */
  target: ReviewTarget | null;
  /** The latest full analysis of that review in this run of the app, if any. */
  analysis: FullAnalysisJob | null;
  /** The repository's worktrees, to say where the branch's terminal opens. */
  worktrees: Worktree[];
  /** Opens the review, on `opening`'s tab when given, with a terminal session of `terminal`'s kind when given. */
  onReview: (target: ReviewTarget, opening?: ReviewOpening, terminal?: TerminalKind) => void;
  onOpenPreferences?: () => void;
}

/** The terminal items, in menu order. */
const TERMINALS: { kind: TerminalKind; label: () => string }[] = [
  { kind: "shell", label: () => i18n.t("project.actions.openTerminal") },
  { kind: "claude", label: () => i18n.t("project.actions.openClaude") },
];

/**
 * The ⋮ at the end of a branch row, attached to it like the toggle of a split button: a menu of what can be done with
 * the branch without opening its review first. Full analysis… opens its confirmation in a dialog; the shortcuts open
 * the review on a tab; the terminal items open it with its terminal expanded on a new session, each saying under it
 * where that session opens (or that a worktree has to be created first). While a full analysis of the branch runs or
 * waits, the ⋮ spins and the menu says so instead of offering another.
 */
export function BranchActions({
  projectId,
  branch,
  target,
  analysis,
  worktrees,
  onReview,
  onOpenPreferences,
}: BranchActionsProps) {
  const { t } = useTranslation();
  const trigger = useRef<HTMLButtonElement>(null);
  const confirmation = useFullAnalysisConfirmation(projectId);
  const [dialogOpen, setDialogOpen] = useState(false);
  const active = analysis !== null && isActive(analysis);
  const where = whereBranchOpens(branch, worktrees);

  function openFullAnalysis() {
    if (target === null) return;
    setDialogOpen(true);
    void confirmation.plan(() => reviewSubject(projectId, target));
  }

  function closeDialog() {
    setDialogOpen(false);
    confirmation.reset();
  }

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger
          ref={trigger}
          disabled={target === null}
          aria-label={t("project.actions.label", { branch })}
          aria-busy={active || undefined}
          title={target === null ? t("project.row.noBaseToReview") : t("project.actions.label", { branch })}
          className="absolute inset-y-0 right-0 z-10 flex w-8 cursor-pointer items-center justify-center border-l text-muted-foreground outline-none hover:bg-background hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-50 disabled:hover:bg-transparent data-popup-open:bg-background data-popup-open:text-foreground"
        >
          {active ? (
            <LoaderCircle aria-hidden="true" className="size-3.5 animate-spin text-primary" />
          ) : (
            <EllipsisVertical aria-hidden="true" className="size-4" />
          )}
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="min-w-52">
          <DropdownMenuItem disabled={active} onClick={openFullAnalysis}>
            {!active
              ? t("project.actions.fullAnalysis")
              : analysis.status === "queued"
                ? t("project.actions.fullAnalysisQueued")
                : t("project.actions.fullAnalysisRunning")}
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuGroup>
            <DropdownMenuLabel>{t("project.actions.openOn")}</DropdownMenuLabel>
            {SHORTCUTS.map(({ opening, label }) => (
              <DropdownMenuItem
                key={opening.diffMode ?? opening.view}
                onClick={() => {
                  if (target !== null) onReview(target, opening);
                }}
              >
                {label()}
              </DropdownMenuItem>
            ))}
          </DropdownMenuGroup>
          <DropdownMenuSeparator />
          <DropdownMenuGroup>
            {TERMINALS.map(({ kind, label }) => (
              <DropdownMenuItem
                key={kind}
                className="grid grid-cols-[16px_minmax(0,1fr)] gap-x-2 gap-y-px"
                onClick={() => {
                  if (target !== null) onReview(target, undefined, kind);
                }}
              >
                <SessionIcon kind={kind} />
                <span>{label()}</span>
                <small
                  className={cn(
                    "col-start-2 truncate text-faint",
                    where.path ? "font-mono text-[11.5px]" : "text-xs",
                  )}
                >
                  {where.text}
                </small>
              </DropdownMenuItem>
            ))}
          </DropdownMenuGroup>
        </DropdownMenuContent>
      </DropdownMenu>
      <FullAnalysisDialog
        open={dialogOpen}
        onClose={closeDialog}
        confirmation={confirmation}
        subject={branch}
        returnFocus={trigger}
        onOpenPreferences={onOpenPreferences}
      />
    </>
  );
}
