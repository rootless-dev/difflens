import "i18next";

import type { en } from "@/i18n/locales/en";

// Types every `t` call against the English catalog, so a missing or misspelled key is a TypeScript error.
declare module "i18next" {
  interface CustomTypeOptions {
    defaultNS: "translation";
    resources: { translation: typeof en };
  }
}
