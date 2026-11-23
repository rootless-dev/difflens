import { LoaderCircle } from "lucide-react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";

import { useAnalyzerProvisions } from "@/hooks/use-analyzer-provisions";
import type { AnalyzerProvision } from "@/lib/api";
import { cn } from "@/lib/utils";

/**
 * "Preparing the Python analyzer…" for each analyzer being prepared now, with an indeterminate bar and its current
 * step; nothing when none is. A view waiting for an analysis shows it in place of its own "analysing" line: the first
 * time a project of a language needs its analyzer, the core builds it (seconds to minutes), then the view goes on.
 * `fallback` is shown when nothing is being prepared.
 */
export function PreparingAnalyzers({
  fallback = null,
  className,
}: {
  fallback?: ReactNode;
  className?: string;
}) {
  const preparing = useAnalyzerProvisions();
  if (preparing.length === 0) return <>{fallback}</>;
  return (
    <div className={cn("grid gap-4", className)}>
      {preparing.map((provision) => (
        <Preparing key={provision.analyzer} provision={provision} />
      ))}
    </div>
  );
}

function Preparing({ provision }: { provision: AnalyzerProvision }) {
  const { t } = useTranslation();
  const language = provision.language;
  return (
    <div role="status" aria-label={t("common.analyzer.preparing", { language })} className="max-w-[480px]">
      <p className="flex items-center gap-2">
        <LoaderCircle aria-hidden="true" className="size-4 flex-none animate-spin text-muted-foreground" />
        {t("common.analyzer.preparing", { language })}
      </p>
      <div
        role="progressbar"
        aria-label={t("common.analyzer.preparing", { language })}
        className="relative mt-2 h-1 overflow-hidden rounded-full bg-muted"
      >
        <div className="absolute inset-y-0 w-1/3 animate-pulse rounded-full bg-muted-foreground/40" />
      </div>
      {provision.step !== null && (
        <p className="mt-1.5 text-xs text-muted-foreground">{t(`common.analyzer.step.${provision.step}`)}</p>
      )}
      <p className="mt-1 text-xs text-faint">{t("common.analyzer.hint", { language })}</p>
    </div>
  );
}
