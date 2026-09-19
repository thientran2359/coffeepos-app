import { en } from "./en";
import type { AppLanguage, TranslationParams } from "./types";
import { vi, type TranslationKey } from "./vi";

export type { AppLanguage } from "./types";
export type { TranslationKey } from "./vi";

const catalogs = { vi, en } as const;
const formatLocales: Record<AppLanguage, string> = { vi: "vi-VN", en: "en-US" };
let locale: AppLanguage = "vi";

export function isAppLanguage(value: unknown): value is AppLanguage {
  return value === "vi" || value === "en";
}

export function getLocale(): AppLanguage {
  return locale;
}

export function setLocale(next: AppLanguage): void {
  if (!isAppLanguage(next)) throw new Error(`Unsupported app locale: ${String(next)}`);
  locale = next;
  document.documentElement.lang = next;
}

export function hasTranslation(key: string): key is TranslationKey {
  return Object.prototype.hasOwnProperty.call(vi, key);
}

function interpolate(template: string, params: TranslationParams, key: string): string {
  const placeholders = new Set(Array.from(template.matchAll(/\{([A-Za-z0-9_]+)\}/g), (match) => match[1]));
  if (import.meta.env.DEV) {
    for (const placeholder of placeholders) {
      if (!(placeholder in params)) throw new Error(`Missing translation placeholder {${placeholder}} for ${key}`);
    }
  }
  return template.replace(/\{([A-Za-z0-9_]+)\}/g, (match, name: string) => (
    Object.prototype.hasOwnProperty.call(params, name) ? String(params[name]) : match
  ));
}

export function t(key: TranslationKey | string, params: TranslationParams = {}): string {
  const current = catalogs[locale] as Record<string, string>;
  const fallback = vi as Record<string, string>;
  const template = current[key] ?? fallback[key];
  if (!template) {
    if (import.meta.env.DEV) throw new Error(`Missing translation key: ${key}`);
    console.warn(`i18n_missing_key:${key}`);
    return fallback["errors.generic"] ?? "CoffeePOS";
  }
  if (!(key in current) && !import.meta.env.DEV) console.warn(`i18n_fallback_key:${key}`);
  return interpolate(template, params, key);
}

export function tFor(language: AppLanguage, key: TranslationKey | string, params: TranslationParams = {}): string {
  const catalog = catalogs[language] as Record<string, string>;
  const fallback = vi as Record<string, string>;
  const template = catalog[key] ?? fallback[key];
  if (!template) {
    if (import.meta.env.DEV) throw new Error(`Missing translation key: ${key}`);
    console.warn(`i18n_missing_key:${key}`);
    return fallback["errors.generic"] ?? "CoffeePOS";
  }
  if (!(key in catalog) && !import.meta.env.DEV) console.warn(`i18n_fallback_key:${key}`);
  return interpolate(template, params, key);
}

export function applyDocumentTranslations(root: ParentNode = document): void {
  for (const node of root.querySelectorAll<HTMLElement>("[data-i18n]")) {
    node.textContent = t(node.dataset.i18n ?? "errors.generic");
  }
  for (const node of root.querySelectorAll<HTMLElement>("[data-i18n-locale][data-i18n-key]")) {
    const language = node.dataset.i18nLocale;
    if (isAppLanguage(language)) node.textContent = tFor(language, node.dataset.i18nKey ?? "errors.generic");
  }
  for (const node of root.querySelectorAll<HTMLElement>("[data-i18n-aria-label]")) {
    node.setAttribute("aria-label", t(node.dataset.i18nAriaLabel ?? "errors.generic"));
  }
  for (const node of root.querySelectorAll<HTMLElement>("[data-i18n-title]")) {
    node.setAttribute("title", t(node.dataset.i18nTitle ?? "errors.generic"));
  }
  document.title = t("app.title");
}

export function formatDateTime(value: Date | number | string): string {
  const date = value instanceof Date ? value : new Date(value);
  if (Number.isNaN(date.getTime())) return "—";
  return new Intl.DateTimeFormat(formatLocales[locale], { dateStyle: "short", timeStyle: "short" }).format(date);
}

export function formatNumber(value: number, options?: Intl.NumberFormatOptions): string {
  return new Intl.NumberFormat(formatLocales[locale], options).format(value);
}

export function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"] as const;
  let amount = value;
  let unit = 0;
  while (amount >= 1024 && unit < units.length - 1) {
    amount /= 1024;
    unit += 1;
  }
  return `${formatNumber(amount, { maximumFractionDigits: amount >= 10 || unit === 0 ? 0 : 1 })} ${units[unit]}`;
}

if (import.meta.env.DEV) {
  const viKeys = Object.keys(vi).sort();
  const enKeys = Object.keys(en).sort();
  if (viKeys.length !== enKeys.length || viKeys.some((key, index) => key !== enKeys[index])) {
    throw new Error("Translation catalogs do not have matching key sets.");
  }
  const placeholders = (value: string): string[] => (
    Array.from(value.matchAll(/\{([A-Za-z0-9_]+)\}/g), (match) => match[1]).sort()
  );
  for (const key of viKeys) {
    const viPlaceholders = placeholders((vi as Record<string, string>)[key]);
    const enPlaceholders = placeholders((en as Record<string, string>)[key]);
    if (
      viPlaceholders.length !== enPlaceholders.length
      || viPlaceholders.some((placeholder, index) => placeholder !== enPlaceholders[index])
    ) {
      throw new Error(`Translation placeholder mismatch for ${key}.`);
    }
  }
}
