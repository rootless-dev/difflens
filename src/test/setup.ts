import "@testing-library/jest-dom/vitest";

import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

import { initI18n, setLanguage } from "@/i18n";

// Tests read the English text; a test that switches language is set back afterwards.
initI18n("en");

afterEach(() => {
  cleanup();
  setLanguage("en");
});
