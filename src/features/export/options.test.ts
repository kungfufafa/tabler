import { describe, expect, it } from "vitest";
import type { ColumnMeta, FilterGroup, Value } from "@/lib/ipc";
import {
  browsePageOf,
  buildExportRequest,
  canExportPage,
  currentPageSql,
  DEFAULT_EXPORT_OPTIONS,
  defaultFileName,
  EXPORT_OPTIONS_KEY,
  buildExportTablesRequest,
  exportProblem,
  fileFilter,
  filterTables,
  multiLayout,
  formatBytes,
  formatElapsed,
  hasCappedBytes,
  loadExportOptions,
  parseExportOptions,
  safeFileBase,
  saveExportOptions,
  stripRowidAlias,
  visibleColumnNames,
  type ExportInput,
  type ExportOptions,
} from "./options";

const opts = (patch: Partial<ExportOptions> = {}): ExportOptions => ({
  ...DEFAULT_EXPORT_OPTIONS,
  csv: { ...DEFAULT_EXPORT_OPTIONS.csv },
  json: { ...DEFAULT_EXPORT_OPTIONS.json },
  sql: { ...DEFAULT_EXPORT_OPTIONS.sql },
  xlsx: { ...DEFAULT_EXPORT_OPTIONS.xlsx },
  multi: { ...DEFAULT_EXPORT_OPTIONS.multi },
  ...patch,
});

const sqlOpts = (patch: Partial<ExportOptions["sql"]>): ExportOptions["sql"] => ({
  ...DEFAULT_EXPORT_OPTIONS.sql,
  ...patch,
});

const filter: FilterGroup = {
  conjunction: "and",
  items: [{ kind: "condition", column: "id", op: "gt", values: [{ t: "int", v: 10 }] }],
};

function tableInput(patch: Partial<Extract<ExportInput, { kind: "table" }>> = {}): ExportInput {
  return {
    kind: "table",
    table: { schema: "public", name: "users" },
    filter,
    sort: [{ column: "name", dir: "asc", nulls: null }],
    columns: null,
    generated: [],
    scope: "all",
    page: { limit: 200, offset: 0, sql: 'SELECT "id" FROM "users" LIMIT 200 OFFSET 0' },
    ...patch,
  };
}

const build = (input: ExportInput, options = opts(), sqlTableName = "") =>
  buildExportRequest({ exportId: "e1", path: "/tmp/out", input, options, sqlTableName });

