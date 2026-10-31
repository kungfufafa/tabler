import type {
  ColumnMeta,
  CsvQuote,
  ExportEncoding,
  ExportFormat,
  ExportLayout,
  ExportRequest,
  ExportResultsRequest,
  ExportTablesRequest,
  ExportSource,
  FilterGroup,
  ResultColumn,
  SessionCaps,
  SortSpec,
  TableRef,
  Value,
} from "@/lib/ipc";
import { csvDefaults as csvDefaultsOfPrefs, decimalClashes } from "@/features/datagrid/csv";
import { isCoreSession } from "@/lib/caps";
import { isCsvDecimal } from "@/stores/prefs";

export type FormatKind = ExportFormat["format"];
export type CsvDelimiter = "," | "\t" | ";";
export type CsvLineBreak = "\n" | "\r\n" | "\r";
/** `.`, `,` or one custom character (see `csvProblem`). */
export type CsvDecimal = string;
/** Which rows of a table tab: everything matching its filter, or only the page on screen. */
export type RowScope = "all" | "page";
/** A result of a run holding several: only the one shown, or every result of the run. */
export type ResultsScope = "this" | "all";

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
  /** Several tables at once: one SQL script / workbook / HTML page, or a file per table. */
  multi: { sqlSingleFile: boolean; xlsxSingleFile: boolean; htmlSingleFile: boolean };
  /** Character encoding of the text formats (CSV, JSON, SQL, HTML). */
  encoding: ExportEncoding;
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
  multi: { sqlSingleFile: true, xlsxSingleFile: true, htmlSingleFile: true },
  encoding: "utf8",
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
  { value: "html", label: "HTML", title: "An HTML table (.html)" },
];

/**
 * The formats a session exports to: a SQL script needs `exportSql` besides `exportTables` (a
 * Redshift session reads its tables but writes no SQL script); every format while the caps are
 * not known.
 */
export function formatsFor(caps: SessionCaps | null): typeof FORMATS {
  return caps === null || caps.features.includes("exportSql")
    ? FORMATS
    : FORMATS.filter((f) => f.value !== "sqlInsert");
}

/** `options` in a format of `formats`: CSV in place of one the session does not offer. */
export function withOfferedFormat(options: ExportOptions, formats: typeof FORMATS): ExportOptions {
  return formats.some((f) => f.value === options.format) ? options : { ...options, format: "csv" };
}

/**
 * Whether the session exports a query (runs a statement for the file): a core session only, as
 * the backend's `require_query_export` decides. The statement runs as written, and only a core
 * server refuses a write on a read-only connection by itself; another session (Redshift) exports
 * its tables and the rows a result shows. True while the caps are not known.
 */
export function exportsQueries(caps: SessionCaps | null): boolean {
  return caps === null || isCoreSession(caps);
}

export const ENCODINGS: ReadonlyArray<{ value: ExportEncoding; label: string; title: string }> = [
  { value: "utf8", label: "UTF-8", title: "Unicode, without a byte order mark" },
  {
    value: "utf8Bom",
    label: "UTF-8 with BOM",
    title: "Unicode with a byte order mark (how Excel recognises a UTF-8 CSV)",
  },
  { value: "utf16Le", label: "UTF-16 LE", title: "Unicode, 16-bit little-endian, with BOM" },
  { value: "windows1252", label: "Windows-1252", title: "Western European (Windows)" },
  { value: "latin1", label: "Latin-1 (ISO-8859-1)", title: "Western European (ISO)" },
];

/** Whether the encoding applies: text formats only (a workbook is zipped UTF-8 XML). */
export function encodingApplies(format: FormatKind): boolean {
  return format !== "xlsx";
}

/** The encoding the request asks for (UTF-8 for formats it does not apply to). */
export function exportEncoding(options: ExportOptions): ExportEncoding {
  return encodingApplies(options.format) ? options.encoding : "utf8";
}

