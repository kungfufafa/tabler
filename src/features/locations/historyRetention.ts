import { yearsAgo } from "@/features/sidebar/historyEntry";
import { ipc } from "@/lib/ipc";
import type { HistoryRetention } from "@/stores/prefs";

const YEARS: Record<Exclude<HistoryRetention, "forever">, number> = { "1y": 1, "2y": 2, "3y": 3 };

/** The "Keep history for" cutoff: entries before it go; null keeps everything. */
export function retentionCutoff(retention: HistoryRetention, now: Date = new Date()): Date | null {
  return retention === "forever" ? null : yearsAgo(YEARS[retention], now);
}

/** Deletes the history older than the retention allows; returns how many entries went. */
export async function pruneHistory(retention: HistoryRetention, now = new Date()): Promise<number> {
  const cutoff = retentionCutoff(retention, now);
  if (!cutoff) return 0;
  return ipc.history.clearBefore(cutoff.toISOString(), null);
}
