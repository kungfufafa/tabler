import { describe, expect, it } from "vitest";
import { BASE_CAPS } from "@/lib/caps";
import type { ColumnMeta, FilterGroup, Value } from "@/lib/ipc";
import {
  browsePageOf,
  buildExportRequest,
  buildExportResultsRequest,
  columnChanges,
  CSV_QUOTES,
  csvProblem,
  currentPageSql,
  DEFAULT_EXPORT_OPTIONS,
  decimalProblem,
  defaultFileName,
  ENCODINGS,
  encodingApplies,
  encodingForCharset,
  EXPORT_OPTIONS_KEY,
  buildExportTablesRequest,
  exportProblem,
  exportsQueries,
  fileFilter,
  filterTables,
  FORMATS,
  formatsFor,
  multiLayout,
  formatBytes,
  formatElapsed,
  hasCappedBytes,
  loadExportOptions,
  parseExportOptions,
  resultsLayout,
  resultsProblem,
  safeFileBase,
  saveExportOptions,
  toExportFormat,
  UNTITLED_TABLE,
  visibleColumnNames,
  withOfferedFormat,
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
    page: { limit: 200, offset: 0 },
    ...patch,
  };
}

const build = (input: ExportInput, options = opts(), sqlTableName = "") =>
  buildExportRequest({ exportId: "e1", path: "/tmp/out", input, options, sqlTableName });

describe("formatsFor", () => {
  it("offers a SQL script only to a session with SQL export", () => {
    const core = BASE_CAPS.postgres;
    expect(formatsFor(core)).toBe(FORMATS);
    expect(formatsFor(null)).toBe(FORMATS);
    // A Redshift session: its tables export, as no SQL script.
    const redshift = {
      ...core,
      dialect: "redshift" as const,
      features: core.features.filter((f) => f !== "exportSql"),
    };
    const offered = formatsFor(redshift);
    expect(offered.map((f) => f.value)).toEqual(["csv", "json", "xlsx", "html"]);
    const script = opts({ format: "sqlInsert" });
    expect(withOfferedFormat(script, offered).format).toBe("csv");
    expect(withOfferedFormat(script, FORMATS)).toBe(script);
    const json = opts({ format: "json" });
    expect(withOfferedFormat(json, offered)).toBe(json);
  });
});

describe("exportsQueries", () => {
  it("runs a query for the file on a core session only", () => {
    expect(exportsQueries(BASE_CAPS.postgres)).toBe(true);
    expect(exportsQueries(BASE_CAPS.sqlite)).toBe(true);
    expect(exportsQueries(null)).toBe(true);
    // Redshift: PostgreSQL's driver, its own dialect, read-only by the classifier alone.
    expect(exportsQueries({ ...BASE_CAPS.postgres, dialect: "redshift" })).toBe(false);
    expect(exportsQueries(BASE_CAPS.libsql)).toBe(false);
  });
});

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
    expect(req.source).toMatchObject({ kind: "table", offset: 0 });
    expect(req.maxRows).toBe(200);
  });

  it("exports a later page as a table query that starts at the page's offset", () => {
    const req = build(
      tableInput({ scope: "page", columns: ["id", "email"], page: { limit: 200, offset: 400 } }),
    );
    // Still a table source: the filter, the sort and the column choice all apply.
    expect(req.source).toEqual({
      kind: "table",
      table: { schema: "public", name: "users" },
      filter,
      sort: [{ column: "name", dir: "asc", nulls: null }],
      columns: ["id", "email"],
      offset: 400,
    });
    expect(req.maxRows).toBe(200);
    // Rows of every page are all rows from the first: no offset.
    expect(
      (build(tableInput({ page: { limit: 200, offset: 400 } })).source as { offset?: number })
        .offset,
    ).toBeUndefined();
  });

  it("exports a later page with hidden columns and without generated ones", () => {
    const hidden = tableInput({
      scope: "page",
      columns: ["id", "email"],
      generated: ["total"],
      page: { limit: 200, offset: 200 },
    });
    // No problem on any page: the offset is part of the table query.
    for (const format of ["csv", "json", "xlsx", "html", "sqlInsert"] as const) {
      expect(exportProblem(hidden, format)).toBeNull();
    }
    expect(build(hidden).source).toMatchObject({ columns: ["id", "email"], offset: 200 });
    // An INSERT export of a later page leaves generated columns out like on the first page.
    const insert = tableInput({
      scope: "page",
      generated: ["total"],
      page: { limit: 200, offset: 200 },
    });
    expect(build(insert, opts({ format: "sqlInsert" })).source).toMatchObject({
      kind: "table",
      columns: null,
      offset: 200,
    });
    const subset = tableInput({
      scope: "page",
      columns: ["id", "total"],
      generated: ["total"],
      page: { limit: 200, offset: 200 },
    });
    expect(build(subset, opts({ format: "sqlInsert" })).source).toMatchObject({
      columns: ["id"],
      offset: 200,
    });
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
  });
});

