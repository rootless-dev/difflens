import type { en } from "@/i18n/locales/en";

/** The same tree of keys as `T`, every leaf a string: what a translation of a catalog must provide. */
export type Translation<T> = { [K in keyof T]: T[K] extends string ? string : Translation<T[K]> };

/** The English catalog is the reference: every other language must have exactly its keys, plural forms included. */
export type Catalog = Translation<typeof en>;

/** Every key of a catalog as a dotted path, plural suffixes kept ("files_one", "files_other"). Sorted. */
export function catalogKeys(catalog: object, prefix = ""): string[] {
  const keys: string[] = [];
  for (const [key, value] of Object.entries(catalog)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === "object" && value !== null) {
      keys.push(...catalogKeys(value, path));
    } else {
      keys.push(path);
    }
  }
  return keys.sort();
}
