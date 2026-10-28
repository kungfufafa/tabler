import type { Driver, ProcessRow } from "@/lib/ipc";

export type ProcessColumnId =
  | "id"
  | "user"
  | "client"
  | "database"
  | "application"
  | "command"
  | "duration"
  | "state"
  | "waitEvent"
  | "query";

export interface ProcessColumn {
  id: ProcessColumnId;
  label: string;
  /** Tailwind width class of the column. */
  width: string;
  numeric?: boolean;
}

/** PostgreSQL: `pg_stat_activity`. */
const PG_COLUMNS: ProcessColumn[] = [
  { id: "id", label: "pid", width: "w-16", numeric: true },
  { id: "user", label: "User", width: "w-24" },
  { id: "database", label: "Database", width: "w-28" },
  { id: "application", label: "Application", width: "w-28" },
  { id: "client", label: "Client", width: "w-32" },
  { id: "state", label: "State", width: "w-28" },
  { id: "duration", label: "Started", width: "w-20", numeric: true },
  { id: "waitEvent", label: "Wait event", width: "w-28" },
  { id: "query", label: "Query", width: "" },
];

/** MySQL: `information_schema.PROCESSLIST`, its own column names. */
const MYSQL_COLUMNS: ProcessColumn[] = [
  { id: "id", label: "Id", width: "w-16", numeric: true },
  { id: "user", label: "User", width: "w-24" },
  { id: "client", label: "Host", width: "w-36" },
  { id: "database", label: "db", width: "w-28" },
  { id: "command", label: "Command", width: "w-24" },
  { id: "duration", label: "Time", width: "w-16", numeric: true },
  { id: "state", label: "State", width: "w-32" },
  { id: "query", label: "Info", width: "" },
];

export function processColumns(driver: Driver): ProcessColumn[] {
  return driver === "mysql" ? MYSQL_COLUMNS : PG_COLUMNS;
}

export interface ProcessSort {
  column: ProcessColumnId;
  desc: boolean;
}

/** The value a column sorts by. */
function sortValue(row: ProcessRow, column: ProcessColumnId): string | number | null {
  switch (column) {
    case "id":
      return row.id;
    case "duration":
      return row.durationMs;
    case "waitEvent":
      return row.waitEvent;
    default:
      return row[column];
  }
}

const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });

/**
 * Process ids in their numeric order (PG pids, MySQL thread ids, which may pass 2^53): fewer
 * digits first, then digit by digit. Ids that are not numbers (other engines name theirs) sort as
 * text.
 */
export function compareIds(a: string, b: string): number {
  const digits = /^\d+$/;
  if (digits.test(a) && digits.test(b)) {
    const x = a.replace(/^0+(?=\d)/, "");
    const y = b.replace(/^0+(?=\d)/, "");
    return x.length - y.length || (x < y ? -1 : x > y ? 1 : 0);
  }
  return collator.compare(a, b);
}

/** Sorted copy; empty values last whatever the direction, ties by id. */
export function sortProcesses(rows: readonly ProcessRow[], sort: ProcessSort): ProcessRow[] {
  return [...rows].sort((a, b) => {
    const va = sortValue(a, sort.column);
    const vb = sortValue(b, sort.column);
    if (va === null || va === "") return vb === null || vb === "" ? compareIds(a.id, b.id) : 1;
    if (vb === null || vb === "") return -1;
    const cmp =
      sort.column === "id"
        ? compareIds(a.id, b.id)
        : typeof va === "number" && typeof vb === "number"
          ? va - vb
          : collator.compare(String(va), String(vb));
    return (sort.desc ? -cmp : cmp) || compareIds(a.id, b.id);
  });
}

/** A session doing nothing: PG `idle`, MySQL `Sleep`. */
export function isIdle(row: ProcessRow): boolean {
  return row.state === "idle" || row.command === "Sleep";
}

export interface ProcessFilter {
  /** Only sessions on this database; null = all. */
  database: string | null;
  hideIdle: boolean;
}

export function filterProcesses(rows: readonly ProcessRow[], filter: ProcessFilter): ProcessRow[] {
  return rows.filter(
    (r) =>
      (filter.database === null || r.database === filter.database) &&
      !(filter.hideIdle && isIdle(r)),
  );
}

/** Databases the sessions are on, for the filter. */
export function processDatabases(rows: readonly ProcessRow[]): string[] {
  const names = new Set<string>();
  for (const r of rows) if (r.database) names.add(r.database);
  return [...names].sort(collator.compare);
}

/** `850 ms`, `12 s`, `3 min 4 s`, `2 h 5 min`, `3 d 4 h`. */
export function formatDuration(ms: number | null): string {
  if (ms === null || !Number.isFinite(ms)) return "";
  if (ms < 0) ms = 0;
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return s % 60 ? `${m} min ${s % 60} s` : `${m} min`;
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `${h} h ${m % 60} min` : `${h} h`;
  const d = Math.floor(h / 24);
  return h % 24 ? `${d} d ${h % 24} h` : `${d} d`;
}

/** The text a cell shows. */
export function cellText(row: ProcessRow, column: ProcessColumnId): string {
  switch (column) {
    case "id":
      return row.id;
    case "duration":
      return formatDuration(row.durationMs);
    case "query":
      // One line in the table; the detail strip shows it whole.
      return (row.query ?? "").replace(/\s+/g, " ").trim();
    case "waitEvent":
      return row.waitEvent ?? "";
    default:
      return row[column] ?? "";
  }
}

/** Ids picked with a click: plain = only it, ⌘ = toggle, ⇧ = range from the anchor. */
export function clickSelection(
  ordered: readonly string[],
  selected: ReadonlySet<string>,
  anchor: string | null,
  id: string,
  mods: { toggle: boolean; range: boolean },
): Set<string> {
  if (mods.range && anchor !== null) {
    const from = ordered.indexOf(anchor);
    const to = ordered.indexOf(id);
    if (from >= 0 && to >= 0) {
      const [a, b] = from < to ? [from, to] : [to, from];
      return new Set(ordered.slice(a, b + 1));
    }
  }
  if (mods.toggle) {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    return next;
  }
  return new Set([id]);
}

/** Summary of a kill run for the toast. */
export function killSummary(
  outcomes: ReadonlyArray<{ id: string; ok: boolean; message: string | null }>,
  scope: "query" | "connection",
): { ok: number; failed: Array<{ id: string; message: string }>; title: string } {
  const ok = outcomes.filter((o) => o.ok).length;
  const failed = outcomes
    .filter((o) => !o.ok)
    .map((o) => ({ id: o.id, message: o.message ?? "failed" }));
  const what = scope === "query" ? "Cancelled the query of" : "Killed the connection of";
  const noun = (n: number) => (n === 1 ? "process" : "processes");
  const title =
    failed.length === 0
      ? `${what} ${ok} ${noun(ok)}`
      : ok === 0
        ? `Could not kill ${failed.length} ${noun(failed.length)}`
        : `${what} ${ok} ${noun(ok)}, ${failed.length} failed`;
  return { ok, failed, title };
}
