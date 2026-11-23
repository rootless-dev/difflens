// The app's translations: one i18next instance, its catalogs bundled (nothing is loaded at run time), and the
// language it shows.
import i18next from "i18next";
import { initReactI18next } from "react-i18next";

import { en } from "@/i18n/locales/en";
import { ptBR } from "@/i18n/locales/pt-BR";

/** The languages DiffLens is written in, as BCP 47 tags. */
export const APP_LANGUAGES = ["pt-BR", "en"] as const;

export type AppLanguage = (typeof APP_LANGUAGES)[number];

export const i18n = i18next;

/**
 * Starts i18next with every catalog, synchronously, so `t` works as soon as this returns. English fills any key a
 * language lacks, which the catalog check makes impossible but costs nothing.
 */
export function initI18n(language: AppLanguage): void {
  if (!i18n.isInitialized) {
    i18n.use(initReactI18next).init({
      resources: { en: { translation: en }, "pt-BR": { translation: ptBR } },
      lng: language,
      fallbackLng: "en",
      supportedLngs: [...APP_LANGUAGES],
      initAsync: false,
      // React escapes what it renders.
      interpolation: { escapeValue: false },
      returnNull: false,
    });
    applyPluralForZeroInPortuguese();
  }
  setLanguage(language);
}

/** Shows `language` at once: every component that translates renders again. */
export function setLanguage(language: AppLanguage): void {
  if (i18n.language !== language) {
    void i18n.changeLanguage(language);
  }
  if (typeof document !== "undefined") {
    document.documentElement.lang = language;
  }
}

/** The language shown now. */
export function currentLanguage(): AppLanguage {
  return i18n.language === "pt-BR" ? "pt-BR" : "en";
}

/**
 * Brazilian Portuguese counts zero in the plural ("0 arquivos"), while the CLDR rule Intl follows puts 0 with 1. The
 * rule i18next takes from Intl is wrapped so 0 picks the "other" form; every other count is left to Intl.
 */
function applyPluralForZeroInPortuguese(): void {
  const resolver = i18n.services.pluralResolver as {
    getRule: (code: string, options?: { ordinal?: boolean }) => Intl.PluralRules | undefined;
  };
  const getRule = resolver.getRule.bind(resolver);
  resolver.getRule = (code, options = {}) => {
    const rule = getRule(code, options);
    if (!rule || options.ordinal || !code.startsWith("pt")) {
      return rule;
    }
    return {
      select: (count: number) => (count === 0 ? "other" : rule.select(count)),
      resolvedOptions: () => rule.resolvedOptions(),
    } as Intl.PluralRules;
  };
}
