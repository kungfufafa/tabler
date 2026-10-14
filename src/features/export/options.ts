import type {
  ColumnMeta,
  CsvQuote,
  ExportFormat,
  ExportLayout,
  ExportRequest,
  ExportTablesRequest,
  ExportSource,
  FilterGroup,
  ResultColumn,
  SortSpec,
  TableRef,
  Value,
} from "@/lib/ipc";
import { csvDefaults as csvDefaultsOfPrefs } from "@/features/datagrid/csv";

export type FormatKind = ExportFormat["format"];
export type CsvDelimiter = "," | "\t" | ";";
export type CsvLineBreak = "\n" | "\r\n" | "\r";
export type CsvDecimal = "." | ",";
/** Which rows of a table tab: everything matching its filter, or only the page on screen. */
export type RowScope = "all" | "page";

/** The user's choices in the export dialog (remembered between exports), as TablePlus names them. */
export interface ExportOptions {
  format: FormatKind;
  csv: {
    delimiter: CsvDelimiter;
    /** "Put field names in the first row". */
    header: boolean;
    quote: CsvQuote;
    /** "Convert NULL to EMPTY"; off writes `NULL`. */
    nullToEmpty: boolean;
    /** "Convert line break to space". */
    lineBreakToSpace: boolean;
    lineBreak: CsvLineBreak;
    decimal: CsvDecimal;
  };
  json: { ndjson: boolean; pretty: boolean };
  sql: {
    batchSize: number;
    dropTable: boolean;
    structure: boolean;
    content: boolean;
    gzip: boolean;
  };
  xlsx: { header: boolean };
  /** Several tables at once: one SQL script / one workbook, or a file per table. */
  multi: { sqlSingleFile: boolean; xlsxSingleFile: boolean };
}

const DEFAULTS: ExportOptions = {
  format: "csv",
  csv: {
    delimiter: ",",
    header: true,
    quote: "ifNeeded",
    nullToEmpty: true,
    lineBreakToSpace: false,
    lineBreak: "\n",
    decimal: ".",
  },
  json: { ndjson: false, pretty: true },
  sql: { batchSize: 100, dropTable: false, structure: false, content: true, gzip: false },
  xlsx: { header: true },
  multi: { sqlSingleFile: true, xlsxSingleFile: true },
};
export const DEFAULT_EXPORT_OPTIONS: Readonly<ExportOptions> = Object.freeze(DEFAULTS);

export const CSV_DELIMITERS: ReadonlyArray<{ value: CsvDelimiter; label: string; title: string }> =
  [
    { value: ",", label: ",", title: "Comma" },
    { value: "\t", label: "Tab", title: "Tab" },
    { value: ";", label: ";", title: "Semicolon" },
  ];

export const FORMATS: ReadonlyArray<{ value: FormatKind; label: string; title: string }> = [
  { value: "csv", label: "CSV", title: "Comma-separated values" },
  { value: "json", label: "JSON", title: "JSON array or newline-delimited JSON" },
  { value: "sqlInsert", label: "SQL", title: "A SQL script: structure and INSERT statements" },
  { value: "xlsx", label: "Excel", title: "Excel workbook (.xlsx)" },
];

export const CSV_QUOTES: ReadonlyArray<{ value: CsvQuote; label: string; title: string }> = [
  { value: "double", label: '"', title: "Put every value in double quotes" },
  { value: "single", label: "'", title: "Put every value in single quotes" },
  { value: "ifNeeded", label: "Quote if needed", title: "Quote only values that need it" },
];

export const CSV_LINE_BREAKS: ReadonlyArray<{ value: CsvLineBreak; label: string; title: string }> =
  [
    { value: "\n", label: "\\n", title: "LF (macOS, Linux)" },
    { value: "\r\n", label: "\\r\\n", title: "CRLF (Windows)" },
    { value: "\r", label: "\\r", title: "CR (classic Mac)" },
  ];

export const CSV_DECIMALS: ReadonlyArray<{ value: CsvDecimal; label: string; title: string }> = [
  { value: ".", label: ".", title: "Point: 12.5" },
  { value: ",", label: ",", title: "Comma: 12,5" },
];

