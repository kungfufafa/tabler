import { beforeEach, describe, expect, it } from "vitest";
import type { ColumnMeta, DdlOp, TableMeta } from "@/lib/ipc";
import {
  addColumn,
  addIndex,
  columnNameError,
  dropColumn,
  dropIndex,
  editColumn,
  entryBase,
  foldStructure,
  restoreColumn,
  restoreIndex,
  retargetOps,
} from "./model";
import { EMPTY_SLICE, pushOps, undoOps, useStructureStore } from "./store";

const T = { schema: "public", name: "users" };

function col(name: string, extra: Partial<ColumnMeta> = {}): ColumnMeta {
  return {
    name,
    ordinal: 0,
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
  };
}

const META: TableMeta = {
  table: T,
  kind: "table",
  columns: [
    col("id", { dataType: "bigint", nullable: false, isPk: true, isAutoIncrement: true }),
    col("email", { nullable: false }),
    col("nickname"),
    col("score", { dataType: "integer", defaultExpr: "0" }),
  ],
  primaryKey: ["id"],
  indexes: [
    {
      name: "users_pkey",
      columns: ["id"],
      unique: true,
      primary: true,
      method: "btree",
      predicate: null,
      definition: null,
    },
    {
      name: "users_email_idx",
      columns: ["email"],
      unique: false,
      primary: false,
      method: "btree",
      predicate: null,
      definition: null,
    },
  ],
  foreignKeys: [],
  triggers: [],
  comment: null,
  rowEstimate: 10,
  ddl: null,
};

const edit = (ops: DdlOp[], key: string, field: Parameters<typeof editColumn>[4], value: unknown) =>
  editColumn(META, ops, "postgres", key, field, value as string | boolean | null);

describe("editColumn", () => {
  it("appends one op per attribute in edit order", () => {
    let ops: DdlOp[] = [];
    ops = edit(ops, "c:nickname", "dataType", "varchar(80)");
    ops = edit(ops, "c:email", "nullable", true);
    ops = edit(ops, "c:score", "defaultExpr", "10");
    expect(ops.map((o) => o.op)).toEqual([
      "alterColumnType",
      "alterColumnNullable",
      "alterColumnDefault",
    ]);
  });

  it("replaces a repeated edit in place and removes it when set back", () => {
    let ops = edit([], "c:nickname", "dataType", "varchar(80)");
    ops = edit(ops, "c:email", "nullable", true);
    ops = edit(ops, "c:nickname", "dataType", "varchar(120)");
    expect(ops).toHaveLength(2);
    expect(ops[0]).toMatchObject({ op: "alterColumnType", newType: "varchar(120)" });
    ops = edit(ops, "c:nickname", "dataType", "text");
    expect(ops.map((o) => o.op)).toEqual(["alterColumnNullable"]);
  });

  it("returns the same list for a no-op edit", () => {
    const ops = edit([], "c:nickname", "dataType", "varchar(80)");
    expect(edit(ops, "c:nickname", "dataType", "varchar(80)")).toBe(ops);
  });

  it("names columns by their name at that point of the list", () => {
    let ops = edit([], "c:nickname", "name", "nick");
    ops = edit(ops, "c:nickname", "dataType", "varchar(40)");
    expect(ops).toEqual([
      { op: "renameColumn", table: T, name: "nickname", newName: "nick" },
      { op: "alterColumnType", table: T, name: "nick", newType: "varchar(40)", using: null },
    ]);
  });

  it("renaming again rewrites the later references; renaming back removes the rename", () => {
    let ops = edit([], "c:nickname", "name", "nick");
    ops = edit(ops, "c:nickname", "defaultExpr", "'x'");
    ops = edit(ops, "c:nickname", "name", "handle");
    expect(ops).toEqual([
      { op: "renameColumn", table: T, name: "nickname", newName: "handle" },
      { op: "alterColumnDefault", table: T, name: "handle", defaultExpr: "'x'" },
    ]);
    ops = edit(ops, "c:nickname", "name", "nickname");
    expect(ops).toEqual([
      { op: "alterColumnDefault", table: T, name: "nickname", defaultExpr: "'x'" },
    ]);
  });

  it("an empty default drops the default", () => {
    const ops = edit([], "c:score", "defaultExpr", null);
    expect(ops).toEqual([{ op: "alterColumnDefault", table: T, name: "score", defaultExpr: null }]);
  });

  it("ignores comment edits on SQLite", () => {
    const ops = editColumn(META, [], "sqlite", "c:email", "comment", "hello");
    expect(ops).toEqual([]);
  });

  it("marks changed attributes in the folded view", () => {
    const ops = edit(edit([], "c:email", "name", "mail"), "c:score", "nullable", false);
    const view = foldStructure(META, ops);
    expect(view.columns.find((c) => c.key === "c:email")?.changed).toEqual(["name"]);
    expect(view.columns.find((c) => c.key === "c:score")?.changed).toEqual(["nullable"]);
    // Existing indexes show the staged column name.
    expect(view.indexes.find((i) => i.name === "users_email_idx")?.columns).toEqual(["mail"]);
  });
});

