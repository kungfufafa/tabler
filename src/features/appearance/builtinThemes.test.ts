import { describe, expect, it } from "vitest";
import { BUILTIN_THEMES, baseTokens, builtinTheme } from "./builtinThemes";
import { colorDistance, composite, contrastRatio, parseColor, type Rgba } from "./color";
import { readIndexCss, resolveVars, themeDecls, topLevelRules, type ThemeCss } from "./cssTokens";
import { THEME_TOKEN_NAMES, THEME_TOKENS, tokenVar, type ThemeToken } from "./tokens";

const rules = topLevelRules(readIndexCss());

describe("token list", () => {
  it("has no duplicates and a CSS variable per token", () => {
    expect(new Set(THEME_TOKEN_NAMES).size).toBe(THEME_TOKENS.length);
    expect(tokenVar("syn-string-double")).toBe("--syn-string-double");
  });

  it("is complete in every built-in theme, with valid colours", () => {
    for (const theme of BUILTIN_THEMES) {
      expect(Object.keys(theme.tokens).sort(), theme.id).toEqual([...THEME_TOKEN_NAMES].sort());
      for (const name of THEME_TOKEN_NAMES)
        expect(parseColor(theme.tokens[name]), `${theme.id}.${name}`).not.toBeNull();
    }
  });

  it("looks themes up by id and gives custom themes the Light / Dark fallback", () => {
    expect(builtinTheme("solarized-dark")?.base).toBe("dark");
    expect(builtinTheme("nope")).toBeUndefined();
    expect(baseTokens("light")).toBe(builtinTheme("light")?.tokens);
    expect(baseTokens("dark")).toBe(builtinTheme("dark")?.tokens);
  });
});

describe("index.css declares the built-in themes", () => {
  it.each(BUILTIN_THEMES.map((t) => [t.id] as const))("%s matches builtinThemes.ts", (id) => {
    const decls = themeDecls(rules, id as ThemeCss);
    const theme = builtinTheme(id)!;
    for (const name of THEME_TOKEN_NAMES) {
      const raw = decls[tokenVar(name)];
      expect(raw, `${id}: ${tokenVar(name)} is not declared`).toBeDefined();
      // Tints that follow the connection's accent colour are declared with color-mix(); the data
      // value is their rendering for the default accent.
      if (raw!.includes("color-mix")) continue;
      const css = parseColor(resolveVars(raw!, decls));
      const data = parseColor(theme.tokens[name]);
      expect(css, `${id}: ${tokenVar(name)} = ${raw}`).not.toBeNull();
      expect(colorDistance(css!, data!), `${id}: ${name}`).toBeLessThan(0.5);
      expect(css!.a, `${id}: ${name} alpha`).toBeCloseTo(data!.a, 2);
    }
  });

  it("keeps the old change-marker names working as aliases of the new tokens", () => {
    const root = rules.get(":root")!;
    expect(root["--changed"]).toBe("var(--change-modified)");
    expect(root["--inserted"]).toBe("var(--change-new)");
    expect(root["--deleted"]).toBe("var(--change-deleted)");
  });

  it("lets a connection's own colour win over a theme's accent", () => {
    for (const id of ["solarized-light", "solarized-dark"]) {
      const plain = rules.get(`:root[data-theme-name="${id}"]`)!;
      const accent = rules.get(`:root[data-theme-name="${id}"]:not([data-accent])`)!;
      expect(plain["--brand"]).toBeUndefined();
      expect(plain["--primary-foreground"]).toBeUndefined();
      expect(accent["--brand"]).toBeDefined();
      expect(accent["--primary-foreground"]).toBeDefined();
    }
  });
});

// --- accessibility -------------------------------------------------------------

const AA = 4.5;
const rgba = (theme: ReturnType<typeof builtinTheme>, name: ThemeToken): Rgba =>
  parseColor(theme!.tokens[name])!;
const opaque = (c: Rgba): Rgba => ({ ...c, a: 1 });

