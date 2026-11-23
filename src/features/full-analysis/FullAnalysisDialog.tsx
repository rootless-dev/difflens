import type { RefObject } from "react";
import { useTranslation } from "react-i18next";

import { ErrorNote } from "@/components/page";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  type FullAnalysisConfirmation,
  FullAnalysisStep,
  type FullAnalysisSubject,
} from "@/features/full-analysis/FullAnalysisConfirm";
import { pullRequestText } from "@/features/review/overview/use-overview";
import { reviewRefOf } from "@/features/review/review-state/review-status";
import { linkPullRequest, listChangedFiles, type ReviewTarget } from "@/lib/api";

/**
 * What a review opened on `target` would show a full analysis: its change set, read the way opening the review reads
 * it, and its pull request, looked for the same way. Without a pull request (none, not connected, unreadable) the
 * analysis is still offered, with none.
 */
export async function reviewSubject(projectId: string, target: ReviewTarget): Promise<FullAnalysisSubject> {
  const review = reviewRefOf(await listChangedFiles(projectId, target));
  if (target.kind === "working_tree") return { review, pullRequest: null };
  const pullRequest = await linkPullRequest(projectId, review).then(pullRequestText, () => null);
  return { review, pullRequest: pullRequest ?? null };
}

interface FullAnalysisDialogProps {
  open: boolean;
  onClose: () => void;
  /** Planned by whoever opened the dialog; started only by the confirmation's button. */
  confirmation: FullAnalysisConfirmation;
  /** What the analysis is of, in the title, such as a branch name. */
  subject: string;
  /** Where focus goes back once the dialog closes. */
  returnFocus: RefObject<HTMLElement | null>;
  onOpenPreferences?: () => void;
}

/**
 * The confirmation of a full analysis in a dialog, outside the review: the same one the Overview's Full analysis
 * button opens. It closes once the analysis started (or was queued); Cancel and Escape close it without starting.
 */
export function FullAnalysisDialog({
  open,
  onClose,
  confirmation,
  subject,
  returnFocus,
  onOpenPreferences,
}: FullAnalysisDialogProps) {
  const { t } = useTranslation();
  const { step } = confirmation;
  const starting = step.kind === "starting";

  async function start() {
    if (await confirmation.start()) onClose();
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        // Once started, the dialog waits for the core's answer.
        if (!next && !starting) onClose();
      }}
    >
      <DialogContent finalFocus={returnFocus}>
        <DialogHeader>
          <DialogTitle>{t("fullAnalysis.dialog.title", { subject })}</DialogTitle>
          <DialogDescription>{t("fullAnalysis.intro")}</DialogDescription>
        </DialogHeader>
        <FullAnalysisStep
          step={step}
          onStart={() => void start()}
          onCancel={onClose}
          onOpenPreferences={onOpenPreferences}
        />
        {step.kind === "error" && (
          <>
            <ErrorNote>{step.message}</ErrorNote>
            <DialogFooter>
              <DialogClose>{t("common.actions.close")}</DialogClose>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}