describe("addColumn / dropColumn", () => {
  it("adds column_<n> and folds later edits into its definition", () => {
    const added = addColumn(META, [], "postgres");
    expect(added.key).toBe("n:0");
    let ops = added.ops;
    ops = edit(ops, "n:0", "name", "bio");
    ops = edit(ops, "n:0", "dataType", "varchar(200)");
    ops = edit(ops, "n:0", "nullable", false);
    expect(ops).toHaveLength(1);
    expect(ops[0]).toMatchObject({
      op: "addColumn",
      after: null,
      column: { name: "bio", dataType: "varchar(200)", nullable: false },
    });
    const row = foldStructure(META, ops).columns.at(-1)!;
    expect(row).toMatchObject({ key: "n:0", status: "added", name: "bio" });
  });

  it("picks a free name and honours AFTER only on MySQL", () => {
    const first = addColumn(META, [], "mysql", "email");
    const second = addColumn(META, first.ops, "mysql", "");
    expect(second.ops[1]).toMatchObject({ column: { name: "column_2" }, after: "" });
    const view = foldStructure(META, second.ops);
    expect(view.columns.map((c) => c.name)).toEqual([
      "column_2",
      "id",
      "email",
      "column_1",
      "nickname",
      "score",
    ]);
    expect(addColumn(META, [], "postgres", "email").ops[0]).toMatchObject({ after: null });
  });

  it("rename then drop of the same column stages a single drop under the database name", () => {
    let ops = edit([], "c:nickname", "name", "nick");
    ops = edit(ops, "c:nickname", "dataType", "varchar(10)");
    ops = edit(ops, "c:email", "nullable", true);
    ops = dropColumn(META, ops, "c:nickname");
    expect(ops).toEqual([
      { op: "alterColumnNullable", table: T, name: "email", nullable: true },
      { op: "dropColumn", table: T, name: "nickname" },
    ]);
    const row = foldStructure(META, ops).columns.find((c) => c.key === "c:nickname")!;
    expect(row.status).toBe("dropped");
    expect(restoreColumn(META, ops, "c:nickname")).toEqual([ops[0]]);
  });

  it("dropping a staged new column removes it and the staged index using it", () => {
    let { ops } = addColumn(META, [], "postgres");
    ops = addIndex(META, ops, {
      name: "users_column_1_idx",
      columns: ["column_1"],
      unique: false,
      method: null,
      predicate: null,
    });
    ops = edit(ops, "c:email", "nullable", true);
    expect(dropColumn(META, ops, "n:0")).toEqual([ops[2]]);
  });

  it("renaming a staged column updates staged indexes that use it", () => {
    let { ops } = addColumn(META, [], "postgres");
    ops = addIndex(META, ops, {
      name: "ix",
      columns: ["column_1 DESC"],
      unique: false,
      method: null,
      predicate: null,
    });
    ops = edit(ops, "n:0", "name", "slug");
    expect(ops[1]).toMatchObject({ index: { columns: ["slug DESC"] } });
  });

  it("rejects names already used by another column (staged or in the database)", () => {
    const ops = edit([], "c:nickname", "name", "nick");
    const view = foldStructure(META, ops);
    expect(columnNameError(view, "c:email", "nickname")).toMatch(/already exists/);
    expect(columnNameError(view, "c:email", "NICK")).toMatch(/already exists/);
    expect(columnNameError(view, "c:nickname", "nickname")).toBeNull();
    expect(columnNameError(view, "c:email", "  ")).toBe("Column name is required");
  });
});

describe("indexes", () => {
  it("drops an existing index, restores it, and never drops the primary key", () => {
    const ops = dropIndex(META, [], "i:users_email_idx");
    expect(ops).toEqual([{ op: "dropIndex", table: T, name: "users_email_idx" }]);
    expect(foldStructure(META, ops).indexes[1]?.status).toBe("dropped");
    expect(restoreIndex(META, ops, "i:users_email_idx")).toEqual([]);
    expect(dropIndex(META, [], "i:users_pkey")).toEqual([]);
  });

  it("discards a staged index instead of dropping it", () => {
    const ops = addIndex(META, [], {
      name: "ix",
      columns: ["email"],
      unique: true,
      method: null,
      predicate: null,
    });
    expect(dropIndex(META, ops, "ni:0")).toEqual([]);
  });

  it("extracts the column of an index entry", () => {
    expect(entryBase("email")).toBe("email");
    expect(entryBase("name(10)")).toBe("name");
    expect(entryBase("created_at DESC")).toBe("created_at");
    expect(entryBase("lower(email)")).toBe("lower(email)");
  });
});

