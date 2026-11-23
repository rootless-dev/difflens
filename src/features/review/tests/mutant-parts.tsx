// Pieces a mutant is shown with, in the Tests tab and in the Diff: its line as it is and as mutated, the tag of its
// status, and the button that copies it as a prompt for a coding agent.

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { mutantPlace, mutantPrompt } from "@/features/review/tests/mutation-text";
import type { Mutant, MutantStatus, MutationRun } from "@/lib/api";
import { cn } from "@/lib/utils";

interface CodeRow {
  sign: "-" | "+";
  /** Null when the lines could not be read and only the pieces are shown. */
  line: number | null;
  /** The text in runs, the changed one marked. */
  parts: { text: string; marked: boolean }[];
}

/** `text` split at code points `start` and `end`, the middle marked; the empty runs left out. */
function split(text: string, start: number, end: number): CodeRow["parts"] {
  const chars = Array.from(text);
  return [
    { text: chars.slice(0, start).join(""), marked: false },
    { text: chars.slice(start, end).join(""), marked: true },
    { text: chars.slice(end).join(""), marked: false },
  ].filter((part) => part.text !== "");
}

/**
 * The rows of a mutant's code: its lines as they are ("-"), then as mutated ("+"), with their numbers. On one line, the
 * piece replaced and what replaced it are marked, when the line holds the piece where the mutant says.
 */
export function mutantRows(mutant: Mutant): CodeRow[] {
  if (mutant.before.length === 0) {
    return [
      { sign: "-", line: null, parts: [{ text: mutant.original, marked: true }] },
      { sign: "+", line: null, parts: mutant.mutated === "" ? [] : [{ text: mutant.mutated, marked: true }] },
    ];
  }
  const start = mutant.column - 1;
  const single = mutant.line === mutant.endLine && mutant.before.length === 1 && mutant.after.length === 1;
  const holds =
    single && Array.from(mutant.before[0]).slice(start, mutant.endColumn - 1).join("") === mutant.original;
  if (holds) {
    return [
      { sign: "-", line: mutant.line, parts: split(mutant.before[0], start, mutant.endColumn - 1) },
      { sign: "+", line: mutant.line, parts: split(mutant.after[0], start, start + Array.from(mutant.mutated).length) },
    ];
  }
  const plain = (sign: CodeRow["sign"]) => (text: string, index: number) => ({
    sign,
    line: mutant.line + index,
    parts: [{ text, marked: false }],
  });
  return [...mutant.before.map(plain("-")), ...mutant.after.map(plain("+"))];
}

/** A mutant's lines as they are, on the deletion tint, then as mutated, on the addition tint; plain text. */
export function MutantCode({ mutant, className }: { mutant: Mutant; className?: string }) {
  const { t } = useTranslation();
  const rows = mutantRows(mutant);
  const width = Math.max(...rows.map((row) => String(row.line ?? "").length));
  return (
    <pre
      aria-label={t("tests.mutation.code", { place: mutantPlace(mutant) })}
      className={cn("overflow-auto rounded-[7px] border bg-card py-2 font-mono text-xs leading-[1.7] [tab-size:4]", className)}
    >
      {rows.map((row, index) => (
        <div
          key={index}
          data-sign={row.sign}
          className={cn("px-3", row.sign === "-" ? "bg-del-bg" : "bg-add-bg")}
        >
          <span aria-hidden="true" className="text-faint select-none">
            {`${row.sign} ${String(row.line ?? "").padStart(width)}  `}
          </span>
          {row.parts.map((part, at) =>
            part.marked ? (
              <mark
                key={at}
                className={cn("rounded-[2px] text-inherit", row.sign === "-" ? "bg-del/25" : "bg-add/25")}
              >
                {part.text}
              </mark>
            ) : (
              <span key={at}>{part.text}</span>
            ),
          )}
        </div>
      ))}
    </pre>
  );
}

const STATUS_LOOK: Record<MutantStatus, string> = {
  survived: "bg-del-bg font-medium text-del",
  timeout: "bg-mod-bg text-mod",
  killed: "bg-add-bg text-add",
  error: "bg-secondary text-muted-foreground",
  no_coverage: "bg-secondary text-muted-foreground",
};

/** "survived", "timeout", "killed"…: what became of a mutant, as a tag. */
export function MutantStatusTag({ status }: { status: MutantStatus }) {
  const { t } = useTranslation();
  return (
    <em
      data-mutant-status={status}
      className={cn("rounded-[9px] px-[7px] text-[11px] leading-[17px] whitespace-nowrap not-italic", STATUS_LOOK[status])}
    >
      {t(`tests.mutation.status.${status}`)}
    </em>
  );
}

/**
 * Copies a mutant that survived as a prompt for a coding agent to write the missing test (see mutantPrompt): a button,
 * or a link inside a block of the Diff.
 */
export function CopyMutantPrompt({
  mutant,
  run,
  look = "button",
}: {
  mutant: Mutant;
  run: MutationRun | null;
  look?: "button" | "link";
}) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState<"copied" | "failed" | null>(null);
  useEffect(() => {
    if (copied === null) return;
    const timer = setTimeout(() => setCopied(null), 2000);
    return () => clearTimeout(timer);
  }, [copied]);
  const copy = () => {
    navigator.clipboard.writeText(mutantPrompt(mutant, run)).then(
      () => setCopied("copied"),
      () => setCopied("failed"),
    );
  };
  return (
    <Button
      variant={look === "link" ? "link" : "secondary"}
      size={look === "link" ? "inline" : "sm"}
      className={look === "link" ? "text-xs text-primary" : undefined}
      onClick={copy}
      aria-live="polite"
    >
      {copied === "copied"
        ? t("common.actions.copied")
        : copied === "failed"
          ? t("tests.mutation.copyFailed")
          : t("tests.mutation.copy")}
    </Button>
  );
}
