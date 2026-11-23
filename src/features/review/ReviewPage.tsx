// Building blocks shared by the review tabs laid out as a page (Tests, Quality): the two-column page, its section
// headings, output blocks, coverage bars and the list of tool problems.

import { type ComponentProps, type ReactNode, useId } from "react";
import { useTranslation } from "react-i18next";

import type { DiffTarget } from "@/features/review/flow/flow-facts";
import { i18n } from "@/i18n";
import type { ChangedFile, CoreMessage, LineCoverage } from "@/lib/api";
import { coreText } from "@/lib/core-text";
import { cn } from "@/lib/utils";

/** The prototype's `.page`: a wide column and a narrow one, centred, scrolling as a whole. */
export function ReviewPage({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      className={cn(
        "grid min-h-0 flex-1 auto-rows-min grid-cols-[minmax(0,1.5fr)_minmax(0,1fr)] gap-x-[52px] gap-y-2 overflow-auto px-[max(28px,calc(50%-600px))] pt-7 pb-12",
        className,
      )}
      {...props}
    />
  );
}

/** A section of a page column, named by its heading. */
export function PageSection({
  title,
  first = false,
  children,
  className,
}: {
  title: ReactNode;
  /** The first section of a column sits closer to the top. */
  first?: boolean;
  children: ReactNode;
  className?: string;
}) {
  const id = useId();
  return (
    <section aria-labelledby={id} className={className}>
      <h2
        id={id}
        className={cn(
          "mb-2.5 flex items-center gap-2 text-sm font-medium text-muted-foreground",
          first ? "mt-1.5" : "mt-[30px]",
        )}
      >
        {title}
      </h2>
      {children}
    </section>
  );
}

// A line of tool output that points at a place in the code, such as "service_test.go:88: expected …".
const LOCATED_LINE = /(^|\s)[\w./-]+\.\w+:\d+(:\d+)?:/;

/** Output of a tool, line by line, with the lines that point at code highlighted (the prototype's `.codeblock`). */
export function OutputBlock({ text, label, className }: { text: string; label?: string; className?: string }) {
  const lines = text.replace(/\n$/, "").split("\n");
  return (
    <pre
      aria-label={label}
      className={cn(
        "my-2.5 max-h-80 overflow-auto rounded-[7px] border bg-card py-2 font-mono text-xs leading-[1.7]",
        className,
      )}
    >
      {lines.map((line, index) => (
        <div key={index} className={cn("px-3 break-words whitespace-pre-wrap", LOCATED_LINE.test(line) && "bg-del-bg")}>
          {line || " "}
        </div>
      ))}
    </pre>
  );
}

/**
 * Problems a tool reported about its own run, folded away. `what` is the run, already in the language shown, read after
 * "while" in English and after "ao" in Portuguese: "running the tests" / "executar os testes".
 */
export function ToolProblems({ diagnostics, what }: { diagnostics: readonly CoreMessage[]; what: string }) {
  const { t } = useTranslation();
  if (diagnostics.length === 0) return null;
  return (
    <details className="col-span-full mt-6 text-xs text-muted-foreground">
      <summary className="cursor-pointer select-none">
        {t("review.page.toolProblems", { count: diagnostics.length, what })}
      </summary>
      <ul className="mt-1.5 grid gap-1 font-mono">
        {diagnostics.map((diagnostic, index) => (
          <li key={index} className="break-words whitespace-pre-wrap">
            {coreText(diagnostic)}
          </li>
        ))}
      </ul>
    </details>
  );
}

/** Share of changed lines covered under which nothing is said; the default of QualityLimits.changedLinesCoverage. */
export const COVERAGE_TARGET = 0.8;

export type CoverageTone = "ok" | "mid" | "low";

/** Covered lines reaching the target read fine, at least half of them a warning, fewer than half bad. */
export function coverageTone(ratio: number, target = COVERAGE_TARGET): CoverageTone {
  if (ratio >= target) return "ok";
  return ratio >= 0.5 ? "mid" : "low";
}

export const TONE_TEXT: Record<CoverageTone, string> = { ok: "", mid: "text-mod", low: "text-del" };
const TONE_BAR: Record<CoverageTone, string> = { ok: "bg-add", mid: "bg-mod", low: "bg-del" };

/** Covered share of some lines, or null when none of them can be covered. */
export function coverageRatio({ covered, coverable }: LineCoverage): number | null {
  return coverable === 0 ? null : covered / coverable;
}

export function CoverageBar({ ratio }: { ratio: number }) {
  return (
    <div aria-hidden="true" className="h-[5px] overflow-hidden rounded-[3px] bg-border">
      <i className={cn("block h-full", TONE_BAR[coverageTone(ratio)])} style={{ width: `${ratio * 100}%` }} />
    </div>
  );
}

/** "3 of 10 coverable lines run by a test", in the language shown. */
export function coverageTitle({ covered, coverable }: LineCoverage): string {
  return i18n.t("review.page.coverageTitle", { covered, count: coverable });
}

/** Where to open a file of the review in the Diff tab, matched on either side; null when the review leaves it alone. */
export function fileDiffTarget(path: string, line: number, files: readonly ChangedFile[]): DiffTarget | null {
  const file = files.find((candidate) => candidate.path === path || candidate.oldPath === path);
  return file ? { path: file.path, line, side: "new" } : null;
}

/** Why a file named somewhere cannot be opened in the Diff tab, in the language shown. No final period. */
export const notInReview = (): string => i18n.t("review.page.notInReview");
