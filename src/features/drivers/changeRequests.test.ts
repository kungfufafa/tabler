import { afterEach, describe, expect, it, vi } from "vitest";
import { buildGridColumns } from "@/features/datagrid/types";
import { insertModesFor } from "@/features/import/mapping";
import {
  headerMenu,
  objectMenu,
  plusMenu,
  schemaNodeMenu,
  tableMenu,
  withFeatures,
  type MenuEntry,
} from "@/features/objects/menu";
import { openNewObject, runNew } from "@/features/objects/actions";
import { useObjectsStore } from "@/features/objects/store";
import { COPY_AS_UPDATE } from "@/features/plugins/builtin/copyAsUpdate";
import { openDummyData, usePluginsStore } from "@/features/plugins/store";
import type { HostServices } from "@/features/plugins/types";
import { applyCommitted, bufferKey } from "@/features/results/changes";
import type { ResultEditTarget } from "@/features/results/editability";
import { dropApplied, emptyStaging, setCell, setRowError } from "@/features/staging/model";
import { rowKeyFor, toStagedChanges } from "@/features/staging/toPayload";
import { tableOptionRows } from "@/features/structure/tableOptions";
import {
  CORE_FILTERS,
  defaultFilterRow,
  filtersOffered,
  opsFor,
} from "@/features/table-tab/filters";
import {
  allowFeature,
  appliesPartially,
  BASE_CAPS,
  connectionDialect,
  dialectFor,
} from "@/lib/caps";
import type { BrowsePage, ColumnMeta, ConnectionInfo, Feature, TableMeta, Value } from "@/lib/ipc";
import {
  cellDisplay,
  displayValue,
  isMissingValue,
  MISSING_PLACEHOLDER,
  parseEditText,
} from "@/lib/values";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";

// The lanes' change requests the renderer answers (docs/drivers/README.md, "Change requests":
// F9b / F9b2), and the helpers of the F9b2 wiring.

