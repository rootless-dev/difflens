// What a function's popover in the Flow says of its signals, in its left column, what was measured before what was
// read: its quality limits (or that no test runs it), the static checks on its added lines, the duplicated code it
// introduces, the mutants of its lines that survived, then the model's findings inside it. Each section is only there
// when it has something to say.

import { type ReactNode, useContext, useId } from "react";
import { useTranslation } from "react-i18next";

import { WithCode } from "@/components/with-code";
import { shownFinding } from "@/features/review/ai-text/versions";
import type { DiffTarget } from "@/features/review/flow/flow-facts";
import { fileDiffTarget } from "@/features/review/ReviewPage";
import { LimitList } from "@/features/review/signals/DiffSignals";
import {
  CheckSeverityTag,
  DuplicateChip,
  LimitChip,
  ModelTag,
  MutantChip,
  SeverityChip,
  ToolChip,
  UntestedChip,
} from "@/features/review/signals/signal-marks";
import { findingLine, type FunctionSignals, SignalsContext } from "@/features/review/signals/signals";
import {
  duplicateKindText,
  duplicateSizeText,
  findingPlace,
  mutantChangeText,
  operatorName,
} from "@/features/review/signals/signals-text";
import { useAppLanguage } from "@/i18n/language";
import type { ChangedFile, QualityLimits } from "@/lib/api";

/** Whether the popover has a "Quality limits" section, which takes the complexity out of its facts. */
export function showsLimits(signals: FunctionSignals | undefined, limits: QualityLimits | null): boolean {
  return limits !== null && signals !== undefined && (signals.limits.length > 0 || signals.untested);
}

const LINK = "cursor-pointer rounded-[3px] text-xs text-primary outline-none hover:underline focus-visible:outline-2 focus-visible:outline-ring";
const MINI =
  "cursor-pointer rounded-[4px] border px-1.5 text-[11px] leading-[16px] whitespace-nowrap text-muted-foreground outline-none hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring";
const TITLE =
  "min-w-0 cursor-pointer rounded-[3px] text-left text-[12.5px] underline decoration-faint underline-offset-2 outline-none hover:decoration-foreground focus-visible:outline-2 focus-visible:outline-ring";

function Section({
  title,
  chip,
  link,
  caveat,
  children,
}: {
  title: ReactNode;
  chip?: ReactNode;
  link: { label: string; title: string; onClick: () => void };
  caveat?: string;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <section aria-labelledby={id} className="mt-3.5 border-t pt-3">
      <h4 className="mb-2 flex items-center gap-2 text-xs font-normal whitespace-nowrap text-faint">
        <span id={id}>{title}</span>
        {chip}
        <button type="button" aria-label={link.title} onClick={link.onClick} className={`${LINK} ml-auto`}>
          {link.label}
        </button>
      </h4>
      {children}
      {caveat && <p className="mt-2 text-[11px] text-faint">{caveat}</p>}
    </section>
  );
}

