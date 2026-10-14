import { describe, expect, it } from "vitest";
import { addSort, nextSort } from "./sort";

const s = (column: string, dir: "asc" | "desc") => ({ column, dir, nulls: null });

describe("header sorting (TablePlus)", () => {
  it("a click cycles DESC → ASC → off", () => {
    expect(nextSort([], "a", false)).toEqual([s("a", "desc")]);
    expect(nextSort([s("a", "desc")], "a", false)).toEqual([s("a", "asc")]);
    expect(nextSort([s("a", "asc")], "a", false)).toEqual([]);
    // Another column replaces the sort.
    expect(nextSort([s("a", "asc")], "b", false)).toEqual([s("b", "desc")]);
  });
  it("⇧-click adds a key, or turns an existing one in place", () => {
    expect(nextSort([s("a", "asc")], "b", true)).toEqual([s("a", "asc"), s("b", "desc")]);
    expect(nextSort([s("a", "desc"), s("b", "asc")], "a", true)).toEqual([
      s("a", "asc"),
      s("b", "asc"),
    ]);
    expect(nextSort([s("a", "asc"), s("b", "asc")], "a", true)).toEqual([s("b", "asc")]);
  });
  it("Add to Sort appends (or moves) the key last", () => {
    expect(addSort([s("a", "asc")], "b", "desc")).toEqual([s("a", "asc"), s("b", "desc")]);
    expect(addSort([s("a", "asc"), s("b", "desc")], "a", "desc")).toEqual([
      s("b", "desc"),
      s("a", "desc"),
    ]);
  });
});
