import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc } from "@/lib/ipc";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import {
  DEFAULT_PREFS,
  flushPrefs,
  getPrefs,
  loadPrefs,
  PREFS_KEY,
  PREFS_LOCAL_KEY,
  resetPrefsForTests,
  sanitizePrefs,
  SAVE_DEBOUNCE_MS,
  setPref,
  usePrefsStore,
} from "./prefs";
import { useTableTabsStore } from "./tableTabs";

describe("sanitizePrefs", () => {
  it("returns the defaults for missing or non-object data", () => {
    for (const raw of [undefined, null, "prefs", 42, [], true]) {
      expect(sanitizePrefs(raw)).toEqual(DEFAULT_PREFS);
    }
    // A fresh copy, never the frozen defaults themselves.
    expect(sanitizePrefs(null)).not.toBe(DEFAULT_PREFS);
  });

  it("merges partial stored data over the defaults and drops unknown keys", () => {
    const out = sanitizePrefs({ pageSize: 500, editorTabSize: 4, theme: "dark", extra: { a: 1 } });
    expect(out).toEqual({ ...DEFAULT_PREFS, pageSize: 500, editorTabSize: 4 });
    expect(out).not.toHaveProperty("theme");
    expect(out).not.toHaveProperty("extra");
  });

  it("falls back per key when a value is invalid", () => {
    const out = sanitizePrefs({
      uiFontSize: "huge",
      pageSize: 300,
      confirmCommit: "no",
      editorFontSize: 30,
      editorTabSize: 3,
      editorLineWrap: 1,
      editorUppercaseKeywords: null,
      connectTimeout: 2.5,
    });
    expect(out).toEqual(DEFAULT_PREFS);
    expect(sanitizePrefs({ editorFontSize: 10.5, connectTimeout: 0 })).toEqual(DEFAULT_PREFS);
    expect(sanitizePrefs({ editorFontSize: "14", pageSize: "500" })).toEqual(DEFAULT_PREFS);
  });

  it("keeps valid boundary values and uses the given base for invalid ones", () => {
    expect(sanitizePrefs({ editorFontSize: 11, connectTimeout: 300 })).toMatchObject({
      editorFontSize: 11,
      connectTimeout: 300,
    });
    const base = { ...DEFAULT_PREFS, pageSize: 1000 as const };
    expect(sanitizePrefs({ pageSize: 7, uiFontSize: "large" }, base)).toEqual({
      ...base,
      uiFontSize: "large",
    });
  });
});

describe("prefs store", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.clear();
    resetMockSettings();
    resetPrefsForTests();
  });
  afterEach(() => {
    resetPrefsForTests();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("debounces saves to the app DB and mirrors to localStorage immediately", async () => {
    const save = vi.spyOn(ipc.settings, "set");
    setPref("pageSize", 500);
    setPref("editorFontSize", 15);
    expect(JSON.parse(localStorage.getItem(PREFS_LOCAL_KEY) ?? "{}")).toMatchObject({
      pageSize: 500,
      editorFontSize: 15,
    });
    await vi.advanceTimersByTimeAsync(SAVE_DEBOUNCE_MS - 1);
    setPref("editorLineWrap", true);
    await vi.advanceTimersByTimeAsync(SAVE_DEBOUNCE_MS - 1);
    expect(save).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(save).toHaveBeenCalledTimes(1);
    expect(save).toHaveBeenCalledWith(PREFS_KEY, {
      ...DEFAULT_PREFS,
      pageSize: 500,
      editorFontSize: 15,
      editorLineWrap: true,
    });
    // It reached the (mock) backend.
    expect((await ipc.settings.get())[PREFS_KEY]).toMatchObject({ pageSize: 500 });
  });

  it("ignores no-op and invalid updates", async () => {
    const save = vi.spyOn(ipc.settings, "set");
    setPref("pageSize", DEFAULT_PREFS.pageSize);
    setPref("editorFontSize", 99);
    usePrefsStore.getState().patchPrefs({ connectTimeout: -1 });
    await vi.advanceTimersByTimeAsync(SAVE_DEBOUNCE_MS * 2);
    expect(save).not.toHaveBeenCalled();
    expect(getPrefs()).toEqual(DEFAULT_PREFS);
  });

  it("flushPrefs writes a pending save right away", async () => {
    const save = vi.spyOn(ipc.settings, "set");
    setPref("confirmCommit", true);
    await flushPrefs();
    expect(save).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(SAVE_DEBOUNCE_MS * 2);
    expect(save).toHaveBeenCalledTimes(1);
  });

  it("loads once from the app DB, sanitizing the stored object", async () => {
    await ipc.settings.set(PREFS_KEY, { pageSize: 1000, editorFontSize: "x", bogus: true });
    const get = vi.spyOn(ipc.settings, "get");
    const save = vi.spyOn(ipc.settings, "set");
    await Promise.all([loadPrefs(), loadPrefs()]);
    expect(get).toHaveBeenCalledTimes(1);
    expect(usePrefsStore.getState().loaded).toBe(true);
    expect(getPrefs()).toEqual({ ...DEFAULT_PREFS, pageSize: 1000 });
    expect(JSON.parse(localStorage.getItem(PREFS_LOCAL_KEY) ?? "{}")).toMatchObject({
      pageSize: 1000,
    });
    await vi.advanceTimersByTimeAsync(SAVE_DEBOUNCE_MS * 2);
    expect(save).not.toHaveBeenCalled();
  });

  it("keeps edits made before loading finished and saves them", async () => {
    await ipc.settings.set(PREFS_KEY, { pageSize: 1000 });
    const loading = loadPrefs();
    setPref("editorTabSize", 4);
    await loading;
    expect(getPrefs().editorTabSize).toBe(4);
    await vi.advanceTimersByTimeAsync(SAVE_DEBOUNCE_MS);
    expect((await ipc.settings.get())[PREFS_KEY]).toMatchObject({ editorTabSize: 4 });
  });

  it("pushes a local-only copy to an empty app DB", async () => {
    resetPrefsForTests({ ...DEFAULT_PREFS, uiFontSize: "large" });
    await loadPrefs();
    expect(getPrefs().uiFontSize).toBe("large");
    await vi.advanceTimersByTimeAsync(SAVE_DEBOUNCE_MS);
    expect((await ipc.settings.get())[PREFS_KEY]).toMatchObject({ uiFontSize: "large" });
  });

  it("survives a failing backend", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.spyOn(ipc.settings, "get").mockRejectedValue(new Error("boom"));
    await loadPrefs();
    expect(usePrefsStore.getState().loaded).toBe(true);
    expect(getPrefs()).toEqual(DEFAULT_PREFS);
  });

  it("new table tabs start with the default page size preference", () => {
    setPref("pageSize", 500);
    expect(useTableTabsStore.getState().ensure("prefs-test-tab").limit).toBe(500);
    useTableTabsStore.getState().remove("prefs-test-tab");
  });
});