export const BATCH_SIZE_RANGE = { min: 1, max: 10_000 } as const;

export function clampBatchSize(n: number): number {
  if (!Number.isFinite(n)) return DEFAULT_EXPORT_OPTIONS.sql.batchSize;
  return Math.min(BATCH_SIZE_RANGE.max, Math.max(BATCH_SIZE_RANGE.min, Math.round(n)));
}

/** A table tab's rows (see `ExportInput`). */
export interface TableExportInput {
  kind: "table";
  table: TableRef;
  filter: FilterGroup | null;
  sort: SortSpec[];
  /** Visible columns in grid order; null = every column. */
  columns: string[] | null;
  /** The table's generated (computed) columns: INSERT exports must not write them. */
  generated: string[];
  scope: RowScope;
  /**
   * The page on screen. `sql` is the backend's (inlined) browse statement for exactly this page,
   * or null while it is not loaded (see `currentPageSql`).
   */
  page: { limit: number; offset: number; sql: string | null };
}

/** Rows the renderer holds (a SQL editor result), written as they are. */
export interface RowsExportInput {
  kind: "rows";
  columns: ResultColumn[];
  rows: Value[][];
  /** Default INSERT target; null = the user must name one. */
  table: string | null;
}

/** What is being exported, resolved from the dialog's source and the table tab's state. */
export type ExportInput = { kind: "sql"; sql: string } | TableExportInput | RowsExportInput;

export function toExportFormat(options: ExportOptions, tableName: string): ExportFormat {
  switch (options.format) {
    case "csv":
      return {
        format: "csv",
        delimiter: options.csv.delimiter,
        header: options.csv.header,
        quote: options.csv.quote,
        nullAs: options.csv.nullToEmpty ? "" : "NULL",
        lineBreak: options.csv.lineBreak,
        decimal: options.csv.decimal,
        lineBreakToSpace: options.csv.lineBreakToSpace,
      };
    case "json":
      return {
        format: "json",
        ndjson: options.json.ndjson,
        // NDJSON is one record per line by definition.
        pretty: options.json.pretty && !options.json.ndjson,
      };
    case "sqlInsert":
      return {
        format: "sqlInsert",
        tableName: tableName.trim(),
        batchSize: clampBatchSize(options.sql.batchSize),
        dropTable: options.sql.dropTable,
        structure: options.sql.structure,
        content: options.sql.content,
        gzip: options.sql.gzip,
      };
    case "xlsx":
      return { format: "xlsx", header: options.xlsx.header };
  }
}

/** SQL scripts: structure and DROP exist for table sources only. */
export function sqlWritesStructure(options: ExportOptions): boolean {
  return options.format === "sqlInsert" && (options.sql.structure || options.sql.dropTable);
}

/** SQLite browses add `rowid AS "_tabler_rowid"` to address rows; an export must not write it. */
export function stripRowidAlias(sql: string): string {
  return sql.replace(/,\s*rowid AS "_tabler_rowid"/, "");
}

/** The page a browse statement reads, from its trailing `LIMIT n OFFSET m`; null if it has none. */
export function browsePageOf(sql: string): { limit: number; offset: number } | null {
  const m = /\bLIMIT\s+(\d+)\s+OFFSET\s+(\d+)\s*;?\s*$/i.exec(sql);
  return m ? { limit: Number(m[1]), offset: Number(m[2]) } : null;
}

/**
 * `page.sql` when it is the browse statement of exactly this page, else null. The tab keeps the
 * last loaded page's statement while the next page loads (or after it failed to load), so a
 * statement for another page must not be re-run as "the current page".
 */
export function currentPageSql(page: {
  limit: number;
  offset: number;
  sql: string | null;
}): string | null {
  if (page.sql === null) return null;
  const read = browsePageOf(page.sql);
  return read && read.limit === page.limit && read.offset === page.offset ? page.sql : null;
}