/**
 * The export encoding that follows a connection's charset (Connection ▸ View Using Encoding): the
 * charset names of MySQL (`latin1`, `cp1252`, `utf16`, `ucs2`) and PostgreSQL (`LATIN1`,
 * `WIN1252`, `UTF16`) are read alike. Null for UTF-8 (the remembered choice stays) and for
 * charsets the export has no encoding for.
 */
export function encodingForCharset(charset: string | null | undefined): ExportEncoding | null {
  const name = (charset ?? "").toLowerCase().replace(/[^a-z0-9]/g, "");
  switch (name) {
    case "latin1":
    case "iso88591":
      return "latin1";
    case "cp1252":
    case "windows1252":
    case "win1252":
      return "windows1252";
    case "utf16":
    case "utf16le":
    case "ucs2":
      return "utf16Le";
    default:
      return null;
  }
}

export const CSV_QUOTES: ReadonlyArray<{ value: CsvQuote; label: string; title: string }> = [
  { value: "double", label: '"', title: "Put every value in double quotes" },
  { value: "single", label: "'", title: "Put every value in single quotes" },
  {
    value: "space",
    label: "Space",
    title: "Put every value between spaces (a space inside a value is doubled)",
  },
  { value: "ifNeeded", label: "Quote if needed", title: "Quote only values that need it" },
];

export const CSV_LINE_BREAKS: ReadonlyArray<{ value: CsvLineBreak; label: string; title: string }> =
  [
    { value: "\n", label: "\\n", title: "LF (macOS, Linux)" },
    { value: "\r\n", label: "\\r\\n", title: "CRLF (Windows)" },
    { value: "\r", label: "\\r", title: "CR (classic Mac)" },
  ];

/** The two usual decimal separators; "Other…" takes any other single character. */
export const CSV_DECIMALS: ReadonlyArray<{ value: CsvDecimal; label: string; title: string }> = [
  { value: ".", label: ".", title: "Point: 12.5" },
  { value: ",", label: ",", title: "Comma: 12,5" },
];

/** A decimal separator that is neither of the two presets: typed in the "Other…" field. */
export const isCustomDecimal = (decimal: CsvDecimal): boolean =>
  !CSV_DECIMALS.some((d) => d.value === decimal);

/**
 * Why a decimal separator cannot be written next to `delimiter`, or null: a custom one is a single
 * character that is not a digit or a quote and differs from the delimiter.
 */
export function decimalProblem(decimal: CsvDecimal, delimiter: string): string | null {
  if (!isCustomDecimal(decimal)) return null;
  if (decimal === "") return "Type the decimal separator, or choose . or ,";
  if (!isCsvDecimal(decimal))
    return "The decimal separator is one character that is not a digit or a quote.";
  if (decimalClashes(decimal, delimiter))
    return "The decimal separator must differ from the delimiter.";
  return null;
}

/**
 * Why the CSV choices cannot be written (shown under the CSV options), or null: see
 * `decimalProblem`; a space delimiter cannot go with the Space quote.
 */
export function csvProblem(csv: ExportOptions["csv"]): string | null {
  if (csv.quote === "space" && (csv.delimiter as string) === " ")
    return "The delimiter cannot be a space when values are quoted with spaces.";
  return decimalProblem(csv.decimal, csv.delimiter);
}

/** The INSERT target of a result or query whose table is unknown (TablePlus names it so too). */
export const UNTITLED_TABLE = "untitled";

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
  /** The page on screen: the rows from `offset` on, `limit` of them. */
  page: { limit: number; offset: number };
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
    case "html":
      return { format: "html" };
  }
}