describe("buildExportRequest", () => {
  it("maps CSV options, including a tab delimiter", () => {
    const req = build(
      tableInput(),
      opts({
        csv: {
          delimiter: "\t",
          header: false,
          quote: "single",
          nullToEmpty: false,
          lineBreakToSpace: true,
          lineBreak: "\r\n",
          decimal: ",",
        },
      }),
    );
    expect(req.format).toEqual({
      format: "csv",
      delimiter: "\t",
      header: false,
      quote: "single",
      nullAs: "NULL",
      lineBreak: "\r\n",
      decimal: ",",
      lineBreakToSpace: true,
    });
    expect(req.exportId).toBe("e1");
    expect(req.path).toBe("/tmp/out");
    // TablePlus's defaults: NULL as empty, quotes only when needed, \n records.
    expect(build(tableInput()).format).toEqual({
      format: "csv",
      delimiter: ",",
      header: true,
      quote: "ifNeeded",
      nullAs: "",
      lineBreak: "\n",
      decimal: ".",
      lineBreakToSpace: false,
    });
  });

  it("maps Excel and the SQL script options", () => {
    expect(build(tableInput(), opts({ format: "xlsx", xlsx: { header: false } })).format).toEqual({
      format: "xlsx",
      header: false,
    });
    expect(
      build(
        tableInput(),
        opts({
          format: "sqlInsert",
          sql: sqlOpts({ dropTable: true, structure: true, content: false, gzip: true }),
        }),
      ).format,
    ).toEqual({
      format: "sqlInsert",
      tableName: "",
      batchSize: 100,
      dropTable: true,
      structure: true,
      content: false,
      gzip: true,
    });
  });

  it("refuses a script with nothing in it, or a query without its content", () => {
    const none = opts({ format: "sqlInsert", sql: sqlOpts({ content: false }) });
    expect(exportProblem(tableInput(), "sqlInsert", none.sql)?.field).toBe("options");
    expect(() => build(tableInput(), none)).toThrow(/Nothing to export/);
    const structureOnly = sqlOpts({ content: false, structure: true });
    expect(exportProblem(tableInput(), "sqlInsert", structureOnly)).toBeNull();
    expect(exportProblem({ kind: "sql", sql: "select 1" }, "sqlInsert", structureOnly)?.field).toBe(
      "options",
    );
  });

  it("maps JSON array / NDJSON; NDJSON is never pretty", () => {
    expect(build(tableInput(), opts({ format: "json" })).format).toEqual({
      format: "json",
      ndjson: false,
      pretty: true,
    });
    expect(
      build(tableInput(), opts({ format: "json", json: { ndjson: true, pretty: true } })).format,
    ).toEqual({ format: "json", ndjson: true, pretty: false });
  });

  it("maps SQL INSERT with a trimmed table name and a clamped batch size", () => {
    expect(
      build(
        tableInput(),
        opts({ format: "sqlInsert", sql: sqlOpts({ batchSize: 250 }) }),
        "  archive  ",
      ).format,
    ).toMatchObject({ format: "sqlInsert", tableName: "archive", batchSize: 250 });
    expect(
      build(tableInput(), opts({ format: "sqlInsert", sql: sqlOpts({ batchSize: 0 }) })).format,
    ).toMatchObject({ format: "sqlInsert", tableName: "", batchSize: 1 });
    expect(
      build(tableInput(), opts({ format: "sqlInsert", sql: sqlOpts({ batchSize: 1e9 }) })).format,
    ).toMatchObject({ batchSize: 10_000 });
  });

  it("exports every row matching the filter with the tab's sort and columns", () => {
    const req = build(tableInput({ columns: ["id", "email"] }));
    expect(req.source).toEqual({
      kind: "table",
      table: { schema: "public", name: "users" },
      filter,
      sort: [{ column: "name", dir: "asc", nulls: null }],
      columns: ["id", "email"],
    });
    expect(req.maxRows).toBeNull();
  });

  it("exports the first page as the table query capped at the page size", () => {
    const req = build(tableInput({ scope: "page" }));
    expect(req.source.kind).toBe("table");
    expect(req.maxRows).toBe(200);
  });

  it("exports a later page by re-running the browse statement (without SQLite's rowid)", () => {
    const sql =
      'SELECT "id", "name", rowid AS "_tabler_rowid"\nFROM "main"."users"\nORDER BY rowid ASC\nLIMIT 200 OFFSET 400';
    const req = build(
      tableInput({ scope: "page", page: { limit: 200, offset: 400, sql } }),
      opts({ format: "sqlInsert" }),
    );
    expect(req.source).toEqual({
      kind: "sql",
      sql: 'SELECT "id", "name"\nFROM "main"."users"\nORDER BY rowid ASC\nLIMIT 200 OFFSET 400',
    });
    expect(req.maxRows).toBe(200);
    // A query source has no table to name the INSERTs after: the table's name is filled in.
    expect(req.format).toMatchObject({ format: "sqlInsert", tableName: "users" });
  });

  it("never exports a later page with hidden columns (the browse statement selects them all)", () => {
    const sql =
      'SELECT "id"::text AS "id", "password_hash"::text AS "password_hash"\nFROM "public"."users"\nLIMIT 200 OFFSET 200';
    const input = tableInput({
      scope: "page",
      columns: ["id", "email"],
      page: { limit: 200, offset: 200, sql },
    });
    expect(exportProblem(input, "csv")).toMatchObject({
      field: "rows",
      message: expect.stringContaining("Only visible columns"),
    });
    expect(() => build(input)).toThrow(/Only visible columns/);
    // With every column chosen explicitly (unchecked), the page re-runs as before.
    const all = build(tableInput({ scope: "page", page: { limit: 200, offset: 200, sql } }));
    expect(all.source).toEqual({ kind: "sql", sql });
    // The first page stays a table export and keeps the column choice.
    const first = build(tableInput({ scope: "page", columns: ["id", "email"] }));
    expect(first.source).toMatchObject({ kind: "table", columns: ["id", "email"] });
  });

  it("leaves generated columns out of INSERT exports of a column subset", () => {
    const input = tableInput({ columns: ["id", "qty", "price", "total"], generated: ["total"] });
    expect(build(input, opts({ format: "sqlInsert" })).source).toMatchObject({
      kind: "table",
      columns: ["id", "qty", "price"],
    });
    // Other formats write what the grid shows.
    expect(build(input).source).toMatchObject({ columns: ["id", "qty", "price", "total"] });
    // Every column (null): the backend skips generated columns itself.
    expect(
      build(tableInput({ generated: ["total"] }), opts({ format: "sqlInsert" })).source,
    ).toMatchObject({ kind: "table", columns: null });
  });

  it("refuses INSERT exports with nothing insertable", () => {
    const onlyGenerated = tableInput({ columns: ["total"], generated: ["total"] });
    expect(exportProblem(onlyGenerated, "sqlInsert")).toMatchObject({ field: "columns" });
    expect(exportProblem(onlyGenerated, "json")).toBeNull();
    // A later page re-runs the browse statement, which can't drop generated columns.
    const laterPage = tableInput({
      scope: "page",
      generated: ["total"],
      page: { limit: 200, offset: 200, sql: "SELECT 1 LIMIT 200 OFFSET 200" },
    });
    expect(exportProblem(laterPage, "sqlInsert")).toMatchObject({ field: "rows" });
    expect(exportProblem(laterPage, "csv")).toBeNull();
    expect(() => build(laterPage, opts({ format: "sqlInsert" }))).toThrow();
  });

  it("passes SQL sources through unchanged", () => {
    const req = build({ kind: "sql", sql: "SELECT 1" }, opts({ format: "json" }));
    expect(req.source).toEqual({ kind: "sql", sql: "SELECT 1" });
    expect(req.maxRows).toBeNull();
  });

  it("sends the rows of a result as they are, INSERTs defaulting to its table", () => {
    const input: ExportInput = {
      kind: "rows",
      columns: [{ name: "n", dataType: "int4", kind: "int", nullable: null, table: "t" }],
      rows: [[{ t: "int", v: 1 }], [{ t: "null" }]],
      table: "t",
    };
    const req = build(input, opts({ format: "sqlInsert" }));
    expect(req.source).toEqual({
      kind: "rows",
      columns: input.columns,
      rows: input.rows,
      table: "t",
    });
    expect(req.format).toMatchObject({ format: "sqlInsert", tableName: "", batchSize: 100 });
    expect(req.maxRows).toBeNull();
    expect(exportProblem(input, "sqlInsert")).toBeNull();
    expect(build(input, opts({ format: "sqlInsert" }), " archive ").format).toMatchObject({
      tableName: "archive",
    });
  });
});

