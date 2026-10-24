import { EmptyState } from "@/components/EmptyState";
import type { Driver } from "@/lib/ipc";
import { notAvailableFor } from "./catalog";

/**
 * What a pane of a core-only feature shows on a connection of another driver, instead of
 * handing its engine to the PostgreSQL, MySQL or SQLite code.
 */
export function NotAvailable({ driver, what }: { driver: Driver; what: string }) {
  return <EmptyState title={notAvailableFor(driver, what)} />;
}
