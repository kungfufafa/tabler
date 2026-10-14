import { beforeEach, describe, expect, it } from "vitest";
import { getPrefs, resetPrefsForTests, sanitizePrefs, setPref } from "@/stores/prefs";
import {
  DEFAULT_TOOLBAR_ITEMS,
  insertItem,
  isToolbarItems,
  moveItem,
  paletteItems,
  removeItemAt,
  splitAtStatus,
  visibleItems,
  type ToolbarItemId,
} from "./toolbarLayout";

beforeEach(() => {
  localStorage.clear();
  resetPrefsForTests();
});

describe("toolbar layout", () => {
  it("validates stored layouts", () => {
    expect(isToolbarItems(DEFAULT_TOOLBAR_ITEMS)).toBe(true);
    expect(isToolbarItems(["space", "space", "flexibleSpace", "refresh"])).toBe(true);
    expect(isToolbarItems(["refresh", "refresh"])).toBe(false);
    expect(isToolbarItems(["nope"])).toBe(false);
    expect(isToolbarItems("refresh")).toBe(false);
    expect(isToolbarItems([])).toBe(true);
  });

  it("inserts, moves (a non-repeatable item moves instead of doubling) and removes", () => {
    const items: ToolbarItemId[] = ["refresh", "status", "panels"];
    expect(insertItem(items, "metrics", 1)).toEqual(["refresh", "metrics", "status", "panels"]);
    expect(insertItem(items, "space", 99)).toEqual(["refresh", "status", "panels", "space"]);
    expect(insertItem(items, "panels", 0)).toEqual(["panels", "refresh", "status"]);
    expect(insertItem(items, "refresh", 3)).toEqual(["status", "panels", "refresh"]);
    expect(insertItem(["space"], "space", 0)).toEqual(["space", "space"]);
    expect(moveItem(items, 0, 2)).toEqual(["status", "panels", "refresh"]);
    expect(removeItemAt(items, 1)).toEqual(["refresh", "panels"]);
    expect(paletteItems(items)).not.toContain("refresh");
    expect(paletteItems(items)).toContain("space");
  });

  it("centres the pill: the items split at the status item", () => {
    expect(splitAtStatus(DEFAULT_TOOLBAR_ITEMS)).toEqual({
      left: ["sidebar", "actions", "safeMode", "databases"],
      status: true,
      right: ["refresh", "openAnything", "workspaces", "users", "metrics", "panels"],
    });
    expect(splitAtStatus(["refresh"])).toEqual({ left: ["refresh"], status: false, right: [] });
    expect(visibleItems(DEFAULT_TOOLBAR_ITEMS, { metrics: false })).not.toContain("metrics");
    expect(visibleItems(DEFAULT_TOOLBAR_ITEMS, { metrics: true })).toContain("metrics");
  });

  it("persists in the toolbarItems / toolbarDisplay preferences", () => {
    expect(getPrefs().toolbarItems).toEqual(DEFAULT_TOOLBAR_ITEMS);
    expect(getPrefs().toolbarDisplay).toBe("icon");
    setPref("toolbarItems", ["status", "refresh"]);
    setPref("toolbarDisplay", "iconAndText");
    const stored = JSON.parse(localStorage.getItem("tabler.prefs")!) as Record<string, unknown>;
    expect(stored.toolbarItems).toEqual(["status", "refresh"]);
    expect(stored.toolbarDisplay).toBe("iconAndText");
    expect(sanitizePrefs(stored).toolbarItems).toEqual(["status", "refresh"]);
    // Damaged layouts fall back to the default set.
    expect(sanitizePrefs({ toolbarItems: ["refresh", "refresh"] }).toolbarItems).toEqual(
      DEFAULT_TOOLBAR_ITEMS,
    );
    expect(sanitizePrefs({ toolbarDisplay: "text" }).toolbarDisplay).toBe("icon");
  });
});
