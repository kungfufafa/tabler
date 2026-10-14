import { afterEach, describe, expect, it } from "vitest";
import {
  defaultOrderSort,
  recallColumnLayout,
  recallDefaultOrder,
  rememberColumnLayout,
  rememberDefaultOrder,
  resetTableViewMemoryForTests,
  TABLE_VIEW_KEY,
  TABLE_VIEW_MAX,
} from "./tableViewMemory";

afterEach(() => resetTableViewMemoryForTests());

describe("table view memory", () => {
  const layout = { widths: { name: 240 }, order: ["name", "id"], hidden: ["email"] };

  it("remembers a table's column layout across tabs and restarts", () => {
    rememberColumnLayout("p\u0000db\u0000public\u0000users", layout);
    expect(recallColumnLayout("p\u0000db\u0000public\u0000users")).toEqual(layout);
    // A restart reads it back from storage.
    const stored = localStorage.getItem(TABLE_VIEW_KEY);
    resetTableViewMemoryForTests();
    localStorage.setItem(TABLE_VIEW_KEY, stored!);
    expect(recallColumnLayout("p\u0000db\u0000public\u0000users")).toEqual(layout);
    expect(recallColumnLayout("other")).toBeNull();
    expect(recallColumnLayout(null)).toBeNull();
  });

  it("forgets the layout once reset to the default", () => {
    rememberColumnLayout("k", layout);
    rememberColumnLayout("k", { widths: {}, order: [], hidden: [] });
    expect(recallColumnLayout("k")).toBeNull();
    expect(localStorage.getItem(TABLE_VIEW_KEY)).toBe("{}");
  });

  it("keeps the default content order next to the layout", () => {
    rememberColumnLayout("k", layout);
    rememberDefaultOrder("k", "pkDesc");
    expect(recallDefaultOrder("k")).toBe("pkDesc");
    expect(recallColumnLayout("k")).toEqual(layout);
    rememberDefaultOrder("k", "none");
    expect(recallDefaultOrder("k")).toBe("none");
    expect(recallColumnLayout("k")).toEqual(layout);
  });

  it("drops invalid entries and the oldest beyond the cap", () => {
    localStorage.setItem(
      TABLE_VIEW_KEY,
      JSON.stringify({
        bad: { at: 1, columns: { widths: { a: "x" } } },
        ok: { at: 1, order: "pkAsc" },
      }),
    );
    resetTableViewMemoryForTestsKeepStorage();
    expect(recallDefaultOrder("ok")).toBe("pkAsc");
    expect(recallColumnLayout("bad")).toBeNull();
    for (let i = 0; i < TABLE_VIEW_MAX + 5; i++) rememberDefaultOrder(`t${i}`, "pkAsc", i + 10);
    expect(recallDefaultOrder("ok")).toBe("none");
    expect(recallDefaultOrder(`t${TABLE_VIEW_MAX + 4}`)).toBe("pkAsc");
  });

  it("turns the default order into a primary-key sort", () => {
    expect(defaultOrderSort("pkDesc", ["a", "b"])).toEqual([
      { column: "a", dir: "desc", nulls: null },
      { column: "b", dir: "desc", nulls: null },
    ]);
    expect(defaultOrderSort("pkAsc", [])).toEqual([]);
    expect(defaultOrderSort("none", ["id"])).toEqual([]);
  });
});

/** Drops the in-memory copy only (the next read loads storage). */
function resetTableViewMemoryForTestsKeepStorage() {
  const raw = localStorage.getItem(TABLE_VIEW_KEY);
  resetTableViewMemoryForTests();
  if (raw !== null) localStorage.setItem(TABLE_VIEW_KEY, raw);
}
