import type { Query, QueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { schemaKeys } from "@/features/sidebar/useSchemaTree";
import { pendingCount } from "@/features/staging/model";
import { tableKey, type TableRef } from "@/lib/ipc";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";

export const ddlKey = (connectionId: string, table: TableRef) =>
  ["tableDdl", connectionId, table.schema ?? "", table.name] as const;

/**
 * Refetches everything a schema change can affect: table lists, editor completion, DDL text and,
 * for `table`, its metadata, rows and counts.
 */
export function invalidateSchema(qc: QueryClient, connectionId: string, table?: TableRef): void {
  void qc.invalidateQueries({ queryKey: ["tables", connectionId] });
  void qc.invalidateQueries({ queryKey: schemaKeys.completion(connectionId) });
  void qc.invalidateQueries({ queryKey: ["tableDdl", connectionId] });
  if (!table) return;
  void qc.invalidateQueries({ queryKey: schemaKeys.table(connectionId, table.schema, table.name) });
  void qc.invalidateQueries({ queryKey: ["rows", connectionId, tableKey(table)] });
  void qc.invalidateQueries({ queryKey: ["count", connectionId, tableKey(table)] });
}

/**
 * Tables of the connection whose tab holds staged (uncommitted) row edits, whichever view (data or
 * structure) the tab shows: the row staging stays while the structure view is up. Staged DDL does
 * not count here. Staged edits name their columns by position in the page they were made on:
 * reloading that page after a schema change (a dropped or added column) would move them onto
 * other columns, so these pages are left as they are until the user commits or discards the edits
 * (then reloads with ⌘R).
 */
export function tablesWithPendingEdits(connectionId: string): TableRef[] {
  const out: TableRef[] = [];
  const slices = useTableTabsStore.getState().slices;
  for (const t of useTabsStore.getState().tabs) {
    if (t.kind === "query" || t.connectionId !== connectionId) continue;
    const slice = slices[t.id];
    if (!slice || pendingCount(slice.staging) === 0) continue;
    if (!out.some((r) => tableKey(r) === tableKey(t.table))) out.push(t.table);
  }
  return out;
}

/**
 * Tells the user that tables with staged row edits were not reloaded after a schema change (see
 * `tablesWithPendingEdits`).
 */
export function reportHeldTables(held: readonly TableRef[]): void {
  if (held.length === 0) return;
  const names = held.map((t) => t.name);
  const list =
    names.length <= 3 ? names.join(", ") : `${names.slice(0, 3).join(", ")} +${names.length - 3}`;
  toast.message(`Unsaved edits in ${list}`, {
    description:
      "Not reloaded, so the edits stay on their columns. Commit or discard them, then press ⌘R to see the new structure.",
  });
}

/** `tableKey` of the table a metadata (`["table", c, schema, name]`), rows or count query is for. */
function queryTable(query: Query): string | null {
  const [kind, , a, b] = query.queryKey;
  if (kind === "table") return typeof a === "string" && typeof b === "string" ? `${a}.${b}` : null;
  return typeof a === "string" ? a : null;
}

/**
 * After DDL of unknown reach (run in the SQL editor): refetches every schema-derived query of the
 * connection — schemas, databases, table lists, completion, DDL text, and every table's metadata,
 * rows and counts. Queries nobody shows are only marked stale. Tables whose tab holds staged row
 * edits keep their page (see `tablesWithPendingEdits`); they are returned so the caller can say so.
 */
export function invalidateConnectionSchema(qc: QueryClient, connectionId: string): TableRef[] {
  for (const queryKey of [
    ["tables", connectionId],
    schemaKeys.schemas(connectionId),
    schemaKeys.databases(connectionId),
    schemaKeys.completion(connectionId),
    ["tableDdl", connectionId],
  ])
    void qc.invalidateQueries({ queryKey });
  const held = tablesWithPendingEdits(connectionId);
  const skip = new Set(held.map(tableKey));
  const predicate = (q: Query) => {
    const key = queryTable(q);
    return key === null || !skip.has(key);
  };
  for (const queryKey of [
    ["table", connectionId],
    ["rows", connectionId],
    ["count", connectionId],
  ])
    void qc.invalidateQueries({ queryKey, predicate });
  return held;
}

/**
 * The connection now points at another database (same connection id): nothing cached for the
 * old one may show again, not even while the new one loads. Metadata, rows, counts, DDL and
 * completion are dropped; the schema and table lists are reset, so the sidebar shows its loading
 * state (and picks the new database's default schema) instead of the old lists; the list of
 * databases (the same server) refetches.
 */
export function resetDatabaseCaches(qc: QueryClient, connectionId: string): void {
  for (const queryKey of [
    ["table", connectionId],
    ["rows", connectionId],
    ["count", connectionId],
    ["tableDdl", connectionId],
    schemaKeys.completion(connectionId),
  ])
    qc.removeQueries({ queryKey });
  void qc.resetQueries({ queryKey: schemaKeys.schemas(connectionId) });
  void qc.resetQueries({ queryKey: ["tables", connectionId] });
  void qc.invalidateQueries({ queryKey: schemaKeys.databases(connectionId) });
}
