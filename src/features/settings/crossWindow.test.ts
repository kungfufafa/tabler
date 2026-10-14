import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { RENDERER_ID } from "@/lib/ipc/events";
import { mockEmit } from "@/lib/ipc/mock/transport";
import { getPrefs, resetPrefsForTests } from "@/stores/prefs";
import { useUiStore } from "@/stores/ui";
import { installCrossWindowSync, themeFromStorage, UI_STORAGE_KEY } from "./crossWindow";

let uninstall: () => void = () => {};

beforeEach(() => {
  localStorage.clear();
  resetPrefsForTests();
  useUiStore.setState({ themePref: "system" });
});
afterEach(() => {
  uninstall();
  resetPrefsForTests();
});

/** Messages reach the window once its (lazily loaded) listener is up: send until it applied. */
async function sendUntil(send: () => void, done: () => boolean) {
  for (let i = 0; i < 50 && !done(); i++) {
    send();
    await new Promise((r) => setTimeout(r, 5));
  }
  expect(done()).toBe(true);
}

describe("settings changed in another window", () => {
  it("applies the preferences it saved (the keyboard shortcuts included), not its own echo", async () => {
    uninstall = installCrossWindowSync();
    // Our own message coming back is not applied.
    mockEmit("prefs:changed", { from: RENDERER_ID, changed: { pageSize: 1000 } });
    await sendUntil(
      () =>
        mockEmit("prefs:changed", {
          from: "another-window",
          changed: { pageSize: 500, keymapOverrides: { "tab.new-query": ["Mod+Alt+T"] } },
        }),
      () => getPrefs().pageSize === 500,
    );
    expect(getPrefs().keymapOverrides).toEqual({ "tab.new-query": ["Mod+Alt+T"] });
    // Untouched by the other window: still the default (4 spaces, like TablePlus).
    expect(getPrefs().editorTabSize).toBe(4);
  });

  it("follows the theme another window picked (a storage event of the UI store)", () => {
    uninstall = installCrossWindowSync();
    const stored = (themePref: string) =>
      JSON.stringify({ state: { themePref, sidebarOpen: true }, version: 1 });
    const send = (key: string, newValue: string | null) =>
      window.dispatchEvent(new StorageEvent("storage", { key, newValue }));
    send(UI_STORAGE_KEY, stored("dark"));
    expect(useUiStore.getState().themePref).toBe("dark");
    // Other keys, invalid or empty values change nothing.
    send("tabler.prefs", stored("light"));
    send(UI_STORAGE_KEY, stored("neon"));
    send(UI_STORAGE_KEY, "{not json");
    send(UI_STORAGE_KEY, null);
    expect(useUiStore.getState().themePref).toBe("dark");
    expect(themeFromStorage(stored("light"))).toBe("light");
    expect(themeFromStorage(null)).toBeNull();
  });
});
