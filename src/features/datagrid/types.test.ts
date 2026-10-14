import { describe, expect, it } from "vitest";
import type { ColumnMeta, ForeignKeyMeta } from "@/lib/ipc";
import { tableMetaFor } from "@/lib/ipc/mock/fixtures";
import {
  buildGridColumns,
  fkColumnsText,
  fkLabel,
  fkPairs,
  fksOf,
  isCompositeFk,
  type FkRef,
} from "./types";

const meta = tableMetaFor("postgres", "public", "posts")!;
const columns: ColumnMeta[] = meta.columns;

const fk = (over: Partial<ForeignKeyMeta>): ForeignKeyMeta => ({
  name: "fk",
  columns: ["user_id"],
  refTable: { schema: "public", name: "users" },
  refColumns: ["id"],
  onUpdate: "NO ACTION",
  onDelete: "NO ACTION",
  definition: null,
  ...over,
});

describe("buildGridColumns foreign keys", () => {
  it("keeps `fk` as the first single-column key and lists it in `fks` too", () => {
    const [id, userId] = buildGridColumns(columns, meta.foreignKeys);
    expect(id!.fk).toBeUndefined();
    expect(id!.fks).toBeUndefined();
    expect(userId!.fk).toEqual({ schema: "public", table: "users", column: "id" });
    expect(userId!.fks).toEqual([
      {
        schema: "public",
        table: "users",
        column: "id",
        name: "posts_user_id_fkey",
        localColumns: ["user_id"],
        refColumns: ["id"],
      },
    ]);
  });

  it("a column in two keys gets both, in the table's order", () => {
    const keys = [
      fk({ name: "a_fkey" }),
      fk({ name: "b_fkey", refTable: { schema: "other", name: "accounts" }, refColumns: ["uid"] }),
    ];
    const userId = buildGridColumns(columns, keys).find((c) => c.id === "user_id")!;
    expect(userId.fks).toHaveLength(2);
    expect(userId.fks!.map((f) => [f.name, f.schema, f.table, f.column])).toEqual([
      ["a_fkey", "public", "users", "id"],
      ["b_fkey", "other", "accounts", "uid"],
    ]);
    // `fk` (Quick Look) is the first single-column key.
    expect(userId.fk).toMatchObject({ table: "users", column: "id" });
    expect(fksOf(userId)).toBe(userId.fks);
  });

  it("a member of a composite key gets an arrow with the key's columns, and no `fk`", () => {
    const composite = fk({
      name: "c_fkey",
      columns: ["user_id", "title"],
      refColumns: ["id", "name"],
    });
    const built = buildGridColumns(columns, [composite]);
    const title = built.find((c) => c.id === "title")!;
    expect(title.fk).toBeUndefined();
    expect(title.fks).toEqual([
      {
        schema: "public",
        table: "users",
        // The referenced column this one maps to.
        column: "name",
        name: "c_fkey",
        localColumns: ["user_id", "title"],
        refColumns: ["id", "name"],
      },
    ]);
    expect(built.find((c) => c.id === "user_id")!.fks![0]).toMatchObject({ column: "id" });
    expect(built.find((c) => c.id === "body")!.fks).toBeUndefined();
  });

  it("a column in a single and a composite key lists both", () => {
    const keys = [
      fk({ name: "single" }),
      fk({ name: "pair", columns: ["user_id", "title"], refColumns: ["id", "name"] }),
    ];
    const userId = buildGridColumns(columns, keys).find((c) => c.id === "user_id")!;
    expect(userId.fks!.map((f) => f.name)).toEqual(["single", "pair"]);
    expect(userId.fks!.map(isCompositeFk)).toEqual([false, true]);
    expect(userId.fk).toMatchObject({ table: "users" });
  });
});

describe("foreign key helpers", () => {
  const single: FkRef = { schema: null, table: "users", column: "id" };
  const pair: FkRef = {
    schema: "public",
    table: "orders",
    column: "b",
    localColumns: ["x", "y"],
    refColumns: ["a", "b"],
  };

  it("fksOf reads a hand-made column's `fk`", () => {
    expect(fksOf({ fk: single })).toEqual([single]);
    expect(fksOf({})).toEqual([]);
    expect(fksOf({ fk: single, fks: [pair] })).toEqual([pair]);
  });

  it("labels a key as 'table (column)', a composite one as 'table (a, b)'", () => {
    expect(fkLabel(single)).toBe("users (id)");
    expect(fkLabel(pair)).toBe("orders (a, b)");
    expect(fkColumnsText(single)).toBe("id");
    expect(fkColumnsText(pair)).toBe("a, b");
  });

  it("pairs each column of the key with the one it references", () => {
    expect(fkPairs(single, "owner_id")).toEqual([{ local: "owner_id", ref: "id" }]);
    expect(fkPairs(pair, "y")).toEqual([
      { local: "x", ref: "a" },
      { local: "y", ref: "b" },
    ]);
    expect(isCompositeFk(single)).toBe(false);
    expect(isCompositeFk(pair)).toBe(true);
  });
});
