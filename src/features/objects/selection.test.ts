import { describe, expect, it } from "vitest";
import {
  clickSelection,
  EMPTY_SELECTION,
  pruneSelection,
  selectAll,
  selectOnly,
  selectRange,
  targetKeys,
  toggleSelected,
} from "./selection";

const ORDER = ["a", "b", "c", "d", "e"];
const keys = (s: { keys: ReadonlySet<string> }) => [...s.keys].sort();

describe("tree selection", () => {
  it("plain click selects one item and anchors there", () => {
    const s = clickSelection(EMPTY_SELECTION, ORDER, "c", { meta: false, shift: false });
    expect(keys(s)).toEqual(["c"]);
    expect(s.anchor).toBe("c");
  });

  it("⌘-click toggles items", () => {
    let s = selectOnly("a");
    s = clickSelection(s, ORDER, "c", { meta: true, shift: false });
    expect(keys(s)).toEqual(["a", "c"]);
    s = toggleSelected(s, "a");
    expect(keys(s)).toEqual(["c"]);
    expect(s.anchor).toBe("a");
  });

  it("⇧-click selects the range from the anchor, in either direction", () => {
    let s = selectOnly("b");
    s = clickSelection(s, ORDER, "d", { meta: false, shift: true });
    expect(keys(s)).toEqual(["b", "c", "d"]);
    expect(s.anchor).toBe("b");
    // The anchor stays: a second ⇧-click replaces the range.
    s = selectRange(s, ORDER, "a");
    expect(keys(s)).toEqual(["a", "b"]);
    // No anchor yet: just the clicked item.
    expect(keys(selectRange(EMPTY_SELECTION, ORDER, "e"))).toEqual(["e"]);
    // An unknown key changes nothing.
    expect(selectRange(s, ORDER, "zz")).toBe(s);
  });

  it("⌘A selects every listed item", () => {
    expect(keys(selectAll(ORDER))).toEqual(ORDER);
    expect(selectAll([]).anchor).toBeNull();
  });

  it("drops items no longer listed", () => {
    const s = selectRange(selectOnly("a"), ORDER, "c");
    expect(pruneSelection(s, ORDER)).toBe(s);
    const pruned = pruneSelection(s, ["b", "c", "d"]);
    expect(keys(pruned)).toEqual(["b", "c"]);
    expect(pruned.anchor).toBeNull();
  });

  it("acts on the selection when the row is part of it, else on the row alone", () => {
    const s = selectRange(selectOnly("a"), ORDER, "c");
    expect(targetKeys(s, "b").sort()).toEqual(["a", "b", "c"]);
    expect(targetKeys(s, "e")).toEqual(["e"]);
  });
});
