import { describe, expect, it } from "vitest";
import type { ColumnRow } from "./model";
import { MIXED, commonValue, nextSelection } from "./columnSelection";

const order = ["a", "b", "c", "d", "e"];
const none = { meta: false, shift: false };

describe("selecting columns", () => {
  it("a plain click selects one column", () => {
    expect(nextSelection(["a", "b"], "a", order, "c", none)).toEqual({
      selected: ["c"],
      anchor: "c",
    });
  });

  it("⌘ toggles a column and moves the anchor", () => {
    const first = nextSelection(["a"], "a", order, "c", { meta: true, shift: false });
    expect(first).toEqual({ selected: ["a", "c"], anchor: "c" });
    expect(
      nextSelection(first.selected, first.anchor, order, "a", { meta: true, shift: false }),
    ).toEqual({
      selected: ["c"],
      anchor: "a",
    });
  });

  it("⇧ selects the run from the anchor, whichever way round", () => {
    expect(nextSelection(["b"], "b", order, "d", { meta: false, shift: true })).toEqual({
      selected: ["b", "c", "d"],
      anchor: "b",
    });
    expect(nextSelection(["d"], "d", order, "b", { meta: false, shift: true }).selected).toEqual([
      "b",
      "c",
      "d",
    ]);
  });

  it("⌘⇧ adds a run to the selection; ⇧ without an anchor is a plain click", () => {
    expect(nextSelection(["a"], "c", order, "e", { meta: true, shift: true }).selected).toEqual([
      "a",
      "c",
      "d",
      "e",
    ]);
    expect(nextSelection([], null, order, "b", { meta: false, shift: true })).toEqual({
      selected: ["b"],
      anchor: "b",
    });
  });
});

describe("common attributes", () => {
  const row = (name: string, dataType: string): ColumnRow => ({ name, dataType }) as ColumnRow;
  it("is the shared value, or MIXED when the columns differ", () => {
    const rows = [row("a", "text"), row("b", "text"), row("c", "int")];
    expect(commonValue(rows.slice(0, 2), (r) => r.dataType)).toBe("text");
    expect(commonValue(rows, (r) => r.dataType)).toBe(MIXED);
    expect(commonValue([], (r) => r.dataType)).toBe(MIXED);
  });
});