describe("staged ops store", () => {
  beforeEach(() => useStructureStore.setState({ byTab: {} }));

  it("pushOps / undoOps keep the history", () => {
    const a: DdlOp[] = [{ op: "rawSql", sql: "a" }];
    const b: DdlOp[] = [...a, { op: "rawSql", sql: "b" }];
    const s1 = pushOps(EMPTY_SLICE, a);
    const s2 = pushOps(s1, b);
    expect(pushOps(s2, s2.ops)).toBe(s2);
    expect(undoOps(s2)).toEqual({ ops: a, past: [[]] });
    expect(undoOps(undoOps(s2)).ops).toEqual([]);
    expect(undoOps(EMPTY_SLICE)).toBe(EMPTY_SLICE);
  });

  it("stages per tab, undoes the last edit and discards everything", () => {
    const { stage, undo, discard } = useStructureStore.getState();
    stage("t1", (ops) => edit(ops, "c:nickname", "dataType", "varchar(80)"));
    stage("t1", (ops) => edit(ops, "c:email", "nullable", true));
    stage("t2", (ops) => dropColumn(META, ops, "c:score"));
    const ops = () => useStructureStore.getState().byTab.t1?.ops ?? [];
    expect(ops().map((o) => o.op)).toEqual(["alterColumnType", "alterColumnNullable"]);
    undo("t1");
    expect(ops().map((o) => o.op)).toEqual(["alterColumnType"]);
    discard("t1");
    expect(ops()).toEqual([]);
    expect(useStructureStore.getState().byTab.t2?.ops).toHaveLength(1);
    useStructureStore.getState().retain(new Set(["t1"]));
    expect(useStructureStore.getState().byTab.t2).toBeUndefined();
  });

  it("keeps the partial-apply marker across edits and undo; discard clears it", () => {
    const { stage, undo, discard, markPartial } = useStructureStore.getState();
    stage("t1", (ops) => edit(ops, "c:nickname", "name", "nick"));
    stage("t1", (ops) => edit(ops, "c:email", "nullable", true));
    markPartial("t1", 1);
    const slice = () => useStructureStore.getState().byTab.t1;
    expect(slice()?.partial).toBe(1);
    stage("t1", (ops) => edit(ops, "c:score", "defaultExpr", null));
    undo("t1");
    expect(slice()?.ops).toHaveLength(2);
    expect(slice()?.partial).toBe(1);
    discard("t1");
    expect(slice()).toBeUndefined();
  });

  it("retarget points staged ops and the undo history at a renamed table", () => {
    const { stage, retarget, undo } = useStructureStore.getState();
    stage("t1", (ops) => edit(ops, "c:nickname", "name", "nick"));
    stage("t1", (ops) => edit(ops, "c:email", "nullable", true));
    const to = { schema: "public", name: "people" };
    retarget(["t1", "missing"], T, to);
    const ops = () => useStructureStore.getState().byTab.t1?.ops ?? [];
    expect(ops().map((o) => (o.op === "rawSql" ? null : o.table))).toEqual([to, to]);
    undo("t1");
    expect(ops()).toEqual([{ op: "renameColumn", table: to, name: "nickname", newName: "nick" }]);
  });

  it("retargetOps rewrites self-referencing foreign keys and leaves other tables alone", () => {
    const to = { schema: "public", name: "people" };
    const other = { schema: "public", name: "posts" };
    const fk = {
      name: null,
      columns: ["manager_id"],
      refTable: T,
      refColumns: ["id"],
      onUpdate: null,
      onDelete: null,
    };
    const ops: DdlOp[] = [
      { op: "addForeignKey", table: T, fk },
      { op: "dropColumn", table: other, name: "x" },
      { op: "rawSql", sql: "select 1" },
    ];
    expect(retargetOps(ops, T, to)).toEqual([
      { op: "addForeignKey", table: to, fk: { ...fk, refTable: to } },
      ops[1],
      ops[2],
    ]);
    const untouched: DdlOp[] = [ops[1]!, ops[2]!];
    expect(retargetOps(untouched, T, to)).toBe(untouched);
  });

  it("does not record a history entry for a no-op edit", () => {
    const { stage } = useStructureStore.getState();
    stage("t1", (ops) => edit(ops, "c:nickname", "dataType", "varchar(80)"));
    stage("t1", (ops) => edit(ops, "c:nickname", "dataType", "varchar(80)"));
    expect(useStructureStore.getState().byTab.t1?.past).toHaveLength(1);
  });
});