/**
 * Whether "Only current page" can be exported for this page. The first page is the table query
 * capped at the page size; later pages re-run the browse statement, so they need it loaded.
 */
export function canExportPage(page: { offset: number; sql: string | null }): boolean {
  return page.offset === 0 || page.sql !== null;
}

/** Why the dialog's choices cannot be exported (shown under the `field` row), or null. */
export interface ExportProblem {
  field: "rows" | "columns" | "options";
  message: string;
  /** "wait" resolves by itself (the page is loading); "fix" needs another choice. */
  severity: "wait" | "fix";
}

/** The columns an INSERT export writes: generated columns are left out (they can't be inserted). */
function insertableColumns(input: TableExportInput): string[] | null {
  if (input.columns === null) return null;
  const generated = new Set(input.generated);
  return input.columns.filter((c) => !generated.has(c));
}

/** Pages after the first re-run the grid's browse statement instead of the table query. */
const rerunsPage = (input: TableExportInput) => input.scope === "page" && input.page.offset > 0;

/** A binary value cut to its preview (also inside arrays): the renderer never holds more. */
function isCappedBytes(v: Value): boolean {
  return (v.t === "bytes" && v.v.truncated) || (v.t === "array" && v.v.some(isCappedBytes));
}

/**
 * Whether rows held by the renderer contain a binary value cut to its 64 KiB preview: written as
 * they are, the file would hold the cut value, so the backend refuses them.
 */
export function hasCappedBytes(rows: readonly (readonly Value[])[]): boolean {
  return rows.some((row) => row.some(isCappedBytes));
}

export function exportProblem(
  input: ExportInput,
  format: FormatKind,
  sql?: ExportOptions["sql"],
): ExportProblem | null {
  if (format === "sqlInsert" && sql && !sql.content) {
    if (input.kind !== "table")
      return {
        field: "options",
        message: "A query or result has no structure: include its content.",
        severity: "fix",
      };
    if (!sql.structure && !sql.dropTable)
      return {
        field: "options",
        message: "Nothing to export: include the table structure or its content.",
        severity: "fix",
      };
  }
  if (input.kind === "rows") {
    return hasCappedBytes(input.rows)
      ? {
          field: "rows",
          message: "A binary value in this result is cut to its 64 KiB preview.",
          severity: "fix",
        }
      : null;
  }
  if (input.kind !== "table") return null;
  if (rerunsPage(input)) {
    if (!canExportPage(input.page)) {
      return {
        field: "rows",
        message: "Wait for the page to load to export it.",
        severity: "wait",
      };
    }
    // The browse statement selects every column; it can neither drop hidden columns nor
    // generated ones (until table exports can start at an offset).
    if (input.columns !== null) {
      return {
        field: "rows",
        message:
          "Pages after the first export every column: uncheck “Only visible columns”, or export all rows.",
        severity: "fix",
      };
    }
    if (format === "sqlInsert" && input.generated.length > 0) {
      return {
        field: "rows",
        message:
          "Pages after the first can't leave out generated columns: export all rows, or as CSV / JSON.",
        severity: "fix",
      };
    }
  }
  if (format === "sqlInsert" && insertableColumns(input)?.length === 0) {
    return {
      field: "columns",
      message: "Every visible column is generated: nothing to insert.",
      severity: "fix",
    };
  }
  return null;
}

