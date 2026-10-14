import { beforeEach, describe, expect, it } from "vitest";
import type { ColumnMeta, DdlOp, TableMeta } from "@/lib/ipc";
import {
  addColumn,
  addIndex,
  checkColumns,
  dropColumn,
  editColumn,
  editIndex,
  foldStructure,
  parseTriggerSql,
  replaceForeignKey,
  restoreIndex,
  dropIndex,
} from "./model";
import {
  addCheck,
  addTrigger,
  columnCheck,
  dropCheck,
  dropTrigger,
  editCheck,
  editTrigger,
  restoreCheck,
  restoreTrigger,
  setColumnCheck,
  setTableComment,
  setTableName,
  setTableOption,
  setTriggerEnabled,
  stagedRename,
  triggerSource,
  triggerTemplate,
  withRenameLast,
} from "./objects";
import { useStructureStore } from "./store";
import { editableViewSql, viewOps } from "./viewEdit";

const T = { schema: "public", name: "items" };

function col(name: string, extra: Partial<ColumnMeta> = {}): ColumnMeta {
  return {
    name,
    ordinal: 0,
    dataType: "integer",
    kind: "int",
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
    col("id", { nullable: false, isPk: true, isAutoIncrement: true, extra: "BY DEFAULT" }),
    col("price", { dataType: "numeric(10,2)" }),
    col("qty"),
    col("legacy", { defaultExpr: "nextval('items_legacy_seq'::regclass)", isAutoIncrement: true }),
  ],
  primaryKey: ["id"],
  indexes: [
    {
      name: "items_pkey",
      columns: ["id"],
      unique: true,
      primary: true,
      method: "btree",
      predicate: null,
      definition: null,
      comment: null,
      include: [],
    },
    {
      name: "items_qty_idx",
      columns: ["qty"],
      unique: false,
      primary: false,
      method: "btree",
      predicate: null,
      definition: null,
      comment: null,
      include: [],
    },
  ],
  foreignKeys: [
    {
      name: "items_qty_fkey",
      columns: ["qty"],
      refTable: { schema: "public", name: "stock" },
      refColumns: ["id"],
      onUpdate: "NO ACTION",
      onDelete: "CASCADE",
      definition: null,
    },
  ],
  triggers: [
    {
      name: "items_touch",
      timing: "BEFORE",
      events: ["UPDATE"],
      enabled: true,
      definition:
        "CREATE TRIGGER items_touch BEFORE UPDATE ON public.items FOR EACH ROW EXECUTE FUNCTION touch()",
    },
  ],
  comment: "stock items",
  rowEstimate: 10,
  ddl: null,
  checks: [{ name: "items_price_check", expression: "price > 0", columns: ["price"] }],
  options: {
    engine: "InnoDB",
    charset: "utf8mb4",
    collation: "utf8mb4_0900_ai_ci",
    autoIncrement: 11,
    rowFormat: "Dynamic",
  },
};

