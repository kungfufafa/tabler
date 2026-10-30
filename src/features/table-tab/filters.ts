import type {
  ColumnMeta,
  Conjunction,
  Dialect,
  FilterGroup,
  FilterNode,
  FilterOp,
  RawFilter,
  Value,
  ValueKind,
} from "@/lib/ipc";
import { parseHexBytes } from "@/features/datagrid/cellText";
import type { FilterDefaultColumn, FilterDefaultOperator } from "@/stores/prefs";
import { newFilterRow, type FilterRowSpec } from "@/stores/tableTabs";

export interface OpDef {
  op: FilterOp;
  label: string;
  arity: 0 | 1 | 2 | "list";
}

/** The operators whose value matches literally or as a pattern: always sent as text. */
const TEXT_VALUE_OPS: ReadonlySet<FilterOp> = new Set<FilterOp>([
  "contains",
  "notContains",
  "containsCase",
  "notContainsCase",
  "hasPrefix",
  "hasSuffix",
  "hasPrefixCase",
  "hasSuffixCase",
  "like",
  "notLike",
  "ilike",
]);

/** Whether `op` compares as text whatever the column's type (patterns, LIKE). */
export function isTextValueOp(op: FilterOp): boolean {
  return TEXT_VALUE_OPS.has(op);
}

/** Every operator in TablePlus's order; the `…Case` labels are the PostgreSQL / SQLite ones. */
export const OPS: OpDef[] = [
  { op: "eq", label: "=", arity: 1 },
  { op: "ne", label: "<>", arity: 1 },
  { op: "lt", label: "<", arity: 1 },
  { op: "gt", label: ">", arity: 1 },
  { op: "le", label: "<=", arity: 1 },
  { op: "ge", label: ">=", arity: 1 },
  { op: "in", label: "IN", arity: "list" },
  { op: "notIn", label: "NOT IN", arity: "list" },
  { op: "isNull", label: "IS NULL", arity: 0 },
  { op: "isNotNull", label: "IS NOT NULL", arity: 0 },
  { op: "between", label: "BETWEEN", arity: 2 },
  { op: "notBetween", label: "NOT BETWEEN", arity: 2 },
  { op: "contains", label: "Contains", arity: 1 },
  { op: "notContains", label: "Not contains", arity: 1 },
  { op: "containsCase", label: "Contains – Case insensitive", arity: 1 },
  { op: "notContainsCase", label: "Not contains – Case insensitive", arity: 1 },
  { op: "hasPrefix", label: "Has prefix", arity: 1 },
  { op: "hasSuffix", label: "Has suffix", arity: 1 },
  { op: "hasPrefixCase", label: "Has prefix – Case insensitive", arity: 1 },
  { op: "hasSuffixCase", label: "Has suffix – Case insensitive", arity: 1 },
  { op: "like", label: "LIKE", arity: 1 },
  { op: "notLike", label: "NOT LIKE", arity: 1 },
  { op: "ilike", label: "ILIKE", arity: 1 },
  { op: "jsonContains", label: "JSON contains", arity: 1 },
  { op: "jsonNotContains", label: "JSON not contains", arity: 1 },
];

const CASE_VARIANTS: ReadonlySet<FilterOp> = new Set<FilterOp>([
  "containsCase",
  "notContainsCase",
  "hasPrefixCase",
  "hasSuffixCase",
]);

/** Whether `op` names a known operator (remembered filters may come from an older version). */
export function isFilterOp(op: string): op is FilterOp {
  return OPS.some((o) => o.op === op);
}

export function opDef(op: FilterOp): OpDef {
  return OPS.find((o) => o.op === op) ?? OPS[0]!;
}

/**
 * An operator's label on a driver: the case variants read "– Case sensitive" on MySQL (its LIKE
 * follows the usually case-insensitive collation) and "– Case insensitive" elsewhere.
 */
export function opLabel(op: FilterOp, dialect: Dialect | null): string {
  const label = opDef(op).label;
  return dialect === "mysql" && CASE_VARIANTS.has(op)
    ? label.replace("Case insensitive", "Case sensitive")
    : label;
}