export function FunctionSignalSections({
  signals,
  files,
  onViewDiff,
}: {
  signals: FunctionSignals | undefined;
  files: readonly ChangedFile[];
  onViewDiff: (target: DiffTarget) => void;
}) {
  const { t } = useTranslation();
  const language = useAppLanguage();
  const handle = useContext(SignalsContext);
  const limits = handle.signals.limits;
  if (!signals) return null;
  const quality = { label: t("signals.limits.quality"), title: t("signals.limits.openQuality"), onClick: handle.onOpenQuality };
  return (
    <>
      {showsLimits(signals, limits) && limits && (
        <Section
          title={t("signals.limits.title")}
          chip={
            <>
              {signals.limits.length > 0 && (
                <LimitChip>{t("signals.chip.crossed", { count: signals.limits.length })}</LimitChip>
              )}
              {signals.untested && <UntestedChip />}
            </>
          }
          link={quality}
        >
          <LimitList signals={signals} limits={limits} />
        </Section>
      )}
      {signals.checks.length > 0 && (
        <Section
          title={t("signals.checks.title")}
          chip={<ToolChip>{t("signals.chip.findings", { count: signals.checks.length })}</ToolChip>}
          link={quality}
          caveat={t("signals.checks.caveat")}
        >
          <ol className="grid gap-2">
            {signals.checks.map((check) => {
              const target = fileDiffTarget(check.file, check.line, files);
              const open = target ? () => onViewDiff({ ...target, reveal: "check" }) : undefined;
              return (
                <li
                  key={`${check.tool}:${check.rule}:${check.line}:${check.message}`}
                  className="grid grid-cols-[auto_minmax(0,1fr)_auto] items-baseline gap-x-2 gap-y-0.5 text-xs"
                >
                  <ToolChip tool={check.tool} rule={check.rule} />
                  {open ? (
                    <button type="button" lang="en" onClick={open} className={TITLE}>
                      {check.message}
                    </button>
                  ) : (
                    <span lang="en" className="text-[12.5px]">
                      {check.message}
                    </span>
                  )}
                  {open ? (
                    <button
                      type="button"
                      aria-label={t("signals.checks.viewInDiff", { line: check.line })}
                      onClick={open}
                      className={MINI}
                    >
                      {t("signals.checks.diff")}
                    </button>
                  ) : (
                    <span />
                  )}
                  <small className="col-start-2 flex items-center gap-1.5 text-[11px] text-faint">
                    <CheckSeverityTag severity={check.severity} />
                    {t("signals.checks.line", { line: check.line })}
                  </small>
                </li>
              );
            })}
          </ol>
        </Section>
      )}
      {signals.duplicates.length > 0 && (
        <Section
          title={t("signals.duplicates.title")}
          chip={<DuplicateChip>{t("signals.chip.duplicates", { count: signals.duplicates.length })}</DuplicateChip>}
          link={quality}
          caveat={t("signals.duplicates.caveat")}
        >
          <ol className="grid gap-2">
            {signals.duplicates.map((duplicate) => {
              const { block } = duplicate;
              const target = fileDiffTarget(block.file, block.startLine, files);
              const open = target ? () => onViewDiff({ ...target, reveal: "check" }) : undefined;
              const where = { start: block.startLine, end: block.endLine };
              return (
                <li
                  key={`${duplicate.kind}:${block.file}:${block.startLine}:${block.endLine}`}
                  className="grid grid-cols-[minmax(0,1fr)_auto] items-baseline gap-x-2 gap-y-0.5 text-xs"
                >
                  <span className="text-[12.5px]">
                    <WithCode text={duplicateKindText(duplicate)} />
                  </span>
                  {open ? (
                    <button
                      type="button"
                      aria-label={t("signals.duplicates.viewInDiff", where)}
                      onClick={open}
                      className={MINI}
                    >
                      {t("signals.checks.diff")}
                    </button>
                  ) : (
                    <span />
                  )}
                  <small className="text-[11px] text-faint">
                    {t("signals.duplicates.lines", where)} · {duplicateSizeText(duplicate)}
                  </small>
                </li>
              );
            })}
          </ol>
        </Section>
      )}
      {signals.mutants.length > 0 && (
        <Section
          title={t("signals.mutants.title")}
          chip={<MutantChip>{t("signals.chip.mutants", { count: signals.mutants.length })}</MutantChip>}
          link={{ label: t("signals.mutants.tests"), title: t("signals.mutants.openTests"), onClick: handle.onOpenTests }}
          caveat={t("signals.mutants.caveat")}
        >
          <ol className="grid gap-2">
            {signals.mutants.map((mutant) => {
              const target = fileDiffTarget(mutant.file, mutant.line, files);
              const open = target ? () => onViewDiff({ ...target, reveal: "check" }) : undefined;
              return (
                <li key={mutant.id} className="grid grid-cols-[minmax(0,1fr)_auto] items-baseline gap-x-2 gap-y-0.5 text-xs">
                  <span className="text-[12.5px] break-words">
                    <WithCode text={mutantChangeText(mutant)} />
                  </span>
                  {open ? (
                    <button
                      type="button"
                      aria-label={t("signals.mutants.viewInDiff", { line: mutant.line })}
                      onClick={open}
                      className={MINI}
                    >
                      {t("signals.checks.diff")}
                    </button>
                  ) : (
                    <span />
                  )}
                  <small className="text-[11px] text-faint">
                    {t("signals.mutants.line", { line: mutant.line })} · {operatorName(mutant.operator)}
                  </small>
                </li>
              );
            })}
          </ol>
        </Section>
      )}
      {signals.findings.length > 0 && (
        <Section
          title={t("signals.findings.title")}
          chip={<ModelTag />}
          link={{ label: t("signals.findings.insights"), title: t("signals.findings.openInsights"), onClick: handle.onOpenInsights }}
          caveat={t("signals.findings.caveat")}
        >
          <ol className="grid gap-2">
            {signals.findings.map((finding) => {
              const line = findingLine(finding);
              const target = fileDiffTarget(finding.path, line, files);
              const open = target ? () => onViewDiff({ ...target, reveal: "finding" }) : undefined;
              const title = shownFinding(finding, language).title;
              return (
                <li key={finding.id} className="grid grid-cols-[auto_minmax(0,1fr)_auto] items-baseline gap-x-2 gap-y-0.5 text-xs">
                  <SeverityChip severity={finding.severity} />
                  {open ? (
                    <button type="button" onClick={open} className={TITLE}>
                      <WithCode text={title} />
                    </button>
                  ) : (
                    <span className="text-[12.5px]">
                      <WithCode text={title} />
                    </span>
                  )}
                  {open ? (
                    <button
                      type="button"
                      aria-label={t("signals.checks.viewInDiff", { line })}
                      onClick={open}
                      className={MINI}
                    >
                      {t("signals.checks.diff")}
                    </button>
                  ) : (
                    <span />
                  )}
                  <small className="col-start-2 text-[11px] text-faint">{findingPlace(finding)}</small>
                </li>
              );
            })}
          </ol>
        </Section>
      )}
    </>
  );
}
