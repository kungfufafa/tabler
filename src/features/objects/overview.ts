import type { Dialect, OverviewItem } from "@/lib/ipc";
import { KIND_LABEL } from "@/features/structure/tableActions";

/** Columns of the Item Overview grid (TablePlus names them like result columns). */
export type OverviewColumn =
  | "name"
  | "kind"
  | "schema"
  | "estimatedRows"
  | "totalSize"
  | "dataSize"
  | "indexSize"
  | "engine"
  | "charset"
  | "collation"
  | "owner"
  | "comment";

export interface OverviewColumnDef {
  id: OverviewColumn;
  label: string;
  numeric: boolean;
}

const COLUMNS: Record<OverviewColumn, OverviewColumnDef> = {
  name: { id: "name", label: "name", numeric: false },
  kind: { id: "kind", label: "kind", numeric: false },
  schema: { id: "schema", label: "schema", numeric: false },
  estimatedRows: { id: "estimatedRows", label: "estimated_rows", numeric: true },
  totalSize: { id: "totalSize", label: "total_size", numeric: true },
  dataSize: { id: "dataSize", label: "data_size", numeric: true },
  indexSize: { id: "indexSize", label: "index_size", numeric: true },
  engine: { id: "engine", label: "engine", numeric: false },
  charset: { id: "charset", label: "charset", numeric: false },
  collation: { id: "collation", label: "collation", numeric: false },
  owner: { id: "owner", label: "owner", numeric: false },
  comment: { id: "comment", label: "comment", numeric: false },
};

/** The grid's columns for a dialect: engine / charset / collation on MySQL, owner on PG. */
export function overviewColumns(dialect: Dialect): OverviewColumnDef[] {
  const ids: OverviewColumn[] = [
    "name",
    "kind",
    "schema",
    "estimatedRows",
    "totalSize",
    "dataSize",
    "indexSize",
  ];
  if (dialect === "mysql") ids.push("engine", "charset", "collation");
  if (dialect === "postgres") ids.push("owner");
  ids.push("comment");
  return ids.map((id) => COLUMNS[id]);
}

export interface OverviewSort {
  column: OverviewColumn;
  desc: boolean;
}

export const DEFAULT_OVERVIEW_SORT: OverviewSort = { column: "name", desc: false };

/** The comparable value of a cell: numbers stay numbers, the kind sorts by its label. */
function sortValue(item: OverviewItem, column: OverviewColumn): string | number | null {
  if (column === "kind") return KIND_LABEL[item.kind];
  return item[column] ?? null;
}

/** Sorted copy of `items`; empty cells go last in both directions, ties keep name order. */
export function sortOverview(items: readonly OverviewItem[], sort: OverviewSort): OverviewItem[] {
  const dir = sort.desc ? -1 : 1;
  return [...items].sort((a, b) => {
    const x = sortValue(a, sort.column);
    const y = sortValue(b, sort.column);
    if (x === null || y === null) {
      if (x === y) return a.name.localeCompare(b.name);
      return x === null ? 1 : -1;
    }
    const c =
      typeof x === "number" && typeof y === "number"
        ? x - y
        : String(x).localeCompare(String(y), undefined, { numeric: true, sensitivity: "base" });
    return c !== 0 ? c * dir : a.name.localeCompare(b.name);
  });
}

/** Header click: a new column sorts ascending (numbers descending), the same one flips. */
export function nextSort(current: OverviewSort, column: OverviewColumnDef): OverviewSort {
  if (current.column === column.id) return { column: column.id, desc: !current.desc };
  return { column: column.id, desc: column.numeric };
}

const UNITS = ["B", "KB", "MB", "GB", "TB"];

/** 1536 → "1.5 KB" (1024-based, like TablePlus). */
export function formatBytes(n: number | null | undefined): string {
  if (n === null || n === undefined) return "";
  let v = n;
  let unit = 0;
  while (Math.abs(v) >= 1024 && unit < UNITS.length - 1) {
    v /= 1024;
    unit++;
  }
  const text = unit === 0 || v >= 100 ? Math.round(v).toString() : v.toFixed(1).replace(/\.0$/, "");
  return `${text} ${UNITS[unit]}`;
}

/** Whether the comment of `item` can be edited on `dialect`. */
export function commentEditable(dialect: Dialect, item: OverviewItem): boolean {
  if (item.kind === "systemView") return false;
  if (dialect === "postgres") return true;
  if (dialect === "mysql") return item.kind === "table" || item.kind === "partitioned";
  return false;
}