/** Builds the `export_data` request for the dialog's choices. */
export function buildExportRequest(args: {
  exportId: string;
  path: string;
  input: ExportInput;
  options: ExportOptions;
  /** INSERT target; empty = the source table's name. */
  sqlTableName: string;
}): ExportRequest {
  const { exportId, path, input, options } = args;
  // The dialog never submits these; refuse rather than silently export something else.
  const problem = exportProblem(input, options.format, options.sql);
  if (problem) throw new Error(problem.message);
  let source: ExportSource;
  let maxRows: number | null = null;
  let tableName = args.sqlTableName.trim();
  if (input.kind === "sql") {
    source = { kind: "sql", sql: input.sql };
  } else if (input.kind === "rows") {
    // Empty table name = the result's table (the backend's default INSERT target).
    source = { kind: "rows", columns: input.columns, rows: input.rows, table: input.table };
  } else {
    source = {
      kind: "table",
      table: input.table,
      filter: input.filter,
      sort: input.sort,
      // With `null` the backend leaves generated columns out of INSERTs itself.
      columns: options.format === "sqlInsert" ? insertableColumns(input) : input.columns,
    };
    if (input.scope === "page") {
      maxRows = input.page.limit;
      if (rerunsPage(input) && input.page.sql) {
        // Table exports always start at the first row; a later page re-runs the grid's statement.
        source = { kind: "sql", sql: stripRowidAlias(input.page.sql) };
        // A query export has no source table to name the INSERTs after.
        if (tableName === "") tableName = input.table.name;
      }
    }
  }
  return { exportId, source, format: toExportFormat(options, tableName), path, maxRows };
}

/**
 * The grid's visible columns in display order (`order` first, then the rest in table order), or
 * null when nothing is hidden (= every column, which also lets the backend skip generated columns
 * for INSERT exports).
 */
export function visibleColumnNames(
  columns: readonly ColumnMeta[],
  state: { order: readonly string[]; hidden: readonly string[] },
): string[] | null {
  const hidden = new Set(state.hidden);
  if (!columns.some((c) => hidden.has(c.name))) return null;
  const names = new Set(columns.map((c) => c.name));
  const out: string[] = [];
  for (const name of state.order) {
    if (names.has(name) && !hidden.has(name) && !out.includes(name)) out.push(name);
  }
  for (const c of columns) {
    if (!hidden.has(c.name) && !out.includes(c.name)) out.push(c.name);
  }
  return out;
}

// ---- several tables ---------------------------------------------------------------------------

/** Where a multi-table export writes for these options (CSV / JSON: always a file per table). */
export function multiLayout(options: ExportOptions): ExportLayout {
  switch (options.format) {
    case "sqlInsert":
      return options.multi.sqlSingleFile ? "singleFile" : "filePerTable";
    case "xlsx":
      return options.multi.xlsxSingleFile ? "singleFile" : "filePerTable";
    default:
      return "filePerTable";
  }
}

/** Builds the `export_tables` request: every table in full, in the order given. */
export function buildExportTablesRequest(args: {
  exportId: string;
  tables: TableRef[];
  options: ExportOptions;
  /** Folder (file per table) or file (one file). */
  target: string;
}): ExportTablesRequest {
  const { exportId, tables, options, target } = args;
  if (tables.length === 0) throw new Error("Choose at least one table to export.");
  if (
    options.format === "sqlInsert" &&
    !options.sql.content &&
    !options.sql.structure &&
    !options.sql.dropTable
  )
    throw new Error("Nothing to export: include the table structure or its content.");
  return {
    exportId,
    tables,
    // Every table's INSERTs name that table.
    format: toExportFormat(options, ""),
    layout: multiLayout(options),
    target,
  };
}

/** Tables a multi-table export lists: tables and views, matching `search` (case-insensitively). */
export function filterTables<T extends { name: string }>(
  tables: readonly T[],
  search: string,
): T[] {
  const q = search.trim().toLowerCase();
  return q === "" ? [...tables] : tables.filter((t) => t.name.toLowerCase().includes(q));
}

// ---- file names -------------------------------------------------------------------------------

export function fileExtension(options: ExportOptions): string {
  switch (options.format) {
    case "csv":
      return options.csv.delimiter === "\t" ? "tsv" : "csv";
    case "json":
      return options.json.ndjson ? "ndjson" : "json";
    case "sqlInsert":
      return options.sql.gzip ? "sql.gz" : "sql";
    case "xlsx":
      return "xlsx";
  }
}

