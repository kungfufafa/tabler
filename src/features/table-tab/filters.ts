import type {
  ColumnMeta,
  Conjunction,
  Dialect,
  FilterGroup,
  FilterNode,
  FilterOp,
  Value,
  ValueKind,
} from "@/lib/ipc";
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
export function opLabel(op: FilterOp, dialect: Dialect): string {
  const label = opDef(op).label;
  return dialect === "mysql" && CASE_VARIANTS.has(op)
    ? label.replace("Case insensitive", "Case sensitive")
    : label;
}

/** The column kind an operator list is for; "any" is the "Any column" pseudo-column. */
export type OpTarget = Pick<ColumnMeta, "kind"> | "any" | null;

/**
 * The operators the filter bar offers on a driver for a column: ILIKE on PostgreSQL only, JSON
 * containment on PostgreSQL json / jsonb columns only.
 */
export function opsFor(dialect: Dialect, target: OpTarget = null): OpDef[] {
  const json = target !== null && target !== "any" && target.kind === "json";
  return OPS.filter((o) => {
    if (o.op === "ilike") return dialect === "postgres";
    if (o.op === "jsonContains" || o.op === "jsonNotContains")
      return dialect === "postgres" && json;
    return true;
  });
}

/** Whether `op` is offered for `target` on `dialect` (see `opsFor`). */
export function opAllowed(op: FilterOp, dialect: Dialect, target: OpTarget = null): boolean {
  return opsFor(dialect, target).some((o) => o.op === op);
}

/** Turns typed filter text into a tagged value by column kind (the backend binds and casts). */
export function parseFilterValue(kind: ValueKind, text: string): Value {
  const t = text.trim();
  switch (kind) {
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

/** The operator-list target of a filter row's column. */
export function targetOf(column: string, columns: readonly ColumnMeta[]): OpTarget {
  if (column === ANY_COLUMN) return "any";
  return columns.find((c) => c.name === column) ?? null;
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
    if (!col) continue;
    const values = rowValues(row, col.kind);
    if (values) items.push({ kind: "condition", column: col.name, op: row.op, values });
  }
  return items.length === 0 ? null : { conjunction, items };
}

/**
 * A new filter row as the filter bar's ⚙ settings ask: on the primary key (its first column, else
 * the first column), on "Any column" or on Raw SQL, with `=` or Contains.
 */
export function defaultFilterRow(
  columns: readonly ColumnMeta[],
  settings: { column: FilterDefaultColumn; operator: FilterDefaultOperator },
): FilterRowSpec {
  const op: FilterOp = settings.operator === "contains" ? "contains" : "eq";
  if (settings.column === "anyColumn") return newFilterRow(ANY_COLUMN, op);
  if (settings.column === "rawSql") return newFilterRow(RAW_SQL_COLUMN, op);
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
