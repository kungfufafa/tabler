import type { QueryClient } from "@tanstack/react-query";
import { schemaKeys } from "@/features/sidebar/useSchemaTree";
import { ipc, tableKey, type DdlOp, type TableRef } from "@/lib/ipc";
import { sameTable } from "@/stores/tabs";
import { ddlKey, invalidateSchema, reportHeldTables, tablesWithPendingEdits } from "./invalidate";
import { stagedRename } from "./objects";
import { changesSchema } from "./previewState";
import { followRename } from "./rename";
import { useStructureStore } from "./store";

/**
 * Reloads the metadata of `target` (the structure view shows the table as it is now); with
 * `clear`, drops the tab's staged ops in the same render. Every list of tables, the DDL text and
 * the table's data tab reload too, unless that tab holds staged row edits: they point at columns
 * by position, which the new structure may have moved.
 */
export async function reloadStructure(
  qc: QueryClient,
  connectionId: string,
  tabId: string,
  target: TableRef,
  clear: boolean,
): Promise<void> {
  const discard = () => useStructureStore.getState().discard(tabId);
  const key = schemaKeys.table(connectionId, target.schema, target.name);
  try {
    await ipc.schema.invalidate(connectionId, target);
    const fresh = await ipc.schema.table(connectionId, target, true);
    qc.setQueryData(key, fresh);
    if (clear) discard();
  } catch {
    if (clear) discard();
    void qc.invalidateQueries({ queryKey: key });
  }
  invalidateSchema(qc, connectionId);
  void qc.invalidateQueries({ queryKey: ddlKey(connectionId, target) });
  if (tablesWithPendingEdits(connectionId).some((t) => sameTable(t, target))) {
    reportHeldTables([target]);
  } else {
    void qc.invalidateQueries({ queryKey: ["rows", connectionId, tableKey(target)] });
    void qc.invalidateQueries({ queryKey: ["count", connectionId, tableKey(target)] });
  }
}

/**
 * After the workspace commit applied a structure tab's staged ops: a rename (the Name field, run
 * last) makes the tab follow the table to its new name, then the structure reloads without the
 * staged ops.
 */
export async function finishStructureCommit(
  qc: QueryClient,
  connectionId: string,
  tabId: string,
  table: TableRef,
  ops: readonly DdlOp[],
): Promise<void> {
  const renamed = stagedRename(ops, table);
  const target = renamed ? followRename(qc, connectionId, table, renamed) : table;
  await reloadStructure(qc, connectionId, tabId, target, true);
}

/**
 * After the workspace commit stopped in a structure tab's ops: when statements of it already ran
 * (MySQL commits DDL statement by statement), the staged ops no longer match the table, which
 * the view says until they are discarded. When none ran (a transaction that rolled back, or the
 * one `ALTER TABLE` of a MySQL plan, which fails as a whole), all changes were reverted: the
 * table is as it was and the staged ops are still here, which the view says too.
 */
export function failStructureCommit(tabId: string, executed: readonly string[]): void {
  const ran = executed.filter(changesSchema).length;
  if (ran > 0) useStructureStore.getState().markPartial(tabId, ran);
  else useStructureStore.getState().markReverted(tabId);
}
