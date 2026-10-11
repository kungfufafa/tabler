import type { HistoryEntry } from "@/lib/ipc";

/** react-query key prefix of every history listing (invalidated after each run). */
export const HISTORY_KEY = ["history"] as const;

export interface HistoryGroup {
  /** Local calendar day, `YYYY-MM-DD`. */
  key: string;
  label: string;
  entries: HistoryEntry[];
}

const pad = (n: number) => String(n).padStart(2, "0");

export function dayKey(d: Date): string {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

function dayLabel(d: Date, now: Date): string {
  const today = dayKey(now);
  const key = dayKey(d);
  if (key === today) return "Today";
  const yesterday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1);
  if (key === dayKey(yesterday)) return "Yesterday";
  return d.toLocaleDateString(undefined, {
    weekday: "short",
    day: "numeric",
    month: "short",
    year: d.getFullYear() === now.getFullYear() ? undefined : "numeric",
  });
}

/** Groups newest-first entries by local day ("Today", "Yesterday", "Mon, 6 Oct"…), keeping order. */
export function groupByDay(
  entries: readonly HistoryEntry[],
  now: Date = new Date(),
): HistoryGroup[] {
  const groups: HistoryGroup[] = [];
  for (const e of entries) {
    const d = new Date(e.startedAt);
    const valid = !Number.isNaN(d.getTime());
    const key = valid ? dayKey(d) : "unknown";
    let group = groups[groups.length - 1];
    if (!group || group.key !== key) {
      group = { key, label: valid ? dayLabel(d, now) : "Unknown date", entries: [] };
      groups.push(group);
    }
    group.entries.push(e);
  }
  return groups;
}

/** "14:03:27" in local time. */
export function timeOfDay(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}
