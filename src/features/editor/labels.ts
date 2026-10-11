import { formatCount, formatDuration } from "@/lib/format/time";
import type { StatementResult } from "@/lib/ipc";
import type { RunSummary } from "./runModel";

/** "0.4 s", "12.3 s", "1m 5s" — compact elapsed time for a running query. */
export function formatElapsed(ms: number): string {
  if (ms < 60_000) return `${(Math.max(0, ms) / 1000).toFixed(1)} s`;
  return formatDuration(ms);
}

/** "1 row", "1,204 rows". */
export const plural = (n: number, one: string, many = `${one}s`): string =>
  `${formatCount(n)} ${n === 1 ? one : many}`;

export function rowsLabel(n: number): string {
  return plural(n, "row");
}

/** "Result 2 · 1,204 rows · 23 ms" */
export function resultTabLabel(n: number, rows: number, durationMs: number | null): string {
  const parts = [`Result ${n}`, rowsLabel(rows)];
  if (durationMs !== null) parts.push(formatDuration(durationMs));
  return parts.join(" · ");
}

/** What a finished statement did: "1,204 rows", "3 rows affected", "OK". */
export function statementOutcome(r: StatementResult): string {
  switch (r.kind) {
    case "rows":
    case "explain":
      return rowsLabel(r.rowCount) + (r.resultSets > 1 ? ` in ${r.resultSets} result sets` : "");
    case "affected":
      return `${plural(r.affected ?? 0, "row")} affected`;
    case "empty":
      return "OK";
  }
}

/** First line of a statement, shortened for one-line lists. */
export function firstLine(sql: string, max = 140): string {
  const line = sql.trim().split(/\r?\n/, 1)[0] ?? "";
  const more = line.length < sql.trim().length;
  if (line.length > max) return `${line.slice(0, max - 1)}…`;
  return more ? `${line} …` : line;
}

/** "3 statements · 45 ms", "1 statement · 1 error · 4 ms", "Cancelled · 1.20 s". */
export function runStatusText(s: RunSummary): string {
  const parts = [s.cancelled ? "Cancelled" : plural(s.statements, "statement")];
  if (s.errors > 0) parts.push(plural(s.errors, "error"));
  if (s.totalMs !== null) parts.push(formatDuration(s.totalMs));
  return parts.join(" · ");
}
