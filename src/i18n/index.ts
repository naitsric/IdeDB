import { useCallback } from "react";
import { create } from "zustand";
import en from "./locales/en";

/**
 * UI texts live in one file per language under `locales/`. Adding a language
 * is adding a file there: it exports its `name` (shown in the language
 * command) and the default export with every key of `en.ts`, the base
 * language. A text missing from a language shows in English.
 */
export type MessageKey = keyof typeof en;
export type Messages = Record<MessageKey, string>;
export type Params = Record<string, string | number>;
export type Locale = string;
export type LanguagePreference = "system" | Locale;
export type Catalogs = Record<Locale, Partial<Messages>>;

interface LocaleModule {
  name: string;
  default: Messages;
}

const modules = import.meta.glob<LocaleModule>("./locales/*.ts", { eager: true });

/** Every language found in `locales/`, by code (the file name). */
export const LOCALES: Record<Locale, { name: string; messages: Messages }> = Object.fromEntries(
  Object.entries(modules).map(([path, mod]) => [
    path.replace(/^.*\/(.+)\.ts$/, "$1"),
    { name: mod.name, messages: mod.default },
  ]),
);

export const BASE_LOCALE: Locale = "en";
const CATALOGS: Catalogs = Object.fromEntries(Object.entries(LOCALES).map(([code, l]) => [code, l.messages]));

/**
 * The language to start in for the system's languages: its first one if
 * there is a translation, else English.
 */
export function detectLocale(languages: readonly string[], available: readonly Locale[]): Locale {
  const primary = languages[0]?.split(/[-_]/)[0]?.toLowerCase();
  return primary && available.includes(primary) ? primary : BASE_LOCALE;
}

/** The text for a key in a language: English when missing there, the key only if English lacks it too. */
export function translate(key: string, params: Params | undefined, locale: Locale, catalogs: Catalogs): string {
  const text = catalogs[locale]?.[key as MessageKey] || catalogs[BASE_LOCALE]?.[key as MessageKey] || key;
  return params ? text.replace(/\{(\w+)\}/g, (whole, name: string) => (name in params ? String(params[name]) : whole)) : text;
}

const STORAGE_KEY = "idedb.language";

function systemLanguages(): readonly string[] {
  if (typeof navigator === "undefined") return [];
  return navigator.languages?.length ? navigator.languages : [navigator.language];
}

function resolve(preference: LanguagePreference): Locale {
  if (preference !== "system" && preference in LOCALES) return preference;
  return detectLocale(systemLanguages(), Object.keys(LOCALES));
}

function readPreference(): LanguagePreference {
  try {
    const stored = typeof localStorage === "undefined" ? null : localStorage.getItem(STORAGE_KEY);
    if (stored && (stored === "system" || stored in LOCALES)) return stored;
  } catch {
    // Storage unavailable: follow the system.
  }
  return "system";
}

interface LanguageState {
  preference: LanguagePreference;
  locale: Locale;
  setPreference: (preference: LanguagePreference) => void;
}

export const useLanguage = create<LanguageState>((set) => {
  const preference = readPreference();
  return {
    preference,
    locale: resolve(preference),
    setPreference: (preference) => {
      try {
        if (typeof localStorage !== "undefined") localStorage.setItem(STORAGE_KEY, preference);
      } catch {
        // Not persisted; still applies for this session.
      }
      set({ preference, locale: resolve(preference) });
    },
  };
});

// The UI tests of the stores run without a DOM.
if (typeof window !== "undefined") {
  const applyLanguage = ({ locale }: LanguageState) => {
    document.documentElement.lang = locale;
  };
  applyLanguage(useLanguage.getState());
  useLanguage.subscribe(applyLanguage);
  window.addEventListener("languagechange", () => {
    const { preference } = useLanguage.getState();
    if (preference === "system") useLanguage.setState({ locale: resolve(preference) });
  });
}

/** Translates for the active language. For code outside components; components use {@link useT}. */
export function t(key: MessageKey, params?: Params): string {
  return translate(key, params, useLanguage.getState().locale, CATALOGS);
}

/** Like {@link t} for keys built at run time (command ids, categories). */
export function tDynamic(key: string, params?: Params): string {
  return translate(key, params, useLanguage.getState().locale, CATALOGS);
}

/** Translate function bound to the active language; the component re-renders when it changes. */
export function useT(): typeof t {
  const locale = useLanguage((s) => s.locale);
  return useCallback((key: MessageKey, params?: Params) => translate(key, params, locale, CATALOGS), [locale]);
}

export function useLocale(): Locale {
  return useLanguage((s) => s.locale);
}
