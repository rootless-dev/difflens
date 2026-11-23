import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { LanguageSection } from "@/features/preferences/LanguageSection";
import { currentLanguage, i18n } from "@/i18n";
import { resetLanguagePreference, resolveLanguage, startLanguage } from "@/i18n/language";
import { getLanguagePreference, setLanguagePreference } from "@/lib/api";
import { apiError } from "@/test/fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  getLanguagePreference: vi.fn(),
  setLanguagePreference: vi.fn(),
}));

function systemLanguageIs(language: string) {
  vi.spyOn(navigator, "language", "get").mockReturnValue(language);
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(setLanguagePreference).mockResolvedValue(undefined);
  systemLanguageIs("en-US");
  resetLanguagePreference();
});

afterEach(() => {
  vi.restoreAllMocks();
  resetLanguagePreference();
});

describe("resolving the language", () => {
  it.each([
    ["pt-BR", "pt-BR"],
    ["pt-PT", "pt-BR"],
    ["pt", "pt-BR"],
    ["en-US", "en"],
    ["de-DE", "en"],
    [undefined, "en"],
  ] as const)("follows a system in %s with %s", (system, expected) => {
    expect(resolveLanguage("system", system)).toBe(expected);
  });

  it("keeps a chosen language whatever the system's", () => {
    expect(resolveLanguage("en", "pt-BR")).toBe("en");
    expect(resolveLanguage("pt-BR", "en-US")).toBe("pt-BR");
  });
});

describe("starting the app", () => {
  it("opens in the language kept by the core", async () => {
    vi.mocked(getLanguagePreference).mockResolvedValue("pt-BR");
    await startLanguage();
    expect(currentLanguage()).toBe("pt-BR");
    expect(document.documentElement.lang).toBe("pt-BR");
  });

  it("follows the system when nothing could be read", async () => {
    systemLanguageIs("pt-BR");
    vi.mocked(getLanguagePreference).mockRejectedValue(apiError("internal", "storage is gone"));
    await startLanguage();
    expect(currentLanguage()).toBe("pt-BR");
  });
});

async function openLanguages() {
  const user = userEvent.setup();
  render(<LanguageSection />);
  await user.click(screen.getByRole("combobox", { name: "Language" }));
  return user;
}

describe("the Language setting", () => {
  it("is a select of System, naming what it resolves to, then each language in its own name", async () => {
    systemLanguageIs("pt-BR");
    await openLanguages();
    const options = await screen.findAllByRole("option");
    expect(options.map((option) => option.textContent)).toEqual(["System (Português)", "Português", "English"]);
    expect(screen.getByRole("option", { name: "System (Português)" })).toHaveAttribute("aria-selected", "true");
  });

  it("switches the screen at once and keeps the choice", async () => {
    const user = await openLanguages();
    await user.click(await screen.findByRole("option", { name: "Português" }));
    expect(currentLanguage()).toBe("pt-BR");
    expect(setLanguagePreference).toHaveBeenCalledWith("pt-BR");
    expect(await screen.findByRole("region", { name: "Idioma" })).toBeInTheDocument();
    expect(screen.getByText("Idioma do DiffLens")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Idioma" })).toHaveTextContent("Português");
    expect(i18n.t("common.actions.cancel")).toBe("Cancelar");
  });

  it("goes back to following the system", async () => {
    resetLanguagePreference("pt-BR");
    i18n.changeLanguage("pt-BR");
    const user = userEvent.setup();
    render(<LanguageSection />);
    await user.click(screen.getByRole("combobox", { name: "Idioma" }));
    await user.click(await screen.findByRole("option", { name: "Sistema (English)" }));
    expect(setLanguagePreference).toHaveBeenCalledWith("system");
    expect(currentLanguage()).toBe("en");
    expect(await screen.findByRole("combobox", { name: "Language" })).toHaveTextContent("System (English)");
  });

  it("says when the choice could not be kept, and still shows it", async () => {
    vi.mocked(setLanguagePreference).mockRejectedValue(apiError("internal", "disk full"));
    const user = await openLanguages();
    await user.click(await screen.findByRole("option", { name: "Português" }));
    expect(await screen.findByText(/Não foi possível salvar o idioma/)).toBeInTheDocument();
    expect(currentLanguage()).toBe("pt-BR");
  });
});
