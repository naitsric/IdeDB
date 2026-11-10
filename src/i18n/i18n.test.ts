// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { buildMenuModel } from "../menu/menuModel";
import type { Command } from "../commands/registry";
import { LOCALES, detectLocale, translate, useLanguage, t, type Catalogs } from "./index";
import en from "./locales/en";
import es from "./locales/es";

const placeholders = (text: string) => [...text.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();

describe("translation files", () => {
  it("every locale has exactly the keys of English", () => {
    for (const [code, messages] of Object.entries(LOCALES)) {
      expect(Object.keys(messages.messages).sort(), code).toEqual(Object.keys(en).sort());
    }
  });

  it("no translation is empty and placeholders match English", () => {
    for (const [code, { messages }] of Object.entries(LOCALES)) {
      for (const [key, text] of Object.entries(messages)) {
        expect(text.trim(), `${code}:${key}`).not.toBe("");
        expect(placeholders(text), `${code}:${key}`).toEqual(placeholders((en as Record<string, string>)[key]));
      }
    }
  });

  it("registers English and Spanish, one file each", () => {
    expect(Object.keys(LOCALES).sort()).toEqual(["en", "es"]);
    expect(LOCALES.es.name).toBe("Español");
  });

  it("keeps technical terms in English inside Spanish text", () => {
    expect(es["command.transaction.commit"]).toBe("Commit");
    expect(es["command.transaction.rollback"]).toBe("Rollback");
    expect(es["command.datasource.new"]).toBe("Nuevo Data Source");
    expect(es["command.console.new"]).toContain("Console");
  });
});

describe("detectLocale", () => {
  const available = ["en", "es"];
  it.each([
    [["es-CO"], "es"],
    [["es"], "es"],
    [["ES-mx"], "es"],
    [["en-US"], "en"],
    [["fr-FR"], "en"],
    [["fr-FR", "es"], "en"], // the system language is the first one
    [[], "en"],
  ])("%j → %s", (languages, expected) => {
    expect(detectLocale(languages, available)).toBe(expected);
  });
});

describe("translate", () => {
  const catalogs: Catalogs = {
    en: { "a": "Hello", "b": "Only English", "c": "Hi {name}, {name}" },
    es: { "a": "Hola", "c": "Hola {name}" },
  } as unknown as Catalogs;

  it("uses the active language", () => {
    expect(translate("a", undefined, "es", catalogs)).toBe("Hola");
  });
  it("falls back to English for a missing text, never the key or empty", () => {
    expect(translate("b", undefined, "es", catalogs)).toBe("Only English");
  });
  it("fills placeholders", () => {
    expect(translate("c", { name: "Ana" }, "en", catalogs)).toBe("Hi Ana, Ana");
  });
  it("shows an empty-string translation as English too", () => {
    const c = { en: { a: "Hello" }, es: { a: "" } } as unknown as Catalogs;
    expect(translate("a", undefined, "es", c)).toBe("Hello");
  });
});

describe("language preference", () => {
  beforeEach(() => {
    localStorage.clear();
    useLanguage.getState().setPreference("system");
  });

  it("switches at once and is remembered", () => {
    useLanguage.getState().setPreference("es");
    expect(useLanguage.getState().locale).toBe("es");
    expect(t("command.transaction.commit")).toBe("Commit");
    expect(t("command.datasource.new")).toBe("Nuevo Data Source");
    expect(localStorage.getItem("idedb.language")).toBe("es");
    useLanguage.getState().setPreference("en");
    expect(t("command.datasource.new")).toBe("New Data Source");
  });

  it("system follows the language of the system", () => {
    useLanguage.getState().setPreference("system");
    expect(useLanguage.getState().locale).toBe(detectLocale(navigator.languages, Object.keys(LOCALES)));
    expect(localStorage.getItem("idedb.language")).toBe("system");
  });
});

describe("native menu in the active language", () => {
  const command = (id: string, category: string): Command => ({ id, title: id, titleKey: `command.${id}`, category, run: () => {} }) as Command;
  it("titles menus and commands in Spanish and English", () => {
    const commands = [command("console.new", "Console"), command("datasource.new", "Data Source")];
    useLanguage.getState().setPreference("es");
    const spanish = buildMenuModel(commands);
    expect(spanish.map((m) => m.title)).toContain("Archivo");
    expect(JSON.stringify(spanish)).toContain("Nueva Query Console");
    expect(spanish.map((m) => m.title)).toEqual(expect.arrayContaining(["Archivo", "Consulta", "Ayuda"]));
    useLanguage.getState().setPreference("en");
    const english = buildMenuModel(commands);
    expect(english.map((m) => m.title)).toContain("File");
    expect(JSON.stringify(english)).toContain("New Query Console");
  });
});

describe("command names", () => {
  it("every app command has a name in every language", () => {
    const source = Object.values(
      import.meta.glob<string>("../commands/appCommands.ts", { query: "?raw", import: "default", eager: true }),
    )[0];
    const ids = [...source.matchAll(/^\s{6}id: "([\w.]+)"/gm)].map((m) => m[1]);
    const tuples = [...source.matchAll(/\["(grid\.\w+)", "\w+"\]/g)].map((m) => m[1]);
    const all = [...ids, ...tuples, "appearance.theme.system", "appearance.theme.light", "appearance.theme.dark"];
    expect(all.length).toBeGreaterThan(40);
    for (const id of all) {
      for (const [code, { messages }] of Object.entries(LOCALES)) {
        expect((messages as Record<string, string>)[`command.${id}`], `${code}: command.${id}`).toBeTruthy();
      }
    }
  });

  it("every command category has a name", () => {
    for (const category of ["Data Source", "Workbench", "Data Editor", "Results", "View", "Database Explorer", "Appearance", "Navigate", "Console", "Transaction", "MCP", "Application"]) {
      expect((en as Record<string, string>)[`category.${category}`], category).toBeTruthy();
    }
  });
});

describe("initial language", () => {
  const fresh = async (stored: string | null, languages: string[]) => {
    vi.resetModules();
    localStorage.clear();
    if (stored !== null) localStorage.setItem("idedb.language", stored);
    vi.spyOn(navigator, "languages", "get").mockReturnValue(languages);
    return (await import("./index")).useLanguage.getState().locale;
  };
  afterEach(() => vi.restoreAllMocks());

  it("follows a Spanish system when nothing was chosen", async () => {
    expect(await fresh(null, ["es-CO", "en"])).toBe("es");
  });
  it("falls back to English for a language with no translation", async () => {
    expect(await fresh(null, ["fr-FR"])).toBe("en");
  });
  it("opens in the chosen language whatever the system says", async () => {
    expect(await fresh("es", ["en-US"])).toBe("es");
    expect(await fresh("en", ["es-ES"])).toBe("en");
  });
  it("ignores a stored language that has no file any more", async () => {
    expect(await fresh("de", ["es-ES"])).toBe("es");
  });
});
