import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { toFilterGroup } from "@/features/table-tab/filters";
import { rowKeys } from "@/features/table-tab/useTableRows";
import { ipc, type RowCount, type SortSpec } from "@/lib/ipc";
import { getPrefs } from "@/stores/prefs";
import { useTableTabsStore } from "@/stores/tableTabs";
import {
  canExportPage,
  currentPageSql,
  formatCount,
  visibleColumnNames,
  type ExportInput,
  type RowScope,
} from "./options";
import type { OpenExportArgs } from "./store";

export interface ResolvedExportSource {
  /** Null while the table's metadata loads (or when it failed). */
  input: ExportInput | null;
  /** `schema.table`, "Query result", or a shown result's label ("Result 2"). */
  title: string;
  /** Short facts about the rows: filters, sort. */
  details: string[];
  /** The tab's filter narrows the rows. */
  filtered: boolean;
  /** File name stem: the table name, or `query`. */
  fileBase: string;
  /**
   * INSERT target used when the "Table name" field is left empty (null for SQL sources and for
   * results without a known source table).
   */
  defaultTableName: string | null;
  /** True when the export comes from a table tab (filter / sort / page apply). */
  fromTab: boolean;
  count: RowCount | null;
  /** Rows on the current page and its first row (1-based); null while the page is not loaded. */
  page: { first: number; rows: number } | null;
  /** "Only current page" is possible (a later page needs its browse statement loaded). */
  pageExportable: boolean;
  columns: { visible: number; total: number };
  loading: boolean;
  error: string | null;
}

/** Why a shown result is incomplete (see `ExportSourceSpec`'s `stopped`). */
const STOPPED_LABEL = { cancelled: "cancelled", error: "failed partway" } as const;

function sortText(sort: SortSpec[]): string | null {
  if (sort.length === 0) return null;
  return `sorted by ${sort.map((s) => `${s.column} ${s.dir === "asc" ? "↑" : "↓"}`).join(", ")}`;
}

/** Resolves what the export dialog will export, from its source and the table tab's state. */
export function useExportSource(
  target: OpenExportArgs,
  scope: RowScope,
  visibleOnly: boolean,
): ResolvedExportSource {
  const table = target.source.kind === "table" ? target.source.table : null;
  const tabId = target.source.kind === "table" ? (target.source.tabId ?? null) : null;
  const slice = useTableTabsStore((s) => (tabId ? s.slices[tabId] : undefined));
  const meta = useQuery({
    queryKey: table ? rowKeys.meta(target.connectionId, table) : ["export", "no-table"],
    queryFn: () => ipc.schema.table(target.connectionId, table!),
    enabled: table !== null,
    staleTime: 5 * 60_000,
  });
  const sql = target.source.kind === "sql" ? target.source.sql : null;
  const held = target.source.kind === "rows" ? target.source : null;

  return useMemo<ResolvedExportSource>(() => {
    if (held) {
      const n = held.columns.length;
      return {
        input: { kind: "rows", columns: held.columns, rows: held.rows, table: held.table },
        title: held.label,
        details: [
          // A result cut at the row limit, or whose statement stopped early, holds only the first
          // rows of the statement.
          `${formatCount(held.rows.length)} row${held.rows.length === 1 ? "" : "s"}${held.truncated || held.stopped ? " shown" : ""}`,
          `${n} column${n === 1 ? "" : "s"}`,
          ...(held.stopped ? [STOPPED_LABEL[held.stopped]] : []),
        ],
        filtered: false,
        fileBase: held.table ?? "query",
        defaultTableName: held.table,
        fromTab: false,
        count: { count: held.rows.length, exact: true },
        page: null,
        pageExportable: false,
        columns: { visible: n, total: n },
        loading: false,
        error: null,
      };
    }
    if (sql !== null || !table) {
      return {
        input: { kind: "sql", sql: sql ?? "" },
        title: "Query result",
        details: [],
        filtered: false,
        fileBase: "query",
        defaultTableName: null,
        fromTab: false,
        count: null,
        page: null,
        pageExportable: false,
        columns: { visible: 0, total: 0 },
        loading: false,
        error: null,
      };
    }
    const title = table.schema ? `${table.schema}.${table.name}` : table.name;
    const columns = meta.data?.columns ?? [];
    const filter =
      slice && meta.data ? toFilterGroup(slice.filters, columns, slice.conjunction) : null;
    const sort = slice?.sort ?? [];
    const visible = slice && meta.data ? visibleColumnNames(columns, slice.columnState) : null;
    const limit = slice?.limit ?? getPrefs().pageSize;
    const offset = slice?.offset ?? 0;
    // `slice.page` still describes the previous page while the next one loads (or if it failed).
    const pageSql = currentPageSql({ limit, offset, sql: slice?.page?.sql ?? null });
    const pageInfo = { limit, offset, sql: pageSql };
    const loadedPage = pageSql !== null ? slice?.page : undefined;
    const details: string[] = [];
    const n = filter?.items.length ?? 0;
    if (n > 0) details.push(`${n} filter${n === 1 ? "" : "s"}`);
    const sorted = sortText(sort);
    if (sorted) details.push(sorted);
    return {
      input: meta.data
        ? {
            kind: "table",
            table: meta.data.table,
            filter,
            sort,
            columns: visibleOnly ? visible : null,
            generated: columns.filter((c) => c.isGenerated).map((c) => c.name),
            scope: slice ? scope : "all",
            page: pageInfo,
          }
        : null,
      title,
      details,
      filtered: n > 0,
      fileBase: table.name,
      defaultTableName: table.name,
      fromTab: slice !== undefined,
      count: slice?.count ?? null,
      page: loadedPage ? { first: offset + 1, rows: loadedPage.rows } : null,
      pageExportable: slice !== undefined && canExportPage(pageInfo),
      columns: { visible: visible?.length ?? columns.length, total: columns.length },
      loading: meta.isPending && table !== null,
      error: meta.isError ? meta.error.message : null,
    };
  }, [
    held,
    sql,
    table,
    slice,
    meta.data,
    meta.isPending,
    meta.isError,
    meta.error,
    scope,
    visibleOnly,
  ]);
}
