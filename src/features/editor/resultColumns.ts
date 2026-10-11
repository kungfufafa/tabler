import type { GridColumn } from "@/features/datagrid/types";
import type { ResultColumn, SortSpec, Value } from "@/lib/ipc";
import { isNumericKind, valueText } from "@/lib/values";

/**
 * Read-only grid columns for a result set. Result columns can repeat a name (`select a.id, b.id`),
 * so the grid id is the column position.
 */
export function resultGridColumns(columns: readonly ResultColumn[]): GridColumn[] {
  return columns.map((c, i) => ({
    id: String(i),
    name: c.name,
    kind: c.kind,
    dataType: c.dataType,
    nullable: c.nullable ?? true,
    isPk: false,
    hasDefault: false,
    isAutoInc: false,
    editable: false,
    align: isNumericKind(c.kind) ? "right" : "left",
  }));
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

function numeric(v: Value): number | bigint | null {
  switch (v.t) {
    case "int":
    case "float":
      return v.v;
    case "bool":
      return v.v ? 1 : 0;
    case "bigInt":
      try {
        return BigInt(v.v);
      } catch {
        return Number(v.v);
      }
    case "decimal": {
      const n = Number(v.v);
      return Number.isNaN(n) ? null : n;
    }
    default:
      return null;
  }
}

/** Orders values for client-side sorting of results: NULLs first, numbers numerically, text naturally. */
export function compareValues(a: Value, b: Value): number {
  const an = a.t === "null" || a.t === "default";
  const bn = b.t === "null" || b.t === "default";
  if (an || bn) return an === bn ? 0 : an ? -1 : 1;
  const x = numeric(a);
  const y = numeric(b);
  if (x !== null && y !== null) {
    if (typeof x === typeof y) return x < y ? -1 : x > y ? 1 : 0;
    const fx = Number(x);
    const fy = Number(y);
    return fx < fy ? -1 : fx > fy ? 1 : 0;
  }
  return collator.compare(valueText(a) ?? "", valueText(b) ?? "");
}

/** Row order for `sort` (grid column ids are column positions); stable. */
export function sortedOrder(rows: readonly Value[][], sort: readonly SortSpec[]): number[] {
  const order = rows.map((_, i) => i);
  const specs = sort
    .map((s) => ({ idx: Number(s.column), dir: s.dir === "desc" ? -1 : 1 }))
    .filter((s) => Number.isInteger(s.idx) && s.idx >= 0);
  if (specs.length === 0) return order;
  const NULL: Value = { t: "null" };
  return order.sort((i, j) => {
    for (const s of specs) {
      const c = compareValues(rows[i]![s.idx] ?? NULL, rows[j]![s.idx] ?? NULL) * s.dir;
      if (c !== 0) return c;
    }
    return i - j;
  });
}
