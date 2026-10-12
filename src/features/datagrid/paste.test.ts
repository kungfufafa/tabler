import { describe, expect, it } from "vitest";
import { holdsWholeRows, pastedRows, pastedValue, planPaste } from "./paste";
import type { GridColumn } from "./types";

const rect = (r0: number, c0: number, r1: number, c1: number) => ({ r0, c0, r1, c1 });

describe("planPaste", () => {
  it("fills every selected cell with a single value", () => {
    const plan = planPaste([["x"]], [rect(1, 1, 2, 2)], { r: 2, c: 2 }, 10, 5);
    expect(plan.targets).toEqual([
      { r: 1, c: 1, text: "x" },
      { r: 1, c: 2, text: "x" },
      { r: 2, c: 1, text: "x" },
      { r: 2, c: 2, text: "x" },
    ]);
    expect(plan.clippedRows).toBe(0);
  });

  it("pastes a single value into a single cell", () => {
    expect(planPaste([["x"]], [rect(3, 0, 3, 0)], { r: 3, c: 0 }, 10, 5).targets).toEqual([
      { r: 3, c: 0, text: "x" },
    ]);
  });

  it("anchors a block at the top-left of the first range and clips it", () => {
    const block = [
      ["a", "b", "c"],
      ["d", "e", "f"],
      ["g", "h", "i"],
    ];
    const plan = planPaste(block, [rect(8, 3, 9, 4)], { r: 9, c: 4 }, 10, 5);
    expect(plan.targets).toEqual([
      { r: 8, c: 3, text: "a" },
      { r: 8, c: 4, text: "b" },
      { r: 9, c: 3, text: "d" },
      { r: 9, c: 4, text: "e" },
    ]);
    expect(plan.clippedRows).toBe(1);
    expect(plan.clippedCols).toBe(1);
  });

  it("anchors at the focus when nothing is selected", () => {
    const plan = planPaste([["a", "b"]], [], { r: 0, c: 1 }, 2, 5);
    expect(plan.targets).toEqual([
      { r: 0, c: 1, text: "a" },
      { r: 0, c: 2, text: "b" },
    ]);
    expect(planPaste([["a"]], [], null, 2, 2).targets).toEqual([]);
  });

  it("fills every range of a multi-range selection once, or anchors a block at the first", () => {
    const ranges = [rect(0, 0, 0, 1), rect(0, 1, 1, 1), rect(5, 0, 5, 0)];
    const fill = planPaste([["z"]], ranges, { r: 5, c: 0 }, 10, 3);
    expect(fill.targets.map((t) => `${t.r}:${t.c}`)).toEqual(["0:0", "0:1", "1:1", "5:0"]);
    const block = planPaste([["1", "2"]], [rect(4, 1, 4, 1), rect(0, 0, 0, 0)], null, 10, 3);
    expect(block.targets).toEqual([
      { r: 4, c: 1, text: "1" },
      { r: 4, c: 2, text: "2" },
    ]);
  });
});

const col = (
  id: string,
  kind: GridColumn["kind"],
  extra: Partial<GridColumn> = {},
): GridColumn => ({
  id,
  name: id,
  kind,
  dataType: kind,
  nullable: true,
  isPk: false,
  hasDefault: false,
  isAutoInc: false,
  editable: true,
  align: "left",
  ...extra,
});

describe("pasting whole rows", () => {
  it("recognises copied rows: one field per visible column on every line", () => {
    expect(holdsWholeRows([["1", "Ann", "a@x"]], 3)).toBe(true);
    expect(
      holdsWholeRows(
        [
          ["1", "Ann", "a@x"],
          ["2", "Bob", ""],
        ],
        3,
      ),
    ).toBe(true);
    expect(
      holdsWholeRows(
        [
          ["1", "Ann", "a@x"],
          ["2", "Bob"],
        ],
        3,
      ),
    ).toBe(false);
    expect(holdsWholeRows([["Ann", "a@x"]], 3)).toBe(false);
    // A single value in a one-column grid is a value, not a row.
    expect(holdsWholeRows([["x"]], 1)).toBe(false);
    expect(holdsWholeRows([], 3)).toBe(false);
  });

  it("types each field by its column, an empty field as NULL outside text columns", () => {
    expect(pastedValue(col("n", "int"), "7")).toEqual({ t: "int", v: 7 });
    expect(pastedValue(col("n", "int"), "")).toEqual({ t: "null" });
    expect(pastedValue(col("s", "text"), "")).toEqual({ t: "text", v: "" });
    expect(pastedValue(col("n", "int"), "seven")).toBeNull();
    expect(pastedValue(col("e", "enum", { enumValues: ["a", "b"] }), "c")).toBeNull();
  });

  it("maps visible fields to source columns and leaves keys, blobs and read-only columns to their defaults", () => {
    // Visible: name (source 2), id (source 0, auto-increment), score (source 1), avatar (3), gen (4).
    const cols = [
      col("name", "text"),
      col("id", "int", { isAutoInc: true, isPk: true }),
      col("score", "int"),
      col("avatar", "bytes"),
      col("gen", "text", { editable: false }),
    ];
    const src = [2, 0, 1, 3, 4];
    const { rows, skipped } = pastedRows(
      [
        ["Ann", "1", "5", "AA==", "x"],
        ["Bob", "2", "lots", "", "y"],
      ],
      cols,
      src,
    );
    expect(rows).toEqual([
      [
        { c: 2, value: { t: "text", v: "Ann" } },
        { c: 1, value: { t: "int", v: 5 } },
      ],
      [{ c: 2, value: { t: "text", v: "Bob" } }],
    ]);
    expect(skipped).toBe(1);
  });
});
