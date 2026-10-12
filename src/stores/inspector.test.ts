import { afterEach, describe, expect, it } from "vitest";
import type { GridColumn } from "@/features/datagrid/types";
import { cellAtColumn, toggleRowDetail, useInspectorStore, type InspectedCell } from "./inspector";
import { useUiStore } from "./ui";

const col = (name: string, kind: GridColumn["kind"]): GridColumn => ({
  id: name,
  name,
  kind,
  dataType: kind,
  nullable: true,
  isPk: false,
  hasDefault: false,
  isAutoInc: false,
  editable: true,
  align: "left",
});

afterEach(() => {
  useInspectorStore.setState({ cell: null, mode: "row" });
  useUiStore.setState({ inspectorOpen: false });
});

describe("inspector store", () => {
  it("defaults to the Details view", () => {
    expect(useInspectorStore.getInitialState().mode).toBe("row");
  });

  it("Space opens the panel on Details and closes it without changing the view", () => {
    useInspectorStore.setState({ mode: "cell" });
    useUiStore.setState({ inspectorOpen: false });
    toggleRowDetail();
    expect(useUiStore.getState().inspectorOpen).toBe(true);
    expect(useInspectorStore.getState().mode).toBe("row");
    useInspectorStore.setState({ mode: "cell" });
    toggleRowDetail();
    expect(useUiStore.getState().inspectorOpen).toBe(false);
    expect(useInspectorStore.getState().mode).toBe("cell");
  });

  it("moves an inspected cell to another column of its row", () => {
    const columns = [col("id", "int"), col("avatar", "bytes")];
    const cell: InspectedCell = {
      column: columns[0]!,
      value: { t: "int", v: 1 },
      rowIndex: 3,
      source: "users",
      row: { columns, cells: [{ t: "int", v: 1 }], focused: 0 },
      fetch: {
        connectionId: "c1",
        table: { schema: null, name: "users" },
        key: { kind: "pk", value: { id: { t: "int", v: 1 } } },
        column: "id",
      },
    };
    const moved = cellAtColumn(cell, 1);
    expect(moved.column.name).toBe("avatar");
    // A missing cell reads as NULL.
    expect(moved.value).toEqual({ t: "null" });
    expect(moved.row?.focused).toBe(1);
    expect(moved.fetch?.column).toBe("avatar");
    expect(moved.rowIndex).toBe(3);
    expect(cellAtColumn(cell, 0)).toBe(cell);
    expect(cellAtColumn(cell, 9)).toBe(cell);
    expect(cellAtColumn({ ...cell, row: undefined }, 1).column.name).toBe("id");
    expect(cellAtColumn({ ...cell, fetch: null }, 1).fetch).toBeNull();
  });

  it("asks a result where the full value of another column comes from", () => {
    // select id, thumbnail as avatar, now() as t from users: the alias is no column of users.
    const columns = [col("id", "int"), col("avatar", "bytes"), col("t", "timestamp")];
    const table = { schema: null, name: "users" };
    const key = { kind: "pk" as const, value: { id: { t: "int" as const, v: 1 } } };
    const origins = ["id", "thumbnail", null];
    const cell: InspectedCell = {
      column: columns[0]!,
      value: { t: "int", v: 1 },
      rowIndex: 0,
      source: "Result 1",
      row: { columns, cells: [{ t: "int", v: 1 }], focused: 0 },
      fetch: { connectionId: "c1", table, key, column: "id" },
      fetchAt: (c) => {
        const column = origins[c];
        return column ? { connectionId: "c1", table, key, column } : null;
      },
    };
    expect(cellAtColumn(cell, 1).fetch?.column).toBe("thumbnail");
    expect(cellAtColumn(cell, 2).fetch).toBeNull();
  });
});
