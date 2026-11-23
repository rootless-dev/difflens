// Words around an explanation of a failed test: which failure it is, the origin as a chip names it, and the prompt it
// is copied as for a coding agent.

import type { ShownDiagnosis } from "@/features/review/ai-text/versions";
import { failureOf } from "@/features/review/tests/FailureCard";
import type { Failure } from "@/features/review/tests/suite";
import { i18n } from "@/i18n";
import type { DiagnosisFault, FailureFrame, RunSelection, TestFailure } from "@/lib/api";

/** The key the core keeps a failure's explanation under: "<package>::<test>", "<package>::" for a package. */
export function failureKey(failure: Failure): string {
  return failure.kind === "test" ? `${failure.test.package}::${failure.test.name}` : `${failure.pkg.name}::`;
}

/** The package and top-level test the core is asked about; test null for a package. */
export function failureSubject(failure: Failure): { pkg: string; test: string | null } {
  return failure.kind === "test" ? { pkg: failure.test.package, test: failure.test.name } : { pkg: failure.pkg.name, test: null };
}

/** "code", "test", "flaky", "environment", in the language shown. */
export function faultName(fault: DiagnosisFault): string {
  return i18n.t(`tests.diagnosis.fault.${fault}`);
}

/** What runs the failure again: its top-level test, or its package. */
function selectionOf(failure: Failure): RunSelection {
  return failure.kind === "test"
    ? { packages: [{ package: failure.test.package, tests: [{ name: failure.test.name, subtests: null }] }] }
    : { packages: [{ package: failure.pkg.name, tests: null }] };
}

function failureOfCard(failure: Failure): TestFailure | null {
  return failure.kind === "test" ? failureOf(failure.test) : (failure.pkg.outcome?.failure ?? null);
}

function kindOf(failure: Failure): string {
  if (failure.kind === "package") {
    return i18n.t(failure.pkg.didNotBuild ? "tests.failures.kind.build" : "tests.failures.kind.outside");
  }
  return i18n.t(`tests.failures.kind.${failureOf(failure.test)?.kind ?? "other"}`);
}

const at = (frame: FailureFrame) =>
  frame.function ? `${frame.file}:${frame.line} (${frame.function})` : `${frame.file}:${frame.line}`;

/** The key part of the failure, as the card shows it, in plain text. */
function excerpt(failure: TestFailure | null): string {
  switch (failure?.kind) {
    case "race":
      return (failure.races[0]?.accesses ?? [])
        .map((access) => {
          const verb = access.kind === "write" ? i18n.t("tests.race.write") : i18n.t("tests.race.read");
          const where = access.frame ? at(access.frame) : "—";
          const created = access.createdAt ? `, ${i18n.t("tests.race.createdAt")} ${at(access.createdAt)}` : "";
          return `- ${verb}: ${where}${created}`;
        })
        .join("\n");
    case "assertion":
      return [
        failure.message,
        failure.expected !== null ? `${i18n.t("tests.assertion.expected")}: ${failure.expected}` : null,
        failure.actual !== null ? `${i18n.t("tests.assertion.actual")}: ${failure.actual}` : null,
        failure.at ? `${failure.at.file}:${failure.at.line}` : null,
      ]
        .filter((line): line is string => line !== null)
        .join("\n");
    case "panic":
      return `panic: ${failure.message}${failure.frame ? `\n${at(failure.frame)}` : ""}`;
    case "timeout":
      return `${failure.message}${failure.frame ? `\n${at(failure.frame)}` : ""}`;
    case "build":
      return failure.errors
        .slice(0, 5)
        .map((error) => {
          const place = error.file !== null && error.line !== null ? `${error.file}:${error.line}: ` : "";
          return [`${place}${error.message}`, ...error.detail.map((line) => `\t${line}`)].join("\n");
        })
        .join("\n");
    case "other":
      return failure.lines.join("\n");
    case undefined:
      return "";
  }
}

/**
 * The explanation as a self-contained prompt for a coding agent: which test, the failure, why it fails (said to be a
 * model's diagnosis), where and how to fix it, and how to check, asking for the smallest fix. The instructions and
 * labels follow the language shown; what the model wrote goes in as shown. `commands` gives the shell lines that run
 * a selection again, in the run's language.
 */
export function diagnosisPrompt(
  failure: Failure,
  diagnosis: ShownDiagnosis,
  commands: (selection: RunSelection) => string[],
): string {
  const parts: string[] = [];
  parts.push(
    failure.kind === "test"
      ? i18n.t("tests.diagnosis.prompt.fixTest", { test: failure.test.name, package: failure.test.package })
      : i18n.t("tests.diagnosis.prompt.fixPackage", { package: failure.pkg.name }),
  );
  const summary = excerpt(failureOfCard(failure));
  parts.push(`${i18n.t("tests.diagnosis.prompt.failure", { kind: kindOf(failure) })}${summary ? `\n${summary}` : ""}`);
  parts.push(`${i18n.t("tests.diagnosis.prompt.cause")} ${diagnosis.cause}`);
  parts.push(i18n.t("tests.diagnosis.prompt.origin", { origin: faultName(diagnosis.fault) }));
  if (diagnosis.location) {
    parts.push(
      i18n.t("tests.diagnosis.prompt.where", { where: `${diagnosis.location.file}:${diagnosis.location.line}` }),
    );
  }
  if (diagnosis.fix || diagnosis.fixCode) {
    const code = diagnosis.fixCode ? `\n\`\`\`\n${diagnosis.fixCode}\n\`\`\`` : "";
    parts.push(`${i18n.t("tests.diagnosis.prompt.fix")} ${diagnosis.fix}${code}`.trim());
  }
  const lines = commands(selectionOf(failure));
  if (lines.length > 0) parts.push(`${i18n.t("tests.diagnosis.prompt.run")}\n${lines.join("\n")}`);
  parts.push(i18n.t("tests.diagnosis.prompt.closing"));
  return parts.join("\n\n");
}
