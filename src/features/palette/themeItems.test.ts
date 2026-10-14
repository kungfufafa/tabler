import { describe, expect, it, vi } from "vitest";
import type { CustomTheme } from "@/features/appearance/customTheme";
import { themeCommandItems } from "./themeItems";

const midnight: CustomTheme = { id: "mid1", name: "Midnight", base: "dark", tokens: {} };
const paper: CustomTheme = { id: "pap2", name: "Paper", base: "light", tokens: {} };

describe("themeCommandItems", () => {
  it("lists Auto, Light, Dark, both Solarized themes, then every custom theme by name", () => {
    const items = themeCommandItems({
      themePref: "system",
      customThemes: [midnight, paper],
      setTheme: vi.fn(),
    });
    expect(items.map((i) => [i.key, i.label])).toEqual([
      ["cmd:theme.system", "Theme: Auto"],
      ["cmd:theme.light", "Theme: Light"],
      ["cmd:theme.dark", "Theme: Dark"],
      ["cmd:theme.solarized-light", "Theme: Solarized Light"],
      ["cmd:theme.solarized-dark", "Theme: Solarized Dark"],
      ["cmd:theme.custom:mid1", "Theme: Midnight"],
      ["cmd:theme.custom:pap2", "Theme: Paper"],
    ]);
    expect(new Set(items.map((i) => i.key)).size).toBe(items.length);
  });

  it("lists the built-in themes alone without custom themes", () => {
    const items = themeCommandItems({ themePref: "dark", setTheme: vi.fn() });
    expect(items).toHaveLength(5);
  });

  it("applies the id of the theme: the built-in name or custom:<id>", () => {
    const setTheme = vi.fn();
    const items = themeCommandItems({ themePref: "system", customThemes: [midnight], setTheme });
    const run = (key: string) => items.find((i) => i.key === key)!.run();
    run("cmd:theme.system");
    run("cmd:theme.solarized-dark");
    run("cmd:theme.custom:mid1");
    expect(setTheme.mock.calls).toEqual([["system"], ["solarized-dark"], ["custom:mid1"]]);
  });

  it("marks exactly the theme in use", () => {
    const current = (pref: Parameters<typeof themeCommandItems>[0]["themePref"]) =>
      themeCommandItems({ themePref: pref, customThemes: [midnight], setTheme: vi.fn() })
        .filter((i) => i.current)
        .map((i) => i.key);
    expect(current("system")).toEqual(["cmd:theme.system"]);
    expect(current("solarized-light")).toEqual(["cmd:theme.solarized-light"]);
    expect(current("custom:mid1")).toEqual(["cmd:theme.custom:mid1"]);
  });

  it("marks Auto when the custom theme in use is gone (what the window falls back to)", () => {
    const items = themeCommandItems({
      themePref: "custom:deleted",
      customThemes: [midnight],
      setTheme: vi.fn(),
    });
    expect(items.filter((i) => i.current).map((i) => i.key)).toEqual(["cmd:theme.system"]);
  });

  it("finds a dark theme by 'dark mode' and Auto by 'system'", () => {
    const items = themeCommandItems({
      themePref: "light",
      customThemes: [midnight, paper],
      setTheme: vi.fn(),
    });
    const words = (key: string) => items.find((i) => i.key === key)!.keywords!.join(" ");
    expect(words("cmd:theme.dark")).toContain("dark mode");
    expect(words("cmd:theme.solarized-dark")).toContain("dark mode");
    expect(words("cmd:theme.custom:mid1")).toContain("dark mode");
    expect(words("cmd:theme.custom:pap2")).not.toContain("dark mode");
    expect(words("cmd:theme.system")).toContain("system");
  });
});