describe("rows with cut binary values", () => {
  const capped: Value = { t: "bytes", v: { base64: "AA==", len: 70_000, truncated: true } };
  const full: Value = { t: "bytes", v: { base64: "AA==", len: 1, truncated: false } };

  it("finds a binary value cut to its preview, also inside arrays", () => {
    expect(hasCappedBytes([[{ t: "int", v: 1 }, full]])).toBe(false);
    expect(hasCappedBytes([[{ t: "int", v: 1 }], [capped]])).toBe(true);
    expect(hasCappedBytes([[{ t: "array", v: [full, capped] }]])).toBe(true);
    expect(hasCappedBytes([])).toBe(false);
  });

  it("is a problem of the rows, not something to send to the backend", () => {
    const input: ExportInput = {
      kind: "rows",
      columns: [{ name: "b", dataType: "bytea", kind: "bytes", nullable: true, table: null }],
      rows: [[capped]],
      table: null,
    };
    expect(exportProblem(input, "csv")).toMatchObject({ field: "rows", severity: "fix" });
    expect(() =>
      buildExportRequest({
        exportId: "e",
        path: "/tmp/x.csv",
        input,
        options: opts(),
        sqlTableName: "",
      }),
    ).toThrow(/64 KiB/);
    expect(exportProblem({ ...input, rows: [[full]] }, "csv")).toBeNull();
  });
});

