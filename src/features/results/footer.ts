import { formatCount, formatDuration } from "@/lib/format/time";

/**
 * The footer's row count when the row limit cut the result, as TablePlus words it:
 * "1,000 rows (auto limited 1,000 rows)". The plain count stays in it.
 */
export function autoLimitLabel(rows: number): string {
  const n = `${formatCount(rows)} row${rows === 1 ? "" : "s"}`;
  return `${n} (auto limited ${n})`;
}

/** Execution time of the statement ("6 ms", "1.20 s"); empty while it runs. */
export function durationLabel(ms: number | null): string {
  return ms === null ? "" : formatDuration(ms);
}

/** A result tab: "Result 2 · 1,204 rows · 23 ms". */
export function resultTabText(label: string, rows: number, durationMs: number | null): string {
  const parts = [label, `${formatCount(rows)} row${rows === 1 ? "" : "s"}`];
  if (durationMs !== null) parts.push(formatDuration(durationMs));
  return parts.join(" · ");
}

/** What a pinned result is called outside its tab (sidebar, export): "Result 2 (pinned)". */
export function pinnedLabel(label: string): string {
  return `${label} (pinned)`;
}

/** Header of a result set stacked in "Append All Results in One Tab": "Result 2 · 1,204 rows · 6 ms". */
export function appendedHeader(n: number, rows: number, durationMs: number | null): string {
  const parts = [`Result ${n}`, `${formatCount(rows)} row${rows === 1 ? "" : "s"}`];
  if (durationMs !== null) parts.push(formatDuration(durationMs));
  return parts.join(" · ");
}
