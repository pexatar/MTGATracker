// Lightweight in-app internationalization. Two languages only: English (the
// default and source of truth) and Italian. Card names are NEVER translated
// here — they always stay in English by design.
//
// Usage in a component:
//   import { t, getLocale, setLocale, LOCALES } from "$lib/i18n.svelte";
//   <h1>{t("settings.title")}</h1>
// `t()` reads the reactive locale, so the markup updates the moment it changes.

export type Locale = "en" | "it";

// The languages the toggle offers. Driven from here so the UI never hardcodes
// its own list — add a language in one place if it ever grows beyond two.
export const LOCALES: { code: Locale; label: string }[] = [
  { code: "en", label: "ENG" },
  { code: "it", label: "ITA" },
];

const STORAGE_KEY = "mtgat.locale";

// Translation table. Every key MUST exist under `en` (the fallback); `it` may
// fill in over time. Group keys by UI area to keep them findable.
const dict: Record<Locale, Record<string, string>> = {
  en: {
    "settings.title": "Settings",
  },
  it: {
    "settings.title": "Impostazioni",
  },
};

function load(): Locale {
  if (typeof localStorage === "undefined") return "en";
  const saved = localStorage.getItem(STORAGE_KEY);
  return saved === "it" || saved === "en" ? saved : "en";
}

let locale = $state<Locale>(load());

export function getLocale(): Locale {
  return locale;
}

export function setLocale(next: Locale): void {
  locale = next;
  if (typeof localStorage !== "undefined") localStorage.setItem(STORAGE_KEY, next);
}

// Translate a key into the active language. Falls back to English, then to the
// raw key, so a missing translation is visible but never crashes the UI.
export function t(key: string): string {
  return dict[locale][key] ?? dict.en[key] ?? key;
}
