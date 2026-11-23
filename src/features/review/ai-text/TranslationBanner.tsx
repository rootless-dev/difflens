import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { ErrorNote } from "@/components/page";
import { Button } from "@/components/ui/button";
import { i18n } from "@/i18n";
import { cancelModelRun, isApiError, type Language, type ModelInUse, type TranslationPlan } from "@/lib/api";
import { modelProblem } from "@/lib/core-text";
import { errorMessage, formatNumber } from "@/lib/format";
import { cn } from "@/lib/utils";

/** One call of a translation, as the banner runs it: `runId` lets Stop cancel it. */
export type RunTranslationCall = (call: TranslationPlan["calls"][number], runId: string) => Promise<void>;

interface TranslationBannerProps {
  /**
   * What was written: some findings, explanations of functions or explanations of failed tests (how many are not in
   * `language`), or the summary.
   */
  subject:
    | { kind: "findings"; count: number }
    | { kind: "explanations"; count: number }
    | { kind: "diagnoses"; count: number }
    | { kind: "summary" };
  /** The language the text shown is in. */
  writtenIn: Language;
  /** What Translate would send, in the language shown now; asks no model. */
  plan: () => Promise<TranslationPlan>;
  /** Asks the model one call of the plan, only after the click on Translate in the confirmation. */
  runCall: RunTranslationCall;
  /** Analyze again or Summarize again, in the new language; none for explanations, each explained again on its box. */
  redo?: { label: string; onClick: () => void; disabled?: boolean };
  className?: string;
}

type Step =
  | { kind: "idle" }
  | { kind: "planning" }
  | { kind: "confirm"; plan: TranslationPlan }
  | { kind: "running"; plan: TranslationPlan; stopping: boolean }
  | { kind: "error"; message: string };

let runs = 0;

function modelName(model: ModelInUse): string {
  return model.model
    ? i18n.t("aiText.model", { provider: model.providerName, model: model.model })
    : model.providerName;
}

/**
 * A discreet line over text the model wrote in another language than the one shown: it says which, and offers
 * Translate (a model call, only on click, after showing who would be asked) or doing the analysis again. Changing the
 * language itself never asks a model.
 */
