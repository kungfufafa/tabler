import { describe, expect, it } from "vitest";
import type { BrowsePage } from "@/lib/ipc";
import { tableMetaFor } from "@/lib/ipc/mock/fixtures";
import { deleteRows, emptyStaging, insertRow, setCell } from "./model";
import { rowKeyFor, toStagedChanges } from "./toPayload";

const meta = tableMetaFor("postgres", "public", "tags")!;
const page: BrowsePage = {
  columns: meta.columns,
  rows: [
    [
      { t: "int", v: 1 },
      { t: "text", v: "a" },
    ],
    [
      { t: "int", v: 2 },
      { t: "text", v: "b" },
    ],
  ],
  pkColumns: ["id"],
  identity: "pk",
  rowIds: null,
  offset: 0,
  limit: 100,
  sql: "",
  durationMs: 1,
};

describe("toStagedChanges", () => {
  it("uses original key values and emits update/insert/delete", () => {
    let s = emptyStaging();
    s = setCell(s, "pk:1", 0, { t: "int", v: 99 }, page.rows[0]);
    s = setCell(s, "pk:1", 1, { t: "text", v: "z" }, page.rows[0]);
    s = deleteRows(s, [{ rowKey: "pk:2", original: page.rows[1] }]);
    s = insertRow(s, "new:x", [{ t: "default" }, { t: "text", v: "n" }]);
    const keys = { "pk:1": rowKeyFor(page, 0), "pk:2": rowKeyFor(page, 1) } as const;
    const changes = toStagedChanges(s, page.columns, (k) => keys[k as keyof typeof keys] ?? null);
    expect(changes).toEqual([
      {
        op: "update",
        rowId: "pk:1",
        key: { kind: "pk", value: { id: { t: "int", v: 1 } } },
        changes: { id: { t: "int", v: 99 }, name: { t: "text", v: "z" } },
      },
      { op: "delete", rowId: "pk:2", key: { kind: "pk", value: { id: { t: "int", v: 2 } } } },
      {
        op: "insert",
        rowId: "new:x",
        values: { id: { t: "default" }, name: { t: "text", v: "n" } },
      },
    ]);
  });

  it("builds rowid and all-column keys", () => {
    expect(rowKeyFor({ ...page, identity: "rowid", rowIds: [7, 8] }, 1)).toEqual({
      kind: "rowid",
      value: 8,
    });
    expect(rowKeyFor({ ...page, identity: "none", pkColumns: [] }, 0)).toEqual({
      kind: "allColumns",
      value: { id: { t: "int", v: 1 }, name: { t: "text", v: "a" } },
    });
  });

  it("keyless rows skip blobs without shifting the other columns", () => {
    const col = (name: string, kind: "int" | "bytes" | "text", ordinal: number) => ({
      ...meta.columns[0]!,
      name,
      kind,
      dataType: kind === "bytes" ? "bytea" : kind === "int" ? "integer" : "text",
      ordinal,
      isPk: false,
      pkOrdinal: null,
      isAutoIncrement: false,
      defaultExpr: null,
    });
    const keyless: BrowsePage = {
      ...page,
      columns: [col("a", "int", 1), col("b", "bytes", 2), col("c", "text", 3), col("d", "int", 4)],
      rows: [
        [
          { t: "int", v: 1 },
          { t: "bytes", v: { base64: "AAE=", len: 2, truncated: false } },
          { t: "text", v: "three" },
          { t: "int", v: 4 },
        ],
      ],
      pkColumns: [],
      identity: "none",
    };
    expect(rowKeyFor(keyless, 0)).toEqual({
      kind: "allColumns",
      value: { a: { t: "int", v: 1 }, c: { t: "text", v: "three" }, d: { t: "int", v: 4 } },
    });
  });
});
