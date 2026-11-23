import { useTranslation } from "react-i18next";

import { ErrorNote } from "@/components/page";
import { Button } from "@/components/ui/button";
import { FullAnalysisStep, useFullAnalysisConfirmation } from "@/features/full-analysis/FullAnalysisConfirm";
import { reviewRefOf } from "@/features/review/review-state/review-status";
import type { ChangeSet, PullRequestText } from "@/lib/api";

interface FullAnalysisOfferProps {
  projectId: string;
  changeSet: ChangeSet | null;
  /** undefined while the pull request is still being looked for. */
  pullRequest: PullRequestText | null | undefined;
  /** A model call of the screen is in progress (Analyze, Summarize): a full analysis waits for it. */
  busy: boolean;
  onOpenPreferences?: () => void;
}

/**
 * The Overview's Full analysis button, and the short confirmation it opens, like Analyze's: what runs, what is sent,
 * to whom and about how much. Only the confirmation's button starts anything.
 */
export function FullAnalysisOffer({ projectId, changeSet, pullRequest, busy, onOpenPreferences }: FullAnalysisOfferProps) {
  const { t } = useTranslation();
  const confirmation = useFullAnalysisConfirmation(projectId);
  const { step } = confirmation;
  const ready = changeSet !== null && pullRequest !== undefined;

  function openPlan() {
    if (changeSet === null || pullRequest === undefined) return;
    const subject = { review: reviewRefOf(changeSet), pullRequest };
    void confirmation.plan(() => Promise.resolve(subject));
  }

  return (
    <div>
      <p className="max-w-[56ch] text-muted-foreground">{t("fullAnalysis.intro")}</p>
      {(step.kind === "idle" || step.kind === "error") && (
        <>
          <Button
            variant="secondary"
            className="mt-3"
            disabled={!ready || busy}
            title={busy ? t("fullAnalysis.busy") : undefined}
            onClick={openPlan}
          >
            {t("fullAnalysis.button")}
          </Button>
          {step.kind === "error" && <ErrorNote className="mt-2.5">{step.message}</ErrorNote>}
        </>
      )}
      <FullAnalysisStep
        step={step}
        onStart={() => void confirmation.start()}
        onCancel={confirmation.reset}
        onOpenPreferences={onOpenPreferences}
        className="mt-3"
      />
    </div>
  );
}