const toast = vi.hoisted(() => ({ error: vi.fn(), message: vi.fn(), success: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

afterEach(() => {
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  vi.clearAllMocks();
});

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

const text = (v: string): Value => ({ t: "text", v });
const MISSING: Value = { t: "other", v: { ty: "missing", text: "" } };
const OID: Value = { t: "other", v: { ty: "objectId", text: "65f0c0ffee0000000000002a" } };

describe("redis: a composite key (key, type) with the type column read-only", () => {
  const columns = [
    col("key", { isPk: true, pkOrdinal: 1, nullable: false }),
    col("type", { isPk: true, pkOrdinal: 2, nullable: false, isGenerated: true }),
    col("value"),
  ];
  const page: BrowsePage = {
    columns,
    rows: [[text("user:1"), text("hash"), text("{}")]],
    pkColumns: ["key", "type"],
    identity: "pk",
    rowIds: null,
    offset: 0,
    limit: 100,
    sql: "SCAN 0",
    durationMs: 1,
    nextCursor: "17",
  };

  it("keys rows by both columns and leaves the type column read-only", () => {
    expect(buildGridColumns(columns).map((c) => [c.name, c.editable])).toEqual([
      ["key", true],
      ["type", false],
      ["value", true],
    ]);
    const key = rowKeyFor(page, 0);
    expect(key).toEqual({ kind: "pk", value: { key: text("user:1"), type: text("hash") } });
    // A rename of the key is matched by the key it had and its type.
    const staged = setCell(emptyStaging(), "k", 0, text("user:2"), page.rows[0]);
    expect(toStagedChanges(staged, columns, () => key)).toEqual([
      { op: "update", rowId: "k", key, changes: { key: text("user:2") } },
    ]);
  });
});

describe("mongodb / dynamodb: Other{ty} values and missing fields", () => {
  it("shows a missing field apart from NULL and an empty string", () => {
    expect(isMissingValue(MISSING)).toBe(true);
    expect(isMissingValue(OID)).toBe(false);
    expect(displayValue(MISSING)).toEqual({ text: MISSING_PLACEHOLDER, cls: "missing" });
    expect(cellDisplay(MISSING)).toEqual({
      text: MISSING_PLACEHOLDER,
      cls: "missing",
      placeholder: true,
    });
    expect(cellDisplay({ t: "null" }).text).toBe("NULL");
    expect(cellDisplay(text("")).text).toBe("EMPTY");
    // An ObjectId reads as its hex.
    expect(displayValue(OID)).toEqual({ text: "65f0c0ffee0000000000002a", cls: "other" });
  });

  it("stages what is typed into a missing field in the column's type, an ObjectId as one", () => {
    expect(parseEditText("text", "shipped", MISSING)).toEqual({ ok: true, value: text("shipped") });
    expect(parseEditText("int", "7", MISSING)).toEqual({ ok: true, value: { t: "int", v: 7 } });
    // Never a "missing" that holds text: the column's kind, as for a NULL.
    expect(parseEditText("other", "abc", MISSING)).toEqual({
      ok: true,
      value: { t: "other", v: { ty: "other", text: "abc" } },
    });
    expect(parseEditText("other", "65f0c0ffee0000000000002b", OID)).toEqual({
      ok: true,
      value: { t: "other", v: { ty: "objectId", text: "65f0c0ffee0000000000002b" } },
    });
  });
});

describe("elasticsearch: a partial RowResult.returned merged into the row", () => {
  const ref = { schema: null, name: "logs" };
  const id = col("_id", { isPk: true });
  const seq = col("_seq_no", { kind: "int" });
  const msg = col("message");
  const target: ResultEditTarget = {
    table: ref,
    meta: { table: ref, primaryKey: ["_id"] } as unknown as TableMeta,
    columns: [id, seq, msg],
    keyIndexes: [0],
  };
  const rows: Value[][] = [[text("a1"), { t: "int", v: 4 }, text("old")]];

  it("takes the columns the server returned and keeps the others as staged", () => {
    const s = setCell(emptyStaging(), bufferKey(0), 2, text("new"), rows[0]);
    const next = applyCommitted(rows, s, target, [
      {
        rowId: bufferKey(0),
        op: "update",
        affected: 1,
        returned: { _seq_no: { t: "int", v: 5 } },
        error: null,
      },
    ]);
    expect(next).toEqual([[text("a1"), { t: "int", v: 5 }, text("new")]]);
  });
});

describe("pg-flavors, clickhouse: distribution and sort key in the Structure tab", () => {
  it("labels a distributed table's options as its distribution, sort key and storage", () => {
    const options = {
      engine: "ao_column (compresstype=zstd)",
      charset: null,
      collation: null,
      autoIncrement: null,
      rowFormat: null,
      distribution: "DISTRIBUTED BY (id)",
    };
    expect(tableOptionRows(options, false)).toEqual([
      ["Distribution", "DISTRIBUTED BY (id)"],
      ["Storage", "ao_column (compresstype=zstd)"],
    ]);
    expect(
      tableOptionRows(
        { ...options, distribution: "DISTSTYLE KEY(id)", sortKey: "SORTKEY (ts, id)" },
        false,
      ),
    ).toEqual([
      ["Distribution", "DISTSTYLE KEY(id)"],
      ["Sort key", "SORTKEY (ts, id)"],
      ["Storage", "ao_column (compresstype=zstd)"],
    ]);
    // ClickHouse: the sorting key and the engine.
    expect(
      tableOptionRows(
        { ...options, distribution: null, sortKey: "tenant, ts", engine: "MergeTree" },
        false,
      ),
    ).toEqual([
      ["Sort key", "tenant, ts"],
      ["Storage", "MergeTree"],
    ]);
    // MySQL's options leave the new fields out.
    const mysql = { engine: "InnoDB", charset: null, collation: null, autoIncrement: null };
    expect(tableOptionRows({ ...mysql, rowFormat: "Dynamic" }, true)).toEqual([
      ["Engine", "InnoDB"],
      ["Row format", "Dynamic"],
    ]);
    expect(tableOptionRows(null, false)).toEqual([]);
  });
});

describe("pg-flavors: the CSV wizard's upsert follows DataCaps.upsert", () => {
  it("drops UPDATE ON DUPLICATE where the server has no upsert", () => {
    expect(insertModesFor("postgres", false, false)).toEqual(["stopOnError"]);
    expect(insertModesFor("mysql", false, false)).toEqual(["stopOnError", "replace"]);
    expect(insertModesFor("postgres", false)).toEqual(["stopOnError", "updateOnDuplicate"]);
  });
});

describe("partial applies (critique D2)", () => {
  it("are possible in every mode but the all-or-nothing ones", () => {
    expect(appliesPartially("atomic")).toBe(false);
    expect(appliesPartially("dataOnly")).toBe(false);
    expect(appliesPartially("batch")).toBe(true);
    expect(appliesPartially("sequential")).toBe(true);
    expect(appliesPartially("none")).toBe(true);
  });

  it("drop the applied changes and their history, keep the failed ones with their errors", () => {
    let s = setCell(emptyStaging(), "a", 0, text("x"), [text("o")]);
    s = setCell(s, "b", 0, text("y"), [text("o")]);
    s = setRowError(s, "b", "old error");
    const next = dropApplied(s, new Set(["a"]), new Map([["b", "conflict"]]));
    expect([...next.changes.keys()]).toEqual(["b"]);
    expect(next.changes.get("b")!.error).toBe("conflict");
    expect(next.undo).toEqual([]);
    expect(next.redo).toEqual([]);
  });
});

describe("features ANDed into the menus", () => {
  const noop = () => {};
  const handlers = {
    newItem: noop,
    openInNewTab: noop,
    openStructure: noop,
    itemOverview: noop,
    copyName: noop,
    togglePin: noop,
    exportItems: noop,
    importCsv: noop,
    importJson: noop,
    importSql: noop,
    clone: noop,
    rename: noop,
    truncate: noop,
    drop: noop,
    copyScript: noop,
    generateDiagram: noop,
  };
  const options = {
    driver: "postgres" as const,
    readOnly: false,
    count: 1,
    truncatable: 1,
    droppable: 1,
    isTable: true,
    renamable: true,
    allPinned: null,
  };
  const ids = (entries: MenuEntry[]): string[] =>
    entries.flatMap((e) =>
      e.kind === "sub" ? [e.id, ...ids(e.items).map((i) => `${e.id}/${i}`)] : [e.id],
    );

  it("leave a core session's menus exactly as they were", () => {
    const all = tableMenu(options, handlers);
    expect(ids(tableMenu({ ...options, features: BASE_CAPS.postgres.features }, handlers))).toEqual(
      ids(all),
    );
    expect(withFeatures(all, BASE_CAPS.postgres.features)).toBe(all);
  });

  it("hide what the session lacks, and what writes core SQL for another engine", () => {
    const menu = ids(
      tableMenu(
        { ...options, features: ["structureView", "exportTables"], dialect: null },
        handlers,
      ),
    );
    // No import, New, Clone, Rename, Truncate, Delete, Copy Script As or diagram: nothing left
    // after Export, so no separator either.
    expect(menu).toEqual(["open-new-tab", "open-structure", "copy-name", "s1", "export"]);
  });

  // A Redshift session: PostgreSQL's driver, its own dialect, so no core one (critique A10).
  const REDSHIFT: Feature[] = [
    "editor",
    "structureView",
    "explain",
    "format",
    "editRows",
    "insertRows",
    "deleteRows",
    "serverExpressions",
    "exactCount",
    "tableDdl",
    "viewDefinition",
    "switchDatabase",
    "processList",
    "killProcess",
    "timezone",
    "startupCommands",
  ];
  const TEMPLATES = ["new-view", "new-materializedView", "new-function", "new-procedure"];
  const newOn = { newItem: noop, itemOverview: noop, generateDiagram: noop };
  const objectOn = { newItem: noop, open: noop, copyName: noop, copyScript: noop, drop: noop };
  const nodeOn = { ...newOn, copyName: noop, reload: noop, collapseAll: noop };
  const header = { driver: "postgres" as const, readOnly: false, overview: false };
  const object = { driver: "postgres" as const, readOnly: false, count: 1 };
  const node = { driver: "postgres" as const, readOnly: false, loaded: true, diagram: true };

  it("offer no core dialect's templates to a session without one, though its driver is core", () => {
    const rs = { features: REDSHIFT, dialect: null };
    const menus = [
      ids(tableMenu({ ...options, ...rs, groups: true }, handlers)),
      ids(headerMenu({ ...header, ...rs, groups: true }, newOn)),
      ids(objectMenu({ ...object, ...rs, groups: true }, objectOn)),
      ids(schemaNodeMenu({ ...node, ...rs, groups: true }, nodeOn)),
      ids(plusMenu("postgres", false, [], newOn, undefined, REDSHIFT, null)),
    ];
    for (const menu of menus)
      for (const t of TEMPLATES) expect(menu.filter((id) => id.endsWith(t))).toEqual([]);
    // No CREATE TABLE or schema either (no such features): New ▸ is Group… alone, or nothing.
    expect(ids(headerMenu({ ...header, ...rs, groups: true }, newOn))).toEqual([
      "new",
      "new/new-group",
    ]);
    expect(ids(headerMenu({ ...header, ...rs }, newOn))).toEqual([]);
    expect(ids(plusMenu("postgres", false, [], newOn, undefined, REDSHIFT, null))).toEqual([
      "new-group",
      "new-query",
      "s-settings",
      "settings",
    ]);
    // Item Overview writes core SQL too, and the session has no diagram.
    expect(ids(schemaNodeMenu({ ...node, ...rs }, nodeOn))).toEqual([
      "copy-name",
      "s1",
      "reload",
      "collapse-all",
    ]);
  });

  it("keep a core session's menus, and PostgreSQL's templates, exactly as they were", () => {
    for (const driver of ["postgres", "mysql", "sqlite"] as const) {
      const core = { features: BASE_CAPS[driver].features, dialect: driver };
      expect(ids(tableMenu({ ...options, driver, ...core }, handlers))).toEqual(
        ids(tableMenu({ ...options, driver }, handlers)),
      );
      expect(ids(headerMenu({ ...header, driver, overview: true, ...core }, newOn))).toEqual(
        ids(headerMenu({ ...header, driver, overview: true }, newOn)),
      );
      expect(ids(objectMenu({ ...object, driver, ...core }, objectOn))).toEqual(
        ids(objectMenu({ ...object, driver }, objectOn)),
      );
      expect(ids(schemaNodeMenu({ ...node, driver, ...core }, nodeOn))).toEqual(
        ids(schemaNodeMenu({ ...node, driver }, nodeOn)),
      );
      expect(
        ids(plusMenu(driver, false, [], newOn, undefined, core.features, core.dialect)),
      ).toEqual(ids(plusMenu(driver, false, [], newOn)));
    }
    expect(ids(plusMenu("postgres", false, [], newOn))).toEqual([
      "new-table",
      ...TEMPLATES,
      "new-schema",
      "new-group",
      "new-query",
      "s-settings",
      "settings",
    ]);
  });
});

describe("the filter bar's operators", () => {
  it("are a session's own, JSON containment on json columns only", () => {
    const offered = ["eq", "ne", "jsonContains"] as const;
    expect(opsFor(offered, { kind: "text" }).map((o) => o.op)).toEqual(["eq", "ne"]);
    expect(opsFor(offered, { kind: "json" }).map((o) => o.op)).toEqual([
      "eq",
      "ne",
      "jsonContains",
    ]);
    // A core dialect's, as before.
    expect(opsFor("mysql", { kind: "text" }).map((o) => o.op)).toEqual(
      opsFor(BASE_CAPS.mysql.data.filterOps, { kind: "text" }).map((o) => o.op),
    );
    expect(opsFor("postgres", { kind: "json" }).map((o) => o.op)).toEqual(
      opsFor(BASE_CAPS.postgres.data.filterOps, { kind: "json" }).map((o) => o.op),
    );
  });

  it("start a new row from what the session offers", () => {
    const columns = [col("_id", { isPk: true })];
    const settings = { column: "rawSql", operator: "contains" } as const;
    expect(defaultFilterRow(columns, settings, CORE_FILTERS)).toMatchObject({
      column: "__raw_sql__",
      op: "contains",
    });
    const session = { ops: ["eq", "ne"] as const, raw: null, anyColumn: false };
    expect(defaultFilterRow(columns, settings, session)).toMatchObject({ column: "_id", op: "eq" });
    expect(filtersOffered(session)).toBe(true);
    expect(filtersOffered({ ops: [], raw: null, anyColumn: false })).toBe(false);
  });
});

describe("openers of features a session lacks", () => {
  const info = (over: Partial<ConnectionInfo>): ConnectionInfo => ({
    connectionId: "c1",
    profileId: "p1",
    name: "Keys",
    color: null,
    driver: "redis",
    serverVersion: "7",
    currentDatabase: null,
    currentUser: null,
    tunnel: null,
    readOnly: false,
    capabilities: BASE_CAPS.redis,
    ...over,
  });

  it("refuse with a toast, and let a core session and an unknown connection through", () => {
    useConnectionsStore.getState().add(info({}));
    expect(allowFeature("c1", "exportTables")).toBe(false);
    expect(toast.error).toHaveBeenCalledWith(
      "Exporting tables is not available for Redis connections",
    );
    expect(allowFeature("c1", "structureView")).toBe(true);
    expect(allowFeature("nope", "exportTables")).toBe(true);
    useConnectionsStore
      .getState()
      .add(info({ connectionId: "c2", driver: "postgres", capabilities: null }));
    expect(allowFeature("c2", "exportTables", "backup")).toBe(true);
  });

  it("read the session's dialect from its caps (Redshift is not PostgreSQL)", () => {
    expect(connectionDialect(info({ driver: "postgres", capabilities: null }))).toBe("postgres");
    expect(
      connectionDialect(
        info({ driver: "postgres", capabilities: { ...BASE_CAPS.postgres, dialect: "redshift" } }),
      ),
    ).toBeNull();
    expect(connectionDialect(info({}))).toBeNull();
  });

  /** A Redshift session (PostgreSQL's driver, Redshift's dialect), with PostgreSQL's features. */
  const redshift = (connectionId: string, without: Feature[] = []) =>
    info({
      connectionId,
      driver: "postgres",
      capabilities: {
        ...BASE_CAPS.postgres,
        dialect: "redshift",
        features: BASE_CAPS.postgres.features.filter((f) => !without.includes(f)),
      },
    });

  it("read a connection's dialect by its id: the session's, the driver's while closed", () => {
    useConnectionsStore.getState().add(redshift("rs"));
    expect(dialectFor("rs", "postgres")).toBeNull();
    expect(dialectFor("closed", "postgres")).toBe("postgres");
    expect(dialectFor(null, "mysql")).toBe("mysql");
    expect(dialectFor(null, "redis")).toBeNull();
  });

  it("write no core dialect's SQL for a Redshift session (critique A10)", async () => {
    useConnectionsStore.getState().add(redshift("rs", ["dummyData"]));
    useConnectionsStore.getState().add(redshift("rs2"));
    const tabs = useTabsStore.getState().tabs.length;
    const ctx = {
      connectionId: "rs",
      driver: "postgres" as const,
      readOnly: false,
      schema: "public",
      pinScope: null,
    };
    const notFor = (what: string) => `${what} is not available for PostgreSQL connections`;
    // New ▸ View… / Function… / Table…: no PostgreSQL template or dialog opens.
    runNew(ctx, "view");
    expect(toast.error).toHaveBeenCalledWith(notFor("Creating a view"));
    expect(() => openNewObject(ctx, "function")).toThrow(notFor("Creating a function"));
    runNew(ctx, "table");
    expect(toast.error).toHaveBeenCalledWith(notFor("Creating a table"));
    expect(useTabsStore.getState().tabs).toHaveLength(tabs);
    expect(useObjectsStore.getState().dialog).toBeNull();
    // Dummy Data: refused without the feature, and without a core dialect for its INSERTs.
    const table = { schema: "public", name: "users", type: "table" as const };
    openDummyData({ connectionId: "rs", table });
    openDummyData({ connectionId: "rs2", table });
    expect(toast.error).toHaveBeenCalledTimes(4);
    expect(toast.error).toHaveBeenLastCalledWith(notFor("Dummy Data"));
    expect(usePluginsStore.getState().dummy).toBeNull();
    // Copy as UPDATE Statement says so, instead of copying PostgreSQL's UPDATEs.
    const row = {
      columns: buildGridColumns([col("id", { isPk: true }), col("name")]),
      cells: [
        { t: "int", v: 1 },
        { t: "text", v: "a" },
      ] as Value[],
    };
    const host = { notify: vi.fn(), insertToClipboard: vi.fn() };
    await COPY_AS_UPDATE.commands[0]!.run(
      {
        source: "grid",
        connectionId: "rs",
        driver: "postgres",
        item: table,
        clickedRow: row,
        clickedColumn: 0,
        selectedRows: [],
        editorTabId: null,
        sql: null,
      },
      host as unknown as HostServices,
    );
    expect(host.notify).toHaveBeenCalledWith("Copy as UPDATE Statement", notFor("Copy as UPDATE"));
    expect(host.insertToClipboard).not.toHaveBeenCalled();
  });
});
