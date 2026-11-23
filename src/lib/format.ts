import { currentLanguage, i18n } from "@/i18n";
import { isApiError } from "@/lib/api";
import { coreText } from "@/lib/core-text";

/**
 * The Intl locale of the language shown. English reads dates the British way ("9 Nov"), the way DiffLens always
 * has; Portuguese is Brazilian ("9 de nov.").
 */
export function intlLocale(): string {
  return currentLanguage() === "pt-BR" ? "pt-BR" : "en-GB";
}

/** A count or measure in the language shown: "12,345" in English, "12.345" in Portuguese. */
export function formatNumber(value: number, maximumFractionDigits = 0): string {
  return new Intl.NumberFormat(intlLocale(), { maximumFractionDigits }).format(value);
}

/** A number with at most one decimal, without a trailing ".0": "1.8", "2", "1,8" in Portuguese. No grouping. */
function oneDecimal(value: number): string {
  return new Intl.NumberFormat(intlLocale(), { maximumFractionDigits: 1, useGrouping: false }).format(value);
}

const LANGUAGE_NAMES: Record<string, string> = {
  c: "C",
  cpp: "C++",
  csharp: "C#",
  go: "Go",
  java: "Java",
  javascript: "JavaScript",
  kotlin: "Kotlin",
  php: "PHP",
  python: "Python",
  ruby: "Ruby",
  rust: "Rust",
  swift: "Swift",
  typescript: "TypeScript",
};

export function languageName(id: string): string {
  return LANGUAGE_NAMES[id] ?? id.charAt(0).toUpperCase() + id.slice(1);
}

/** "9 Nov" ("9 de nov."), with the year appended when it is not the current one. */
export function formatDate(iso: string, now: Date = new Date()): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) {
    return "";
  }
  return new Intl.DateTimeFormat(intlLocale(), {
    day: "numeric",
    month: "short",
    year: date.getFullYear() === now.getFullYear() ? undefined : "numeric",
  }).format(date);
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/**
 * "just now", "3 min ago", "2 h ago", "yesterday", "4 days ago", and the date from a week on.
 * A time slightly in the future (clock skew) reads as "just now".
 */
export function formatRelativeTime(iso: string, now: Date = new Date()): string {
  const time = new Date(iso).getTime();
  if (Number.isNaN(time)) {
    return "";
  }
  const elapsed = now.getTime() - time;
  if (elapsed < MINUTE) {
    return i18n.t("common.time.justNow");
  }
  if (elapsed < HOUR) {
    return i18n.t("common.time.minutesAgo", { count: Math.floor(elapsed / MINUTE) });
  }
  if (elapsed < DAY) {
    return i18n.t("common.time.hoursAgo", { count: Math.floor(elapsed / HOUR) });
  }
  const days = Math.floor(elapsed / DAY);
  if (days === 1) {
    return i18n.t("common.time.yesterday");
  }
  if (days < 7) {
    return i18n.t("common.time.daysAgo", { count: days });
  }
  return formatDate(iso, now);
}

/** "820 bytes", "14 KB", "3.1 MB" ("3,1 MB"), in powers of 1024. */
export function formatBytes(size: number): string {
  if (size < 1024) {
    return i18n.t("common.bytes", { count: size });
  }
  const units = ["KB", "MB", "GB"];
  let value = size / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value < 10 ? oneDecimal(value) : Math.round(value)} ${units[unit]}`;
}

/** "3 ms", "1.8 s" ("1,8 s"), "42 s", "2 min 5 s". */
export function formatDuration(ms: number): string {
  if (ms < 1000) {
    return `${Math.round(ms)} ms`;
  }
  if (ms < 10_000) {
    return `${oneDecimal(ms / 1000)} s`;
  }
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) {
    return `${seconds} s`;
  }
  const rest = seconds % 60;
  return rest === 0 ? `${seconds / 60} min` : `${Math.floor(seconds / 60)} min ${rest} s`;
}

/**
 * A share from 0 to 1 as a whole percentage, rounded down so a value just under a target never reads as reaching
 * it: 0.799 is "79%".
 */
export function formatPercent(ratio: number): string {
  return `${Math.floor(ratio * 100 + 1e-9)}%`;
}

/**
 * What went wrong, in the language shown: an error of the core worded from its reason and params (see `coreText`),
 * else its English message; a JavaScript error's or a string's own text; else "Something went wrong".
 */
export function errorMessage(error: unknown): string {
  if (isApiError(error)) {
    return coreText(error);
  }
  if (error instanceof Error && error.message) {
    return error.message;
  }
  return typeof error === "string" && error ? error : i18n.t("common.somethingWentWrong");
}
