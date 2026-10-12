import type {
  ColumnMeta,
  FilterGroup,
  FilterNode,
  FilterOp,
  Conjunction,
  Value,
  ValueKind,
} from "@/lib/ipc";
import type { FilterRowSpec } from "@/stores/tableTabs";

export interface OpDef {
  op: FilterOp;
  label: string;
  arity: 0 | 1 | 2 | "list";
}

export const OPS: OpDef[] = [
  { op: "eq", label: "=", arity: 1 },
  { op: "ne", label: "≠", arity: 1 },
  { op: "gt", label: ">", arity: 1 },
  { op: "ge", label: "≥", arity: 1 },
  { op: "lt", label: "<", arity: 1 },
  { op: "le", label: "≤", arity: 1 },
  { op: "like", label: "LIKE", arity: 1 },
  { op: "notLike", label: "NOT LIKE", arity: 1 },
  { op: "in", label: "IN", arity: "list" },
  { op: "notIn", label: "NOT IN", arity: "list" },
  { op: "between", label: "BETWEEN", arity: 2 },
  { op: "isNull", label: "IS NULL", arity: 0 },
  { op: "isNotNull", label: "IS NOT NULL", arity: 0 },
];

export function opDef(op: FilterOp): OpDef {
  return OPS.find((o) => o.op === op) ?? OPS[0]!;
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

/** Splits an IN list: commas, with quotes honoured. */
export function splitList(text: string): string[] {
  const out: string[] = [];
  let cur = "";
  let quote: string | null = null;
  for (const ch of text) {
    if (quote) {
      if (ch === quote) quote = null;
      else cur += ch;
    } else if (ch === '"' || ch === "'") quote = ch;
    else if (ch === ",") {
      out.push(cur.trim());
      cur = "";
    } else cur += ch;
  }
  if (cur.trim() !== "" || out.length > 0) out.push(cur.trim());
  return out.filter((s) => s !== "");
}

/**
 * The filter-bar "column" of a raw SQL condition: its `value` is a WHERE fragment that the backend
 * wraps in parentheses as is (`FilterNode` kind "raw"); the operator is ignored.
 */
export const RAW_SQL_COLUMN = "__raw_sql__";

export function isRawSqlRow(row: Pick<FilterRowSpec, "column">): boolean {
  return row.column === RAW_SQL_COLUMN;
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
    const col = columns.find((c) => c.name === row.column);
    if (!col) continue;
    const def = opDef(row.op);
    if (def.arity === 0) {
      items.push({ kind: "condition", column: col.name, op: row.op, values: [] });
      continue;
    }
    if (def.arity === "list") {
      const vals = splitList(row.value).map((v) => parseFilterValue(col.kind, v));
      if (vals.length === 0) continue;
      items.push({ kind: "condition", column: col.name, op: row.op, values: vals });
      continue;
    }
    if (row.value.trim() === "") continue;
    if (def.arity === 2) {
      if (row.value2.trim() === "") continue;
      items.push({
        kind: "condition",
        column: col.name,
        op: row.op,
        values: [parseFilterValue(col.kind, row.value), parseFilterValue(col.kind, row.value2)],
      });
      continue;
    }
    const value =
      row.op === "like" || row.op === "notLike"
        ? { t: "text" as const, v: row.value }
        : parseFilterValue(col.kind, row.value);
    items.push({ kind: "condition", column: col.name, op: row.op, values: [value] });
  }
  return items.length === 0 ? null : { conjunction, items };
}
