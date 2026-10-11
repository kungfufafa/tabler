import type { QueryClient } from "@tanstack/react-query";
import { schemaKeys } from "@/features/sidebar/useSchemaTree";
import { tableKey, type TableRef } from "@/lib/ipc";

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
