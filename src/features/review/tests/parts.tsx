// Pieces the parts of the Tests tab share: what every row and card needs to know (the review's files, the selection,
// the run going on), outcome marks, tags, places in the code and the checkbox with a mixed state.

import { Check, Minus, X } from "lucide-react";
import { type ComponentProps, createContext, useContext, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";

import type { DiffTarget } from "@/features/review/flow/flow-facts";
import { fileDiffTarget, notInReview } from "@/features/review/ReviewPage";
import type { Check as CheckState, SelectionIndex } from "@/features/review/tests/selection";
import type { Running } from "@/features/review/tests/use-tests";
import { currentLanguage } from "@/i18n";
import type { ChangedFile, RunSelection, TestCase, TestOutcome } from "@/lib/api";
import { cn } from "@/lib/utils";

export interface TestsContextValue {
  files: readonly ChangedFile[];
  onViewDiff: (target: DiffTarget) => void;
  /** A run goes on: nothing else can start. */
  busy: boolean;
  /** The run going on. */
  running: Running | null;
  /** The shell lines that run a selection again, in the run's language (see testCommands). */
  commands: (selection: RunSelection) => string[];
  index: SelectionIndex;
  /** Turns these leaves on or off; the box that did it gets the focus back after "Clear selection". */
  onToggle: (leaves: readonly string[], on: boolean, box: HTMLInputElement) => void;
  onRun: (selection: RunSelection) => void;
  onCancel: () => void;
}

export const TestsContext = createContext<TestsContextValue | null>(null);

export function useTestsContext(): TestsContextValue {
  const value = useContext(TestsContext);
  if (!value) throw new Error("outside the Tests tab");
  return value;
}

/** "1:05": minutes and seconds since `since`. */
export function clock(since: number, now: number): string {
  const seconds = Math.max(0, Math.floor((now - since) / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

/** "a, b and c", in the language shown. */
export function allOf(items: readonly string[]): string {
  return new Intl.ListFormat(currentLanguage(), { type: "conjunction" }).format(items);
}

const OUTCOME_LOOK = {
  pass: { icon: Check, className: "text-add" },
  fail: { icon: X, className: "text-del" },
  skip: { icon: Minus, className: "text-faint" },
} as const;

export function OutcomeMark({ outcome, label }: { outcome: TestOutcome; label?: string }) {
  const { t } = useTranslation();
  const look = OUTCOME_LOOK[outcome];
  const Icon = look.icon;
  return (
    <span role="img" aria-label={label ?? t(`tests.outcome.${outcome}`)} className="inline-flex w-3.5 flex-none justify-center">
      <Icon aria-hidden="true" className={cn("size-3.5", look.className)} />
    </span>
  );
}

export function Spin({ className }: { className?: string }) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        "inline-block size-3 flex-none animate-spin rounded-full border-[1.6px] border-border border-t-primary motion-reduce:[animation-duration:3s]",
        className,
      )}
    />
  );
}

const TAG = {
  new: "bg-add-bg text-add",
  modified: "bg-mod-bg text-mod",
} as const;

export function StateTag({ state, children }: { state: TestCase["state"]; children?: string }) {
  const { t } = useTranslation();
  if (state === "untouched") return null;
  return (
    <em className={cn("rounded-[9px] px-[7px] text-[11px] whitespace-nowrap not-italic", TAG[state])}>
      {children ?? t(`tests.state.${state}`)}
    </em>
  );
}

/** "service.go:661", opening the Diff tab there when the review has the file; plain, with why, otherwise. */
export function Location({ file, line, full = false }: { file: string; line: number; full?: boolean }) {
  const { files, onViewDiff } = useTestsContext();
  const target = fileDiffTarget(file, line, files);
  const text = `${full ? file : (file.split("/").at(-1) ?? file)}:${line}`;
  if (target === null) {
    return (
      <span title={`${file}:${line}. ${notInReview()}`} className="font-mono text-xs text-muted-foreground">
        {text}
      </span>
    );
  }
  return (
    <button
      type="button"
      title={`${file}:${line}`}
      onClick={() => onViewDiff(target)}
      className="cursor-pointer rounded-sm font-mono text-xs text-primary outline-none hover:underline focus-visible:outline-2 focus-visible:outline-ring"
    >
      {text}
    </button>
  );
}

/** A checkbox that can also be mixed. */
export function SelectBox({
  state,
  label,
  leaves,
  className,
}: {
  state: CheckState;
  label: string;
  leaves: readonly string[];
  className?: string;
}) {
  const { busy, running, onToggle } = useTestsContext();
  const box = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (box.current) box.current.indeterminate = state === "some";
  }, [state]);
  // While the whole suite runs, there is no selection.
  const hidden = running !== null && running.selection === null;
  return (
    <input
      ref={box}
      type="checkbox"
      aria-label={label}
      checked={state === "all"}
      disabled={busy}
      onChange={(event) => onToggle(leaves, event.currentTarget.checked, event.currentTarget)}
      className={cn("size-3.5 cursor-pointer accent-primary disabled:cursor-default disabled:opacity-50", hidden && "invisible", className)}
    />
  );
}

/** A small "Run" that shows on hover or focus of its row, saying the commands it takes. */
export function RowRun({ selection, ...props }: { selection: RunSelection } & ComponentProps<"button">) {
  const { t } = useTranslation();
  const { busy, commands, onRun } = useTestsContext();
  return (
    <button
      type="button"
      disabled={busy}
      title={busy ? t("tests.busy") : commands(selection).join("\n")}
      onClick={() => onRun(selection)}
      {...props}
      className={cn(
        "cursor-pointer justify-self-end rounded-[5px] px-1.5 py-px text-xs text-primary opacity-0 outline-none group-hover/row:opacity-100 group-focus-within/row:opacity-100 hover:bg-primary-soft focus-visible:opacity-100 focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:text-muted-foreground",
        props.className,
      )}
    >
      {t("tests.failures.run")}
    </button>
  );
}
