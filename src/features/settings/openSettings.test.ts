import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/lib/ipc/mock";
import { mockSettingsOpens, resetMockSettingsWindow } from "@/lib/ipc/mock/windows";
import { isSettingsWindow, setSettingsOwnWindowForTests, windowView } from "@/lib/windowKind";
import { isSettingsSection, openSettings, useSettingsDialog } from "./store";

const setUrl = (search: string) => window.history.replaceState(null, "", `/${search}`);
const tick = () => new Promise((r) => setTimeout(r, 0));

describe("openSettings", () => {
  beforeEach(() => {
    resetMockSettingsWindow();
    useSettingsDialog.setState({ open: false, section: "general", query: "" });
    setUrl("");
  });
  afterEach(() => {
    setSettingsOwnWindowForTests(null);
    setUrl("");
    vi.restoreAllMocks();
  });

  it("opens the Settings window in the app, like TablePlus, not the dialog", async () => {
    setSettingsOwnWindowForTests(true);
    openSettings("shortcuts");
    openSettings();
    await tick();
    expect(mockSettingsOpens).toEqual(["shortcuts", null]);
    expect(useSettingsDialog.getState().open).toBe(false);
  });

  it("falls back to the dialog when the window cannot be opened", async () => {
    setSettingsOwnWindowForTests(true);
    const { ipc } = await import("@/lib/ipc");
    vi.spyOn(ipc.windows, "openSettings").mockRejectedValueOnce(new Error("no webview"));
    vi.spyOn(console, "warn").mockImplementation(() => undefined);
    openSettings("security");
    await tick();
    expect(useSettingsDialog.getState()).toMatchObject({ open: true, section: "security" });
  });

  it("keeps the in-window dialog in the browser mock", async () => {
    setSettingsOwnWindowForTests(false);
    openSettings("mcp");
    await tick();
    expect(mockSettingsOpens).toEqual([]);
    expect(useSettingsDialog.getState()).toMatchObject({ open: true, section: "mcp" });
    // No section: the one viewed last.
    useSettingsDialog.setState({ open: false });
    openSettings();
    expect(useSettingsDialog.getState()).toMatchObject({ open: true, section: "mcp" });
  });

  it("only switches the section inside the Settings window", async () => {
    setSettingsOwnWindowForTests(true);
    setUrl("?view=settings");
    expect(windowView()).toBe("settings");
    expect(isSettingsWindow()).toBe(true);
    openSettings("appearance");
    await tick();
    expect(mockSettingsOpens).toEqual([]);
    expect(useSettingsDialog.getState()).toMatchObject({ open: false, section: "appearance" });
  });

  it("tells the Settings window by its label", () => {
    const w = window as unknown as Record<string, unknown>;
    expect(windowView()).toBe("app");
    w.__TAURI_INTERNALS__ = { metadata: { currentWindow: { label: "settings" } } };
    try {
      expect(windowView()).toBe("settings");
      w.__TAURI_INTERNALS__ = { metadata: { currentWindow: { label: "w-2" } } };
      expect(windowView()).toBe("app");
    } finally {
      delete w.__TAURI_INTERNALS__;
    }
  });

  it("knows the section ids", () => {
    expect(isSettingsSection("security")).toBe(true);
    expect(isSettingsSection("nope")).toBe(false);
    expect(isSettingsSection(null)).toBe(false);
  });
});
