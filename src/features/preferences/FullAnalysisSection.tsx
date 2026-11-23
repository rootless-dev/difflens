import { useState } from "react";
import { useTranslation } from "react-i18next";

import { ErrorNote } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { PreferencesSection } from "@/features/preferences/PreferencesSection";
import { useAsyncData } from "@/hooks/use-async-data";
import { i18n } from "@/i18n";
import { getFullAnalysisLimit, setFullAnalysisLimit } from "@/lib/api";
import { errorMessage, formatNumber } from "@/lib/format";

/** The limits offered; the core keeps any from 1 to 16. */
const LIMITS = [1, 2, 3, 4, 5, 6, 8, 12, 16];

/**
 * How many full analyses run at once (4 until the user picks another): beyond it a new one waits in the queue. A
 * higher limit starts queued ones at once.
 */
export function FullAnalysisSection() {
  const { t } = useTranslation();
  const { state, reload, update } = useAsyncData(getFullAnalysisLimit);
  const [saveError, setSaveError] = useState<string | null>(null);

  async function choose(limit: number) {
    setSaveError(null);
    try {
      const kept = await setFullAnalysisLimit(limit);
      update(() => kept);
    } catch (cause: unknown) {
      setSaveError(i18n.t("fullAnalysis.preferences.saveFailed", { message: errorMessage(cause) }));
    }
  }

  const current = state.status === "ready" ? state.data : null;
  const values = current !== null && !LIMITS.includes(current) ? [...LIMITS, current].sort((a, b) => a - b) : LIMITS;
  const options = values.map((value) => ({ value, label: formatNumber(value) }));

  return (
    <PreferencesSection title={t("fullAnalysis.preferences.title")}>
      <div className="grid grid-cols-[1fr_auto] items-center gap-4 border-b px-2 py-3">
        <div>
          <strong className="block font-medium">{t("fullAnalysis.preferences.limit")}</strong>
          <span className="text-muted-foreground">{t("fullAnalysis.preferences.limitDescription")}</span>
        </div>
        {state.status === "loading" && <Skeleton className="h-[29px] w-24" />}
        {state.status === "error" && (
          <Button variant="secondary" size="sm" onClick={reload}>
            {t("common.actions.tryAgain")}
          </Button>
        )}
        {current !== null && (
          <Select
            items={options}
            value={current}
            onValueChange={(next) => {
              if (next !== null && next !== current) void choose(next);
            }}
          >
            <SelectTrigger aria-label={t("fullAnalysis.preferences.limit")} className="min-w-24">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {options.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
      </div>
      {state.status === "error" && <ErrorNote className="px-2 pt-2">{state.message}</ErrorNote>}
      {saveError && <ErrorNote className="px-2 pt-2">{saveError}</ErrorNote>}
    </PreferencesSection>
  );
}