describe("visibleColumnNames", () => {
  const cols = ["id", "name", "email", "bio"].map((name) => ({ name }) as ColumnMeta);

  it("is null when nothing is hidden and the order is the table's", () => {
    expect(visibleColumnNames(cols, { order: [], hidden: [] })).toBeNull();
    expect(
      visibleColumnNames(cols, { order: ["id", "name", "email", "bio"], hidden: [] }),
    ).toBeNull();
    expect(visibleColumnNames(cols, { order: [], hidden: ["gone"] })).toBeNull();
  });

  it("follows the grid order when columns were only dragged, nothing hidden", () => {
    expect(visibleColumnNames(cols, { order: ["email"], hidden: [] })).toEqual([
      "email",
      "id",
      "name",
      "bio",
    ]);
    expect(columnChanges(cols, { order: ["email"], hidden: [] })).toEqual({
      hidden: false,
      reordered: true,
    });
    expect(columnChanges(cols, { order: [], hidden: [] })).toEqual({
      hidden: false,
      reordered: false,
    });
  });

  it("follows the grid order and drops hidden columns", () => {
    expect(visibleColumnNames(cols, { order: ["email", "id"], hidden: ["bio"] })).toEqual([
      "email",
      "id",
      "name",
    ]);
    expect(columnChanges(cols, { order: ["email", "id"], hidden: ["bio"] })).toEqual({
      hidden: true,
      reordered: true,
    });
    // Hiding alone is not a reorder.
    expect(visibleColumnNames(cols, { order: [], hidden: ["bio"] })).toEqual([
      "id",
      "name",
      "email",
    ]);
    expect(columnChanges(cols, { order: [], hidden: ["bio"] })).toEqual({
      hidden: true,
      reordered: false,
    });
  });
});

describe("CSV Space quote and custom decimal", () => {
  it("offers the Space quote between the quotes and Quote if needed", () => {
    expect(CSV_QUOTES.map((q) => q.value)).toEqual(["double", "single", "space", "ifNeeded"]);
    const format = toExportFormat(
      opts({ csv: { ...DEFAULT_EXPORT_OPTIONS.csv, quote: "space" } }),
      "",
    );
    expect(format).toMatchObject({ format: "csv", quote: "space" });
  });

  it("sends a custom decimal separator and refuses one the backend would", () => {
    const hash = opts({ csv: { ...DEFAULT_EXPORT_OPTIONS.csv, delimiter: ";", decimal: "#" } });
    expect(toExportFormat(hash, "")).toMatchObject({ format: "csv", decimal: "#", delimiter: ";" });
    expect(build(tableInput(), hash).format).toMatchObject({ decimal: "#" });
    expect(csvProblem(hash.csv)).toBeNull();
    const bad = (decimal: string, delimiter: ExportOptions["csv"]["delimiter"] = ";") =>
      csvProblem({ ...DEFAULT_EXPORT_OPTIONS.csv, delimiter, decimal });
    expect(bad("")).toMatch(/Type the decimal separator/);
    expect(bad("5")).toMatch(/not a digit or a quote/);
    expect(bad('"')).toMatch(/not a digit or a quote/);
    expect(bad("ab")).toMatch(/not a digit or a quote/);
    expect(bad(";")).toMatch(/differ from the delimiter/);
    expect(bad(";", "\t")).toBeNull();
    // `,` is the classic choice: numbers holding it are quoted, also with a comma delimiter.
    expect(bad(",", ",")).toBeNull();
    expect(bad(".", ",")).toBeNull();
    expect(decimalProblem("#", ",")).toBeNull();
    expect(() => build(tableInput(), opts({ csv: { ...hash.csv, decimal: "7" } }))).toThrow(
      /decimal separator/,
    );
    // Only CSV has them.
    expect(
      build(tableInput(), opts({ format: "json", csv: { ...hash.csv, decimal: "7" } })).format,
    ).toMatchObject({ format: "json" });
  });

  it("remembers a custom decimal and drops one that clashes with the delimiter", () => {
    const semicolons = { ...DEFAULT_CSV_FIELDS, delimiter: ";" } as const;
    const keep = parseExportOptions(
      { csv: { decimal: "#", delimiter: ";" }, csvDefaults: semicolons },
      semicolons,
    );
    // The CSV default options say what they said when the options were saved: the remembered `#`
    // wins.
    expect(keep.csv.decimal).toBe("#");
    expect(parseExportOptions({ csv: { decimal: "7" } }).csv.decimal).toBe(".");
    expect(parseExportOptions({ csv: { quote: "space" } }).csv.quote).toBe("space");
    // The preference decides when it changed since: a custom character comes through as it is.
    expect(
      parseExportOptions(
        { csv: { decimal: "." }, csvDefaults: DEFAULT_CSV_FIELDS },
        { ...DEFAULT_CSV_FIELDS, decimal: "#" },
      ).csv.decimal,
    ).toBe("#");
    // …unless it equals the delimiter: it could not be told apart.
    expect(
      parseExportOptions(
        { csv: {}, csvDefaults: DEFAULT_CSV_FIELDS },
        { ...DEFAULT_CSV_FIELDS, delimiter: ";", decimal: ";" },
      ).csv.decimal,
    ).toBe(".");
  });
});