describe("table name, comment and options", () => {
  it("stages a rename (Name field) and keeps it last", () => {
    let ops = setTableName(META, [], " goods ");
    expect(ops).toEqual([{ op: "renameTable", table: T, newName: "goods" }]);
    expect(foldStructure(META, ops).info.name).toBe("goods");
    ops = setTableName(META, ops, "wares");
    expect(ops).toEqual([{ op: "renameTable", table: T, newName: "wares" }]);
    expect(setTableName(META, ops, "items")).toEqual([]);
    const later = withRenameLast([...ops, { op: "dropColumn", table: T, name: "qty" }], T);
    expect(later.map((o) => o.op)).toEqual(["dropColumn", "renameTable"]);
    expect(stagedRename(later, T)).toBe("wares");
    expect(stagedRename([], T)).toBeNull();
  });

  it("stages the table comment, removed again when set back", () => {
    const ops = setTableComment(META, [], "goods");
    expect(ops).toEqual([{ op: "setTableComment", table: T, comment: "goods" }]);
    expect(setTableComment(META, ops, "stock items")).toEqual([]);
    expect(setTableComment(META, [], "  ")).toEqual([
      { op: "setTableComment", table: T, comment: null },
    ]);
  });

  it("merges MySQL options into one alterTableOptions op", () => {
    let ops = setTableOption(META, [], "engine", "MyISAM");
    ops = setTableOption(META, ops, "collation", "utf8mb4_bin");
    ops = setTableOption(META, ops, "autoIncrement", 100);
    expect(ops).toEqual([
      {
        op: "alterTableOptions",
        table: T,
        options: {
          engine: "MyISAM",
          charset: null,
          collation: "utf8mb4_bin",
          autoIncrement: 100,
          rowFormat: null,
        },
      },
    ]);
    expect(foldStructure(META, ops).info.options).toMatchObject({
      engine: "MyISAM",
      charset: "utf8mb4",
      collation: "utf8mb4_bin",
    });
    // A charset change resets the collation to the set's default.
    ops = setTableOption(META, ops, "charset", "latin1");
    expect(ops[0]).toMatchObject({ options: { charset: "latin1", collation: null } });
    // Back to the database's values: nothing left to change.
    ops = setTableOption(META, ops, "charset", "utf8mb4");
    ops = setTableOption(META, ops, "engine", "innodb");
    ops = setTableOption(META, ops, "autoIncrement", 11);
    expect(ops).toEqual([]);
  });
});

describe("indexes", () => {
  it("an edit of an existing index stages drop + create, shown as one changed row", () => {
    const ops = editIndex(META, [], "i:items_qty_idx", { unique: true });
    expect(ops).toEqual([
      { op: "dropIndex", table: T, name: "items_qty_idx" },
      {
        op: "addIndex",
        table: T,
        index: {
          name: "items_qty_idx",
          columns: ["qty"],
          unique: true,
          method: "btree",
          predicate: null,
          comment: null,
          include: [],
        },
      },
    ]);
    const row = foldStructure(META, ops).indexes.find((x) => x.key === "i:items_qty_idx")!;
    expect(row).toMatchObject({ status: "changed", unique: true, opIndex: 0, addAt: 1 });
    expect(foldStructure(META, ops).indexes).toHaveLength(2);
    // A second edit replaces the create; setting it back removes both.
    const more = editIndex(META, ops, "i:items_qty_idx", { comment: "by qty" });
    expect(more).toHaveLength(2);
    expect(more[1]).toMatchObject({ index: { unique: true, comment: "by qty" } });
    expect(editIndex(META, more, "i:items_qty_idx", { unique: false, comment: null })).toEqual([]);
    // Revert and drop of a changed row.
    expect(restoreIndex(META, ops, "i:items_qty_idx")).toEqual([]);
    expect(dropIndex(META, ops, "i:items_qty_idx")).toEqual([ops[0]]);
    // The primary key index is not edited here.
    expect(editIndex(META, [], "i:items_pkey", { unique: false })).toEqual([]);
  });

  it("a staged index changes in place", () => {
    const ops: DdlOp[] = [
      {
        op: "addIndex",
        table: T,
        index: {
          name: "ix",
          columns: ["price"],
          unique: false,
          method: null,
          predicate: null,
          comment: null,
          include: [],
        },
      },
    ];
    expect(editIndex(META, ops, "ni:0", { columns: ["price", "qty"] })).toEqual([
      { ...ops[0], index: { ...(ops[0] as { index: object }).index, columns: ["price", "qty"] } },
    ]);
  });
});