export function fileFilter(options: ExportOptions): { name: string; extensions: string[] } {
  switch (fileExtension(options)) {
    case "tsv":
      return { name: "Tab-separated values", extensions: ["tsv", "txt"] };
    case "csv":
      return { name: "CSV", extensions: ["csv", "txt"] };
    case "ndjson":
      return { name: "NDJSON", extensions: ["ndjson", "jsonl"] };
    case "json":
      return { name: "JSON", extensions: ["json"] };
    case "sql.gz":
      return { name: "Gzip-compressed SQL", extensions: ["gz"] };
    case "xlsx":
      return { name: "Excel workbook", extensions: ["xlsx"] };
    default:
      return { name: "SQL", extensions: ["sql"] };
  }
}

const pad = (n: number) => String(n).padStart(2, "0");

/** `YYYYMMDD-HHmm` in local time. */
export function timestamp(d: Date): string {
  return `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}`;
}

/**
 * A file-system friendly version of a table name (`Order Items` → `Order_Items`). Letters and
 * digits of any script are kept (`pedidos_año`, `顧客`); separators, punctuation and control
 * characters become `_`.
 */
export function safeFileBase(name: string): string {
  const cleaned = name
    .normalize("NFC")
    .replace(/[^\p{L}\p{M}\p{N}_.-]+/gu, "_")
    .replace(/_+/g, "_")
    .replace(/^[_.]+|_+$/g, "");
  return cleaned || "export";
}

/** `users_20261011-0930.csv`; SQL sources are named `query_…`. */
export function defaultFileName(base: string, options: ExportOptions, now: Date): string {
  return `${safeFileBase(base)}_${timestamp(now)}.${fileExtension(options)}`;
}

export function dirname(path: string): string | null {
  const i = path.lastIndexOf("/");
  return i > 0 ? path.slice(0, i) : null;
}