const DEFAULT_CSV_FIELDS = {
  delimiter: ",",
  quote: "ifNeeded",
  lineBreak: "\n",
  decimal: ".",
} as const;

describe("a result or query of an unknown table", () => {
  it("inserts into untitled without a name, and into the name typed", () => {
    const query: ExportInput = { kind: "sql", sql: "select 1" };
    const sqlOptions = opts({ format: "sqlInsert" });
    expect(build(query, sqlOptions).format).toMatchObject({ tableName: UNTITLED_TABLE });
    expect(build(query, sqlOptions, " archive ").format).toMatchObject({ tableName: "archive" });
    const join: ExportInput = {
      kind: "rows",
      columns: [{ name: "n", dataType: "int4", kind: "int", nullable: null, table: null }],
      rows: [[{ t: "int", v: 1 }]],
      table: null,
    };
    expect(exportProblem(join, "sqlInsert", sqlOptions.sql)).toBeNull();
    expect(build(join, sqlOptions).format).toMatchObject({ tableName: "untitled" });
    expect(build(join, sqlOptions, "archive").format).toMatchObject({ tableName: "archive" });
    // A known table stays the backend's default (no name sent).
    expect(build({ ...join, table: "users" }, sqlOptions).format).toMatchObject({ tableName: "" });
  });
});

describe("the encoding of the connection (View Using Encoding)", () => {
  it("maps MySQL and PostgreSQL charset names to the export's encodings", () => {
    for (const name of ["latin1", "LATIN1", "iso-8859-1", "ISO_8859_1", " latin1 "])
      expect(encodingForCharset(name)).toBe("latin1");
    for (const name of ["cp1252", "windows-1252", "WIN1252", "Windows1252"])
      expect(encodingForCharset(name)).toBe("windows1252");
    for (const name of ["utf16", "UTF16", "ucs2", "UTF-16LE"])
      expect(encodingForCharset(name)).toBe("utf16Le");
    // UTF-8 keeps the remembered choice, and so does a charset the export has no encoding for.
    for (const name of ["utf8", "utf8mb4", "UTF8", "utf8mb3", "UTF-8", "sjis", "EUC_JP", "", null])
      expect(encodingForCharset(name)).toBeNull();
    expect(encodingForCharset(undefined)).toBeNull();
    // Every mapped encoding is one the dialog offers.
    for (const name of ["latin1", "cp1252", "utf16"]) {
      const mapped = encodingForCharset(name);
      expect(ENCODINGS.some((e) => e.value === mapped)).toBe(true);
    }
  });
});

