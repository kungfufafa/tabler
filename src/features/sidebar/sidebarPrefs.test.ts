import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  COLLAPSED_STORAGE_KEY,
  DEFAULT_SIDEBAR_OPTIONS,
  LEGACY_OPTIONS_STORAGE_KEY,
  migrateLegacyOptions,
  OPTIONS_STORAGE_KEY,
  PANE_STORAGE_KEY,
  parseCollapsed,
  parseOptions,
  parsePane,
  reloadSidebarPrefs,
  setSectionCollapsed,
  setSidebarOption,
  setSidebarPane,
  toggleSection,
  useSidebarPrefs,
} from "./sidebarPrefs";

const state = () => useSidebarPrefs.getState();
const stored = (key: string) => JSON.parse(localStorage.getItem(key) ?? "null") as unknown;

describe("sidebar preferences", () => {
  beforeEach(() => {
    localStorage.clear();
    reloadSidebarPrefs();
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("starts on the Items pane, every section expanded, the default options", () => {
    expect(state().pane).toBe("items");
    expect(state().collapsed.size).toBe(0);
    expect(state().options).toEqual(DEFAULT_SIDEBAR_OPTIONS);
  });

  it("leaves Recent and the row counts off by default (TablePlus lists names only)", () => {
    expect(DEFAULT_SIDEBAR_OPTIONS).toEqual({
      showRecent: false,
      showRowCounts: false,
      historyAllConnections: false,
    });
  });

  it("moves the first build's options to the new key, keeping only the history scope", () => {
    // The first build saved every option at once, so `true` here was most likely the old default.
    localStorage.setItem(
      LEGACY_OPTIONS_STORAGE_KEY,
      JSON.stringify({ showRecent: true, showRowCounts: true, historyAllConnections: true }),
    );
    reloadSidebarPrefs();
    expect(state().options).toEqual({ ...DEFAULT_SIDEBAR_OPTIONS, historyAllConnections: true });
    expect(stored(OPTIONS_STORAGE_KEY)).toEqual(state().options);
    expect(localStorage.getItem(LEGACY_OPTIONS_STORAGE_KEY)).toBeNull();

    // From then on the new key wins (a turned-on Recent stays on).
    setSidebarOption("showRecent", true);
    localStorage.setItem(LEGACY_OPTIONS_STORAGE_KEY, JSON.stringify({ showRecent: false }));
    reloadSidebarPrefs();
    expect(state().options.showRecent).toBe(true);
    expect(state().options.historyAllConnections).toBe(true);

    expect(migrateLegacyOptions("garbage")).toEqual(DEFAULT_SIDEBAR_OPTIONS);
  });

  it("persists the pane, the collapsed sections and the options", () => {
    setSidebarPane("history");
    toggleSection("Views");
    toggleSection("Recent");
    toggleSection("Views");
    setSidebarOption("showRowCounts", true);
    expect(stored(PANE_STORAGE_KEY)).toBe("history");
    expect(stored(COLLAPSED_STORAGE_KEY)).toEqual(["Recent"]);
    expect(stored(OPTIONS_STORAGE_KEY)).toEqual({
      ...DEFAULT_SIDEBAR_OPTIONS,
      showRowCounts: true,
    });

    useSidebarPrefs.setState({ pane: "items" });
    reloadSidebarPrefs();
    expect(state().pane).toBe("history");
    expect([...state().collapsed]).toEqual(["Recent"]);
    expect(state().options.showRowCounts).toBe(true);
  });

  it("only changes the state when a value changes", () => {
    const before = state();
    setSidebarPane("items");
    setSidebarOption("showRecent", false);
    setSectionCollapsed("Tables", false);
    expect(state()).toBe(before);
    setSectionCollapsed("Tables", true);
    setSectionCollapsed("Tables", true);
    expect([...state().collapsed]).toEqual(["Tables"]);
  });

  it("ignores malformed stored values", () => {
    expect(parsePane("nope")).toBe("items");
    expect(parsePane(null)).toBe("items");
    expect([...parseCollapsed(["Views", 3, "Bogus", "Recent"])]).toEqual(["Views", "Recent"]);
    expect(parseCollapsed("Views").size).toBe(0);
    expect(parseOptions({ showRecent: true, showRowCounts: "no", extra: true })).toEqual({
      ...DEFAULT_SIDEBAR_OPTIONS,
      showRecent: true,
    });
    expect(parseOptions([true])).toEqual(DEFAULT_SIDEBAR_OPTIONS);
    localStorage.setItem(PANE_STORAGE_KEY, "{not json");
    localStorage.setItem(COLLAPSED_STORAGE_KEY, "[");
    reloadSidebarPrefs();
    expect(state().pane).toBe("items");
    expect(state().collapsed.size).toBe(0);
  });

  it("keeps working when storage is unavailable", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    reloadSidebarPrefs();
    expect(state().pane).toBe("items");
    setSidebarPane("history");
    toggleSection("Pinned");
    setSidebarOption("historyAllConnections", true);
    expect(state().pane).toBe("history");
    expect(state().collapsed.has("Pinned")).toBe(true);
    expect(state().options.historyAllConnections).toBe(true);
  });
});
