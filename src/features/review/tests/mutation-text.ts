// Words around a mutation run: its score, and the prompt a mutant that survived is copied as for a coding agent.

import { mutantLines, oneLine, operatorName } from "@/features/review/signals/signals-text";
import { allOf } from "@/features/review/tests/parts";
import { i18n } from "@/i18n";
import type { Mutant, MutationRun, MutationSummary } from "@/lib/api";

/** Tests named in a prompt as catching other changes nearby, at most. */
const NEARBY_TESTS = 5;

/**
 * The score of the change: mutants killed out of those whose tests ended one way or another (killed, survived or
 * timed out); errors and mutants no test runs say nothing about the tests and are left out.
 */
export function mutationScore(summary: MutationSummary): { killed: number; total: number } {
  return { killed: summary.killed, total: summary.killed + summary.survived + summary.timeout };
}

/** "internal/refund/service.go:52". */
export function mutantPlace(mutant: Mutant): string {
  return `${mutant.file}:${mutant.line}`;
}

/** The lines a mutant spans as a diff, "- " as they are and "+ " as mutated; the pieces alone when unread. */
export function mutantDiff(mutant: Mutant): string {
  const before = mutant.before.length > 0 ? mutant.before : [mutant.original];
  const after = mutant.after.length > 0 || mutant.before.length > 0 ? mutant.after : [mutant.mutated];
  return [...before.map((line) => `- ${line}`), ...after.map((line) => `+ ${line}`)].join("\n");
}

/** Tests that killed another mutant of the same function, each once, in the run's order. */
function nearbyTests(mutant: Mutant, run: MutationRun | null): string[] {
  if (!run || !mutant.function) return [];
  const id = mutant.function.id;
  const names = run.mutants
    .filter((other) => other.status === "killed" && other.killedBy !== null && other.function?.id === id)
    .map((other) => other.killedBy as string);
  return [...new Set(names)].slice(0, NEARBY_TESTS);
}

/**
 * A mutant that survived as a self-contained prompt for a coding agent: where it is, the line as it is and as
 * mutated, that no test failed, the tests that catch other changes of its function when the run tells, and the ask:
 * a test that fails with the change and passes without it, production code untouched. The instructions and labels
 * follow the language shown; the code and the engine's words go in as they are.
 */
export function mutantPrompt(mutant: Mutant, run: MutationRun | null): string {
  const place = mutantPlace(mutant);
  const where = mutant.function
    ? i18n.t("tests.mutation.prompt.whereIn", { place, function: mutant.function.qualifiedName })
    : place;
  const parts: string[] = [];
  parts.push(i18n.t("tests.mutation.prompt.write", { where }));
  parts.push(i18n.t("tests.mutation.prompt.intro"));
  parts.push(
    `${i18n.t("tests.mutation.prompt.change", { operator: operatorName(mutant.operator).toLowerCase() })}\n` +
      `\`\`\`diff\n${mutantDiff(mutant)}\n\`\`\``,
  );
  if (oneLine(mutant.description) !== "") {
    parts.push(i18n.t("tests.mutation.prompt.engine", { description: oneLine(mutant.description) }));
  }
  const nearby = nearbyTests(mutant, run);
  if (nearby.length > 0) parts.push(i18n.t("tests.mutation.prompt.nearby", { tests: allOf(nearby) }));
  parts.push(i18n.t("tests.mutation.prompt.closing"));
  return parts.join("\n\n");
}

/** The one-line form of a mutant's change, for a list: "`>=` → `>`", or the whole lines when they were read. */
export function mutantShortChange(mutant: Mutant): string {
  const { before, after } = mutantLines(mutant);
  return after === "" ? `\`${before}\` → —` : `\`${before}\` → \`${after}\``;
}
