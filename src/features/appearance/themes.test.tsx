import { act, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc } from "@/lib/ipc";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import { ROW_H, setRowHeight, DEFAULT_ROW_H } from "@/features/datagrid/types";
import { DEFAULT_PREFS, getPrefs, resetPrefsForTests, setPref } from "@/stores/prefs";
import { useUiStore } from "@/stores/ui";
import { AppearanceHost } from "./AppearanceHost";
import { applyThemeName, customThemeCss, resolveSelection } from "./selection";
import {
  deleteCustomTheme,
  draftFromSelection,
  exportText,
  importThemeFromFile,
  importThemeText,
  newThemeDraft,
  saveNewTheme,
  saveThemeEdit,
  exportThemeToFile,
  themeLabel,
} from "./themeActions";
import {
  THEMES_KEY,
  THEMES_LOCAL_KEY,
  flushThemes,
  loadThemes,
  resetThemesForTests,
  useThemes,
} from "./themesStore";
import { baseTokens, builtinTheme } from "./builtinThemes";
import { serializeTheme, type CustomTheme } from "./customTheme";

const dusk: CustomTheme = {
  id: "tdusk",
  name: "Dusk",
  base: "dark",
  tokens: {
    background: "#101418",
    brand: "#ff8800",
    "primary-foreground": "#000000",
    foreground: "#eeeeee",
  },
};

beforeEach(() => {
  localStorage.clear();
  resetMockSettings();
  resetPrefsForTests();
  resetThemesForTests();
  useUiStore.setState({ themePref: "system" });
});
afterEach(() => {
  document.documentElement.removeAttribute("style");
  document.documentElement.removeAttribute("data-theme-name");
  document.documentElement.removeAttribute("data-accent");
  document.getElementById("tabler-custom-theme")?.remove();
  setRowHeight(DEFAULT_ROW_H);
  resetPrefsForTests();
  resetThemesForTests();
  vi.restoreAllMocks();
});

describe("resolveSelection", () => {
  it("resolves Auto to the system's light or dark", () => {
    const light = resolveSelection("system", false, []);
    expect(light).toMatchObject({ key: "light", base: "light", name: null, custom: null });
    expect(light.tokens).toBe(baseTokens("light"));
    expect(resolveSelection("system", true, [])).toMatchObject({
      key: "dark",
      base: "dark",
      name: null,
    });
  });

  it("ignores the system for the explicit themes", () => {
    expect(resolveSelection("light", true, [])).toMatchObject({
      key: "light",
      base: "light",
      name: null,
    });
    expect(resolveSelection("dark", false, [])).toMatchObject({
      key: "dark",
      base: "dark",
      name: null,
    });
    const sl = resolveSelection("solarized-light", true, []);
    expect(sl).toMatchObject({ key: "solarized-light", base: "light", name: "solarized-light" });
    expect(sl.tokens).toBe(builtinTheme("solarized-light")!.tokens);
    expect(resolveSelection("solarized-dark", false, [])).toMatchObject({
      key: "solarized-dark",
      base: "dark",
      name: "solarized-dark",
    });
  });

  it("resolves a custom theme, filling the tokens it leaves out from its base", () => {
    const s = resolveSelection("custom:tdusk", false, [dusk]);
    expect(s).toMatchObject({ key: "custom:tdusk", base: "dark", name: "custom", custom: dusk });
    expect(s.tokens.background).toBe("#101418");
    expect(s.tokens.surface).toBe(baseTokens("dark").surface);
  });

  it("falls back to the system when the custom theme is gone", () => {
    expect(resolveSelection("custom:tgone", true, [dusk])).toMatchObject({
      key: "dark",
      name: null,
      custom: null,
    });
    expect(resolveSelection("custom:tgone", false, [])).toMatchObject({ key: "light", name: null });
  });
});