/** The row backgrounds a cell can sit on: plain, zebra-striped, hovered. */
function rowBackgrounds(theme: ReturnType<typeof builtinTheme>): Record<string, Rgba> {
  const surface = opaque(rgba(theme, "surface"));
  const fg = rgba(theme, "foreground");
  return {
    surface,
    zebra: composite({ ...fg, a: 0.025 }, surface),
    hover: opaque(rgba(theme, "row-hover")),
  };
}

describe.each(BUILTIN_THEMES)("$label meets WCAG AA", (theme) => {
  it("body text on every surface (4.5:1)", () => {
    const fg = rgba(theme, "foreground");
    for (const name of [
      "background",
      "surface",
      "surface-2",
      "sidebar",
      "row-hover",
      "accent",
    ] as const)
      expect(
        contrastRatio(fg, opaque(rgba(theme, name))),
        `text on ${name}`,
      ).toBeGreaterThanOrEqual(AA);
    for (const [name, bg] of Object.entries(rowBackgrounds(theme)))
      expect(contrastRatio(fg, bg), `text on ${name} row`).toBeGreaterThanOrEqual(AA);
  });

  it("secondary text on the window, content and sidebar (4.5:1)", () => {
    const muted = rgba(theme, "muted-foreground");
    for (const name of ["background", "surface", "sidebar"] as const)
      expect(
        contrastRatio(muted, opaque(rgba(theme, name))),
        `secondary text on ${name}`,
      ).toBeGreaterThanOrEqual(AA);
  });

  it("text on each change marker, over every row background (4.5:1)", () => {
    const fg = opaque(rgba(theme, "foreground"));
    for (const [rowName, row] of Object.entries(rowBackgrounds(theme))) {
      for (const marker of ["change-new", "change-modified", "change-deleted"] as const) {
        const bg = composite(rgba(theme, marker), row);
        expect(contrastRatio(fg, bg), `${marker} on ${rowName}`).toBeGreaterThanOrEqual(AA);
      }
    }
  });

  it("text in a soft deleted row, which the grid draws at 70% opacity (3:1: it is de-emphasised)", () => {
    const fg = opaque(rgba(theme, "foreground"));
    for (const [rowName, row] of Object.entries(rowBackgrounds(theme))) {
      const bg = composite(rgba(theme, "change-deleted"), row);
      const faded = (c: Rgba) => composite({ ...c, a: 0.7 }, row);
      expect(contrastRatio(faded(fg), faded(bg)), `deleted on ${rowName}`).toBeGreaterThanOrEqual(
        3,
      );
    }
  });

  it("change markers stand out from the row and from each other", () => {
    const { surface } = rowBackgrounds(theme);
    const shown = (["change-new", "change-modified", "change-deleted"] as const).map((m) =>
      composite(rgba(theme, m), surface!),
    );
    for (const c of shown) expect(colorDistance(c, surface!)).toBeGreaterThanOrEqual(2.5);
    for (let i = 0; i < shown.length; i++)
      for (let j = i + 1; j < shown.length; j++)
        expect(colorDistance(shown[i]!, shown[j]!)).toBeGreaterThanOrEqual(3);
  });

  it("text on the accent colour (3:1: it labels buttons)", () => {
    expect(
      contrastRatio(rgba(theme, "primary-foreground"), opaque(rgba(theme, "brand"))),
    ).toBeGreaterThanOrEqual(3);
  });

  it("SQL syntax colours on the editor background (3:1)", () => {
    const surface = opaque(rgba(theme, "surface"));
    for (const name of [
      "syn-comment",
      "syn-number",
      "syn-string",
      "syn-string-double",
      "syn-string-backtick",
      "syn-string-bracket",
      "syn-keyword",
      "syn-error",
      "syn-other",
    ] as const)
      expect(contrastRatio(rgba(theme, name), surface), name).toBeGreaterThanOrEqual(3);
  });

  it("the date colour reads like body text (4.5:1)", () => {
    expect(
      contrastRatio(rgba(theme, "grid-date"), opaque(rgba(theme, "surface"))),
    ).toBeGreaterThanOrEqual(AA);
  });
});
