// Words of the Insights tab: the cost shape of an analysis, labels, where a finding is, and the prompt it is copied
// as for a coding agent.

import type {
  Finding,
  FindingCategory,
  FindingVerdict,
  InsightsAnalysis,
  InsightsPlan,
  LocationProblem,
  ModelInUse,
  Severity,
} from "@/lib/api";
import { i18n } from "@/i18n";
import { formatDate, formatNumber, formatRelativeTime, intlLocale } from "@/lib/format";

export const SEVERITIES: readonly Severity[] = ["high", "medium", "low"];

/** "High", in the language shown. */
export function severityName(severity: Severity): string {
  return i18n.t(`insights.severity.${severity}`);
}

/** "Data loss", in the language shown. */
export function categoryName(category: FindingCategory): string {
  return i18n.t(`insights.category.${category}`);
}

export const VERDICTS: readonly FindingVerdict[] = ["pending", "confirmed", "dismissed"];

/** "Confirmed", in the language shown. */
export function verdictName(verdict: FindingVerdict): string {
  return i18n.t(`insights.verdict.${verdict}`);
}

/** "820", "48k", "1.2M" ("1,2M" in Portuguese): an estimate, so rounded. */
export function formatTokens(tokens: number): string {
  if (tokens < 1000) return formatNumber(tokens);
  if (tokens < 999_500) return `${formatNumber(Math.round(tokens / 1000))}k`;
  return `${formatNumber(tokens / 1_000_000, 1)}M`;
}

/** "12 files in 1 call, about 48k tokens in". */
export function planShape(plan: InsightsPlan): string {
  return i18n.t("insights.plan.shape", {
    files: i18n.t("insights.plan.files", { count: plan.files }),
    calls: i18n.t("insights.plan.calls", { count: plan.calls.length }),
    tokens: formatTokens(plan.tokens),
  });
}

/** "Claude Code, model opus". */
export function modelName({ providerName, model }: Pick<ModelInUse, "providerName" | "model">): string {
  return model ? i18n.t("insights.modelName", { provider: providerName, model }) : providerName;
}

/** "1, 2 and 3" ("1, 2 e 3"), British in English like the dates. */
function and(items: readonly string[]): string {
  return new Intl.ListFormat(intlLocale(), { type: "conjunction" }).format(items);
}

/** "2 files are listed to the model but not sent: 1 deleted, 1 binary." or null when every file is sent. */
export function skippedText(plan: InsightsPlan): string | null {
  if (plan.skipped.length === 0) return null;
  const counts = new Map<InsightsPlan["skipped"][number]["reason"], number>();
  for (const file of plan.skipped) counts.set(file.reason, (counts.get(file.reason) ?? 0) + 1);
  const parts = [...counts].map(([reason, count]) => i18n.t(`insights.plan.skip.${reason}`, { count }));
  return i18n.t("insights.plan.skipped", { count: plan.skipped.length, parts: and(parts) });
}

/** "Call 2 of 4", "Calls 1, 2 and 3 of 4" for the calls asked now (0-based indexes). */
export function callsText(active: readonly number[], total: number): string {
  const numbers = [...active].sort((a, b) => a - b).map((index) => String(index + 1));
  if (numbers.length === 1) return i18n.t("insights.calls.one", { number: numbers[0], total });
  return i18n.t("insights.calls.many", { numbers: and(numbers), total });
}

/** "Read by Claude Code, model claude-opus-5-5 at e7f3b10": who wrote an analysis, of which head. */
export function writtenBy(analysis: Pick<InsightsAnalysis, "providerName" | "model" | "headSha">): string {
  const model = modelName(analysis);
  return analysis.headSha
    ? i18n.t("insights.writtenBy.atHead", { model, sha: analysis.headSha.slice(0, 7) })
    : i18n.t("insights.writtenBy.workingTree", { model });
}

/** "internal/refund/service.go:52–56" (":52" for one line), where the finding is now, else where it was cited. */
export function whereText(finding: Finding): string {
  const { line, endLine } = finding.location.kind === "found" ? finding.location : finding;
  return `${finding.path}:${line}${endLine > line ? `–${endLine}` : ""}`;
}