describe("applying a theme to <html>", () => {
  it("sets and clears data-theme-name", () => {
    applyThemeName(resolveSelection("solarized-dark", false, []));
    expect(document.documentElement.dataset.themeName).toBe("solarized-dark");
    applyThemeName(resolveSelection("dark", false, []));
    expect(document.documentElement.dataset.themeName).toBeUndefined();
  });

  it("writes a custom theme's colours as a style element and removes it with the theme", () => {
    applyThemeName(resolveSelection("custom:tdusk", false, [dusk]));
    const style = document.getElementById("tabler-custom-theme");
    expect(document.documentElement.dataset.themeName).toBe("custom");
    expect(style?.textContent).toContain("--background: #101418;");
    expect(style?.textContent).toContain("--foreground: #eeeeee;");
    applyThemeName(resolveSelection("light", false, []));
    expect(document.getElementById("tabler-custom-theme")).toBeNull();
  });

  it("updates the same style element when the theme is edited", () => {
    applyThemeName(resolveSelection("custom:tdusk", false, [dusk]));
    applyThemeName(
      resolveSelection("custom:tdusk", false, [{ ...dusk, tokens: { background: "#222222" } }]),
    );
    expect(document.querySelectorAll("#tabler-custom-theme")).toHaveLength(1);
    expect(document.getElementById("tabler-custom-theme")?.textContent).toContain("#222222");
  });

  it("keeps the theme's accent from overriding a connection's own colour", () => {
    const css = customThemeCss(dusk);
    const [main, accent] = css.split("\n}\n");
    expect(main).toContain(':root[data-theme-name="custom"] {');
    expect(main).not.toContain("--brand");
    expect(accent).toContain(':root[data-theme-name="custom"]:not([data-accent])');
    expect(accent).toContain("--brand: #ff8800;");
    expect(accent).toContain("--primary-foreground: #000000;");
  });

  it("only writes known tokens with valid colours", () => {
    const css = customThemeCss({
      ...dusk,
      tokens: {
        background: "#111111",
        foreground: "red; } body { display: none",
        nonsense: "#fff",
      } as CustomTheme["tokens"],
    });
    expect(css).toContain("--background: #111111;");
    expect(css).not.toContain("display");
    expect(css).not.toContain("nonsense");
  });
});