describe("CHECK constraints", () => {
  it("adds, edits (drop + add under the same name), drops and restores checks", () => {
    let ops = addCheck(META, [], { name: "items_qty_check", expression: "qty >= 0" });
    let view = foldStructure(META, ops);
    expect(view.checks.map((c) => [c.name, c.status, c.columns])).toEqual([
      ["items_price_check", "existing", ["price"]],
      ["items_qty_check", "added", ["qty"]],
    ]);
    // An emptied new check goes away.
    expect(editCheck(META, ops, "nk:0", { expression: "  " })).toEqual([]);
    ops = editCheck(META, [], "k:items_price_check", { expression: "price >= 0" });
    expect(ops).toEqual([
      { op: "dropCheck", table: T, name: "items_price_check" },
      {
        op: "addCheck",
        table: T,
        check: { name: "items_price_check", expression: "price >= 0" },
      },
    ]);
    view = foldStructure(META, ops);
    expect(view.checks).toHaveLength(1);
    expect(view.checks[0]).toMatchObject({ status: "changed", expression: "price >= 0" });
    expect(editCheck(META, ops, "k:items_price_check", { expression: "price > 0" })).toEqual([]);
    expect(restoreCheck(META, ops, "k:items_price_check")).toEqual([]);
    const dropped = dropCheck(META, [], "k:items_price_check");
    expect(dropped).toEqual([{ op: "dropCheck", table: T, name: "items_price_check" }]);
    expect(restoreCheck(META, dropped, "k:items_price_check")).toEqual([]);
  });

  it("the PostgreSQL check cell adds, replaces and drops the column's check", () => {
    const view = foldStructure(META, []);
    expect(columnCheck(view, "price")?.expression).toBe("price > 0");
    expect(columnCheck(view, "qty")).toBeNull();
    expect(setColumnCheck(META, [], "qty", "qty >= 0")).toEqual([
      { op: "addCheck", table: T, check: { name: "items_qty_check", expression: "qty >= 0" } },
    ]);
    // Must use that column (and only it).
    expect(setColumnCheck(META, [], "qty", "price > 1")).toEqual([]);
    expect(setColumnCheck(META, [], "price", "price < 100")).toHaveLength(2);
    expect(setColumnCheck(META, [], "price", "")).toEqual([
      { op: "dropCheck", table: T, name: "items_price_check" },
    ]);
  });

  it("finds the columns an expression uses", () => {
    expect(checkColumns(`"Price" > 0 AND qty <> 'price'`, ["price", "qty", "id"])).toEqual([
      "price",
      "qty",
    ]);
    expect(checkColumns("`qty` > 1", ["qty"])).toEqual(["qty"]);
  });
});