/** The column kind an operator list is for; "any" is the "Any column" pseudo-column. */
export type OpTarget = Pick<ColumnMeta, "kind"> | "any" | null;

const JSON_OPS: ReadonlySet<FilterOp> = new Set<FilterOp>(["jsonContains", "jsonNotContains"]);

/**
 * What the filter bar offers: a session's operators (`DataCaps.filterOps`), or a core dialect's
 * (its sessions offer every operator, ILIKE and JSON containment on PostgreSQL only).
 */
export type OfferedOps = Dialect | readonly FilterOp[];

function offeredList(offered: OfferedOps): readonly FilterOp[] {
  if (typeof offered !== "string") return offered;
  return OPS.filter((o) => offered === "postgres" || (o.op !== "ilike" && !JSON_OPS.has(o.op))).map(
    (o) => o.op,
  );
}

/**
 * The operators the filter bar offers for a column, in TablePlus's order: the offered ones (see
 * `OfferedOps`), with JSON containment on json / jsonb columns only.
 */
export function opsFor(offered: OfferedOps, target: OpTarget = null): OpDef[] {
  const json = target !== null && target !== "any" && target.kind === "json";
  const list = offeredList(offered);
  return OPS.filter((o) => list.includes(o.op) && (json || !JSON_OPS.has(o.op)));
}

/** Whether `op` is offered for `target` (see `opsFor`). */
export function opAllowed(op: FilterOp, offered: OfferedOps, target: OpTarget = null): boolean {
  return opsFor(offered, target).some((o) => o.op === op);
}

/**
 * Turns typed filter text into a tagged value by column kind (the backend binds and casts). Text
 * that is not a value of the kind stays text (the server then answers); only a blob's hex is
 * checked up front, because a text value compared with a binary column silently matches nothing
 * (`filterValueError` says what is wrong, `toFilterGroup` leaves such a row out).
 */
export function parseFilterValue(kind: ValueKind, text: string): Value {
  const t = text.trim();
  switch (kind) {
    case "bytes": {
      // Hex, with or without a `0x` / `\x` prefix: a foreign-key jump on a binary key types it.
      const bytes = parseHexBytes(t);
      return bytes.ok ? bytes.value : { t: "text", v: t };
    }
    case "int":
    case "bigInt": {
      if (/^-?\d+$/.test(t)) {
        const n = Number(t);
        return Number.isSafeInteger(n) ? { t: "int", v: n } : { t: "bigInt", v: t };
      }
      return { t: "text", v: t };
    }
    case "float": {
      const n = Number(t);
      return Number.isFinite(n) && t !== "" ? { t: "float", v: n } : { t: "text", v: t };
    }
    case "decimal":
      return { t: "decimal", v: t };
    case "bool":
      return /^(true|t|1|yes)$/i.test(t)
        ? { t: "bool", v: true }
        : /^(false|f|0|no)$/i.test(t)
          ? { t: "bool", v: false }
          : { t: "text", v: t };
    case "json":
      return { t: "json", v: t };
    case "uuid":
      return { t: "uuid", v: t };
    case "enum":
      return { t: "enum", v: t };
    case "date":
      return { t: "date", v: t };
    case "time":
      return { t: "time", v: t };
    case "timestamp":
      return { t: "timestamp", v: t };
    case "timestampTz":
      return { t: "timestampTz", v: t };
    default:
      return { t: "text", v: text };
  }
}

/**
 * Splits an IN list on commas. A value in quotes (`"…"` or `'…'`) may contain commas and keeps its
 * spaces; a doubled quote inside it stands for the quote itself (`"say ""hi"""`).
 */
export function splitList(text: string): string[] {
  const out: string[] = [];
  let cur = "";
  /** The current item has a quoted part: kept verbatim, even when empty. */
  let quoted = false;
  let quote: string | null = null;
  const finish = () => {
    const item = quoted ? cur : cur.trim();
    if (quoted || item !== "") out.push(item);
    cur = "";
    quoted = false;
  };
  for (let i = 0; i < text.length; i++) {
    const ch = text[i]!;
    if (quote) {
      if (ch !== quote) cur += ch;
      else if (text[i + 1] === quote) {
        cur += ch;
        i++;
      } else quote = null;
    } else if (ch === '"' || ch === "'") {
      if (!quoted) cur = cur.trim();
      quote = ch;
      quoted = true;
    } else if (ch === ",") finish();
    // Spaces around a quoted value are not part of it.
    else if (!quoted || ch.trim() !== "") cur += ch;
  }
  finish();
  return out;
}