describe("the themes store", () => {
  it("adds themes with unique names and ids", () => {
    const a = useThemes.getState().add({ name: "Night", base: "dark", tokens: {} })!;
    const b = useThemes.getState().add({ name: "night", base: "dark", tokens: {} })!;
    expect(a.name).toBe("Night");
    expect(b.name).toBe("night 2");
    expect(a.id).not.toBe(b.id);
    expect(useThemes.getState().themes).toHaveLength(2);
  });

  it("renames keeping names unique, updates tokens and removes", () => {
    const a = useThemes.getState().add({ name: "A", base: "light", tokens: {} })!;
    const b = useThemes.getState().add({ name: "B", base: "light", tokens: {} })!;
    useThemes.getState().update(b.id, { name: "a" });
    expect(useThemes.getState().themes.find((t) => t.id === b.id)?.name).toBe("a 2");
    useThemes.getState().update(a.id, { name: "A" });
    expect(useThemes.getState().themes.find((t) => t.id === a.id)?.name).toBe("A");
    useThemes.getState().update(a.id, { name: "   " });
    expect(useThemes.getState().themes.find((t) => t.id === a.id)?.name).toBe("A");
    useThemes.getState().update(a.id, { tokens: { surface: "#fff" }, base: "dark" });
    expect(useThemes.getState().themes.find((t) => t.id === a.id)).toMatchObject({
      base: "dark",
      tokens: { surface: "#fff" },
    });
    useThemes.getState().remove(a.id);
    expect(useThemes.getState().themes.map((t) => t.id)).toEqual([b.id]);
  });

  it("saves to the app DB and keeps a localStorage copy", async () => {
    useThemes.getState().add({ name: "Night", base: "dark", tokens: { surface: "#000" } });
    await flushThemes();
    const stored = (await ipc.settings.get())[THEMES_KEY] as CustomTheme[];
    expect(stored).toHaveLength(1);
    expect(stored[0]).toMatchObject({ name: "Night", base: "dark", tokens: { surface: "#000" } });
    expect(JSON.parse(localStorage.getItem(THEMES_LOCAL_KEY)!)).toEqual(stored);
  });

  it("loads the app DB copy, dropping invalid entries", async () => {
    await ipc.settings.set(THEMES_KEY, [dusk, { id: "bad", name: "", base: "x", tokens: {} }]);
    await loadThemes();
    expect(useThemes.getState().themes).toEqual([dusk]);
    expect(useThemes.getState().loaded).toBe(true);
  });

  it("pushes a local-only copy (first run, mock backend) to the app DB", async () => {
    localStorage.setItem(THEMES_LOCAL_KEY, JSON.stringify([dusk]));
    resetThemesForTests();
    localStorage.setItem(THEMES_LOCAL_KEY, JSON.stringify([dusk]));
    // Re-create the store state from the localStorage copy, as a fresh start does.
    useThemes.setState({ themes: [dusk] });
    await loadThemes();
    await flushThemes();
    expect((await ipc.settings.get())[THEMES_KEY]).toEqual([dusk]);
  });

  it("lets edits made before the load win over the stored copy", async () => {
    await ipc.settings.set(THEMES_KEY, [dusk]);
    useThemes.getState().add({ name: "Mine", base: "light", tokens: {} });
    await loadThemes();
    expect(useThemes.getState().themes.map((t) => t.name)).toEqual(["Mine"]);
  });

  it("applies another window's list without saving it again", async () => {
    useThemes.getState().applyExternal([dusk, { nonsense: true }]);
    expect(useThemes.getState().themes).toEqual([dusk]);
    expect(JSON.parse(localStorage.getItem(THEMES_LOCAL_KEY)!)).toEqual([dusk]);
    await flushThemes();
    expect((await ipc.settings.get())[THEMES_KEY]).toBeUndefined();
  });
});