describe("triggers", () => {
  it("parses CREATE TRIGGER heads", () => {
    expect(
      parseTriggerSql(
        'CREATE TRIGGER "a b" AFTER INSERT OR UPDATE OF x, y ON t FOR EACH ROW EXECUTE FUNCTION f()',
      ),
    ).toEqual({ name: "a b", timing: "AFTER", events: ["INSERT", "UPDATE"] });
    expect(
      parseTriggerSql(
        "CREATE DEFINER=`root`@`%` TRIGGER `shop`.`t_bi` BEFORE INSERT ON `t` FOR EACH ROW SET NEW.x = 1",
      ),
    ).toEqual({ name: "t_bi", timing: "BEFORE", events: ["INSERT"] });
    expect(
      parseTriggerSql(
        "CREATE TRIGGER IF NOT EXISTS v_ins INSTEAD OF DELETE ON v BEGIN SELECT 1; END",
      ),
    ).toEqual({
      name: "v_ins",
      timing: "INSTEAD OF",
      events: ["DELETE"],
    });
    expect(parseTriggerSql("SELECT 1").name).toBeNull();
  });

  it("templates per dialect and MySQL edit text from the body", () => {
    expect(triggerTemplate("postgres", T, "items_trigger")).toMatch(
      /^CREATE TRIGGER "items_trigger"\nBEFORE INSERT OR UPDATE ON "public"\."items"/,
    );
    expect(triggerTemplate("mysql", { schema: "shop", name: "t" }, "t_trigger")).toContain(
      "BEFORE INSERT ON `shop`.`t`\nFOR EACH ROW\nBEGIN",
    );
    expect(triggerTemplate("sqlite", { schema: "main", name: "t" }, "x")).toContain(
      'AFTER INSERT ON "t"',
    );
    expect(
      triggerSource(
        "mysql",
        { schema: "shop", name: "t" },
        {
          name: "t_bi",
          timing: "BEFORE",
          events: ["INSERT"],
          definition: "SET NEW.x = 1",
          sql: null,
        },
      ),
    ).toBe("CREATE TRIGGER `t_bi`\nBEFORE INSERT ON `shop`.`t`\nFOR EACH ROW\nSET NEW.x = 1");
  });

  it("adds, replaces (drop + create), drops and restores triggers", () => {
    const sql =
      "CREATE TRIGGER items_audit AFTER DELETE ON items FOR EACH ROW EXECUTE FUNCTION audit()";
    const added = addTrigger(META, [], sql);
    expect(added).toEqual({
      ok: true,
      ops: [{ op: "createTrigger", table: T, name: "items_audit", sql }],
    });
    expect(
      addTrigger(META, [], "CREATE TRIGGER items_touch AFTER INSERT ON items EXECUTE FUNCTION f()"),
    ).toMatchObject({ ok: false });
    expect(addTrigger(META, [], "nonsense")).toMatchObject({ ok: false });

    const original = META.triggers[0]!.definition;
    expect(editTrigger(META, [], "postgres", "t:items_touch", original)).toEqual({
      ok: true,
      ops: [],
    });
    const changed = original.replace("BEFORE", "AFTER");
    const r = editTrigger(META, [], "postgres", "t:items_touch", changed);
    expect(r).toEqual({
      ok: true,
      ops: [
        { op: "dropTrigger", table: T, name: "items_touch" },
        { op: "createTrigger", table: T, name: "items_touch", sql: changed },
      ],
    });
    if (!r.ok) return;
    const row = foldStructure(META, r.ops).triggers[0]!;
    expect(row).toMatchObject({ status: "changed", timing: "AFTER", sql: changed });
    expect(editTrigger(META, r.ops, "postgres", "t:items_touch", original)).toEqual({
      ok: true,
      ops: [],
    });
    expect(restoreTrigger(META, r.ops, "t:items_touch")).toEqual([]);
    expect(dropTrigger(META, [], "t:items_touch")).toEqual([
      { op: "dropTrigger", table: T, name: "items_touch" },
    ]);
    expect(dropTrigger(META, added.ok ? added.ops : [], "nt:0")).toEqual([]);
  });
});

describe("trigger enable / disable", () => {
  const disabledMeta: TableMeta = {
    ...META,
    triggers: [{ ...META.triggers[0]!, enabled: false }],
  };

  it("stages DISABLE / ENABLE TRIGGER; toggling back unstages it", () => {
    const ops = setTriggerEnabled(META, [], "t:items_touch", false);
    expect(ops).toEqual([{ op: "disableTrigger", table: T, name: "items_touch" }]);
    const row = foldStructure(META, ops).triggers[0]!;
    expect(row).toMatchObject({ enabled: false, enabledAt: 0, status: "existing" });
    // the same state again: nothing changes; back on: the op goes
    expect(setTriggerEnabled(META, ops, "t:items_touch", false)).toBe(ops);
    expect(setTriggerEnabled(META, ops, "t:items_touch", true)).toEqual([]);
    // a disabled trigger is enabled
    expect(setTriggerEnabled(disabledMeta, [], "t:items_touch", true)).toEqual([
      { op: "enableTrigger", table: T, name: "items_touch" },
    ]);
    expect(foldStructure(disabledMeta, []).triggers[0]!.enabled).toBe(false);
  });

  it("a staged new trigger can be disabled; dropping a trigger drops its toggle", () => {
    const sql =
      "CREATE TRIGGER items_audit AFTER DELETE ON items FOR EACH ROW EXECUTE FUNCTION audit()";
    const added = addTrigger(META, [], sql);
    if (!added.ok) throw new Error(added.error);
    let ops = setTriggerEnabled(META, added.ops, "nt:0", false);
    expect(ops.map((o) => o.op)).toEqual(["createTrigger", "disableTrigger"]);
    expect(foldStructure(META, ops).triggers[1]).toMatchObject({ enabled: false, enabledAt: 1 });
    expect(dropTrigger(META, ops, "nt:0")).toEqual([]);

    ops = setTriggerEnabled(META, [], "t:items_touch", false);
    expect(dropTrigger(META, ops, "t:items_touch")).toEqual([
      { op: "dropTrigger", table: T, name: "items_touch" },
    ]);
  });

  it("an edited trigger is created enabled; a later toggle follows the new statement", () => {
    const original = META.triggers[0]!.definition;
    let ops = setTriggerEnabled(META, [], "t:items_touch", false);
    const r = editTrigger(
      META,
      ops,
      "postgres",
      "t:items_touch",
      original.replace("BEFORE", "AFTER"),
    );
    if (!r.ok) throw new Error(r.error);
    ops = r.ops;
    expect(ops.map((o) => o.op)).toEqual(["disableTrigger", "dropTrigger", "createTrigger"]);
    const row = foldStructure(META, ops).triggers[0]!;
    expect(row).toMatchObject({ status: "changed", enabled: true, enabledAt: null });
    ops = setTriggerEnabled(META, ops, "t:items_touch", false);
    expect(ops.at(-1)).toEqual({ op: "disableTrigger", table: T, name: "items_touch" });
    expect(foldStructure(META, ops).triggers[0]).toMatchObject({ enabled: false, enabledAt: 3 });
    // dropping the edited trigger takes its toggles along
    expect(dropTrigger(META, ops, "t:items_touch")).toEqual([
      { op: "dropTrigger", table: T, name: "items_touch" },
    ]);
  });
});

