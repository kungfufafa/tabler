import { EmptyState } from "@/components/EmptyState";
import type { DatabaseKind } from "@/lib/ipc";
import { notAvailableFor } from "./catalog";

/**
 * What a pane of a core-only feature shows on a connection of another database (`sessionKind`),
 * instead of handing its engine to the PostgreSQL, MySQL or SQLite code.
 */
export function NotAvailable({ kind, what }: { kind: DatabaseKind; what: string }) {
  return <EmptyState title={notAvailableFor(kind, what)} />;
}
