import type { QueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { create } from "zustand";
import { tablesWithPendingEdits } from "@/features/structure/invalidate";
import { ipc, toIpcError } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";

/** Connection ▸ View Using Encoding…: which connection's dialog is open (see EncodingDialog). */
export const useEncodingDialog = create<{ connectionId: string | null }>()(() => ({
  connectionId: null,
}));

export function openEncodingDialog(connectionId: string): void {
  useEncodingDialog.setState({ connectionId });
}

/** UTF-8 encoding names of MySQL and PostgreSQL (case-insensitive); none or blank: the default. */
const UTF8_ENCODINGS = new Set(["utf8", "utf-8", "utf8mb3", "utf8mb4", "unicode"]);

/**
 * Whether `name` is a UTF-8 encoding. Tabler reads and writes text as UTF-8, so a session with
 * another one (latin1, sjis…) is read-only: the backend refuses writes (`conn_options`).
 */
export function isUtf8Encoding(name: string | null | undefined): boolean {
  const n = name?.trim().toLowerCase();
  return !n || UTF8_ENCODINGS.has(n);
}

/** Why a session that reads another encoding is read-only (View Using Encoding…). */
export const NON_UTF8_NOTE =
  "Tabler reads and writes text as UTF-8. With another encoding the session is read-only, so edits cannot be stored mis-encoded. Use Reset to leave it.";

export const encodingKey = (connectionId: string) => ["connectionEncoding", connectionId] as const;

/** Every cached query of the connection refetches (tabs, sidebar, completion…). */
export function reloadConnection(qc: QueryClient, connectionId: string): void {
  void qc.invalidateQueries({
    predicate: (q) => q.queryKey.some((part) => part === connectionId),
  });
}

/** Applies `encoding` (null: the connection's own) to the open session and reloads its tabs. */
export async function applyEncoding(
  qc: QueryClient,
  connectionId: string,
  encoding: string | null,
): Promise<boolean> {
  const held = tablesWithPendingEdits(connectionId);
  if (held.length > 0) {
    toast.error("Commit or discard the changes first", {
      description: `Tables with uncommitted edits: ${held.map((t) => t.name).join(", ")}`,
    });
    return false;
  }
  try {
    const info = await ipc.connections.setEncoding(connectionId, encoding);
    useConnectionsStore.getState().setInfo(info);
  } catch (err) {
    toast.error("Could not change the encoding", { description: toIpcError(err).message });
    return false;
  }
  reloadConnection(qc, connectionId);
  return true;
}