/** SQL scripts: structure and DROP exist for table sources only. */
export function sqlWritesStructure(options: ExportOptions): boolean {
  return options.format === "sqlInsert" && (options.sql.structure || options.sql.dropTable);
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
  const csvIssue = options.format === "csv" ? csvProblem(options.csv) : null;
  if (csvIssue) throw new Error(csvIssue);
  let source: ExportSource;
  let maxRows: number | null = null;
  let tableName = args.sqlTableName.trim();
  if (input.kind === "sql") {
    source = { kind: "sql", sql: input.sql };
    // A query has no source table to name the INSERTs after.
    if (tableName === "") tableName = UNTITLED_TABLE;
  } else if (input.kind === "rows") {
    // Empty table name = the result's table (the backend's default INSERT target).
    source = { kind: "rows", columns: input.columns, rows: input.rows, table: input.table };
    if (tableName === "" && input.table === null) tableName = UNTITLED_TABLE;
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
      // The page is a slice of the table query: the same columns and generated-column rules as
      // every other table export, from the page's first row.
      maxRows = input.page.limit;
      source.offset = input.page.offset;
    }
  }
  return {
    exportId,
    source,
    format: toExportFormat(options, tableName),
    path,
    maxRows,
    encoding: exportEncoding(options),
  };
}

/**
 * The grid's visible columns in display order (`order` first, then the rest in table order), or
 * null when nothing is hidden and the order is the table's own (= every column, which also lets
 * the backend skip generated columns for INSERT exports). Columns only dragged into another order
 * count: the file follows the grid.
 */
export function visibleColumnNames(
  columns: readonly ColumnMeta[],
  state: { order: readonly string[]; hidden: readonly string[] },
): string[] | null {
  const hidden = new Set(state.hidden);
  const names = new Set(columns.map((c) => c.name));
  const out: string[] = [];
  for (const name of state.order) {
    if (names.has(name) && !hidden.has(name) && !out.includes(name)) out.push(name);
  }
  for (const c of columns) {
    if (!hidden.has(c.name) && !out.includes(c.name)) out.push(c.name);
  }
  const sameAsTable = out.length === columns.length && out.every((n, i) => n === columns[i]?.name);
  return sameAsTable ? null : out;
}

/** What a grid's column state changes for an export: hidden columns, another order, both. */
export function columnChanges(
  columns: readonly ColumnMeta[],
  state: { order: readonly string[]; hidden: readonly string[] },
): { hidden: boolean; reordered: boolean } {
  const visible = visibleColumnNames(columns, state);
  const hidden = columns.some((c) => state.hidden.includes(c.name));
  // Reordered: the visible columns are not in the table's order (hidden ones aside).
  const tableOrder = columns.filter((c) => !state.hidden.includes(c.name)).map((c) => c.name);
  const reordered = visible !== null && visible.some((n, i) => n !== tableOrder[i]);
  return { hidden, reordered };
}

// ---- several tables ---------------------------------------------------------------------------

