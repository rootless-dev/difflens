import { describe, expect, it } from "vitest";

import { i18n } from "@/i18n";
import { catalogKeys } from "@/i18n/catalog";
import { en } from "@/i18n/locales/en";
import { ptBR } from "@/i18n/locales/pt-BR";

describe("the catalogs", () => {
  it("have exactly the same keys in Portuguese and English, plural forms included", () => {
    expect(catalogKeys(ptBR)).toEqual(catalogKeys(en));
  });

  it("have no empty text", () => {
    for (const catalog of [en, ptBR]) {
      const empty = catalogKeys(catalog).filter((key) => {
        const value = key.split(".").reduce<unknown>((node, part) => (node as Record<string, unknown>)[part], catalog);
        return typeof value !== "string" || value.trim() === "";
      });
      expect(empty).toEqual([]);
    }
  });

  it("use the same placeholders in both languages", () => {
    const placeholders = (text: string) => [...text.matchAll(/\{\{\s*(\w+)/g)].map((match) => match[1]).sort();
    const read = (catalog: object, key: string) =>
      key.split(".").reduce<unknown>((node, part) => (node as Record<string, unknown>)[part], catalog) as string;
    const mismatched = catalogKeys(en).filter((key) => {
      const english = placeholders(read(en, key));
      const portuguese = placeholders(read(ptBR, key));
      // A plural's "one" form may spell the count out instead of using it.
      const optionalCount = key.endsWith("_one");
      const strip = (list: string[]) => (optionalCount ? list.filter((name) => name !== "count") : list);
      return JSON.stringify(strip([...new Set(english)])) !== JSON.stringify(strip([...new Set(portuguese)]));
    });
    expect(mismatched).toEqual([]);
  });
});

describe("plurals", () => {
  it("count zero in the plural in Portuguese and in English", async () => {
    await i18n.changeLanguage("pt-BR");
    expect(i18n.t("common.time.daysAgo", { count: 0 })).toBe("há 0 dias");
    expect(i18n.t("common.time.daysAgo", { count: 1 })).toBe("há 1 dia");
    expect(i18n.t("common.time.daysAgo", { count: 2 })).toBe("há 2 dias");
    expect(i18n.t("common.bytes", { count: 1_000_000 })).toBe("1000000 bytes");
    await i18n.changeLanguage("en");
    expect(i18n.t("common.time.daysAgo", { count: 0 })).toBe("0 days ago");
    expect(i18n.t("common.time.daysAgo", { count: 1 })).toBe("1 day ago");
  });
});