describe("theme actions", () => {
  it("names a copy of the current theme and folds the colour overrides in", () => {
    useUiStore.setState({ themePref: "dark" });
    setPref("colorOverrides", { dark: { "syn-keyword": "#abcdef" } });
    const draft = newThemeDraft();
    expect(draft.name).toBe("Dark copy");
    expect(draft.base).toBe("dark");
    expect(draft.tokens["syn-keyword"]).toBe("#abcdef");
    expect(draft.tokens.background).toBe(baseTokens("dark").background);
    // Every token is there: the editor shows them all.
    expect(Object.keys(draft.tokens).length).toBeGreaterThanOrEqual(30);
  });

  it("labels the choices", () => {
    expect(themeLabel("system")).toBe("Auto");
    expect(themeLabel("solarized-light")).toBe("Solarized Light");
    expect(themeLabel("custom:tdusk", [dusk])).toBe("Dusk");
    expect(themeLabel("custom:tgone", [])).toBe("Custom theme");
  });

  it("saves a new theme and switches to it", () => {
    const created = saveNewTheme(
      draftFromSelection(resolveSelection("light", false, []), {}, "Light"),
    )!;
    expect(created.name).toBe("Light copy");
    expect(useUiStore.getState().themePref).toBe(`custom:${created.id}`);
    expect(useThemes.getState().themes).toHaveLength(1);
  });

  it("saves edits to a theme", () => {
    const created = saveNewTheme({ name: "T", base: "light", tokens: {} }, false)!;
    expect(useUiStore.getState().themePref).toBe("system");
    saveThemeEdit(created.id, { name: "T2", base: "dark", tokens: { surface: "#123456" } });
    expect(useThemes.getState().themes[0]).toMatchObject({
      name: "T2",
      base: "dark",
      tokens: { surface: "#123456" },
    });
  });

  it("deleting the theme in use falls back to Auto and drops its colour overrides", () => {
    const created = saveNewTheme({ name: "T", base: "light", tokens: {} })!;
    setPref("colorOverrides", {
      [`custom:${created.id}`]: { "syn-keyword": "#ff0000" },
      dark: { "syn-keyword": "#00ff00" },
    });
    deleteCustomTheme(created.id);
    expect(useUiStore.getState().themePref).toBe("system");
    expect(useThemes.getState().themes).toEqual([]);
    expect(getPrefs().colorOverrides).toEqual({ dark: { "syn-keyword": "#00ff00" } });
  });

  it("deleting another theme keeps the choice", () => {
    const a = saveNewTheme({ name: "A", base: "light", tokens: {} }, false)!;
    const b = saveNewTheme({ name: "B", base: "light", tokens: {} })!;
    deleteCustomTheme(a.id);
    expect(useUiStore.getState().themePref).toBe(`custom:${b.id}`);
  });

  it("exports the theme in effect, overrides included, and imports it back identically", () => {
    useUiStore.setState({ themePref: "solarized-dark" });
    setPref("colorOverrides", { "solarized-dark": { "change-new": "#33aa5599" } });
    const selection = resolveSelection("solarized-dark", false, []);
    const text = exportText(selection, "Solarized Dark");
    const result = importThemeText(text);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.theme).toMatchObject({ name: "Solarized Dark", base: "dark" });
    expect(result.theme.tokens["change-new"]).toBe("#33aa5599");
    expect(result.theme.tokens.background).toBe(builtinTheme("solarized-dark")!.tokens.background);
    // The import is selected.
    expect(useUiStore.getState().themePref).toBe(`custom:${result.theme.id}`);
  });

  it("reports a bad file instead of importing it", () => {
    const result = importThemeText("{}");
    expect(result).toMatchObject({ ok: false });
    expect(useThemes.getState().themes).toEqual([]);
    expect(useUiStore.getState().themePref).toBe("system");
  });

  it("writes and reads .tablertheme files through the file commands", async () => {
    const prompt = vi.spyOn(window, "prompt").mockReturnValue("/tmp/Night.tablertheme");
    const selection = resolveSelection("custom:tdusk", false, [dusk]);
    const path = await exportThemeToFile(selection, "Dusk");
    expect(path).toBe("/tmp/Night.tablertheme");
    expect(prompt).toHaveBeenCalled();
    expect(await ipc.files.readText("/tmp/Night.tablertheme")).toContain(
      '"format": "tabler-theme"',
    );
    const result = await importThemeFromFile();
    expect(result).toMatchObject({ ok: true, theme: { name: "Dusk", base: "dark" } });
  });

  it("does nothing when the file panel is cancelled", async () => {
    vi.spyOn(window, "prompt").mockReturnValue(null);
    expect(await exportThemeToFile(resolveSelection("light", false, []), "x")).toBeNull();
    expect(await importThemeFromFile()).toBeNull();
  });

  it("explains a missing or invalid import file", async () => {
    vi.spyOn(window, "prompt").mockReturnValue("/tmp/does-not-exist.tablertheme");
    const missing = await importThemeFromFile();
    expect(missing).toMatchObject({ ok: false });
    await ipc.files.writeText("/tmp/junk.tablertheme", "not json");
    vi.spyOn(window, "prompt").mockReturnValue("/tmp/junk.tablertheme");
    expect(await importThemeFromFile()).toMatchObject({
      ok: false,
      error: expect.stringContaining("not valid JSON"),
    });
  });
});

