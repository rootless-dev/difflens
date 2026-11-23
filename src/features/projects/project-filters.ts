import { useState } from "react";

import type { Project } from "@/lib/api";
import { intlLocale, languageName } from "@/lib/format";

/** "recent": most recently modified first. "oldest": least recently modified first. */
export type ProjectSort = "recent" | "oldest";

/** The language filter's option for projects without a detected language; no language id is empty. */
export const NO_LANGUAGE = "";

export interface LanguageOption {
  /** A language id, or `NO_LANGUAGE`. */
  id: string;
  /** Projects that have it. */
  count: number;
}

/** What the projects screen keeps while the app runs. The search text is not kept. */
export interface ProjectFilters {
  sort: ProjectSort;
  /** Chosen language ids, `NO_LANGUAGE` among them; none chosen shows every project. */
  languages: string[];
}

const DEFAULT_FILTERS: ProjectFilters = { sort: "recent", languages: [] };

let remembered = DEFAULT_FILTERS;

/** The sort and the language filter, kept while the app runs, so coming back from a project finds them as they were. */
export function useProjectFilters(): [ProjectFilters, (change: Partial<ProjectFilters>) => void] {
  const [filters, setFilters] = useState(remembered);
  function update(change: Partial<ProjectFilters>) {
    remembered = { ...remembered, ...change };
    setFilters(remembered);
  }
  return [filters, update];
}

/** Only for tests: forgets the choices, back to the defaults. */
export function resetProjectFilters() {
  remembered = DEFAULT_FILTERS;
}

/** Lowercase without accents, so "Ação" and "acao" match. */
function fold(text: string): string {
  return text.normalize("NFD").replace(/\p{M}/gu, "").toLocaleLowerCase();
}

/** True when the name contains the query, ignoring case and accents. An empty query matches every project. */
export function matchesQuery(project: Project, query: string): boolean {
  const folded = fold(query.trim());
  return folded === "" || fold(project.name).includes(folded);
}

/** True when the project has any of the chosen languages. Nothing chosen matches every project. */
export function matchesLanguages(project: Project, chosen: readonly string[]): boolean {
  if (chosen.length === 0) {
    return true;
  }
  if (project.languages.length === 0) {
    return chosen.includes(NO_LANGUAGE);
  }
  return project.languages.some((language) => chosen.includes(language));
}

/** The languages found among the projects, by name, each with how many projects have it; "no language" last. */
export function languageOptions(projects: readonly Project[]): LanguageOption[] {
  const counts = new Map<string, number>();
  let without = 0;
  for (const project of projects) {
    if (project.languages.length === 0) {
      without++;
    }
    for (const language of new Set(project.languages)) {
      counts.set(language, (counts.get(language) ?? 0) + 1);
    }
  }
  const collator = new Intl.Collator(intlLocale(), { sensitivity: "base" });
  const options = [...counts]
    .map(([id, count]) => ({ id, count }))
    .sort((a, b) => collator.compare(languageName(a.id), languageName(b.id)));
  return without > 0 ? [...options, { id: NO_LANGUAGE, count: without }] : options;
}

function modifiedTime(project: Project): number | null {
  const time = project.lastModifiedAt === null ? Number.NaN : Date.parse(project.lastModifiedAt);
  return Number.isNaN(time) ? null : time;
}

/** By last modification in either direction; projects without one come last both ways. Ties go by name. */
export function sortProjects(projects: readonly Project[], sort: ProjectSort): Project[] {
  const collator = new Intl.Collator(intlLocale(), { sensitivity: "base", numeric: true });
  return [...projects].sort((a, b) => {
    const left = modifiedTime(a);
    const right = modifiedTime(b);
    if (left !== right) {
      if (left === null) return 1;
      if (right === null) return -1;
      return sort === "recent" ? right - left : left - right;
    }
    return collator.compare(a.name, b.name);
  });
}
