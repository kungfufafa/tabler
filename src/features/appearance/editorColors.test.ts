/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { sqlHighlightStyle } from "@/features/editor/cmTheme";
import { DEFAULT_PREFS } from "@/stores/prefs";
import { withColorOverride } from "./appearancePrefs";
import { computeAppearanceStyle } from "./appearanceVars";
import { EDITOR_COLOR_TOKENS, THEME_TOKENS, tokenVar } from "./tokens";

const editorDir = path.resolve(import.meta.dirname, "../editor");
const cmTheme = readFileSync(path.join(editorDir, "cmTheme.ts"), "utf8");
const cmExtensions = readFileSync(path.join(editorDir, "cmExtensions.ts"), "utf8");

/** The `--custom-properties` the editor's CodeMirror theme and highlight style read. */
const editorSource = `${cmTheme}\n${cmExtensions}`;

describe("SQL editor colours (Settings ▸ Appearance ▸ SQL editor)", () => {
  it("lists the colours TablePlus's Fonts & Colors has, in its words", () => {
    const labels = EDITOR_COLOR_TOKENS.map((t) => THEME_TOKENS.find((d) => d.name === t)?.label);
    expect(labels).toEqual([
      "Comment",
      "Number",
      "Single quote string",
      "Double quote string",
      "Backtick quote string",
      "Bracket quote string",
      "Reserved keyword",
      "Current query",
      "Selected background",
      "Error highlight",
      "Others",
      "Invisible characters",
    ]);
  });

  it.each(EDITOR_COLOR_TOKENS)("the editor reads %s", (token) => {
    expect(editorSource).toContain(`var(${tokenVar(token)}`);
  });

  it("colours the highlight style from the syntax tokens", () => {
    const colours = new Set<string>();
    for (const spec of sqlHighlightStyle.specs) {
      const c = (spec as { color?: string }).color;
      if (c) colours.add(c);
    }
    expect(colours).toContain("var(--syn-keyword)");
    expect(colours).toContain("var(--syn-string)");
    expect(colours).toContain("var(--syn-number)");
    expect(colours).toContain("var(--syn-comment)");
    expect(colours).toContain("var(--syn-error)");
    // Everything the highlight style uses is a token the settings can override, or a stylesheet default.
    for (const c of colours) {
      const name = /^var\(--([\w-]+)\)$/.exec(c)?.[1];
      expect(name, c).toBeDefined();
      expect([...EDITOR_COLOR_TOKENS, "syn-ident", "syn-operator", "muted-foreground"]).toContain(
        name,
      );
    }
  });

  it("maps each override to the custom property the editor reads, per theme", () => {
    let overrides = DEFAULT_PREFS.colorOverrides;
    EDITOR_COLOR_TOKENS.forEach((token, i) => {
      overrides = withColorOverride(overrides, "dark", token, `#0000${(i + 16).toString(16)}`);
    });
    const prefs = { ...DEFAULT_PREFS, colorOverrides: overrides };
    const dark = computeAppearanceStyle(prefs, "dark").vars;
    for (const [i, token] of EDITOR_COLOR_TOKENS.entries())
      expect(dark[tokenVar(token)], token).toBe(`#0000${(i + 16).toString(16)}`);
    // Another theme is untouched: "Reset to theme default" is just removing the override.
    expect(computeAppearanceStyle(prefs, "light").vars).toEqual({});
    const reset = withColorOverride(overrides, "dark", "syn-keyword", null);
    expect(
      computeAppearanceStyle({ ...prefs, colorOverrides: reset }, "dark").vars["--syn-keyword"],
    ).toBeUndefined();
  });

  it("uses the font and line height variables, falling back to the old values", () => {
    expect(cmTheme).toContain("var(--editor-font-family, var(--font-mono))");
    expect(cmTheme).toContain("var(--editor-line-height, 1.6)");
  });
});
