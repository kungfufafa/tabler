import { afterEach, describe, expect, it } from "vitest";
import {
  DEFAULT_GRID_ROW_PADDING,
  EMPTY_COLOR_OVERRIDES,
  fontFamilyCss,
  GRID_ROW_PADDING_PRESETS,
  isColorOverrides,
  isFontFamily,
  MAX_FONT_FAMILY_LENGTH,
  withColorOverride,
  withoutThemeOverrides,
} from "./appearancePrefs";
import { DEFAULT_PREFS, resetPrefsForTests, sanitizePrefs } from "@/stores/prefs";

afterEach(() => resetPrefsForTests());

describe("font families", () => {
  it("accepts names with letters, digits, spaces, dots, underscores, plus and dashes", () => {
    for (const ok of [
      "",
      "SF Mono",
      "JetBrains Mono",
      "Fira Code, Menlo",
      "Source Code Pro",
      "Inter-Var",
      "Noto Sans CJK JP",
      "Iosevka 2.0",
      "Ünicode Fönt",
    ])
      expect(isFontFamily(ok), ok).toBe(true);
  });

  it.each([
    'Menlo"',
    "a;b",
    "a{b}",
    "a(b)",
    "url(x)",
    "a\\b",
    "a\nb",
    "a/*b",
    "x".repeat(MAX_FONT_FAMILY_LENGTH + 1),
  ])("rejects %j", (bad) => expect(isFontFamily(bad)).toBe(false));

  it("is false for non-strings", () => {
    expect(isFontFamily(3)).toBe(false);
    expect(isFontFamily(null)).toBe(false);
  });

  it("builds a quoted CSS font-family with a fallback", () => {
    expect(fontFamilyCss("JetBrains Mono", "monospace")).toBe('"JetBrains Mono", monospace');
    expect(fontFamilyCss(" Fira  Code , Menlo ,", "ui-monospace, monospace")).toBe(
      '"Fira Code", "Menlo", ui-monospace, monospace',
    );
  });

  it("gives null for empty or invalid text, so the stylesheet default stays", () => {
    expect(fontFamilyCss("", "monospace")).toBeNull();
    expect(fontFamilyCss(" , ", "monospace")).toBeNull();
    expect(fontFamilyCss('x"; color: red', "monospace")).toBeNull();
  });
});

describe("colour overrides", () => {
  it("validates the shape: known themes, known tokens, real colours", () => {
    expect(isColorOverrides(EMPTY_COLOR_OVERRIDES)).toBe(true);
    expect(isColorOverrides({ dark: { "syn-keyword": "#ff0000" } })).toBe(true);
    expect(isColorOverrides({ "custom:t1abc": { "change-new": "oklch(0.9 0.05 150)" } })).toBe(
      true,
    );
    expect(isColorOverrides({ "solarized-light": { "grid-date": "#33445566" } })).toBe(true);
    for (const bad of [
      null,
      [],
      "x",
      { nope: { "syn-keyword": "#fff" } },
      { dark: { background: "#fff" } },
      { dark: { "syn-keyword": "red" } },
      { dark: [] },
      { dark: null },
      { "custom:": {} },
      { "custom:a b": {} },
    ])
      expect(isColorOverrides(bad), JSON.stringify(bad)).toBe(false);
  });

  it("sets, replaces and removes one colour immutably", () => {
    const a = withColorOverride(EMPTY_COLOR_OVERRIDES, "dark", "syn-keyword", "#ff0000");
    expect(a).toEqual({ dark: { "syn-keyword": "#ff0000" } });
    expect(EMPTY_COLOR_OVERRIDES).toEqual({});
    const b = withColorOverride(a, "dark", "syn-number", "#00ff00");
    expect(b).toEqual({ dark: { "syn-keyword": "#ff0000", "syn-number": "#00ff00" } });
    expect(a).toEqual({ dark: { "syn-keyword": "#ff0000" } });
    const c = withColorOverride(b, "dark", "syn-keyword", null);
    expect(c).toEqual({ dark: { "syn-number": "#00ff00" } });
    // The last colour of a theme takes the theme's entry with it.
    expect(withColorOverride(c, "dark", "syn-number", null)).toEqual({});
  });

  it("returns the same object when nothing changes", () => {
    const a = withColorOverride(EMPTY_COLOR_OVERRIDES, "light", "syn-keyword", "#ff0000");
    expect(withColorOverride(a, "light", "syn-keyword", "#ff0000")).toBe(a);
    expect(withColorOverride(a, "light", "syn-number", null)).toBe(a);
    expect(withColorOverride(a, "dark", "syn-number", null)).toBe(a);
  });

  it("drops a whole theme's overrides", () => {
    const a = withColorOverride(
      withColorOverride(EMPTY_COLOR_OVERRIDES, "dark", "syn-keyword", "#ff0000"),
      "light",
      "syn-keyword",
      "#00ff00",
    );
    expect(withoutThemeOverrides(a, "dark")).toEqual({ light: { "syn-keyword": "#00ff00" } });
    expect(withoutThemeOverrides(a, "custom:x")).toBe(a);
  });
});

describe("row padding presets", () => {
  it("has Compact < Default < Comfortable, with the default at the default padding", () => {
    const [compact, normal, comfortable] = GRID_ROW_PADDING_PRESETS;
    expect(compact.padding).toBeLessThan(normal.padding);
    expect(normal.padding).toBeLessThan(comfortable.padding);
    expect(normal.padding).toBe(DEFAULT_GRID_ROW_PADDING);
  });
});

describe("the preferences hold them", () => {
  it("has the defaults that leave the app looking as it did", () => {
    expect(DEFAULT_PREFS).toMatchObject({
      editorFontFamily: "",
      editorLineHeight: 1.6,
      gridFontFamily: "",
      gridFontSize: 12,
      gridRowPadding: 3,
      sidebarFontSize: 11,
      gridNoElastic: false,
      gridAutoExpandText: false,
    });
    expect(DEFAULT_PREFS.colorOverrides).toEqual({});
  });

  it("accepts valid values and ignores out-of-range or ill-typed ones", () => {
    const ok = sanitizePrefs({
      editorFontFamily: "JetBrains Mono",
      editorLineHeight: 1.25,
      gridFontFamily: "Menlo",
      gridFontSize: 14,
      gridLineHeight: 1.5,
      gridRowPadding: 0,
      sidebarFontSize: 13,
      gridNoElastic: true,
      gridAutoExpandText: true,
      colorOverrides: { dark: { "syn-comment": "#778899" } },
    });
    expect(ok).toMatchObject({
      editorFontFamily: "JetBrains Mono",
      editorLineHeight: 1.25,
      gridFontSize: 14,
      gridRowPadding: 0,
      sidebarFontSize: 13,
      gridNoElastic: true,
      gridAutoExpandText: true,
    });
    expect(ok.colorOverrides).toEqual({ dark: { "syn-comment": "#778899" } });

    const bad = sanitizePrefs({
      editorFontFamily: 'x"',
      editorLineHeight: 3,
      gridFontSize: 40,
      gridLineHeight: 0.5,
      gridRowPadding: 99,
      sidebarFontSize: 9,
      gridNoElastic: "yes",
      colorOverrides: { dark: { "syn-comment": "grey" } },
    });
    expect(bad).toEqual(DEFAULT_PREFS);
  });
});