export function basename(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

// ---- formatting -------------------------------------------------------------------------------

export function formatCount(n: number): string {
  return n.toLocaleString("en-US");
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
}

/** `0:07`, `12:30`, `1:02:03`. */
export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

export function formatDuration(ms: number): string {
  return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`;
}

// ---- persistence ------------------------------------------------------------------------------

/** Settings ▸ General ▸ CSV default options. */
const csvPrefs = (): CsvDefaults => csvDefaultsOfPrefs();

export const EXPORT_OPTIONS_KEY = "tabler.export.options";
export const EXPORT_DIR_KEY = "tabler.export.lastDir";

type StorageLike = Pick<Storage, "getItem" | "setItem">;

function defaultStorage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    // Accessing localStorage throws when site data is blocked.
    return null;
  }
}

const isObj = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const bool = (v: unknown, d: boolean) => (typeof v === "boolean" ? v : d);

function oneOf<T>(options: ReadonlyArray<{ value: T }>, v: unknown, d: T): T {
  return options.some((o) => o.value === v) ? (v as T) : d;
}

/** The CSV fields Settings ▸ General ▸ CSV default options decides. */
export type CsvDefaults = Pick<
  ExportOptions["csv"],
  "delimiter" | "quote" | "lineBreak" | "decimal"
>;

const DEFAULT_CSV_FIELDS: CsvDefaults = {
  delimiter: DEFAULTS.csv.delimiter,
  quote: DEFAULTS.csv.quote,
  lineBreak: DEFAULTS.csv.lineBreak,
  decimal: DEFAULTS.csv.decimal,
};

const sameCsvDefaults = (a: Record<string, unknown>, b: CsvDefaults) =>
  a.delimiter === b.delimiter &&
  a.quote === b.quote &&
  a.lineBreak === b.lineBreak &&
  a.decimal === b.decimal;

/**
 * Validates stored options field by field; anything unknown falls back to the default. Options
 * stored by earlier versions (`quoteAll`, `nullAs`) keep their meaning.
 *
 * `csvDefaults` (the CSV default options of the preferences) seed the delimiter, quote, line
 * break and decimal: the remembered ones win only while the preferences still say what they said
 * when those were saved (`csvDefaults` stored next to them; options saved before it existed count
 * as saved under the defaults), so changing the preference shows on the next export.
 */
export function parseExportOptions(
  raw: unknown,
  csvDefaults: CsvDefaults = DEFAULT_CSV_FIELDS,
): ExportOptions {
  const d = DEFAULT_EXPORT_OPTIONS;
  const r = isObj(raw) ? raw : {};
  const remembered = sameCsvDefaults(
    isObj(r.csvDefaults) ? r.csvDefaults : DEFAULT_CSV_FIELDS,
    csvDefaults,
  );
  const c = csvDefaults;
  const csv = isObj(r.csv) ? r.csv : {};
  const json = isObj(r.json) ? r.json : {};
  const sql = isObj(r.sql) ? r.sql : {};
  const xlsx = isObj(r.xlsx) ? r.xlsx : {};
  const multi = isObj(r.multi) ? r.multi : {};
  const formats: FormatKind[] = ["csv", "json", "sqlInsert", "xlsx"];
  const legacyQuote = csv.quoteAll === true ? "double" : undefined;
  const legacyNull = typeof csv.nullAs === "string" ? csv.nullAs === "" : undefined;
  return {
    format: formats.includes(r.format as FormatKind) ? (r.format as FormatKind) : d.format,
    csv: {
      delimiter: remembered ? oneOf(CSV_DELIMITERS, csv.delimiter, c.delimiter) : c.delimiter,
      header: bool(csv.header, d.csv.header),
      quote: remembered ? oneOf(CSV_QUOTES, csv.quote ?? legacyQuote, c.quote) : c.quote,
      nullToEmpty: bool(csv.nullToEmpty ?? legacyNull, d.csv.nullToEmpty),
      lineBreakToSpace: bool(csv.lineBreakToSpace, d.csv.lineBreakToSpace),
      lineBreak: remembered ? oneOf(CSV_LINE_BREAKS, csv.lineBreak, c.lineBreak) : c.lineBreak,
      decimal: remembered ? oneOf(CSV_DECIMALS, csv.decimal, c.decimal) : c.decimal,
    },
    json: { ndjson: bool(json.ndjson, d.json.ndjson), pretty: bool(json.pretty, d.json.pretty) },
    sql: {
      batchSize:
        typeof sql.batchSize === "number" ? clampBatchSize(sql.batchSize) : d.sql.batchSize,
      dropTable: bool(sql.dropTable, d.sql.dropTable),
      structure: bool(sql.structure, d.sql.structure),
      content: bool(sql.content, d.sql.content),
      gzip: bool(sql.gzip, d.sql.gzip),
    },
    xlsx: { header: bool(xlsx.header, d.xlsx.header) },
    multi: {
      sqlSingleFile: bool(multi.sqlSingleFile, d.multi.sqlSingleFile),
      xlsxSingleFile: bool(multi.xlsxSingleFile, d.multi.xlsxSingleFile),
    },
  };
}

/** The last export's options, seeded by the preferences' CSV default options. */
export function loadExportOptions(
  storage: StorageLike | null = defaultStorage(),
  csvDefaults: CsvDefaults = csvPrefs(),
): ExportOptions {
  try {
    const raw = storage?.getItem(EXPORT_OPTIONS_KEY);
    return parseExportOptions(raw ? JSON.parse(raw) : null, csvDefaults);
  } catch {
    return parseExportOptions(null, csvDefaults);
  }
}

/** Remembers the options, and the CSV default options they were chosen under. */
export function saveExportOptions(
  options: ExportOptions,
  storage: StorageLike | null = defaultStorage(),
  csvDefaults: CsvDefaults = csvPrefs(),
): void {
  try {
    storage?.setItem(EXPORT_OPTIONS_KEY, JSON.stringify({ ...options, csvDefaults }));
  } catch {
    // Quota exceeded / storage disabled: the next export starts from the defaults.
  }
}

export function loadLastDir(storage: StorageLike | null = defaultStorage()): string | null {
  try {
    return storage?.getItem(EXPORT_DIR_KEY) || null;
  } catch {
    return null;
  }
}

export function saveLastDir(dir: string, storage: StorageLike | null = defaultStorage()): void {
  try {
    storage?.setItem(EXPORT_DIR_KEY, dir);
  } catch {
    // Ignored, see saveExportOptions.
  }
}
