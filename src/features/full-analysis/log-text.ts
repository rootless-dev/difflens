// The words of a full analysis: its log lines, sent by the core as codes with values and worded here in the language
// shown, and the label of the steps in progress.
import { i18n } from "@/i18n";
import type { AnalysisStep, FullAnalysisJob, LogEvent } from "@/lib/api";
import { errorMessage, intlLocale } from "@/lib/format";

export const stepName = (step: AnalysisStep) => i18n.t(`fullAnalysis.steps.${step}`);

function stepList(steps: readonly AnalysisStep[]): string {
  return new Intl.ListFormat(intlLocale(), { type: "conjunction" }).format(steps.map(stepName));
}

/** "3 changed functions": a full analysis explains no function of the context. */
export function functionsText(changed: number): string {
  return i18n.t("fullAnalysis.confirm.changed", { count: changed });
}

/** One line of the log, in the language shown. */
export function logText(event: LogEvent): string {
  const t = i18n.t;
  switch (event.code) {
    case "tests_finished": {
      const values = {
        passed: t("fullAnalysis.log.passed", { count: event.passed }),
        failed: t("fullAnalysis.log.failed", { count: event.failed }),
        skipped: t("fullAnalysis.log.skipped", { count: event.skipped }),
        seconds: Math.round(event.durationMs / 1000),
      };
      return event.skipped > 0
        ? t("fullAnalysis.log.tests_finished_skipped", values)
        : t("fullAnalysis.log.tests_finished", values);
    }
    case "flow_measured":
      return t("fullAnalysis.log.flow_measured", { count: event.changed });
    case "quality_measured":
      return t("fullAnalysis.log.quality_measured", { count: event.crossed });
    case "coverage_measured":
      return t("fullAnalysis.log.coverage_measured", { count: event.crossed });
    case "insights_planned":
      return t("fullAnalysis.log.insights_planned", {
        files: t("fullAnalysis.confirm.files", { count: event.files }),
        calls: t("fullAnalysis.confirm.calls", { count: event.calls }),
      });
    case "insights_call_started":
    case "explanations_call_started":
      return t(`fullAnalysis.log.${event.code}`, { number: event.number, total: event.total });
    case "insights_call_answered":
      return t("fullAnalysis.log.insights_call_answered", {
        number: event.number,
        total: event.total,
        findings: t("fullAnalysis.log.findings", { count: event.findings }),
      });
    case "insights_call_failed":
    case "explanations_call_failed":
      return t(`fullAnalysis.log.${event.code}`, {
        number: event.number,
        total: event.total,
        error: errorMessage(event.error),
      });
    case "explanations_planned":
      return t("fullAnalysis.log.explanations_planned", {
        functions: functionsText(event.changed),
        calls: t("fullAnalysis.confirm.calls", { count: event.calls }),
      });
    case "explanations_call_answered":
      return t("fullAnalysis.log.explanations_call_answered", {
        number: event.number,
        total: event.total,
        functions: t("fullAnalysis.log.explained", { count: event.functions }),
      });
    case "step_failed":
      return t("fullAnalysis.log.step_failed", { step: stepName(event.step), error: errorMessage(event.error) });
    case "step_cancelled":
      return t("fullAnalysis.log.step_cancelled", { step: stepName(event.step) });
    case "finished_with_failures":
      return t("fullAnalysis.log.finished_with_failures", { steps: stepList(event.steps) });
    default:
      return t(`fullAnalysis.log.${event.code}`);
  }
}

/** "14:03:27", the time of a log line. */
export function logTime(at: string): string {
  const date = new Date(at);
  if (Number.isNaN(date.getTime())) return "";
  return new Intl.DateTimeFormat(intlLocale(), {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).format(date);
}

/** What the progress bar says the job is doing now: the steps in progress, each with its calls, or why it waits. */
export function currentLabel(job: FullAnalysisJob): string {
  if (job.status === "queued") return i18n.t("fullAnalysis.screen.queued");
  if (job.cancelling) return i18n.t("fullAnalysis.screen.cancelling");
  const running = job.steps.filter((step) => step.status === "running");
  if (running.length === 0) return i18n.t("fullAnalysis.screen.starting");
  return running
    .map((step) =>
      step.total > 0
        ? i18n.t("fullAnalysis.screen.stepProgress", { step: stepName(step.step), done: step.done, total: step.total })
        : stepName(step.step),
    )
    .join(" · ");
}