/** Where a multi-table export writes for these options (CSV / JSON: always a file per table). */
export function multiLayout(options: ExportOptions): ExportLayout {
  switch (options.format) {
    case "sqlInsert":
      return options.multi.sqlSingleFile ? "singleFile" : "filePerTable";
    case "xlsx":
      return options.multi.xlsxSingleFile ? "singleFile" : "filePerTable";
    case "html":
      return options.multi.htmlSingleFile ? "singleFile" : "filePerTable";
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
  /** The tables' database; null = the connection's current one. */
  database?: string | null;
  /** File per table: create the folder (a multi-database export writes one per database). */
  createFolder?: boolean;
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
  const csvIssue = options.format === "csv" ? csvProblem(options.csv) : null;
  if (csvIssue) throw new Error(csvIssue);
  return {
    exportId,
    tables,
    // Every table's INSERTs name that table.
    format: toExportFormat(options, ""),
    layout: multiLayout(options),
    target,
    encoding: exportEncoding(options),
    database: args.database ?? null,
    createFolder: args.createFolder ?? false,
  };
}

// ---- every result of a run --------------------------------------------------------------------

/**
 * Where an "All results" export writes: one file where the format can hold several results (an
 * Excel workbook, an HTML page, a SQL script), else (CSV, JSON) a file per result in a folder.
 */
export function resultsLayout(format: FormatKind): ExportLayout {
  return format === "csv" || format === "json" ? "filePerTable" : "singleFile";
}

/** A result of a run, as the "All results" export takes it. */
export interface ResultsExportItem {
  label: string;
  columns: ResultColumn[];
  rows: Value[][];
  table: string | null;
}

/** Why the results cannot be exported as they are (shown in the dialog), or null. */
export function resultsProblem(
  items: readonly ResultsExportItem[],
  format: FormatKind,
  sql?: ExportOptions["sql"],
): ExportProblem | null {
  if (format === "sqlInsert" && sql && !sql.content)
    return {
      field: "options",
      message: "A query or result has no structure: include its content.",
      severity: "fix",
    };
  const capped = items.find((i) => hasCappedBytes(i.rows));
  if (capped)
    return {
      field: "rows",
      message: `A binary value in ${capped.label} is cut to its 64 KiB preview.`,
      severity: "fix",
    };
  return null;
}

/**
 * Builds the `export_results` request: every result in the order given; `target` is the file, or
 * the folder of a file per result (named `<name>-1`, `<name>-2`…).
 */
export function buildExportResultsRequest(args: {
  exportId: string;
  items: readonly ResultsExportItem[];
  options: ExportOptions;
  target: string;
  name?: string;
}): ExportResultsRequest {
  const { exportId, items, options, target } = args;
  if (items.length === 0) throw new Error("There are no results to export.");
  const problem = resultsProblem(items, options.format, options.sql);
  if (problem) throw new Error(problem.message);
  const csvIssue = options.format === "csv" ? csvProblem(options.csv) : null;
  if (csvIssue) throw new Error(csvIssue);
  return {
    exportId,
    results: items.map((i) => ({
      label: i.label,
      columns: i.columns,
      rows: i.rows,
      table: i.table,
    })),
    // Each result's INSERTs name its own table (or `untitled`).
    format: toExportFormat(options, ""),
    layout: resultsLayout(options.format),
    target,
    encoding: exportEncoding(options),
    ...(args.name ? { name: args.name } : {}),
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
    case "html":
      return "html";
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
    case "html":
      return { name: "HTML", extensions: ["html", "htm"] };
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
  const formats: FormatKind[] = ["csv", "json", "sqlInsert", "xlsx", "html"];
  const legacyQuote = csv.quoteAll === true ? "double" : undefined;
  const legacyNull = typeof csv.nullAs === "string" ? csv.nullAs === "" : undefined;
  const delimiter = remembered ? oneOf(CSV_DELIMITERS, csv.delimiter, c.delimiter) : c.delimiter;
  const rememberedDecimal = isCsvDecimal(csv.decimal) ? (csv.decimal as string) : c.decimal;
  // A custom decimal separator equal to the delimiter could not be told apart: points stay.
  const decimalChoice = remembered ? rememberedDecimal : c.decimal;
  const decimal = decimalClashes(decimalChoice, delimiter) ? DEFAULTS.csv.decimal : decimalChoice;
  return {
    format: formats.includes(r.format as FormatKind) ? (r.format as FormatKind) : d.format,
    csv: {
      delimiter,
      header: bool(csv.header, d.csv.header),
      quote: remembered ? oneOf(CSV_QUOTES, csv.quote ?? legacyQuote, c.quote) : c.quote,
      nullToEmpty: bool(csv.nullToEmpty ?? legacyNull, d.csv.nullToEmpty),
      lineBreakToSpace: bool(csv.lineBreakToSpace, d.csv.lineBreakToSpace),
      lineBreak: remembered ? oneOf(CSV_LINE_BREAKS, csv.lineBreak, c.lineBreak) : c.lineBreak,
      decimal,
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
      htmlSingleFile: bool(multi.htmlSingleFile, d.multi.htmlSingleFile),
    },
    encoding: oneOf(ENCODINGS, r.encoding, d.encoding),
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
