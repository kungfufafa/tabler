import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { DEFAULT_PREFS, type Prefs } from "@/stores/prefs";
import {
  applyAppearanceStyle,
  computeAppearanceStyle,
  type AppearancePrefs,
} from "./appearanceVars";

const prefs = (over: Partial<AppearancePrefs> = {}): AppearancePrefs => ({
  editorFontFamily: DEFAULT_PREFS.editorFontFamily,
  editorLineHeight: DEFAULT_PREFS.editorLineHeight,
  gridFontFamily: DEFAULT_PREFS.gridFontFamily,
  gridFontSize: DEFAULT_PREFS.gridFontSize,
  gridLineHeight: DEFAULT_PREFS.gridLineHeight,
  gridRowPadding: DEFAULT_PREFS.gridRowPadding,
  sidebarFontSize: DEFAULT_PREFS.sidebarFontSize,
  sidebarFontFamily: DEFAULT_PREFS.sidebarFontFamily,
  colorOverrides: DEFAULT_PREFS.colorOverrides,
  gridNoElastic: DEFAULT_PREFS.gridNoElastic,
  ...over,
});

describe("computeAppearanceStyle", () => {
  it("sets nothing for the defaults, and a 22px row", () => {
    expect(computeAppearanceStyle(prefs(), "light")).toEqual({
      vars: {},
      attrs: {},
      rowHeight: 22,
    });
  });

  it("maps the editor font and line height to custom properties", () => {
    const { vars } = computeAppearanceStyle(
      prefs({ editorFontFamily: "JetBrains Mono, Menlo", editorLineHeight: 1.9 }),
      "light",
    );
    expect(vars["--editor-font-family"]).toBe(
      '"JetBrains Mono", "Menlo", ui-monospace, Menlo, Monaco, monospace',
    );
    expect(vars["--editor-line-height"]).toBe("1.9");
  });

  it("maps the data table font, size and line height, and derives the row height", () => {
    const s = computeAppearanceStyle(
      prefs({ gridFontFamily: "Menlo", gridFontSize: 14, gridLineHeight: 1.5, gridRowPadding: 5 }),
      "dark",
    );
    expect(s.vars["--grid-font-family"]).toBe(
      '"Menlo", -apple-system, BlinkMacSystemFont, system-ui, sans-serif',
    );
    expect(s.vars["--grid-font-size"]).toBe("14px");
    expect(s.vars["--grid-line-height"]).toBe("21px");
    // 21px line + 5px above and below.
    expect(s.rowHeight).toBe(31);
  });

  it("changes the row height with the padding alone", () => {
    expect(computeAppearanceStyle(prefs({ gridRowPadding: 1 }), "light").rowHeight).toBe(18);
    expect(computeAppearanceStyle(prefs({ gridRowPadding: 6 }), "light").rowHeight).toBe(28);
    expect(computeAppearanceStyle(prefs({ gridRowPadding: 0 }), "light").rowHeight).toBe(16);
  });

  it("only sets the sidebar font when it differs from the default", () => {
    expect(computeAppearanceStyle(prefs({ sidebarFontSize: 11 }), "light").attrs).toEqual({});
    const s = computeAppearanceStyle(prefs({ sidebarFontSize: 13 }), "light");
    expect(s.vars["--sidebar-font-size"]).toBe("13px");
    expect(s.attrs["data-sidebar-font"]).toBe("");
  });

  it("emits --sidebar-font-family for a sidebar font, nothing for the default", () => {
    expect(computeAppearanceStyle(prefs({ sidebarFontFamily: "" }), "light").vars).toEqual({});
    const s = computeAppearanceStyle(prefs({ sidebarFontFamily: "Menlo" }), "light");
    expect(s.vars["--sidebar-font-family"]).toBe(
      '"Menlo", -apple-system, BlinkMacSystemFont, system-ui, sans-serif',
    );
    // The size attribute belongs to the size alone.
    expect(s.attrs).toEqual({});
    const list = computeAppearanceStyle(
      prefs({ sidebarFontFamily: "Helvetica Neue, Arial" }),
      "light",
    );
    expect(list.vars["--sidebar-font-family"]).toMatch(/^"Helvetica Neue", "Arial", /);
  });

  it("never lets a sidebar font that is not a font name reach the stylesheet", () => {
    const s = computeAppearanceStyle(prefs({ sidebarFontFamily: 'Menlo"; x: y' }), "light");
    expect(s.vars).toEqual({});
  });

  it("flags Disable scroll elastically", () => {
    expect(computeAppearanceStyle(prefs({ gridNoElastic: true }), "light").attrs).toEqual({
      "data-grid-no-elastic": "",
    });
  });

  it("applies the colour overrides of the theme in effect only", () => {
    const colorOverrides = {
      dark: { "syn-comment": "#778899", "change-new": "#00ff0040" },
      light: { "syn-comment": "#112233" },
    } as Prefs["colorOverrides"];
    const dark = computeAppearanceStyle(prefs({ colorOverrides }), "dark").vars;
    expect(dark).toMatchObject({ "--syn-comment": "#778899", "--change-new": "#00ff0040" });
    expect(computeAppearanceStyle(prefs({ colorOverrides }), "light").vars).toEqual({
      "--syn-comment": "#112233",
    });
    expect(computeAppearanceStyle(prefs({ colorOverrides }), "solarized-dark").vars).toEqual({});
  });

  it("ignores an override that is not a colour, so it cannot reach the stylesheet", () => {
    const colorOverrides = {
      dark: { "syn-comment": "red; background: url(x)" },
    } as unknown as Prefs["colorOverrides"];
    expect(computeAppearanceStyle(prefs({ colorOverrides }), "dark").vars).toEqual({});
  });

  it("names the variables the editor and grid read", () => {
    const all = {
      "syn-comment": "#000001",
      "syn-number": "#000002",
      "syn-string": "#000003",
      "syn-string-double": "#000004",
      "syn-string-backtick": "#000005",
      "syn-string-bracket": "#000006",
      "syn-keyword": "#000007",
      "editor-statement-bg": "#00000808",
      "editor-selection-bg": "#00000909",
      "syn-error": "#00000a",
      "syn-other": "#00000b",
      "syn-invisible": "#00000c",
      "change-deleted": "#00000d",
      "change-modified": "#00000e",
      "change-new": "#00000f",
      "grid-date": "#000010",
    };
    const { vars } = computeAppearanceStyle(
      prefs({ colorOverrides: { light: all } as Prefs["colorOverrides"] }),
      "light",
    );
    expect(Object.keys(vars).sort()).toEqual(
      Object.keys(all)
        .map((k) => `--${k}`)
        .sort(),
    );
  });
});

