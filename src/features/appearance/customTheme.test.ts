import { describe, expect, it } from "vitest";
import { BUILTIN_THEMES, builtinTheme } from "./builtinThemes";
import {
  checkTokens,
  cleanThemeName,
  MAX_CUSTOM_THEMES,
  MAX_THEME_FILE_BYTES,
  MAX_THEME_NAME,
  newThemeId,
  parseThemeFile,
  sanitizeThemes,
  serializeTheme,
  THEME_FILE_FORMAT,
  uniqueThemeName,
  type CustomTheme,
} from "./customTheme";
import { THEME_TOKEN_NAMES } from "./tokens";

const file = (over: Record<string, unknown> = {}) =>
  JSON.stringify({
    format: THEME_FILE_FORMAT,
    version: 1,
    name: "Night",
    base: "dark",
    tokens: { background: "#101418", foreground: "oklch(0.9 0.01 250)" },
    ...over,
  });

describe("parseThemeFile", () => {
  it("accepts a valid file, with only some tokens", () => {
    const r = parseThemeFile(file());
    expect(r).toEqual({
      ok: true,
      theme: {
        name: "Night",
        base: "dark",
        tokens: { background: "#101418", foreground: "oklch(0.9 0.01 250)" },
      },
    });
  });

  it("accepts a theme with every token", () => {
    const all = BUILTIN_THEMES[0]!.tokens;
    const r = parseThemeFile(file({ tokens: all }));
    expect(r.ok).toBe(true);
    if (r.ok) expect(Object.keys(r.theme.tokens)).toHaveLength(THEME_TOKEN_NAMES.length);
  });

  it.each([
    ["not JSON", "{nope", /not valid JSON/],
    ["an array", "[]", /not a theme file/],
    ["null", "null", /not a theme file/],
    ["another format", file({ format: "something" }), /not a Tabler theme/],
    ["a newer version", file({ version: 2 }), /newer version/],
    ["no version", file({ version: undefined }), /newer version/],
    ["an empty name", file({ name: "   " }), /needs a name/],
    ["a long name", file({ name: "x".repeat(MAX_THEME_NAME + 1) }), /needs a name/],
    ["a numeric name", file({ name: 5 }), /needs a name/],
    ["a bad base", file({ base: "sepia" }), /base must be/],
    ["tokens that are not an object", file({ tokens: [] }), /must be an object/],
    ["no tokens field", file({ tokens: undefined }), /must be an object/],
    ["an unknown token", file({ tokens: { nonsense: "#fff" } }), /Unknown colour “nonsense”/],
    [
      "a prototype key",
      `{"format":"tabler-theme","version":1,"name":"a","base":"light","tokens":{"__proto__":"#fff"}}`,
      /Unknown colour/,
    ],
    ["a non-colour value", file({ tokens: { background: "red" } }), /“background” is not a colour/],
    ["a number as a colour", file({ tokens: { background: 12 } }), /not a colour/],
    [
      "css injection in a colour",
      file({ tokens: { background: "#fff; } body { display:none" } }),
      /not a colour/,
    ],
  ])("rejects %s", (_, text, message) => {
    const r = parseThemeFile(text);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error).toMatch(message);
  });

  it("rejects a file over the size limit", () => {
    const big = file({ padding: "x".repeat(MAX_THEME_FILE_BYTES) });
    const r = parseThemeFile(big);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error).toMatch(/too large/);
  });

  it("measures the size in bytes, not characters", () => {
    // 11k three-byte characters is 33k bytes: over the limit though under it in characters.
    const r = parseThemeFile(file({ padding: "€".repeat(11_000) }));
    expect(r.ok).toBe(false);
  });

  it("ignores fields it does not know at the top level (a newer file can add some)", () => {
    expect(parseThemeFile(file({ author: "me" })).ok).toBe(true);
  });

  it("trims names and collapses their whitespace", () => {
    const r = parseThemeFile(file({ name: "  My   night\tthemé " }));
    expect(r.ok && r.theme.name).toBe("My night themé");
  });
});

