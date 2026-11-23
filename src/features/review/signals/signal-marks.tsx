// The one visual language of the review's signals (design/findings-and-limits.html): a model finding is three rising
// bars in its severity's hue; what DiffLens measured is drawn in ink: a gauge for a limit crossed, a wrench for a
// static check finding, two sheets for duplicated code, a bug for a mutant that survived, a struck-through flask for a
// function no test runs. In text each comes as a chip with its word; on a box of the Flow only the mark and the count,
// more than one. A static check finding's severity, as its tool reports it, is a tag with its word beside the wrench.

import { useTranslation } from "react-i18next";

import { severityName } from "@/features/review/insights/insights-text";
import type { Badge } from "@/features/review/signals/signals";
import {
  BUG,
  COPY,
  GAUGE,
  type IconShape,
  NO_TEST,
  severityIcon,
  WRENCH,
} from "@/features/review/signals/signal-icons";
import { checkSeverityName, SEVERITY_TEXT } from "@/features/review/signals/signals-text";
import type { CheckSeverity, Severity } from "@/lib/api";
import { cn } from "@/lib/utils";

export function Mark({ icon, className }: { icon: IconShape; className?: string }) {
  return (
    <svg
      aria-hidden="true"
      width={icon.width}
      height={icon.height}
      viewBox={icon.viewBox}
      className={cn("flex-none", className)}
      // A fixed shape of signal-icons.ts, never data.
      dangerouslySetInnerHTML={{ __html: icon.body }}
    />
  );
}

export function SeverityMark({ severity, className }: { severity: Severity; className?: string }) {
  return <Mark icon={severityIcon(severity)} className={cn(SEVERITY_TEXT[severity], className)} />;
}

export const GaugeMark = ({ className }: { className?: string }) => (
  <Mark icon={GAUGE} className={cn("text-foreground", className)} />
);
export const WrenchMark = ({ className }: { className?: string }) => (
  <Mark icon={WRENCH} className={cn("text-foreground", className)} />
);
export const NoTestMark = ({ className }: { className?: string }) => (
  <Mark icon={NO_TEST} className={cn("text-foreground", className)} />
);
export const CopyMark = ({ className }: { className?: string }) => (
  <Mark icon={COPY} className={cn("text-foreground", className)} />
);
export const BugMark = ({ className }: { className?: string }) => (
  <Mark icon={BUG} className={cn("text-foreground", className)} />
);

const CHIP = "inline-flex flex-none items-center gap-[5px] rounded-[9px] py-0 pr-[7px] pl-1.5 text-[11px] whitespace-nowrap not-italic";
const INK_CHIP = cn(CHIP, "border bg-secondary leading-[15px] text-foreground");

/** "▮▮▮ High", tinted with the severity's hue. */
export function SeverityChip({ severity, children }: { severity: Severity; children?: string }) {
  return (
    <em className={cn(CHIP, "bg-current/13 leading-[17px]", SEVERITY_TEXT[severity])}>
      <SeverityMark severity={severity} />
      {children ?? severityName(severity)}
    </em>
  );
}

/** "◠ 3 limits", or another text with the gauge. */
export function LimitChip({ children }: { children: string }) {
  return (
    <em className={INK_CHIP}>
      <GaugeMark />
      {children}
    </em>
  );
}

/** "⚲ no test". */
export function UntestedChip({ children }: { children?: string }) {
  const { t } = useTranslation();
  return (
    <em className={INK_CHIP}>
      <NoTestMark />
      {children ?? t("signals.chip.untested")}
    </em>
  );
}

/** "🔧 staticcheck SA4006": the tool, and its rule in the tool's own spelling; or another text with the wrench. */
export function ToolChip({ tool, rule = null, children }: { tool?: string; rule?: string | null; children?: string }) {
  return (
    <em className={INK_CHIP}>
      <WrenchMark />
      {children ?? (
        <>
          {tool}
          {rule && <code className="text-[10.5px] text-muted-foreground">{rule}</code>}
        </>
      )}
    </em>
  );
}