describe("page export helpers", () => {
  it("needs the browse statement only for later pages", () => {
    expect(canExportPage({ offset: 0, sql: null })).toBe(true);
    expect(canExportPage({ offset: 200, sql: null })).toBe(false);
    expect(canExportPage({ offset: 200, sql: "SELECT 1" })).toBe(true);
  });

  it("reads the page of a browse statement (backend and mock spellings)", () => {
    expect(browsePageOf('SELECT "a"\nFROM "t" ORDER BY "a" ASC\nLIMIT 200 OFFSET 400')).toEqual({
      limit: 200,
      offset: 400,
    });
    expect(browsePageOf('SELECT "a" FROM "t" LIMIT 20 OFFSET 0')).toEqual({ limit: 20, offset: 0 });
    expect(browsePageOf("SELECT 1")).toBeNull();
  });

  it("accepts the tab's statement only for the page on screen", () => {
    const page1 = 'SELECT "post_id" FROM "public"."post_tags" LIMIT 200 OFFSET 0';
    const page2 = 'SELECT "post_id" FROM "public"."post_tags" LIMIT 200 OFFSET 200';
    expect(currentPageSql({ limit: 200, offset: 200, sql: page2 })).toBe(page2);
    // Page 2 is still loading (or failed): the tab still holds page 1's statement.
    expect(currentPageSql({ limit: 200, offset: 200, sql: page1 })).toBeNull();
    // Page 3 → 2 while loading.
    expect(
      currentPageSql({ limit: 200, offset: 200, sql: page2.replace("OFFSET 200", "OFFSET 400") }),
    ).toBeNull();
    // The page size changed.
    expect(currentPageSql({ limit: 100, offset: 200, sql: page2 })).toBeNull();
    expect(currentPageSql({ limit: 200, offset: 200, sql: null })).toBeNull();
    expect(currentPageSql({ limit: 200, offset: 200, sql: "SELECT 1" })).toBeNull();
    // Not loaded = "wait for the page", never another page's rows.
    const stale = tableInput({ scope: "page", page: { limit: 200, offset: 200, sql: null } });
    expect(exportProblem(stale, "csv")).toMatchObject({ field: "rows", message: /Wait/ });
  });

  it("strips only the synthetic rowid column", () => {
    expect(stripRowidAlias('SELECT "a", rowid AS "_tabler_rowid" FROM t')).toBe(
      'SELECT "a" FROM t',
    );
    expect(stripRowidAlias('SELECT "rowid" FROM t')).toBe('SELECT "rowid" FROM t');
  });
});

describe("visibleColumnNames", () => {
  const cols = ["id", "name", "email", "bio"].map((name) => ({ name }) as ColumnMeta);

  it("is null when nothing is hidden", () => {
    expect(visibleColumnNames(cols, { order: ["email"], hidden: [] })).toBeNull();
    expect(visibleColumnNames(cols, { order: [], hidden: ["gone"] })).toBeNull();
  });

  it("follows the grid order and drops hidden columns", () => {
    expect(visibleColumnNames(cols, { order: ["email", "id"], hidden: ["bio"] })).toEqual([
      "email",
      "id",
      "name",
    ]);
  });
});

