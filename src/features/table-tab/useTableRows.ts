import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { ipc, tableKey, type BrowseRequest, type FilterGroup, type TableRef } from "@/lib/ipc";

export const rowKeys = {
  page: (connectionId: string, table: TableRef, req: Omit<BrowseRequest, "table">, token: number) =>
    ["rows", connectionId, tableKey(table), req, token] as const,
  count: (connectionId: string, table: TableRef, filter: FilterGroup | null, token: number) =>
    ["count", connectionId, tableKey(table), filter, token] as const,
  meta: (connectionId: string, table: TableRef) =>
    ["table", connectionId, table.schema ?? "", table.name] as const,
};

export function useTableMeta(connectionId: string, table: TableRef) {
  return useQuery({
    queryKey: rowKeys.meta(connectionId, table),
    queryFn: () => ipc.schema.table(connectionId, table),
    staleTime: 5 * 60_000,
  });
}

/**
 * One page of rows. `hold`: the page carries staged edits, which name their columns by position in
 * it; it is then never refetched behind the user's back (remounting after switching to the
 * structure view, window focus, reconnect) nor garbage-collected, so a schema change cannot move
 * the edits onto other columns. Changing the request (paging, sorting, filtering, ⌘R) still goes
 * through the pending-edits guard.
 */
export function useTableRows(
  connectionId: string,
  table: TableRef,
  req: Omit<BrowseRequest, "table"> | null,
  token: number,
  hold = false,
) {
  return useQuery({
    queryKey: rowKeys.page(
      connectionId,
      table,
      req ?? { columns: null, filter: null, sort: [], limit: 0, offset: 0 },
      token,
    ),
    queryFn: () => ipc.browse.rows(connectionId, { table, ...req! }),
    enabled: req !== null,
    placeholderData: keepPreviousData,
    staleTime: hold ? Infinity : 15_000,
    gcTime: hold ? Infinity : 5 * 60_000,
    refetchOnMount: !hold,
    refetchOnWindowFocus: !hold,
    refetchOnReconnect: !hold,
  });
}

export function useRowCount(
  connectionId: string,
  table: TableRef,
  filter: FilterGroup | null,
  token: number,
  enabled: boolean,
) {
  return useQuery({
    queryKey: rowKeys.count(connectionId, table, filter, token),
    queryFn: () => ipc.browse.count(connectionId, table, filter, false),
    enabled,
    placeholderData: keepPreviousData,
    staleTime: 15_000,
  });
}
