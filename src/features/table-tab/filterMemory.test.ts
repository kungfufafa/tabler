import { afterEach, describe, expect, it } from "vitest";
import { newFilterRow } from "@/stores/tableTabs";
import {
  FILTER_MEMORY_KEY,
  FILTER_MEMORY_MAX,
  filterMemoryKey,
  initialFilterState,
  recallFilters,
  rememberFilters,
  resetFilterMemoryForTests,
} from "./filterMemory";
import { isFilterOp } from "./filters";

const users = { schema: "public", name: "users" };

afterEach(() => resetFilterMemoryForTests());

describe("filter memory", () => {
  it("keys tables by profile, database and table", () => {
    const a = filterMemoryKey("p1", "app", users);
    expect(filterMemoryKey("p1", "app", users)).toBe(a);
    expect(filterMemoryKey("p2", "app", users)).not.toBe(a);
    expect(filterMemoryKey("p1", "other", users)).not.toBe(a);
    expect(filterMemoryKey("p1", "app", { schema: "x", name: "users" })).not.toBe(a);
  });

  it("remembers, persists and forgets a table's filters", () => {
    const key = filterMemoryKey("p1", "app", users);
    const filters = [newFilterRow("name", "contains", "ann")];
    rememberFilters(key, { filters, conjunction: "or", visible: true }, 1);
    expect(recallFilters(key)).toEqual({ filters, conjunction: "or", visible: true, at: 1 });
    // Written to localStorage, read back after a restart.
    expect(localStorage.getItem(FILTER_MEMORY_KEY)).toContain("ann");
    const stored = localStorage.getItem(FILTER_MEMORY_KEY);
    resetFilterMemoryForTests();
    localStorage.setItem(FILTER_MEMORY_KEY, stored!);
    expect(recallFilters(key)?.filters).toEqual(filters);
    // Nothing left to remember: forgotten.
    rememberFilters(key, { filters: [], conjunction: "and", visible: false });
    expect(recallFilters(key)).toBeNull();
  });

  it("drops the oldest tables past the cap and ignores broken storage", () => {
    for (let i = 0; i <= FILTER_MEMORY_MAX; i++)
      rememberFilters(`k${i}`, { filters: [], conjunction: "and", visible: true }, i);
    expect(recallFilters("k0")).toBeNull();
    expect(recallFilters(`k${FILTER_MEMORY_MAX}`)).not.toBeNull();
    resetFilterMemoryForTests();
    localStorage.setItem(FILTER_MEMORY_KEY, '{"k":{"filters":"nope"}');
    expect(recallFilters("k")).toBeNull();
  });

  it("restores per the Default filter state setting", () => {
    const key = filterMemoryKey("p1", null, users);
    const filters = [newFilterRow("id", "eq", "1"), { ...newFilterRow("id"), op: "gone" as never }];
    rememberFilters(key, { filters, conjunction: "and", visible: false });
    expect(initialFilterState(key, "restore", isFilterOp)).toEqual({
      filters: [filters[0]],
      conjunction: "and",
      filtersVisible: false,
    });
    expect(initialFilterState(key, "show", isFilterOp)).toEqual({ filtersVisible: true });
    expect(initialFilterState(key, "hide", isFilterOp)).toEqual({ filtersVisible: false });
    expect(initialFilterState("unknown", "restore", isFilterOp)).toEqual({});
    expect(initialFilterState(null, "restore", isFilterOp)).toEqual({});
  });
});
