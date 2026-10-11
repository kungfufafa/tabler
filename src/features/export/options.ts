import type {
  ColumnMeta,
  ExportFormat,
  ExportRequest,
  ExportSource,
  FilterGroup,
  SortSpec,
  TableRef,
} from "@/lib/ipc";

export type FormatKind = ExportFormat["format"];
export type CsvDelimiter = "," | ";" | "\t" | "|";
/** Which rows of a table tab: everything matching its filter, or only the page on screen. */
export type RowScope = "all" | "page";

/** The user's choices in the export dialog (remembered between exports). */
export interface ExportOptions {
  format: FormatKind;
  csv: { delimiter: CsvDelimiter; header: boolean; quoteAll: boolean; nullAs: string };
  json: { ndjson: boolean; pretty: boolean };
  sql: { batchSize: number };
}

const DEFAULTS: ExportOptions = {
  format: "csv",
  csv: { delimiter: ",", header: true, quoteAll: false, nullAs: "" },
  json: { ndjson: false, pretty: true },
  sql: { batchSize: 100 },
};
export const DEFAULT_EXPORT_OPTIONS: Readonly<ExportOptions> = Object.freeze(DEFAULTS);

export const CSV_DELIMITERS: ReadonlyArray<{ value: CsvDelimiter; label: string; title: string }> =
  [
    { value: ",", label: ",", title: "Comma" },
    { value: ";", label: ";", title: "Semicolon" },
    { value: "\t", label: "Tab", title: "Tab" },
    { value: "|", label: "|", title: "Pipe" },
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

/** What is being exported, resolved from the dialog's source and the table tab's state. */
export type ExportInput = { kind: "sql"; sql: string } | TableExportInput;

export function toExportFormat(options: ExportOptions, tableName: string): ExportFormat {
  switch (options.format) {
    case "csv":
      return {
        format: "csv",
        delimiter: options.csv.delimiter,
        header: options.csv.header,
        quoteAll: options.csv.quoteAll,
        nullAs: options.csv.nullAs,
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
      };
  }
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
  field: "rows" | "columns";
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

export function exportProblem(input: ExportInput, format: FormatKind): ExportProblem | null {
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
  const problem = exportProblem(input, options.format);
  if (problem) throw new Error(problem.message);
  let source: ExportSource;
  let maxRows: number | null = null;
  let tableName = args.sqlTableName.trim();
  if (input.kind === "sql") {
    source = { kind: "sql", sql: input.sql };
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

// ---- file names -------------------------------------------------------------------------------

export function fileExtension(options: ExportOptions): string {
  switch (options.format) {
    case "csv":
      return options.csv.delimiter === "\t" ? "tsv" : "csv";
    case "json":
      return options.json.ndjson ? "ndjson" : "json";
    case "sqlInsert":
      return "sql";
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

/** Validates stored options field by field; anything unknown falls back to the default. */
export function parseExportOptions(raw: unknown): ExportOptions {
  const d = DEFAULT_EXPORT_OPTIONS;
  if (!isObj(raw))
    return { format: d.format, csv: { ...d.csv }, json: { ...d.json }, sql: { ...d.sql } };
  const csv = isObj(raw.csv) ? raw.csv : {};
  const json = isObj(raw.json) ? raw.json : {};
  const sql = isObj(raw.sql) ? raw.sql : {};
  const formats: FormatKind[] = ["csv", "json", "sqlInsert"];
  return {
    format: formats.includes(raw.format as FormatKind) ? (raw.format as FormatKind) : d.format,
    csv: {
      delimiter: CSV_DELIMITERS.some((o) => o.value === csv.delimiter)
        ? (csv.delimiter as CsvDelimiter)
        : d.csv.delimiter,
      header: bool(csv.header, d.csv.header),
      quoteAll: bool(csv.quoteAll, d.csv.quoteAll),
      nullAs: typeof csv.nullAs === "string" ? csv.nullAs.slice(0, 32) : d.csv.nullAs,
    },
    json: { ndjson: bool(json.ndjson, d.json.ndjson), pretty: bool(json.pretty, d.json.pretty) },
    sql: {
      batchSize:
        typeof sql.batchSize === "number" ? clampBatchSize(sql.batchSize) : d.sql.batchSize,
    },
  };
}

export function loadExportOptions(storage: StorageLike | null = defaultStorage()): ExportOptions {
  try {
    const raw = storage?.getItem(EXPORT_OPTIONS_KEY);
    return parseExportOptions(raw ? JSON.parse(raw) : null);
  } catch {
    return parseExportOptions(null);
  }
}

export function saveExportOptions(
  options: ExportOptions,
  storage: StorageLike | null = defaultStorage(),
): void {
  try {
    storage?.setItem(EXPORT_OPTIONS_KEY, JSON.stringify(options));
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