describe("file names", () => {
  const at = new Date(2026, 9, 11, 9, 5);

  it("uses table_YYYYMMDD-HHmm with the format's extension", () => {
    expect(defaultFileName("users", opts(), at)).toBe("users_20261011-0905.csv");
    expect(defaultFileName("users", opts({ format: "json" }), at)).toBe("users_20261011-0905.json");
    expect(defaultFileName("users", opts({ format: "sqlInsert" }), at)).toBe(
      "users_20261011-0905.sql",
    );
  });

  it("names tab-separated and NDJSON files by what they hold", () => {
    const tsv = opts({ csv: { ...DEFAULT_EXPORT_OPTIONS.csv, delimiter: "\t" } });
    expect(defaultFileName("users", tsv, at)).toBe("users_20261011-0905.tsv");
    expect(fileFilter(tsv).extensions).toContain("tsv");
    const nd = opts({ format: "json", json: { ndjson: true, pretty: false } });
    expect(defaultFileName("query", nd, at)).toBe("query_20261011-0905.ndjson");
  });

  it("makes table names safe for the file system", () => {
    expect(safeFileBase("Order Items")).toBe("Order_Items");
    expect(safeFileBase("a/b:c")).toBe("a_b_c");
    expect(safeFileBase("///")).toBe("export");
    expect(safeFileBase('q*?"<>|\u0000x')).toBe("q_x");
    expect(safeFileBase(".hidden")).toBe("hidden");
  });

  it("keeps accented and non-Latin table names readable", () => {
    expect(safeFileBase("pedidos_año")).toBe("pedidos_año");
    expect(safeFileBase("Größe")).toBe("Größe");
    expect(safeFileBase("usuários")).toBe("usuários");
    expect(safeFileBase("顧客")).toBe("顧客");
    expect(safeFileBase("заказы 2026")).toBe("заказы_2026");
    // Decomposed input is composed (NFC), not split around its accents.
    expect(safeFileBase("cafe\u0301")).toBe("caf\u00e9");
    expect(defaultFileName("顧客", opts(), at)).toBe("顧客_20261011-0905.csv");
  });
});

describe("persistence", () => {
  const DEFAULT_CSV = { delimiter: ",", quote: "ifNeeded", lineBreak: "\n", decimal: "." } as const;
  function memory(): Pick<Storage, "getItem" | "setItem"> & { data: Map<string, string> } {
    const data = new Map<string, string>();
    return {
      data,
      getItem: (k) => data.get(k) ?? null,
      setItem: (k, v) => void data.set(k, v),
    };
  }

  it("round-trips the last used options", () => {
    const storage = memory();
    const o = opts({ format: "json", json: { ndjson: true, pretty: false } });
    saveExportOptions(o, storage);
    expect(loadExportOptions(storage)).toEqual(o);
  });

  it("falls back to defaults for garbage, field by field", () => {
    expect(parseExportOptions("nope")).toEqual(DEFAULT_EXPORT_OPTIONS);
    expect(parseExportOptions({ format: "xml", csv: { delimiter: "::", header: false } })).toEqual({
      ...opts(),
      csv: { ...DEFAULT_EXPORT_OPTIONS.csv, header: false },
    });
  });

  it("keeps the meaning of options stored by earlier versions", () => {
    const old = parseExportOptions({
      format: "csv",
      csv: { delimiter: ";", header: true, quoteAll: true, nullAs: "NULL" },
      sql: { batchSize: 50 },
    });
    expect(old.csv).toMatchObject({ delimiter: ";", quote: "double", nullToEmpty: false });
    expect(old.sql).toEqual({ ...DEFAULT_EXPORT_OPTIONS.sql, batchSize: 50 });
    expect(parseExportOptions({ csv: { delimiter: "|" } }).csv.delimiter).toBe(",");
  });

  it("starts CSV from Settings ▸ CSV default options; remembered choices win until they change", () => {
    const storage = memory();
    const semi = { delimiter: ";", quote: "double", lineBreak: "\r\n", decimal: "," } as const;
    // First export: the preferences seed the four CSV fields.
    expect(loadExportOptions(storage, semi).csv).toMatchObject(semi);
    // The user picks other values in the dialog: they are remembered under the same preferences.
    const tab = opts({ csv: { ...DEFAULT_EXPORT_OPTIONS.csv, ...semi, delimiter: "\t" } });
    saveExportOptions(tab, storage, semi);
    expect(loadExportOptions(storage, semi)).toEqual(tab);
    // Changing the preference shows on the next export.
    const comma = { ...semi, delimiter: ",", decimal: "." } as const;
    expect(loadExportOptions(storage, comma).csv).toMatchObject(comma);
    // Options saved before the preference existed count as saved under the defaults.
    storage.data.set(EXPORT_OPTIONS_KEY, JSON.stringify({ csv: { delimiter: ";" } }));
    expect(loadExportOptions(storage, DEFAULT_CSV).csv.delimiter).toBe(";");
    expect(loadExportOptions(storage, { ...DEFAULT_CSV, delimiter: "\t" }).csv.delimiter).toBe(
      "\t",
    );
  });

  it("survives storage that throws", () => {
    const broken = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    };
    expect(loadExportOptions(broken)).toEqual(DEFAULT_EXPORT_OPTIONS);
    expect(() => saveExportOptions(opts(), broken)).not.toThrow();
  });
});