/** "internal/refund/service.go:52": the first line only, as a list row shows it. */
export function startText(finding: Finding): string {
  const { line } = finding.location.kind === "found" ? finding.location : finding;
  return `${finding.path}:${line}`;
}

/** Why a finding cannot be shown in the code, or null when it can. */
export function locationNote(finding: Finding): string | null {
  if (finding.location.kind === "found") return null;
  const problem: LocationProblem = finding.location.problem;
  return i18n.t(`insights.location.${problem}`, { path: finding.path, line: finding.line });
}

/** Marked fixed, by a later analysis or by the user. */
export function isFixed(finding: Finding): boolean {
  return finding.resolution !== null;
}

/**
 * Who said a fixed finding is fixed and when: "Checked by claude-opus-5-5 2 h ago", "Marked by you just now", or that
 * its file left the change; null for an open finding.
 */
export function resolutionByline(finding: Finding, now: Date = new Date()): string | null {
  const resolution = finding.resolution;
  if (resolution === null) return null;
  const when = formatRelativeTime(resolution.at, now);
  switch (resolution.kind) {
    case "model_checked":
      return i18n.t("insights.fixed.checkedBy", { model: resolution.model ?? finding.source.model, when });
    case "user_marked":
      return i18n.t("insights.fixed.markedByYou", { when });
    case "file_left_change":
      return i18n.t("insights.fixed.fileLeft", { when });
  }
}

/** "You confirmed this on 12 Nov. The code around it changed since then, so check it again." or null. */
export function recheckNote(finding: Finding): string | null {
  if (!finding.recheck || finding.verdict === "pending") return null;
  const did = finding.verdict === "confirmed" ? "confirmed" : "dismissed";
  const verdict = finding.verdictAt
    ? i18n.t(`insights.recheck.${did}On`, { date: formatDate(finding.verdictAt) })
    : i18n.t(`insights.recheck.${did}`);
  const what =
    finding.location.kind === "found" ? i18n.t("insights.recheck.codeChanged") : i18n.t("insights.recheck.codeGone");
  return `${verdict} ${what}`;
}

/**
 * The finding as a self-contained prompt for a coding agent: where, the problem, what shows it, the code as it is now
 * and the suggested direction, asking for a fix and a test where one fits.
 */
export function agentPrompt(finding: Finding): string {
  const parts: string[] = [];
  const found = finding.location.kind === "found";
  // The instructions and labels follow the language shown; what the model wrote goes in as it is.
  parts.push(
    found
      ? i18n.t("insights.prompt.fixAt", { where: whereText(finding) })
      : i18n.t("insights.prompt.fixGone", { path: finding.path, line: finding.line }),
  );
  parts.push(`${i18n.t("insights.prompt.problem")} ${finding.title}\n${finding.explanation}`);
  if (finding.evidence.length > 0) {
    parts.push(`${i18n.t("insights.prompt.evidence")}\n${bullets(finding.evidence)}`);
  }
  if (finding.related.length > 0) parts.push(`${i18n.t("insights.prompt.related")}\n${bullets(finding.related)}`);
  if (finding.excerpt.length > 0) {
    const width = String(finding.excerpt[finding.excerpt.length - 1].line).length;
    const code = finding.excerpt.map((line) => `${String(line.line).padStart(width)}  ${line.text}`).join("\n");
    parts.push(`${i18n.t("insights.prompt.codeNow", { where: whereText(finding) })}\n\`\`\`\n${code}\n\`\`\``);
  }
  if (finding.suggestedDirection) {
    parts.push(`${i18n.t("insights.prompt.suggestedDirection")} ${finding.suggestedDirection}`);
  }
  parts.push(i18n.t("insights.prompt.closing"));
  return parts.join("\n\n");
}

function bullets(items: readonly string[]): string {
  return items.map((item) => `- ${item}`).join("\n");
}