describe("serializeTheme", () => {
  const theme: CustomTheme = {
    id: "tabc",
    name: "Dusk",
    base: "light",
    tokens: { foreground: "#222222", background: "#fafafa" },
  };

  it("round trips through parseThemeFile", () => {
    const text = serializeTheme(theme);
    const back = parseThemeFile(text);
    expect(back).toEqual({
      ok: true,
      theme: {
        name: "Dusk",
        base: "light",
        tokens: { background: "#fafafa", foreground: "#222222" },
      },
    });
  });

  it("round trips every built-in theme", () => {
    for (const t of BUILTIN_THEMES) {
      const back = parseThemeFile(
        serializeTheme({ name: t.label, base: t.base, tokens: t.tokens }),
      );
      expect(back, t.id).toEqual({
        ok: true,
        theme: { name: t.label, base: t.base, tokens: t.tokens },
      });
    }
  });

  it("writes the tokens in canonical order and ends with a newline", () => {
    const text = serializeTheme(theme);
    expect(text.endsWith("\n")).toBe(true);
    const keys = Object.keys((JSON.parse(text) as { tokens: object }).tokens);
    expect(keys).toEqual(["background", "foreground"]);
    expect((JSON.parse(text) as { format: string }).format).toBe("tabler-theme");
  });

  it("stays well under the size limit even with every token", () => {
    const light = builtinTheme("light")!;
    expect(serializeTheme({ name: "x", base: "light", tokens: light.tokens }).length).toBeLessThan(
      MAX_THEME_FILE_BYTES / 4,
    );
  });
});

describe("names and ids", () => {
  it("cleans names", () => {
    expect(cleanThemeName("  a  b ")).toBe("a b");
    expect(cleanThemeName("a\u0000b")).toBe("a b");
    expect(cleanThemeName("")).toBeNull();
    expect(cleanThemeName("x".repeat(49))).toBeNull();
    expect(cleanThemeName(3)).toBeNull();
  });

  it("makes names unique, case-insensitively", () => {
    expect(uniqueThemeName("Night", [])).toBe("Night");
    expect(uniqueThemeName("Night", ["night"])).toBe("Night 2");
    expect(uniqueThemeName("Night", ["Night", "Night 2"])).toBe("Night 3");
    expect(uniqueThemeName("Night 2", ["Night 2"])).toBe("Night 3");
    expect(
      uniqueThemeName("x".repeat(MAX_THEME_NAME), ["x".repeat(MAX_THEME_NAME)]).length,
    ).toBeLessThanOrEqual(MAX_THEME_NAME);
  });

  it("generates safe, distinct ids", () => {
    const a = newThemeId();
    expect(a).toMatch(/^[A-Za-z0-9_-]{1,64}$/);
    expect(newThemeId()).not.toBe(a);
  });
});

describe("checkTokens", () => {
  it("trims values", () => {
    expect(checkTokens({ background: "  #fff " })).toEqual({
      ok: true,
      tokens: { background: "#fff" },
    });
  });
  it("rejects non-objects", () => {
    expect(checkTokens("x").ok).toBe(false);
    expect(checkTokens(null).ok).toBe(false);
  });
});

describe("sanitizeThemes", () => {
  const good = { id: "t1", name: "A", base: "dark", tokens: { background: "#000" } };

  it("keeps valid themes and drops the rest", () => {
    const out = sanitizeThemes([
      good,
      { ...good, id: "t1" },
      { ...good, id: "t2", name: "" },
      { ...good, id: "bad id" },
      { ...good, id: "t3", base: "x" },
      { ...good, id: "t4", tokens: { nope: "#000" } },
      { ...good, id: "t5", tokens: { background: "blue" } },
      null,
      "x",
      { ...good, id: "t6", name: "B" },
    ]);
    expect(out.map((t) => t.id)).toEqual(["t1", "t6"]);
  });

  it("returns [] for anything that is not a list", () => {
    expect(sanitizeThemes(undefined)).toEqual([]);
    expect(sanitizeThemes({})).toEqual([]);
  });

  it("caps the list", () => {
    const many = Array.from({ length: MAX_CUSTOM_THEMES + 20 }, (_, i) => ({
      ...good,
      id: `t${i}`,
    }));
    expect(sanitizeThemes(many)).toHaveLength(MAX_CUSTOM_THEMES);
  });
});