describe("formatting", () => {
  it("formats sizes and elapsed time", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(4.2 * 1024 * 1024)).toBe("4.2 MB");
    expect(formatElapsed(3_400)).toBe("0:03");
    expect(formatElapsed(3_723_000)).toBe("1:02:03");
  });
});

describe("several tables", () => {
  const tables = [
    { schema: "public", name: "users" },
    { schema: "public", name: "posts" },
  ];

  it("writes CSV and JSON per table, SQL and Excel as one file unless asked", () => {
    expect(multiLayout(opts())).toBe("filePerTable");
    expect(multiLayout(opts({ format: "json" }))).toBe("filePerTable");
    expect(multiLayout(opts({ format: "sqlInsert" }))).toBe("singleFile");
    expect(multiLayout(opts({ format: "xlsx" }))).toBe("singleFile");
    expect(
      multiLayout(
        opts({ format: "xlsx", multi: { ...DEFAULT_EXPORT_OPTIONS.multi, xlsxSingleFile: false } }),
      ),
    ).toBe("filePerTable");
  });

  it("builds the request with every table's own INSERT target", () => {
    const req = buildExportTablesRequest({
      exportId: "m",
      tables,
      options: opts({ format: "sqlInsert", sql: sqlOpts({ structure: true }) }),
      target: "/tmp/all.sql",
    });
    expect(req).toMatchObject({
      exportId: "m",
      tables,
      layout: "singleFile",
      target: "/tmp/all.sql",
      format: { format: "sqlInsert", tableName: "", structure: true, content: true },
    });
    expect(() =>
      buildExportTablesRequest({ exportId: "m", tables: [], options: opts(), target: "/tmp" }),
    ).toThrow();
  });

  it("filters the table list case-insensitively", () => {
    expect(filterTables(tables, "POS").map((t) => t.name)).toEqual(["posts"]);
    expect(filterTables(tables, " ")).toHaveLength(2);
  });

  it("names compressed scripts and workbooks", () => {
    expect(fileFilter(opts({ format: "sqlInsert", sql: sqlOpts({ gzip: true }) }))).toEqual({
      name: "Gzip-compressed SQL",
      extensions: ["gz"],
    });
    const at = new Date(2026, 9, 11, 9, 5);
    expect(defaultFileName("t", opts({ format: "xlsx" }), at)).toBe("t_20261011-0905.xlsx");
    expect(
      defaultFileName("t", opts({ format: "sqlInsert", sql: sqlOpts({ gzip: true }) }), at),
    ).toBe("t_20261011-0905.sql.gz");
  });
});