export function TranslationBanner({ subject, writtenIn, plan, runCall, redo, className }: TranslationBannerProps) {
  const { t } = useTranslation();
  const [step, setStep] = useState<Step>({ kind: "idle" });
  const active = useRef<Set<string>>(new Set());
  const stopped = useRef(false);

  const writtenInName = t(`aiText.languageInSentence.${writtenIn}`);
  const message =
    subject.kind === "summary"
      ? t("aiText.writtenIn.summary", { language: writtenInName })
      : subject.kind === "explanations"
        ? t("aiText.writtenIn.explanations", { count: subject.count, language: writtenInName })
        : subject.kind === "diagnoses"
          ? t("aiText.writtenIn.diagnoses", { count: subject.count, language: writtenInName })
          : t("aiText.writtenIn.findings", { count: subject.count, language: writtenInName });

  async function openPlan() {
    setStep({ kind: "planning" });
    try {
      setStep({ kind: "confirm", plan: await plan() });
    } catch (cause: unknown) {
      setStep({ kind: "error", message: i18n.t("aiText.failed", { message: errorMessage(cause) }) });
    }
  }

  async function translate(chosen: TranslationPlan) {
    setStep({ kind: "running", plan: chosen, stopping: false });
    stopped.current = false;
    const queue = [...chosen.calls];
    const parallel = Math.max(1, chosen.model.parallelCalls);
    let failure: unknown = null;
    async function worker() {
      while (queue.length > 0 && failure === null && !stopped.current) {
        const call = queue.shift();
        if (!call) return;
        const runId = `translate-${++runs}`;
        active.current.add(runId);
        try {
          await runCall(call, runId);
        } catch (cause: unknown) {
          failure ??= cause;
        } finally {
          active.current.delete(runId);
        }
      }
    }
    await Promise.all(Array.from({ length: Math.min(parallel, chosen.calls.length) }, worker));
    if (failure !== null && !(isApiError(failure) && failure.code === "cancelled")) {
      setStep({ kind: "error", message: i18n.t("aiText.failed", { message: errorMessage(failure) }) });
    } else {
      setStep({ kind: "idle" });
    }
  }

  function stop() {
    stopped.current = true;
    setStep((current) => (current.kind === "running" ? { ...current, stopping: true } : current));
    for (const runId of active.current) {
      void cancelModelRun(runId);
    }
  }

  const busy = step.kind === "planning" || step.kind === "running";

  return (
    <section
      aria-label={t("aiText.label")}
      className={cn("mb-3 rounded-md border border-dashed px-3 py-2 text-sm text-muted-foreground", className)}
    >
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <span className="min-w-0 flex-1">{message}</span>
        {step.kind !== "confirm" && step.kind !== "running" && (
          <>
            <Button variant="secondary" size="sm" disabled={busy} onClick={() => void openPlan()}>
              {t("aiText.translate")}
            </Button>
            {redo && (
              <Button variant="outline" size="sm" disabled={redo.disabled} onClick={redo.onClick}>
                {redo.label}
              </Button>
            )}
          </>
        )}
      </div>
      {step.kind === "planning" && <p role="status" className="mt-1.5">{t("aiText.planning")}</p>}
      {step.kind === "confirm" && <Confirm plan={step.plan} subject={subject} onCancel={() => setStep({ kind: "idle" })} onTranslate={() => void translate(step.plan)} />}
      {step.kind === "running" && (
        <div role="status" className="mt-1.5 flex flex-wrap items-center gap-3">
          <span>
            {step.stopping ? t("aiText.stopping") : t("aiText.translating", { model: modelName(step.plan.model) })}
          </span>
          <Button variant="outline" size="sm" disabled={step.stopping} onClick={stop}>
            {t("aiText.stop")}
          </Button>
        </div>
      )}
      {step.kind === "error" && <ErrorNote className="mt-1.5">{step.message}</ErrorNote>}
    </section>
  );
}

interface ConfirmProps {
  plan: TranslationPlan;
  subject: TranslationBannerProps["subject"];
  onCancel: () => void;
  onTranslate: () => void;
}

/** Who would be asked and what would be sent, before anything is: like Analyze's. */
function Confirm({ plan, subject, onCancel, onTranslate }: ConfirmProps) {
  const { t } = useTranslation();
  if (!plan.model.ready) {
    return (
      <div className="mt-1.5 flex flex-wrap items-center gap-3">
        <ErrorNote>{t("aiText.notReady", { problem: modelProblem(plan.model) ?? t("aiText.chooseModel") })}</ErrorNote>
        <Button variant="outline" size="sm" onClick={onCancel}>
          {t("common.actions.cancel")}
        </Button>
      </div>
    );
  }
  const values = {
    model: modelName(plan.model),
    tokens: formatNumber(plan.tokens),
    calls: t("aiText.calls", { count: plan.calls.length }),
  };
  return (
    <div className="mt-1.5 grid gap-2">
      <p>
        {subject.kind === "summary"
          ? t("aiText.confirm.summary", values)
          : subject.kind === "explanations"
            ? t("aiText.confirm.explanations", { ...values, count: plan.items })
            : subject.kind === "diagnoses"
              ? t("aiText.confirm.diagnoses", { ...values, count: plan.items })
              : t("aiText.confirm.findings", { ...values, count: plan.items })}
      </p>
      <div className="flex gap-2">
        <Button size="sm" onClick={onTranslate}>
          {t("aiText.translate")}
        </Button>
        <Button variant="outline" size="sm" onClick={onCancel}>
          {t("common.actions.cancel")}
        </Button>
      </div>
    </div>
  );
}