describe("the results of a run", () => {
  const col = (name: string) => ({
    name,
    dataType: "int4",
    kind: "int" as const,
    nullable: null,
    table: null,
  });
  const items = [
    {
      label: "Result 1",
      columns: [col("id")],
      rows: [[{ t: "int" as const, v: 1 }]],
      table: "users",
    },
    { label: "Result 2", columns: [col("n")], rows: [], table: null },
  ];
  const buildAll = (options = opts(), target = "/tmp/all.xlsx") =>
    buildExportResultsRequest({ exportId: "r1", items, options, target, name: "results" });

  it("writes one file where the format holds several results, else a file per result", () => {
    expect(resultsLayout("xlsx")).toBe("singleFile");
    expect(resultsLayout("html")).toBe("singleFile");
    expect(resultsLayout("sqlInsert")).toBe("singleFile");
    expect(resultsLayout("csv")).toBe("filePerTable");
    expect(resultsLayout("json")).toBe("filePerTable");
  });

  it("builds the export_results request with every result and its table", () => {
    const req = buildAll(opts({ format: "xlsx" }));
    expect(req).toMatchObject({
      exportId: "r1",
      layout: "singleFile",
      target: "/tmp/all.xlsx",
      format: { format: "xlsx", header: true },
      encoding: "utf8",
      name: "results",
    });
    expect(req.results.map((r) => [r.label, r.table, r.rows.length])).toEqual([
      ["Result 1", "users", 1],
      ["Result 2", null, 0],
    ]);
    expect(buildAll(opts(), "/tmp/out").layout).toBe("filePerTable");
    // INSERTs name each result's own table: no name in the format.
    expect(buildAll(opts({ format: "sqlInsert" })).format).toMatchObject({ tableName: "" });
    // The encoding of a text format.
    expect(buildAll(opts({ format: "html", encoding: "latin1" })).encoding).toBe("latin1");
  });

  it("refuses what the backend would: no content, a cut binary value, a bad CSV choice", () => {
    const none = sqlOpts({ content: false, structure: true });
    expect(resultsProblem(items, "sqlInsert", none)).toMatchObject({ field: "options" });
    expect(() => buildAll(opts({ format: "sqlInsert", sql: none }))).toThrow(/no structure/);
    const capped = [
      ...items,
      {
        label: "Result 3",
        columns: [col("b")],
        rows: [[{ t: "bytes" as const, v: { base64: "", len: 70_000, truncated: true } }]],
        table: null,
      },
    ];
    expect(resultsProblem(capped, "csv")).toMatchObject({
      field: "rows",
      message: expect.stringContaining("Result 3"),
    });
    expect(resultsProblem(items, "csv")).toBeNull();
    expect(() =>
      buildExportResultsRequest({
        exportId: "r",
        items,
        options: opts({ csv: { ...DEFAULT_EXPORT_OPTIONS.csv, decimal: "5" } }),
        target: "/tmp/x",
      }),
    ).toThrow(/decimal/);
    expect(() =>
      buildExportResultsRequest({ exportId: "r", items: [], options: opts(), target: "/tmp/x" }),
    ).toThrow(/no results/);
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

describe("HTML and encodings", () => {
  const input: ExportInput = { kind: "sql", sql: "select 1" };

  it("maps HTML and sends the encoding of text formats only", () => {
    const req = build(input, opts({ format: "html", encoding: "windows1252" }));
    expect(req.format).toEqual({ format: "html" });
    expect(req.encoding).toBe("windows1252");
    expect(build(input, opts({ format: "csv", encoding: "utf16Le" })).encoding).toBe("utf16Le");
    // A workbook is zipped UTF-8 XML: never transcoded.
    expect(build(input, opts({ format: "xlsx", encoding: "latin1" })).encoding).toBe("utf8");
    expect(encodingApplies("xlsx")).toBe(false);
    expect(encodingApplies("sqlInsert")).toBe(true);
    expect(ENCODINGS.map((e) => e.value)).toEqual([
      "utf8",
      "utf8Bom",
      "utf16Le",
      "windows1252",
      "latin1",
    ]);
  });

  it("names HTML files and lays several tables out in one page unless asked", () => {
    const at = new Date(2026, 9, 11, 9, 5);
    expect(defaultFileName("t", opts({ format: "html" }), at)).toBe("t_20261011-0905.html");
    expect(fileFilter(opts({ format: "html" }))).toEqual({
      name: "HTML",
      extensions: ["html", "htm"],
    });
    expect(multiLayout(opts({ format: "html" }))).toBe("singleFile");
    expect(
      multiLayout(
        opts({ format: "html", multi: { ...DEFAULT_EXPORT_OPTIONS.multi, htmlSingleFile: false } }),
      ),
    ).toBe("filePerTable");
  });

  it("remembers the encoding and the HTML layout; unknown values fall back", () => {
    const parsed = parseExportOptions({
      format: "html",
      encoding: "utf8Bom",
      multi: { htmlSingleFile: false },
    });
    expect(parsed.format).toBe("html");
    expect(parsed.encoding).toBe("utf8Bom");
    expect(parsed.multi.htmlSingleFile).toBe(false);
    expect(parseExportOptions({ encoding: "ebcdic" }).encoding).toBe("utf8");
  });

  it("names another database and asks for its folder to be created", () => {
    const req = buildExportTablesRequest({
      exportId: "m",
      tables: [{ schema: "public", name: "orders" }],
      options: opts({ format: "csv", encoding: "utf8Bom" }),
      target: "/tmp/x/shop",
      database: "shop",
      createFolder: true,
    });
    expect(req).toMatchObject({
      database: "shop",
      createFolder: true,
      encoding: "utf8Bom",
      layout: "filePerTable",
    });
    const plain = buildExportTablesRequest({
      exportId: "m",
      tables: [{ schema: "public", name: "orders" }],
      options: opts(),
      target: "/tmp/x",
    });
    expect(plain).toMatchObject({ database: null, createFolder: false, encoding: "utf8" });
  });
});
