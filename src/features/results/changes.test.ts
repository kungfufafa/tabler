import { describe, expect, it } from "vitest";
import { deleteRows, emptyStaging, insertRow, insertRows, setCell } from "@/features/staging/model";
import type { ColumnMeta, RowResult, TableMeta, TableRef, Value } from "@/lib/ipc";
import {
  applyCommitted,
  bufferIndex,
  bufferKey,
  duplicateCells,
  newRowCells,
  resultRow,
  resultRowCount,
  toResultChanges,
} from "./changes";
import type { ResultEditTarget } from "./editability";

const USERS: TableRef = { schema: "public", name: "users" };

function cm(name: string, extra: Partial<ColumnMeta> = {}): ColumnMeta {
  return {
    name,
    ordinal: 1,
    dataType: "text",
    kind: "text",
    nullable: true,
    defaultExpr: null,
    isPk: false,
    pkOrdinal: null,
    isAutoIncrement: false,
    isGenerated: false,
    isUnsigned: false,
    comment: null,
    enumValues: null,
    charMaxLen: null,
    numericPrecision: null,
    numericScale: null,
    collation: null,
    arrayElemType: null,
    extra: null,
    ...extra,
  } as ColumnMeta;
}

const id = cm("id", { kind: "int", nullable: false, isPk: true, isAutoIncrement: true });
const email = cm("email", { nullable: false });
const name = cm("name");

/** `select u.id, u.email as mail, p.title, u.name from users u join posts p …` */
const target: ResultEditTarget = {
  table: USERS,
  meta: { table: USERS, primaryKey: ["id"] } as unknown as TableMeta,
  columns: [id, email, null, name],
  keyIndexes: [0],
};

const int = (v: number): Value => ({ t: "int", v });
const text = (v: string): Value => ({ t: "text", v });
const NULL: Value = { t: "null" };

const rows: Value[][] = [
  [int(1), text("ana@x"), text("Hello"), text("Ana")],
  [int(2), text("bo@x"), text("World"), NULL],
  [int(3), text("cy@x"), text("Again"), text("Cy")],
];

describe("result row keys", () => {
  it("round-trips buffer positions", () => {
    expect(bufferKey(7)).toBe("r7");
    expect(bufferIndex("r7")).toBe(7);
    expect(bufferIndex("new:abc")).toBeNull();
  });
});

describe("result rows with staged edits", () => {
  it("merges edits, keeps the sort order and appends new rows", () => {
    let s = setCell(emptyStaging(), bufferKey(1), 1, text("b@y"), rows[1]);
    s = deleteRows(s, [{ rowKey: bufferKey(2), original: rows[2] }]);
    s = insertRow(s, "new:1", newRowCells(target));
    expect(resultRowCount(rows.length, s)).toBe(4);
    const order = [2, 1, 0];
    expect(resultRow(rows, order, s, 0)).toMatchObject({ key: "r2", status: "deleted" });
    const edited = resultRow(rows, order, s, 1)!;
    expect(edited.cells[1]).toEqual(text("b@y"));
    expect(edited.changed).toEqual(new Set([1]));
    // Untouched rows are the buffer's own arrays.
    expect(resultRow(rows, null, s, 0)!.cells).toBe(rows[0]);
    const added = resultRow(rows, order, s, 3)!;
    expect(added).toMatchObject({ key: "new:1", status: "inserted" });
    // The id defaults, NOT NULL text starts empty, the joined column is NULL.
    expect(added.cells).toEqual([{ t: "default" }, text(""), NULL, NULL]);
    expect(resultRow(rows, order, s, 4)).toBeUndefined();
  });

  it("duplicates the target's cells with a fresh auto-increment value", () => {
    expect(duplicateCells(target, rows[0]!)).toEqual([
      { t: "default" },
      text("ana@x"),
      NULL,
      text("Ana"),
    ]);
  });
});

describe("result changes", () => {
  it("become updates, deletes and inserts on the target table, keyed by original values", () => {
    let s = setCell(emptyStaging(), bufferKey(0), 0, int(10), rows[0]); // the key itself
    s = setCell(s, bufferKey(0), 3, text("Anna"), rows[0]);
    s = setCell(s, bufferKey(1), 2, text("ignored"), rows[1]); // another table's column
    s = deleteRows(s, [{ rowKey: bufferKey(2), original: rows[2] }]);
    s = insertRows(s, [{ rowKey: "new:1", values: [{ t: "default" }, text("d@x"), NULL, NULL] }]);
    expect(toResultChanges(s, target)).toEqual([
      {
        op: "update",
        rowId: "r0",
        key: { kind: "pk", value: { id: int(1) } },
        changes: { id: int(10), name: text("Anna") },
      },
      { op: "delete", rowId: "r2", key: { kind: "pk", value: { id: int(3) } } },
      {
        op: "insert",
        rowId: "new:1",
        values: { id: { t: "default" }, email: text("d@x"), name: NULL },
      },
    ]);
  });

  it("update the buffer in place after a commit", () => {
    let s = setCell(emptyStaging(), bufferKey(0), 3, text("Anna"), rows[0]);
    s = deleteRows(s, [{ rowKey: bufferKey(1), original: rows[1] }]);
    s = insertRows(s, [{ rowKey: "new:1", values: [{ t: "default" }, text("d@x"), NULL, NULL] }]);
    const results: RowResult[] = [
      { rowId: "r1", op: "delete", affected: 1, returned: null, error: null },
      {
        rowId: "r0",
        op: "update",
        affected: 1,
        returned: { id: int(1), email: text("ana@x"), name: text("ANNA") },
        error: null,
      },
      {
        rowId: "new:1",
        op: "insert",
        affected: 1,
        returned: { id: int(4), email: text("d@x"), name: NULL },
        error: null,
      },
    ];
    const next = applyCommitted(rows, s, target, results);
    expect(next).toEqual([
      // The stored row wins (a trigger upper-cased the name).
      [int(1), text("ana@x"), text("Hello"), text("ANNA")],
      rows[2],
      // The server-assigned id; the joined column is unknown.
      [int(4), text("d@x"), NULL, NULL],
    ]);
    // The original buffer is not mutated.
    expect(rows[0]![3]).toEqual(text("Ana"));
    // Without RETURNING (MySQL) the staged values stay, DEFAULT included.
    const plain = applyCommitted(rows, s, target, []);
    expect(plain[0]![3]).toEqual(text("Anna"));
    expect(plain[2]).toEqual([{ t: "default" }, text("d@x"), NULL, NULL]);
  });
});
