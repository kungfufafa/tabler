import { timeOfDay } from "@/features/editor/historyGroups";
import { relativeTime } from "@/lib/format/time";
import type { HistoryEntry } from "@/lib/ipc";

/**
 * When an entry ran: relative within the last day ("just now", "5 min ago", "3 h ago"), else the
 * time of day (its day header gives the date).
 */
export function entryTime(iso: string, now: Date = new Date()): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "";
  if (now.getTime() - at.getTime() < 86_400_000) return relativeTime(iso, now);
  return timeOfDay(iso).slice(0, 5);
}

/** Tooltip of a history row: the SQL (capped), when and where it ran, and its error. */
export function entryTitle(e: HistoryEntry): string {
  const sql = e.sql.length > 600 ? `${e.sql.slice(0, 600)}…` : e.sql;
  const where = e.database ? `${e.connectionName} / ${e.database}` : e.connectionName;
  const lines = [sql, "", `${new Date(e.startedAt).toLocaleString()} · ${where}`];
  if (!e.success && e.error) lines.push(e.error);
  return lines.join("\n");
}
