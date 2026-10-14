import { timeOfDay } from "@/features/editor/historyGroups";
import { relativeTime } from "@/lib/format/time";
import type { HistoryEntry } from "@/lib/ipc";
import { hasNestedQuantifier, MAX_REGEX_LENGTH } from "./groups";

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

/** History search options (TablePlus b818): Match Case, Whole Word, Regex. */
export interface HistorySearchOptions {
  matchCase: boolean;
  wholeWord: boolean;
  regex: boolean;
}

export const NO_SEARCH_OPTIONS: HistorySearchOptions = {
  matchCase: false,
  wholeWord: false,
  regex: false,
};

/** localStorage: the search options, shared by the History pane and the ⌘H sheet. */
export const HISTORY_SEARCH_OPTIONS_KEY = "tabler.history.searchOptions";

export function readHistorySearchOptions(): HistorySearchOptions {
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(HISTORY_SEARCH_OPTIONS_KEY) ?? "null");
    if (!raw || typeof raw !== "object") return NO_SEARCH_OPTIONS;
    const o = raw as Record<string, unknown>;
    return {
      matchCase: o.matchCase === true,
      wholeWord: o.wholeWord === true,
      regex: o.regex === true,
    };
  } catch {
    return NO_SEARCH_OPTIONS;
  }
}

export function writeHistorySearchOptions(o: HistorySearchOptions): void {
  try {
    localStorage.setItem(HISTORY_SEARCH_OPTIONS_KEY, JSON.stringify(o));
  } catch {
    // Not persisted.
  }
}

/** Whether an entry came from an MCP client (`source` is "mcp"): its row carries a badge. */
export function isMcpEntry(e: Pick<HistoryEntry, "source">): boolean {
  return e.source === "mcp";
}

/**
 * The renderer-side search (what the list shows while the debounced fetch is on its way): null
 * for no search, `"invalid"` for a regex that does not compile. The backend applies the same
 * rules with Rust's `regex` (whose syntax is close to JavaScript's).
 *
 * A pattern that can backtrack catastrophically here (a repeated group that repeats inside, like
 * `(\w+,?\s*)+`, or one over `MAX_REGEX_LENGTH`) is never run in the renderer: V8 and JavaScriptCore
 * take seconds to minutes per statement on it, on the main thread, for every keystroke. Such a
 * term lets every entry through (`() => true`) and leaves the filtering to the backend, whose
 * `regex` crate matches in linear time.
 */
export function historyMatcher(
  term: string,
  o: HistorySearchOptions,
): ((sql: string) => boolean) | null | "invalid" {
  if (!term) return null;
  if (!o.matchCase && !o.wholeWord && !o.regex) {
    const needle = term.toLowerCase();
    return (sql) => sql.toLowerCase().includes(needle);
  }
  let body = o.regex ? term : term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  if (o.wholeWord) body = `\\b(?:${body})\\b`;
  let re: RegExp;
  try {
    re = new RegExp(body, o.matchCase ? "u" : "iu");
  } catch {
    return "invalid";
  }
  if (o.regex && (term.length > MAX_REGEX_LENGTH || hasNestedQuantifier(term))) return () => true;
  return (sql) => re.test(sql);
}

/** Several entries copied at once: each statement on its own line(s), `;`-terminated. */
export function entriesSql(entries: readonly Pick<HistoryEntry, "sql">[]): string {
  return entries.map((e) => e.sql.trim().replace(/;?$/, ";")).join("\n");
}

/** `n` years before `now` (the "Keep history for" cutoff). */
export function yearsAgo(n: number, now: Date = new Date()): Date {
  const d = new Date(now);
  d.setFullYear(d.getFullYear() - n);
  return d;
}
