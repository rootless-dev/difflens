import { useState } from "react";
import { useTranslation } from "react-i18next";

import { ErrorNote } from "@/components/page";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { PreferencesSection } from "@/features/preferences/PreferencesSection";
import { APP_LANGUAGES, i18n } from "@/i18n";
import { chooseLanguage, resolveLanguage, systemLanguage, useLanguagePreference } from "@/i18n/language";
import type { LanguagePreference } from "@/lib/api";
import { errorMessage } from "@/lib/format";

/**
 * The language of the app, as a select so more languages fit later: System (the operating system's, naming what it
 * resolves to), then each language named in itself, so it can be found whatever language is shown. A choice applies
 * at once, no restart.
 */
export function LanguageSection() {
  const { t } = useTranslation();
  const preference = useLanguagePreference();
  const [saveError, setSaveError] = useState<string | null>(null);
  const system = resolveLanguage("system", systemLanguage());
  const options: { value: LanguagePreference; label: string }[] = [
    { value: "system", label: t("common.language.systemResolved", { language: t(`common.language.names.${system}`) }) },
    ...APP_LANGUAGES.map((language) => ({ value: language, label: t(`common.language.names.${language}`) })),
  ];

  async function choose(next: LanguagePreference) {
    if (next === preference) return;
    setSaveError(null);
    try {
      await chooseLanguage(next);
    } catch (cause: unknown) {
      // In the language just chosen: this render's `t` is still bound to the one before.
      setSaveError(i18n.t("common.language.saveFailed", { message: errorMessage(cause) }));
    }
  }

  return (
    <PreferencesSection title={t("common.language.label")}>
      <div className="grid grid-cols-[1fr_auto] items-center gap-4 border-b px-2 py-3">
        <div>
          <strong className="block font-medium">{t("common.language.title")}</strong>
          <span className="text-muted-foreground">{t("common.language.description")}</span>
        </div>
        <Select
          items={options}
          value={preference}
          onValueChange={(next) => {
            if (next !== null) void choose(next);
          }}
        >
          <SelectTrigger aria-label={t("common.language.label")} className="min-w-44">
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
      </div>
      {saveError && <ErrorNote className="px-2 pt-2">{saveError}</ErrorNote>}
    </PreferencesSection>
  );
}
