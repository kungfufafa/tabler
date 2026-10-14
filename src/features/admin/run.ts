import type { QueryClient } from "@tanstack/react-query";
import { guardExecution } from "@/features/safety/guard";
import { schemaKeys } from "@/features/sidebar/useSchemaTree";
import { invalidateSchema } from "@/features/structure/invalidate";
import { ipc, toIpcError, type AdminDdl, type AdminResult, type ErrorPayload } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { isTableTab, useTabsStore } from "@/stores/tabs";

export interface AdminErrorInfo {
  message: string;
  detail: string | null;
  hint: string | null;
}

export function errorInfo(e: unknown): AdminErrorInfo {
  const err = toIpcError(e);
  return {
    message: err.message,
    detail: err.payload.detail ?? null,
    hint: err.payload.hint ?? null,
  };
}

export function payloadInfo(p: ErrorPayload): AdminErrorInfo {
  return { message: p.message, detail: p.detail ?? null, hint: p.hint ?? null };
}

/** How an admin change ended: ran, declined at the safe-mode prompt, or failed. */
export type AdminOutcome =
  | { status: "done"; result: AdminResult }
  | { status: "cancelled" }
  | { status: "failed"; error: AdminErrorInfo; result: AdminResult | null };

/**
 * Asks the connection's safe mode first (`label` is what the prompt names), then runs `call`.
 * A failure inside the result and a rejected call both come back as `failed`.
 */
export async function guarded(
  connectionId: string,
  label: string,
  call: () => Promise<AdminResult>,
): Promise<AdminOutcome> {
  if (!(await guardExecution(connectionId, { kind: "operation", label, write: true })))
    return { status: "cancelled" };
  try {
    const result = await call();
    return result.error
      ? { status: "failed", error: payloadInfo(result.error), result }
      : { status: "done", result };
  } catch (e) {
    return { status: "failed", error: errorInfo(e), result: null };
  }
}

/** What the safe-mode prompt calls each database / schema change. */
export function ddlLabel(op: AdminDdl): string {
  switch (op.op) {
    case "createDatabase":
      return `Create database ${op.name}`;
    case "renameDatabase":
      return `Rename database ${op.name} to ${op.newName}`;
    case "dropDatabase":
      return `Drop database ${op.name}`;
    case "createSchema":
      return `Create schema ${op.name}`;
    case "dropSchema":
      return `Drop schema ${op.name}${op.cascade ? " (cascade)" : ""}`;
  }
}

/** Runs one database / schema change behind the safe-mode check and refreshes what it touched. */
export async function runAdminDdl(
  qc: QueryClient,
  connectionId: string,
  op: AdminDdl,
): Promise<AdminOutcome> {
  const outcome = await guarded(connectionId, ddlLabel(op), () =>
    ipc.admin.applyDdl(connectionId, op),
  );
  if (outcome.status !== "done") return outcome;
  if (op.op === "createSchema" || op.op === "dropSchema") {
    void qc.invalidateQueries({ queryKey: schemaKeys.schemas(connectionId) });
    invalidateSchema(qc, connectionId);
  } else {
    void qc.invalidateQueries({ queryKey: schemaKeys.databases(connectionId) });
  }
  if (op.op === "dropSchema") forgetSchema(connectionId, op.name);
  return outcome;
}

/**
 * After DROP SCHEMA: its tables' tabs close (nothing is left to show or commit), and the sidebar
 * falls back to the default schema if it showed the dropped one.
 */
function forgetSchema(connectionId: string, schema: string) {
  const tabs = useTabsStore.getState();
  const ids = tabs.tabs
    .filter((t) => isTableTab(t) && t.connectionId === connectionId && t.table.schema === schema)
    .map((t) => t.id);
  if (ids.length > 0) tabs.forceClose(ids);
  const conns = useConnectionsStore.getState();
  if (conns.byId[connectionId]?.currentSchema === schema)
    conns.setCurrentSchema(connectionId, null);
}