describe("index INCLUDE columns", () => {
  const index = {
    name: "items_price_idx",
    columns: ["price"],
    unique: false,
    method: null,
    predicate: null,
    comment: null,
    include: ["qty"],
  };

  it("folds INCLUDE columns of staged and existing indexes and follows renames", () => {
    const meta: TableMeta = {
      ...META,
      indexes: [META.indexes[0]!, { ...META.indexes[1]!, columns: ["price"], include: ["qty"] }],
    };
    let ops = addIndex(meta, [], index);
    let rows = foldStructure(meta, ops).indexes;
    expect(rows.find((x) => x.key === "ni:0")).toMatchObject({ include: ["qty"] });
    // the include list is part of the definition: editing it changes the staged op
    ops = editIndex(meta, ops, "ni:0", { include: [] });
    expect(ops).toEqual([{ op: "addIndex", table: T, index: { ...index, include: [] } }]);
    ops = editColumn(meta, ops, "postgres", "c:qty", "name", "amount");
    rows = foldStructure(meta, ops).indexes;
    expect(rows.find((x) => x.key === "i:items_qty_idx")).toMatchObject({ include: ["amount"] });
    // an index from metadata without the field (older backends) has none
    const bare = { ...meta, indexes: [{ ...META.indexes[1]!, include: undefined }] };
    expect(foldStructure(bare, []).indexes[0]!.include).toEqual([]);
  });

  it("dropping a staged new column removes the staged index that includes it", () => {
    const added = addColumn(META, [], "postgres");
    let ops = addIndex(META, added.ops, { ...index, include: ["column_1"] });
    ops = dropColumn(META, ops, added.key);
    expect(ops).toEqual([]);
  });
});

describe("columns: auto increment, ON UPDATE, collation", () => {
  it("stages identity on / off and shows the default it drops", () => {
    const ops = editColumn(META, [], "postgres", "c:legacy", "autoIncrement", false);
    expect(ops).toEqual([
      { op: "alterColumnAutoIncrement", table: T, name: "legacy", autoIncrement: false },
    ]);
    const row = foldStructure(META, ops).columns.find((c) => c.name === "legacy")!;
    expect(row).toMatchObject({ autoIncrement: false, defaultExpr: null });
    expect(row.changed).toEqual(expect.arrayContaining(["autoIncrement", "defaultExpr"]));
    expect(editColumn(META, ops, "postgres", "c:legacy", "autoIncrement", true)).toEqual([]);
  });

  it("stages ON UPDATE and collation edits per column", () => {
    let ops = editColumn(META, [], "mysql", "c:qty", "onUpdate", "CURRENT_TIMESTAMP");
    ops = editColumn(META, ops, "mysql", "c:qty", "collation", "utf8mb4_bin");
    expect(ops.map((o) => o.op)).toEqual(["alterColumnOnUpdate", "alterColumnCollation"]);
    const row = foldStructure(META, ops).columns.find((c) => c.name === "qty")!;
    expect(row).toMatchObject({ onUpdate: "CURRENT_TIMESTAMP", collation: "utf8mb4_bin" });
    // A rename keeps them pointed at the column.
    ops = editColumn(META, ops, "mysql", "c:qty", "name", "amount");
    expect(ops.map((o) => ("name" in o ? o.name : null))).toEqual(["qty", "qty", "qty"]);
  });
});