/** The IN-list text `splitList` reads back as `items` (quoting the values that need it). */
export function joinList(items: readonly string[]): string {
  return items
    .map((item) =>
      item !== "" && item === item.trim() && !/[,"']/.test(item)
        ? item
        : `"${item.replaceAll('"', '""')}"`,
    )
    .join(", ");
}

/**
 * The filter-bar "column" of a raw SQL condition: its `value` is a WHERE fragment that the backend
 * wraps in parentheses as is (`FilterNode` kind "raw"); the operator is ignored.
 */
export const RAW_SQL_COLUMN = "__raw_sql__";

/**
 * The filter-bar "Any column" pseudo-column: the operator applies to every column's textual form
 * (`FilterNode` kind "anyColumn", expanded by the backend).
 */
export const ANY_COLUMN = "__any_column__";

export function isRawSqlRow(row: Pick<FilterRowSpec, "column">): boolean {
  return row.column === RAW_SQL_COLUMN;
}

export function isAnyColumnRow(row: Pick<FilterRowSpec, "column">): boolean {
  return row.column === ANY_COLUMN;
}

/**
 * What a session's filter bar can do: its operators, its raw filter (a SQL WHERE, a JSON query or
 * a query string; null: none) and whether "Any column" is offered.
 */
export interface FilterSession {
  ops: OfferedOps;
  raw: RawFilter | null;
  anyColumn: boolean;
}

/** The filter bar of a core session (every operator of PostgreSQL's, a SQL WHERE). */
export const CORE_FILTERS: FilterSession = { ops: "postgres", raw: "sqlWhere", anyColumn: true };

/** Whether a session's filter bar has anything to offer. */
export function filtersOffered(session: FilterSession): boolean {
  return offeredList(session.ops).length > 0 || session.raw !== null;
}

/** The operator-list target of a filter row's column. */
export function targetOf(column: string, columns: readonly ColumnMeta[]): OpTarget {
  if (column === ANY_COLUMN) return "any";
  return columns.find((c) => c.name === column) ?? null;
}

/**
 * Why filter text cannot be sent for a column of `kind` (the cell editor's message), null when it
 * can. Only binary columns are strict: their value must be hex.
 */
export function filterValueError(kind: ValueKind | null, text: string): string | null {
  if (kind !== "bytes" || text.trim() === "") return null;
  const bytes = parseHexBytes(text);
  return bytes.ok ? null : bytes.error;
}

/**
 * Why a condition on a column cannot be applied as typed (`filterValueError` of its value, its
 * second value or the items of its list), null when it can. Patterns, raw SQL and "Any column"
 * compare as text and never fail.
 */
export function rowValueError(row: FilterRowSpec, columns: readonly ColumnMeta[]): string | null {
  if (isRawSqlRow(row) || isAnyColumnRow(row) || TEXT_VALUE_OPS.has(row.op)) return null;
  const col = columns.find((c) => c.name === row.column);
  if (!col) return null;
  const { arity } = opDef(row.op);
  if (arity === 0) return null;
  const texts =
    arity === "list" ? splitList(row.value) : arity === 2 ? [row.value, row.value2] : [row.value];
  for (const text of texts) {
    const error = filterValueError(col.kind, text);
    if (error) return error;
  }
  return null;
}

/** The values of one row for `kind` (null: the row is not complete and is skipped). */
function rowValues(row: FilterRowSpec, kind: ValueKind | null): Value[] | null {
  const def = opDef(row.op);
  const typed = (text: string): Value =>
    kind === null || TEXT_VALUE_OPS.has(row.op)
      ? { t: "text", v: text }
      : row.op === "jsonContains" || row.op === "jsonNotContains"
        ? { t: "json", v: text }
        : parseFilterValue(kind, text);
  if (def.arity === 0) return [];
  if (def.arity === "list") {
    const vals = splitList(row.value).map(typed);
    return vals.length === 0 ? null : vals;
  }
  if (row.value.trim() === "") return null;
  if (def.arity === 2) {
    if (row.value2.trim() === "") return null;
    return [typed(row.value), typed(row.value2)];
  }
  return [typed(row.value)];
}

export function toFilterGroup(
  rows: FilterRowSpec[],
  columns: ColumnMeta[],
  conjunction: Conjunction,
): FilterGroup | null {
  const items: FilterNode[] = [];
  for (const row of rows) {
    if (!row.enabled) continue;
    if (isRawSqlRow(row)) {
      const sql = row.value.trim();
      if (sql !== "") items.push({ kind: "raw", sql });
      continue;
    }
    if (isAnyColumnRow(row)) {
      // Every column compares as text.
      const values = rowValues(row, null);
      if (values) items.push({ kind: "anyColumn", op: row.op, values });
      continue;
    }
    const col = columns.find((c) => c.name === row.column);
    // A blob compared with text matches nothing: a row that is not hex waits to be corrected.
    if (!col || rowValueError(row, columns)) continue;
    const values = rowValues(row, col.kind);
    if (values) items.push({ kind: "condition", column: col.name, op: row.op, values });
  }
  return items.length === 0 ? null : { conjunction, items };
}

/** The last row the applied filter holds (checked and complete): where ⌘F puts the focus. */
export function lastAppliedRowId(rows: FilterRowSpec[], columns: ColumnMeta[]): string | null {
  for (let i = rows.length - 1; i >= 0; i--) {
    const row = rows[i]!;
    if (toFilterGroup([row], columns, "and") !== null) return row.id;
  }
  return null;
}

/**
 * A new filter row as the filter bar's ⚙ settings ask: on the primary key (its first column, else
 * the first column), on "Any column" or on Raw SQL, with `=` or Contains.
 */
export function defaultFilterRow(
  columns: readonly ColumnMeta[],
  settings: { column: FilterDefaultColumn; operator: FilterDefaultOperator },
  session: FilterSession = CORE_FILTERS,
): FilterRowSpec {
  const wanted: FilterOp = settings.operator === "contains" ? "contains" : "eq";
  // A session without the operator starts from its first one.
  const offered = offeredList(session.ops);
  const op = offered.includes(wanted) || offered.length === 0 ? wanted : offered[0]!;
  if (settings.column === "anyColumn" && session.anyColumn) return newFilterRow(ANY_COLUMN, op);
  if (settings.column === "rawSql" && session.raw !== null) return newFilterRow(RAW_SQL_COLUMN, op);
  const pk = columns.find((c) => c.isPk) ?? columns[0];
  return newFilterRow(pk?.name ?? "", op);
}

// A generated ORDER BY term: a quoted column (or SQLite's rowid), MySQL's `(col IS NULL)` nulls
// flag, then the direction and the optional NULLS FIRST / LAST.
const ORDER_IDENT = '(?:"(?:[^"]|"")*"|`(?:[^`]|``)*`|rowid)';
const ORDER_TERM = `(?:${ORDER_IDENT}|\\(${ORDER_IDENT} IS NULL\\)) (?:ASC|DESC)(?: NULLS (?:FIRST|LAST))?`;
const ORDER_TAIL = new RegExp(` ORDER BY ${ORDER_TERM}(?:, ${ORDER_TERM})*$`);

/**
 * The condition of a browse page's SELECT (`BrowsePage.sql`, values inlined for display), without
 * the WHERE keyword: what the filter bar's "SQL" button shows. Null when the page is unfiltered.
 */
export function whereOfPageSql(sql: string | null | undefined): string | null {
  if (!sql) return null;
  const match = /\nWHERE /.exec(sql) ?? / WHERE /.exec(sql);
  if (!match) return null;
  const start = match.index + match[0].length;
  let end = sql.lastIndexOf("\nLIMIT ");
  if (end < start) end = sql.lastIndexOf(" LIMIT ");
  if (end < start) end = sql.length;
  const body = sql.slice(start, end).replace(ORDER_TAIL, "").trim();
  return body === "" ? null : body;
}