/** A check's severity as a tag: the word carries it, the hue only repeats it, error the strongest. */
const CHECK_SEVERITY_LOOK: Record<CheckSeverity, string> = {
  error: "bg-del-bg font-medium text-del",
  warning: "bg-mod-bg text-mod",
  info: "bg-secondary text-muted-foreground",
};

/** "Error", "Warning", "Info": how serious the tool says a static check finding is. */
export function CheckSeverityTag({ severity, className }: { severity: CheckSeverity; className?: string }) {
  return (
    <em
      data-check-severity={severity}
      className={cn(
        "rounded-[9px] px-[7px] text-[11px] leading-[17px] whitespace-nowrap not-italic",
        CHECK_SEVERITY_LOOK[severity],
        className,
      )}
    >
      {checkSeverityName(severity)}
    </em>
  );
}

/** "⧉ duplication", or another text with the two sheets. */
export function DuplicateChip({ children }: { children?: string }) {
  const { t } = useTranslation();
  return (
    <em className={INK_CHIP}>
      <CopyMark />
      {children ?? t("signals.chip.duplication")}
    </em>
  );
}

/** "🐞 mutant", or another text with the bug. */
export function MutantChip({ children }: { children?: string }) {
  const { t } = useTranslation();
  return (
    <em className={INK_CHIP}>
      <BugMark />
      {children ?? t("signals.chip.mutant")}
    </em>
  );
}

/** The model's tag (the prototype's `.t-ai`). */
export function ModelTag() {
  const { t } = useTranslation();
  return (
    <em className="rounded-[9px] bg-ai-bg px-[7px] text-[11px] font-normal whitespace-nowrap text-ai not-italic">
      {t("overview.tag.model")}
    </em>
  );
}

/** The measured tag (the prototype's `.t-m`). */
export function MeasuredTag() {
  const { t } = useTranslation();
  return (
    <em className="rounded-[9px] bg-primary-soft px-[7px] text-[11px] font-normal whitespace-nowrap text-primary not-italic">
      {t("overview.tag.measured")}
    </em>
  );
}

const count = (value: number) => (value > 1 ? <span>{value}</span> : null);

/**
 * A box's badge, across its bottom-right border: up to six parts, always in this order, split by a hairline. Hidden
 * from screen readers: the box's own label says the same.
 */
export function SignalBadge({ badge, className }: { badge: Badge; className?: string }) {
  const parts = [
    badge.findings > 0 && badge.severity !== null && (
      <span key="findings" className={cn("inline-flex items-center gap-[3px]", SEVERITY_TEXT[badge.severity])}>
        <SeverityMark severity={badge.severity} />
        {count(badge.findings)}
      </span>
    ),
    badge.limits > 0 && (
      <span key="limits" className="inline-flex items-center gap-[3px]">
        <GaugeMark />
        {count(badge.limits)}
      </span>
    ),
    badge.checks > 0 && (
      <span key="checks" className="inline-flex items-center gap-[3px]">
        <WrenchMark />
        {count(badge.checks)}
      </span>
    ),
    badge.duplicates > 0 && (
      <span key="duplicates" className="inline-flex items-center gap-[3px]">
        <CopyMark />
        {count(badge.duplicates)}
      </span>
    ),
    badge.mutants > 0 && (
      <span key="mutants" className="inline-flex items-center gap-[3px]">
        <BugMark />
        {count(badge.mutants)}
      </span>
    ),
    badge.untested > 0 && (
      <span key="untested" className="inline-flex items-center gap-[3px]">
        <NoTestMark />
        {count(badge.untested)}
      </span>
    ),
  ].filter(Boolean);
  return (
    <span
      aria-hidden="true"
      data-signal-badge=""
      className={cn(
        "pointer-events-none absolute -right-[9px] -bottom-[9px] z-[2] inline-flex h-[18px] items-center gap-[5px] rounded-[9px] border border-foreground/25 bg-background px-1.5 font-mono text-[10.5px] leading-none font-medium text-foreground shadow-[0_1px_2px_rgba(0,0,0,0.25)]",
        className,
      )}
    >
      {parts.flatMap((part, index) =>
        index === 0 ? [part] : [<i key={`rule-${index}`} className="h-2.5 w-px bg-border" />, part],
      )}
    </span>
  );
}