describe("foreign keys from the column popover", () => {
  it("replaces an existing key with drop + add, a staged one in place", () => {
    const fk = {
      name: null,
      columns: ["qty"],
      refTable: { schema: "public", name: "stock" },
      refColumns: ["id"],
      onUpdate: null,
      onDelete: "SET NULL",
    };
    const ops = replaceForeignKey(META, [], "f:items_qty_fkey", fk);
    expect(ops).toEqual([
      { op: "dropForeignKey", table: T, name: "items_qty_fkey" },
      { op: "addForeignKey", table: T, fk },
    ]);
    const same = { ...fk, onDelete: "CASCADE" };
    expect(replaceForeignKey(META, [], "f:items_qty_fkey", same)).toEqual([]);
    const staged: DdlOp[] = [{ op: "addForeignKey", table: T, fk }];
    expect(replaceForeignKey(META, staged, "nf:0", same)).toEqual([
      { op: "addForeignKey", table: T, fk: same },
    ]);
  });
});

describe("views", () => {
  it("replaces PG / MySQL views in place, drops and creates the others", () => {
    expect(editableViewSql("postgres", "view", 'CREATE VIEW "public"."v" AS\nSELECT 1;')).toBe(
      'CREATE OR REPLACE VIEW "public"."v" AS\nSELECT 1;',
    );
    expect(
      editableViewSql(
        "mysql",
        "view",
        "CREATE ALGORITHM=UNDEFINED DEFINER=`root`@`%` VIEW `v` AS select 1",
      ),
    ).toBe("CREATE OR REPLACE ALGORITHM=UNDEFINED DEFINER=`root`@`%` VIEW `v` AS select 1");
    expect(
      editableViewSql("postgres", "materializedView", "CREATE MATERIALIZED VIEW m AS SELECT 1"),
    ).toBe("CREATE MATERIALIZED VIEW m AS SELECT 1");
    const v = { schema: "public", name: "v" };
    expect(viewOps("postgres", "view", v, "a", "a")).toEqual([]);
    expect(viewOps("postgres", "view", v, "a", "b")).toEqual([{ op: "rawSql", sql: "b" }]);
    expect(viewOps("postgres", "materializedView", v, "a", "b")).toEqual([
      { op: "rawSql", sql: 'DROP MATERIALIZED VIEW "public"."v"' },
      { op: "rawSql", sql: "b" },
    ]);
    expect(viewOps("sqlite", "view", { schema: "main", name: "v" }, "a", "b")[0]).toEqual({
      op: "rawSql",
      sql: 'DROP VIEW "main"."v"',
    });
  });
});

describe("redo", () => {
  beforeEach(() => useStructureStore.setState({ byTab: {} }));

  it("redoes undone edits until a new edit clears them", () => {
    const { stage, undo, redo } = useStructureStore.getState();
    stage("t", (ops) => setTableComment(META, ops, "a"));
    stage("t", (ops) => setTableName(META, ops, "b"));
    const ops = () => useStructureStore.getState().byTab.t?.ops ?? [];
    undo("t");
    expect(ops()).toHaveLength(1);
    redo("t");
    expect(ops()).toHaveLength(2);
    undo("t");
    stage("t", (o) => setTableName(META, o, "c"));
    redo("t");
    expect(ops()[1]).toMatchObject({ newName: "c" });
  });
});
