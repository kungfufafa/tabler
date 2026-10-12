import { describe, expect, it } from "vitest";
import { effectiveOrder, fitWidth, hideColumn, idAfter, moveColumn, showAll } from "./columnState";
import {
  EMPTY_COLUMN_STATE,
  layoutColumns,
  MAX_COL_W,
  MIN_COL_W,
  visibleColumns,
  type ColumnState,
  type GridColumn,
} from "./types";

const col = (id: string): GridColumn => ({
  id,
  name: id,
  kind: "text",
  dataType: "text",
  nullable: true,
  isPk: false,
  hasDefault: false,
  isAutoInc: false,
  editable: true,
  align: "left",
});
const columns = ["id", "name", "email", "bio"].map(col);
const ids = columns.map((c) => c.id);
const state = (s: Partial<ColumnState>): ColumnState => ({ ...EMPTY_COLUMN_STATE, ...s });

describe("layoutColumns", () => {
  it("maps visible columns back to their source index", () => {
    const l = layoutColumns(columns, state({ hidden: ["id"] }));
    expect(l.cols.map((c) => c.id)).toEqual(["name", "email", "bio"]);
    expect(l.src).toEqual([1, 2, 3]);
  });

  it("applies order, ignores unknown ids and appends new columns", () => {
    const l = layoutColumns(columns, state({ order: ["bio", "gone", "id"], hidden: ["email"] }));
    expect(l.cols.map((c) => c.id)).toEqual(["bio", "id", "name"]);
    expect(l.src).toEqual([3, 0, 1]);
    expect(visibleColumns(columns, state({ order: ["bio"] })).map((c) => c.id)).toEqual([
      "bio",
      "id",
      "name",
      "email",
    ]);
  });

  it("is the identity without state", () => {
    const l = layoutColumns(columns, EMPTY_COLUMN_STATE);
    expect(l.src).toEqual([0, 1, 2, 3]);
  });
});

describe("column state helpers", () => {
  it("moveColumn writes every id, hidden ones included", () => {
    const s = moveColumn(ids, state({ hidden: ["email"] }), "bio", "id");
    expect(s.order).toEqual(["bio", "id", "name", "email"]);
    expect(s.hidden).toEqual(["email"]);
    expect(moveColumn(ids, s, "bio", null).order).toEqual(["id", "name", "email", "bio"]);
    // Moving next to itself changes nothing.
    expect(moveColumn(ids, s, "bio", "bio")).toBe(s);
    expect(moveColumn(ids, s, "bio", "id")).toBe(s);
  });

  it("moveColumn keeps a hidden column's slot", () => {
    const s = moveColumn(ids, state({ hidden: ["name"] }), "id", "bio");
    expect(s.order).toEqual(["name", "email", "id", "bio"]);
    expect(layoutColumns(columns, s).cols.map((c) => c.id)).toEqual(["email", "id", "bio"]);
    expect(idAfter(ids, s, "email")).toBe("id");
    expect(idAfter(ids, s, "bio")).toBeNull();
    expect(effectiveOrder(ids, state({ order: ["x", "bio"] }))).toEqual([
      "bio",
      "id",
      "name",
      "email",
    ]);
  });

  it("hides and shows", () => {
    const s = hideColumn(hideColumn(EMPTY_COLUMN_STATE, "id"), "id");
    expect(s.hidden).toEqual(["id"]);
    expect(showAll(s).hidden).toEqual([]);
    expect(showAll(EMPTY_COLUMN_STATE)).toBe(EMPTY_COLUMN_STATE);
  });

  it("fitWidth samples the header and the cells, clamped", () => {
    expect(fitWidth({ name: "id" }, ["1", "22"])).toBe(Math.round(2 * 7.5 + 24 + 18));
    expect(fitWidth({ name: "id" }, ["x".repeat(20)])).toBe(20 * 7.5 + 24);
    expect(fitWidth({ name: "" }, [])).toBe(MIN_COL_W);
    expect(fitWidth({ name: "n" }, ["x".repeat(10_000)])).toBe(MAX_COL_W);
  });
});
