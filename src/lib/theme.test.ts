import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { applyTheme, nativeThemeFor, resolveTheme, setNativeTheme } from "./theme";

const native = vi.hoisted(() => ({
  setAppTheme: vi.fn<(theme?: string | null) => Promise<void>>(),
  setWindowTheme: vi.fn<(theme?: string | null) => Promise<void>>(),
}));
vi.mock("@tauri-apps/api/app", () => ({ setTheme: native.setAppTheme }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ setTheme: native.setWindowTheme }),
}));

/** Pretends to run inside the Tauri shell (the repo's `isTauri` looks for this global). */
function inTauri(on: boolean) {
  const w = window as unknown as Record<string, unknown>;
  if (on) w.__TAURI_INTERNALS__ = {};
  else delete w.__TAURI_INTERNALS__;
}

describe("resolveTheme", () => {
  it("follows the system preference when pref is system", () => {
    expect(resolveTheme("system", true)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
  });
  it("ignores the system preference for explicit prefs", () => {
    expect(resolveTheme("light", true)).toBe("light");
    expect(resolveTheme("dark", false)).toBe("dark");
  });
});

describe("resolveTheme with the extra themes", () => {
  it("gives Solarized Light / Dark their own base, whatever the system says", () => {
    expect(resolveTheme("solarized-light", true)).toBe("light");
    expect(resolveTheme("solarized-dark", false)).toBe("dark");
  });
  it("asks for a custom theme's base, and follows the system when it is unknown", () => {
    const bases: Record<string, "light" | "dark"> = { tnight: "dark", tday: "light" };
    expect(resolveTheme("custom:tnight", false, (id) => bases[id])).toBe("dark");
    expect(resolveTheme("custom:tday", true, (id) => bases[id])).toBe("light");
    expect(resolveTheme("custom:tgone", true, (id) => bases[id])).toBe("dark");
    expect(resolveTheme("custom:tgone", false)).toBe("light");
  });
});

describe("applyTheme", () => {
  it("sets the dark class, data-theme and colour-scheme", () => {
    applyTheme("dark");
    const root = document.documentElement;
    expect(root.classList.contains("dark")).toBe(true);
    expect(root.dataset.theme).toBe("dark");
    expect(root.style.colorScheme).toBe("dark");
    applyTheme("light");
    expect(root.classList.contains("dark")).toBe(false);
    expect(root.dataset.theme).toBe("light");
  });
});

describe("nativeThemeFor (the native app appearance a theme choice asks for)", () => {
  it("hands control back to the system for Auto", () => {
    expect(nativeThemeFor("system")).toBeNull();
  });
  it("forces light / dark for the explicit choices and the Solarized pair", () => {
    expect(nativeThemeFor("light")).toBe("light");
    expect(nativeThemeFor("dark")).toBe("dark");
    expect(nativeThemeFor("solarized-light")).toBe("light");
    expect(nativeThemeFor("solarized-dark")).toBe("dark");
  });
  it("uses a custom theme's own base, and follows the system when it is gone", () => {
    const bases: Record<string, "light" | "dark"> = { tnight: "dark", tday: "light" };
    expect(nativeThemeFor("custom:tnight", (id) => bases[id])).toBe("dark");
    expect(nativeThemeFor("custom:tday", (id) => bases[id])).toBe("light");
    expect(nativeThemeFor("custom:tgone", (id) => bases[id])).toBeNull();
    expect(nativeThemeFor("custom:tgone")).toBeNull();
  });
});

describe("setNativeTheme", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    native.setAppTheme.mockResolvedValue(undefined);
    native.setWindowTheme.mockResolvedValue(undefined);
    inTauri(true);
  });
  afterEach(() => inTauri(false));

  it("sets the app-wide theme: dark, light and null (system)", async () => {
    await setNativeTheme("dark");
    expect(native.setAppTheme).toHaveBeenLastCalledWith("dark");
    await setNativeTheme("light");
    expect(native.setAppTheme).toHaveBeenLastCalledWith("light");
    await setNativeTheme(null);
    expect(native.setAppTheme).toHaveBeenLastCalledWith(null);
    expect(native.setAppTheme).toHaveBeenCalledTimes(3);
    expect(native.setWindowTheme).not.toHaveBeenCalled();
  });

  it("does nothing in a plain browser (the mock transport)", async () => {
    inTauri(false);
    await setNativeTheme("dark");
    expect(native.setAppTheme).not.toHaveBeenCalled();
  });

  it("falls back to this window's theme when the app-wide call fails", async () => {
    native.setAppTheme.mockRejectedValue(new Error("not allowed"));
    await expect(setNativeTheme("dark")).resolves.toBeUndefined();
    expect(native.setWindowTheme).toHaveBeenCalledWith("dark");
  });

  it("never throws, even when both calls fail", async () => {
    native.setAppTheme.mockRejectedValue(new Error("not allowed"));
    native.setWindowTheme.mockRejectedValue(new Error("not allowed"));
    await expect(setNativeTheme("light")).resolves.toBeUndefined();
    await expect(setNativeTheme(null)).resolves.toBeUndefined();
  });

  it("applies rapid changes in order, the last one wins", async () => {
    const calls: Array<string | null | undefined> = [];
    native.setAppTheme.mockImplementation(async (t) => {
      await new Promise((r) => setTimeout(r, t === "dark" ? 20 : 0));
      calls.push(t);
    });
    await Promise.all([setNativeTheme("dark"), setNativeTheme("light"), setNativeTheme(null)]);
    expect(calls).toEqual(["dark", "light", null]);
  });
});
