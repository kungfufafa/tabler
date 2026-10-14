/**
 * Bookkeeping for the console log's real SQL. The backend records the statements its sessions
 * really ran (`TracedSql`, drained with `drain_sql_trace`); the facade logs them after each of its
 * calls. Calls that already log the SQL they ran themselves (browse rows, row edits, DDL…) must
 * not log it twice, and a `-- …` description of a request is only worth a line when no SQL was
 * reported for it. Calls run concurrently, so a statement can be drained by another call than the
 * one that ran it: the ledger below pairs such statements up by their text.
 */

/** Whitespace and trailing semicolons do not tell two statements apart. */
export function normalizeSql(sql: string): string {
  return sql
    .replace(/\s+/g, " ")
    .trim()
    .replace(/[\s;]+$/, "");
}

/** The backend caps long statements and ends them with this marker (`cap_trace_text`). */
const CAP_MARKER = " /* … ";

/** Whether two normalized statements are the same one (the backend's text may be capped). */
export function sameStatement(a: string, b: string): boolean {
  if (a === b) return true;
  for (const [full, capped] of [
    [a, b],
    [b, a],
  ] as const) {
    const at = capped.lastIndexOf(CAP_MARKER);
    if (at > 0 && capped.endsWith(" */") && full.startsWith(capped.slice(0, at))) return true;
  }
  return false;
}

/** What replaces a password in a logged statement (the mask of the backend's previews). */
export const SECRET_MASK = "********";

/**
 * `text` with the secrets of the call (a password it sent) masked in every form the SQL could
 * have spelled them: as typed, with quotes doubled, with backslashes doubled too. The backend
 * already masks the passwords in the statements it records (`drivers/redact.rs`); this is the
 * second lock: whatever reaches the console log is scrubbed of the call's own secrets, so a
 * statement that slipped through unmasked still pairs with its masked twin and shows no password.
 * A secret of two characters or fewer is left alone (masking it would mangle the statement).
 */
export function maskSecrets(text: string, secrets: readonly string[] | undefined): string {
  if (!secrets?.length) return text;
  const forms = new Set<string>();
  for (const secret of secrets) {
    if (secret.length < 3) continue;
    forms.add(secret);
    forms.add(secret.replaceAll("'", "''"));
    forms.add(secret.replaceAll("\\", "\\\\").replaceAll("'", "''"));
    forms.add(secret.replaceAll("\\", "\\\\"));
  }
  // The longest first: `it''s` before `it`.
  return [...forms]
    .sort((a, b) => b.length - a.length)
    .reduce((acc, form) => acc.replaceAll(form, SECRET_MASK), text);
}

/** Who logged a statement: the call that ran it (`own`) or the drained trace (`trace`). */
export type LogSource = "own" | "trace";

interface Mark {
  text: string;
  source: LogSource;
  at: number;
  /** Already paired with the same statement from the other source. */
  matched: boolean;
}

/** How long a logged statement waits for its twin from another call. */
export const TWIN_WINDOW_MS = 2_000;
const MAX_MARKS = 200;

const marks = new Map<string, Mark[]>();
const lastTraceAt = new Map<string, number>();

const now = () => performance.now();

function recent(connectionId: string): Mark[] {
  const list = marks.get(connectionId) ?? [];
  const cutoff = now() - TWIN_WINDOW_MS;
  const keep = list.filter((m) => m.at >= cutoff).slice(-MAX_MARKS);
  marks.set(connectionId, keep);
  return keep;
}

/** Remembers that `text` was logged (`matched`: it already had its twin). */
export function noteLogged(
  connectionId: string,
  text: string,
  source: LogSource,
  matched = false,
): void {
  recent(connectionId).push({ text, source, at: now(), matched });
  if (source === "trace") lastTraceAt.set(connectionId, now());
}

/**
 * Whether the other source already logged `text` a moment ago and nobody paired it yet; pairs it
 * (each statement cancels at most one twin).
 */
export function takeTwin(connectionId: string, text: string, source: LogSource): boolean {
  const twin = recent(connectionId).find(
    (m) => !m.matched && m.source !== source && sameStatement(m.text, text),
  );
  if (!twin) return false;
  twin.matched = true;
  return true;
}

/** The trace reported a statement for the connection (whoever logged it): no description needed. */
export function markTraceSeen(connectionId: string): void {
  lastTraceAt.set(connectionId, now());
}

/** Whether the trace reported any statement for the connection since `since` (a `now()`). */
export function traceLoggedSince(connectionId: string, since: number): boolean {
  return (lastTraceAt.get(connectionId) ?? -1) >= since;
}

/** A time to compare with `traceLoggedSince`. */
export const traceClock = now;

/** Test helper: forget everything. */
export function resetTraceLedger(): void {
  marks.clear();
  lastTraceAt.clear();
}