describe("applyAppearanceStyle (jsdom)", () => {
  let root: HTMLElement;
  beforeEach(() => {
    root = document.createElement("html");
  });
  afterEach(() => root.remove());

  it("sets the properties and attributes on the element", () => {
    applyAppearanceStyle(
      computeAppearanceStyle(
        prefs({ gridFontSize: 15, sidebarFontSize: 14, gridNoElastic: true, editorLineHeight: 2 }),
        "light",
      ),
      root,
    );
    expect(root.style.getPropertyValue("--grid-font-size")).toBe("15px");
    expect(root.style.getPropertyValue("--editor-line-height")).toBe("2");
    expect(root.style.getPropertyValue("--sidebar-font-size")).toBe("14px");
    expect(root).toHaveAttribute("data-sidebar-font");
    expect(root).toHaveAttribute("data-grid-no-elastic");
  });

  it("sets and clears the sidebar font family", () => {
    applyAppearanceStyle(
      computeAppearanceStyle(prefs({ sidebarFontFamily: "Menlo" }), "light"),
      root,
    );
    expect(root.style.getPropertyValue("--sidebar-font-family")).toContain('"Menlo"');
    applyAppearanceStyle(computeAppearanceStyle(prefs(), "light"), root);
    expect(root.style.getPropertyValue("--sidebar-font-family")).toBe("");
  });

  it("removes what an earlier apply set and the new one does not", () => {
    applyAppearanceStyle(
      computeAppearanceStyle(
        prefs({
          gridFontSize: 15,
          gridNoElastic: true,
          colorOverrides: { light: { "change-new": "#00ff00" } } as Prefs["colorOverrides"],
        }),
        "light",
      ),
      root,
    );
    expect(root.style.getPropertyValue("--change-new")).toBe("#00ff00");
    applyAppearanceStyle(computeAppearanceStyle(prefs(), "light"), root);
    expect(root.style.getPropertyValue("--grid-font-size")).toBe("");
    expect(root.style.getPropertyValue("--change-new")).toBe("");
    expect(root).not.toHaveAttribute("data-grid-no-elastic");
  });

  it("leaves other inline styles alone (the accent colour, the interface text size)", () => {
    root.style.setProperty("--brand", "#ff0000");
    root.style.fontSize = "108%";
    applyAppearanceStyle(computeAppearanceStyle(prefs({ gridFontSize: 15 }), "light"), root);
    applyAppearanceStyle(computeAppearanceStyle(prefs(), "light"), root);
    expect(root.style.getPropertyValue("--brand")).toBe("#ff0000");
    expect(root.style.fontSize).toBe("108%");
  });
});
