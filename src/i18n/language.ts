// The language the user chose in Preferences, and the one it resolves to. The choice is kept by the core
// (`getLanguagePreference`); this module holds it for the session so the screen reads it without waiting.
import { useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";

import { type AppLanguage, currentLanguage, initI18n, setLanguage } from "@/i18n";
import {
  getLanguagePreference,
  type LanguagePreference,
  setLanguagePreference as saveLanguagePreference,
} from "@/lib/api";

/** pt-* reads as Brazilian Portuguese; any other language, or none, as English. */
export function resolveLanguage(preference: LanguagePreference, systemLanguage: string | undefined): AppLanguage {
  if (preference !== "system") {
    return preference;
  }
  return systemLanguage?.toLowerCase().startsWith("pt") ? "pt-BR" : "en";
}

/** The language the operating system gives the webview. */
export function systemLanguage(): string | undefined {
  return typeof navigator === "undefined" ? undefined : navigator.language;
}

let preference: LanguagePreference = "system";
const listeners = new Set<() => void>();

function notify() {
  for (const listener of listeners) {
    listener();
  }
}

function apply() {
  setLanguage(resolveLanguage(preference, systemLanguage()));
  notify();
}

/**
 * Starts the translations in the language the user chose, read from the core; System when it cannot be read, so a
 * failure never keeps the app from opening. Following the system, a change of the OS language applies at once.
 */
export async function startLanguage(): Promise<void> {
  try {
    preference = await getLanguagePreference();
  } catch {
    preference = "system";
  }
  initI18n(resolveLanguage(preference, systemLanguage()));
  if (typeof window !== "undefined") {
    window.addEventListener("languagechange", () => {
      if (preference === "system") {
        apply();
      }
    });
  }
}

/** Shows the chosen language at once and keeps it. Rejects when it could not be kept; the screen stays switched. */
export async function chooseLanguage(next: LanguagePreference): Promise<void> {
  preference = next;
  apply();
  await saveLanguagePreference(next);
}

/** Only for tests: forgets the choice, back to System. */
export function resetLanguagePreference(next: LanguagePreference = "system") {
  preference = next;
  notify();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** The choice made in Preferences. */
export function useLanguagePreference(): LanguagePreference {
  return useSyncExternalStore(subscribe, () => preference);
}

/** The language shown now: what Analyze, Summarize and Translate ask the model to write in. */
export function useAppLanguage(): AppLanguage {
  // Renders again when the language changes.
  useTranslation();
  return currentLanguage();
}
