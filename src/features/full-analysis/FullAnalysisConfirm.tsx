import { useCallback, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { Hint } from "@/components/page";
import { Button } from "@/components/ui/button";
import { useFullAnalyses } from "@/features/full-analysis/full-analyses";
import { functionsText } from "@/features/full-analysis/log-text";
import { Spinner } from "@/features/review/flow/FlowView";
import { formatTokens, modelName } from "@/features/review/insights/insights-text";
import { i18n } from "@/i18n";
import {
  type FullAnalysisPlan,
  type ModelInUse,
  planFullAnalysis,
  type PullRequestText,
  type ReviewRef,
} from "@/lib/api";
import { modelProblem } from "@/lib/core-text";
import { errorMessage } from "@/lib/format";
import { cn } from "@/lib/utils";

/** What a full analysis is about: the review as its change set froze it, and its pull request (null without one). */
export interface FullAnalysisSubject {
  review: ReviewRef;
  pullRequest: PullRequestText | null;
}

/**
 * Where the confirmation of a full analysis is. What is confirmed is what was planned: the start sends the same
 * review and pull request the plan was worked out for.
 */
export type ConfirmationStep =
  | { kind: "idle" }
  | { kind: "planning" }
  | { kind: "confirm"; subject: FullAnalysisSubject; plan: FullAnalysisPlan }
  | { kind: "starting"; subject: FullAnalysisSubject; plan: FullAnalysisPlan }
  | { kind: "error"; message: string };

export interface FullAnalysisConfirmation {
  step: ConfirmationStep;
  /** Works out what a full analysis of what `subject` resolves to would do and send; asks no model. */
  plan: (subject: () => Promise<FullAnalysisSubject>) => Promise<void>;
  /** Starts (or queues) what was planned; resolves to whether it started. The only way one starts from here. */
  start: () => Promise<boolean>;
  /** Back to idle; an answer still on its way is then ignored. */
  reset: () => void;
}

/**
 * The steps of a full analysis's confirmation, shared by the Overview's offer and a branch row's dialog: plan, show
 * what runs and what is sent, and start only on the confirmation's button.
 */
export function useFullAnalysisConfirmation(projectId: string): FullAnalysisConfirmation {
  const { t } = useTranslation();
  const analyses = useFullAnalyses();
  const [step, setStep] = useState<ConfirmationStep>({ kind: "idle" });
  // Only the answer to the latest request is kept.
  const latest = useRef(0);

  const plan = useCallback(
    async (subject: () => Promise<FullAnalysisSubject>) => {
      const request = ++latest.current;
      setStep({ kind: "planning" });
      try {
        const resolved = await subject();
        const planned = await planFullAnalysis(projectId, resolved.review, resolved.pullRequest);
        if (request === latest.current) setStep({ kind: "confirm", subject: resolved, plan: planned });
      } catch (cause: unknown) {
        if (request === latest.current) {
          setStep({ kind: "error", message: t("fullAnalysis.confirm.failed", { message: errorMessage(cause) }) });
        }
      }
    },
    [projectId, t],
  );

  const start = useCallback(async () => {
    if (step.kind !== "confirm") return false;
    const { subject, plan: planned } = step;
    const request = ++latest.current;
    setStep({ kind: "starting", subject, plan: planned });
    try {
      await analyses.start(projectId, subject.review, subject.pullRequest);
      if (request === latest.current) setStep({ kind: "idle" });
      return true;
    } catch (cause: unknown) {
      if (request === latest.current) setStep({ kind: "error", message: errorMessage(cause) });
      return false;
    }
  }, [step, analyses, projectId]);

  const reset = useCallback(() => {
    latest.current++;
    setStep({ kind: "idle" });
  }, []);

  return { step, plan, start, reset };
}

interface FullAnalysisStepProps {
  step: ConfirmationStep;
  onStart: () => void;
  onCancel: () => void;
  onOpenPreferences?: () => void;
  className?: string;
}

/**
 * What the confirmation shows while it is worked out and once it is: what runs, what is sent, to whom and about how
 * much, or why it cannot start. Nothing while idle or after an error, which each place shows its own way.
 */
export function FullAnalysisStep({ step, onStart, onCancel, onOpenPreferences, className }: FullAnalysisStepProps) {
  const { t } = useTranslation();
  if (step.kind === "planning") {
    return (
      <p role="status" className={cn("flex items-center gap-2 text-muted-foreground", className)}>
        <Spinner />
        {t("fullAnalysis.confirm.working")}
      </p>
    );
  }
  if (step.kind === "confirm" || step.kind === "starting") {
    return (
      <Confirm
        plan={step.plan}
        starting={step.kind === "starting"}
        onStart={onStart}
        onCancel={onCancel}
        onOpenPreferences={onOpenPreferences}
        className={className}
      />
    );
  }
  return null;
}

interface ConfirmProps {
  plan: FullAnalysisPlan;
  starting: boolean;
  onStart: () => void;
  onCancel: () => void;
  onOpenPreferences?: () => void;
  className?: string;
}

/** A task whose model was chosen for it says on its own what is wrong; the default model's is said to be missing. */
function notReadyText(model: ModelInUse): string {
  const problem = modelProblem(model);
  if (problem !== null && model.problem?.reason === "task_provider_not_set_up") return problem;
  return i18n.t("fullAnalysis.confirm.notReady", { problem: problem ?? i18n.t("fullAnalysis.confirm.chooseModel") });
}

function Confirm({ plan, starting, onStart, onCancel, onOpenPreferences, className }: ConfirmProps) {
  const { t } = useTranslation();
  // Each part asks the model of its own task: it starts only when every one of them can be asked.
  const models = [plan.insights.model, plan.summary.model, plan.explanations.model];
  const notReady = models.find((model) => !model.ready);
  if (notReady) {
    return (
      <div className={className}>
        <p>{notReadyText(notReady)}</p>
        <div className="mt-2.5 flex gap-2">
          {onOpenPreferences && (
            <Button variant="secondary" size="sm" onClick={onOpenPreferences}>
              {t("fullAnalysis.confirm.openPreferences")}
            </Button>
          )}
          <Button variant="outline" size="sm" onClick={onCancel}>
            {t("common.actions.cancel")}
          </Button>
        </div>
      </div>
    );
  }
  const { insights, summary, explanations } = plan;
  const calls = (count: number) => t("fullAnalysis.confirm.calls", { count });
  const lines = [
    plan.tests ? t("fullAnalysis.confirm.tests") : t("fullAnalysis.confirm.noTests"),
    t("fullAnalysis.confirm.metrics"),
    insights.calls.length > 0
      ? t("fullAnalysis.confirm.insights", {
          files: t("fullAnalysis.confirm.files", { count: insights.files }),
          calls: calls(insights.calls.length),
          tokens: formatTokens(insights.tokens),
          model: modelName(insights.model),
        })
      : t("fullAnalysis.confirm.insightsNothing"),
    summary.files > 0
      ? t("fullAnalysis.confirm.summary", { tokens: formatTokens(summary.tokens), model: modelName(summary.model) })
      : t("fullAnalysis.confirm.summaryNothing"),
    explanations.calls.length > 0
      ? t("fullAnalysis.confirm.explanations", {
          functions: functionsText(explanations.changed),
          calls: calls(explanations.calls.length),
          tokens: formatTokens(explanations.tokens),
          model: modelName(explanations.model),
        })
      : t("fullAnalysis.confirm.explanationsNothing"),
  ];
  const names = new Set(models.map(modelName));
  return (
    <div className={cn("rounded-[8px] border bg-card px-3.5 py-3", className)}>
      <ul aria-label={t("fullAnalysis.title")} className="grid list-disc gap-1 pl-[18px]">
        {lines.map((line) => (
          <li key={line}>{line}</li>
        ))}
      </ul>
      <Hint>
        {names.size === 1
          ? t("fullAnalysis.confirm.sentTo", { model: modelName(plan.insights.model) })
          : t("fullAnalysis.confirm.sentToEach")}
      </Hint>
      {plan.running >= plan.limit && (
        <Hint className="mt-1">
          {t("fullAnalysis.confirm.queued", { running: plan.running, limit: plan.limit })}
        </Hint>
      )}
      <div className="mt-3 flex gap-2">
        <Button disabled={starting} onClick={onStart}>
          {t("fullAnalysis.confirm.start")}
        </Button>
        <Button variant="outline" disabled={starting} onClick={onCancel}>
          {t("common.actions.cancel")}
        </Button>
      </div>
    </div>
  );
}
