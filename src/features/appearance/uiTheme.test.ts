import { afterEach, describe, expect, it } from "vitest";
import { themeFromStorage } from "@/features/settings/crossWindow";
import { BUILTIN_THEME_PREFS, isThemePref, useUiStore } from "@/stores/ui";

afterEach(() => {
  localStorage.clear();
  useUiStore.setState({ themePref: "system" });
});

describe("isThemePref", () => {
  it("accepts Auto, the built-in themes and custom theme ids", () => {
    for (const ok of [...BUILTIN_THEME_PREFS, "custom:t1abc", "custom:a_b-C9"])
      expect(isThemePref(ok), ok).toBe(true);
  });

  it.each([
    "",
    "auto",
    "Dark",
    "custom:",
    "custom:a b",
    "custom:a/b",
    "custom:" + "x".repeat(65),
    "solarized",
    null,
    undefined,
    3,
    { x: 1 },
  ])("rejects %j", (bad) => expect(isThemePref(bad)).toBe(false));
});

describe("the theme choice is persisted", () => {
  it("keeps any theme choice, built-in or custom", () => {
    useUiStore.getState().setThemePref("solarized-dark");
    expect(JSON.parse(localStorage.getItem("tabler.ui")!).state.themePref).toBe("solarized-dark");
    useUiStore.getState().setThemePref("custom:t1abc");
    expect(JSON.parse(localStorage.getItem("tabler.ui")!).state.themePref).toBe("custom:t1abc");
  });

  it("restores a stored choice, and falls back to Auto for one that is not valid", async () => {
    const restore = async (stored: unknown) => {
      // A fresh start: the store holds its default (Auto) when it rehydrates.
      useUiStore.setState({ themePref: "system" });
      localStorage.setItem(
        "tabler.ui",
        JSON.stringify({ state: { themePref: stored }, version: 1 }),
      );
      await useUiStore.persist.rehydrate();
      return useUiStore.getState().themePref;
    };
    expect(await restore("solarized-light")).toBe("solarized-light");
    expect(await restore("custom:t1abc")).toBe("custom:t1abc");
    expect(await restore("neon")).toBe("system");
    expect(await restore(42)).toBe("system");
  });

  it("tells another window's choice apart from junk (the storage event)", () => {
    const stored = (t: unknown) => JSON.stringify({ state: { themePref: t } });
    expect(themeFromStorage(stored("solarized-dark"))).toBe("solarized-dark");
    expect(themeFromStorage(stored("custom:tnight"))).toBe("custom:tnight");
    expect(themeFromStorage(stored("neon"))).toBeNull();
    expect(themeFromStorage("{nope")).toBeNull();
  });
});