describe("AppearanceHost", () => {
  const html = document.documentElement;

  it("applies a built-in theme's name and the preferences' custom properties", () => {
    useUiStore.setState({ themePref: "solarized-light" });
    setPref("gridFontSize", 14);
    setPref("sidebarFontSize", 13);
    setPref("gridNoElastic", true);
    render(<AppearanceHost />);
    expect(html.dataset.themeName).toBe("solarized-light");
    expect(html.style.getPropertyValue("--grid-font-size")).toBe("14px");
    expect(html.style.getPropertyValue("--sidebar-font-size")).toBe("13px");
    expect(html).toHaveAttribute("data-grid-no-elastic");
  });

  it("follows later changes: theme, preferences, colour overrides and the row height", () => {
    render(<AppearanceHost />);
    expect(html.dataset.themeName).toBeUndefined();
    expect(ROW_H).toBe(22);
    act(() => useUiStore.setState({ themePref: "solarized-dark" }));
    expect(html.dataset.themeName).toBe("solarized-dark");
    act(() => setPref("gridRowPadding", 6));
    expect(ROW_H).toBe(28);
    act(() => setPref("gridFontSize", 14));
    expect(ROW_H).toBe(gridRow(14));
    act(() => setPref("colorOverrides", { "solarized-dark": { "change-modified": "#ffaa0080" } }));
    expect(html.style.getPropertyValue("--change-modified")).toBe("#ffaa0080");
    // Another theme: the override of Solarized Dark does not follow.
    act(() => useUiStore.setState({ themePref: "dark" }));
    expect(html.style.getPropertyValue("--change-modified")).toBe("");
    act(() => setPref("gridRowPadding", DEFAULT_PREFS.gridRowPadding));
    act(() => setPref("gridFontSize", 12));
    expect(ROW_H).toBe(22);
  });

  it("applies a custom theme at once and drops it when it is deleted", () => {
    render(<AppearanceHost />);
    let created!: CustomTheme;
    act(() => {
      created = saveNewTheme({ name: "Mine", base: "dark", tokens: { surface: "#0a0b0c" } })!;
    });
    expect(html.dataset.themeName).toBe("custom");
    expect(document.getElementById("tabler-custom-theme")?.textContent).toContain(
      "--surface: #0a0b0c;",
    );
    act(() =>
      saveThemeEdit(created.id, { name: "Mine", base: "dark", tokens: { surface: "#111213" } }),
    );
    expect(document.getElementById("tabler-custom-theme")?.textContent).toContain("#111213");
    act(() => deleteCustomTheme(created.id));
    expect(html.dataset.themeName).toBeUndefined();
    expect(document.getElementById("tabler-custom-theme")).toBeNull();
  });

  it("loads the stored themes and picks up another window's changes", async () => {
    await ipc.settings.set(THEMES_KEY, [dusk]);
    useUiStore.setState({ themePref: "custom:tdusk" });
    render(<AppearanceHost />);
    await vi.waitFor(() => expect(html.dataset.themeName).toBe("custom"));
    const changed = [{ ...dusk, tokens: { ...dusk.tokens, background: "#202428" } }];
    act(() => {
      window.dispatchEvent(
        new StorageEvent("storage", { key: THEMES_LOCAL_KEY, newValue: JSON.stringify(changed) }),
      );
    });
    expect(document.getElementById("tabler-custom-theme")?.textContent).toContain("#202428");
  });

  it("ignores unrelated or unreadable storage events", () => {
    render(<AppearanceHost />);
    act(() => {
      window.dispatchEvent(new StorageEvent("storage", { key: "other", newValue: "[]" }));
      window.dispatchEvent(
        new StorageEvent("storage", { key: THEMES_LOCAL_KEY, newValue: "{half" }),
      );
      window.dispatchEvent(new StorageEvent("storage", { key: THEMES_LOCAL_KEY, newValue: null }));
    });
    expect(useThemes.getState().themes).toEqual([]);
  });

  it("serializes what the import test needs", () => {
    expect(serializeTheme(dusk)).toContain("Dusk");
  });
});

/** The row height at `fontSize` with the default line height and a 6px padding. */
function gridRow(fontSize: number): number {
  return Math.max(10, Math.round(fontSize * DEFAULT_PREFS.gridLineHeight)) + 2 * 6;
}
