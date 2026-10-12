import type { Value, ValueKind } from "@/lib/ipc";
import { getPrefs } from "@/stores/prefs";

/**
 * Set Value ▸ "Now (local time)" (TablePlus calls it NOW()). A staged cell holds a value, not an
 * expression, so this is this computer's clock, not the server's NOW(): a plain timestamp gets the
 * local wall-clock time, which a server in another time zone reads as its own. `timestampTz` gets
 * the local UTC offset, so the server stores the right instant.
 */
export const NOW_LABEL = "Now (local time)";

/** Kinds Set Value ▸ Now applies to. */
export const NOW_KINDS: ReadonlySet<ValueKind> = new Set<ValueKind>([
  "date",
  "time",
  "timestamp",
  "timestampTz",
]);

const pad = (n: number, w = 2) => String(Math.trunc(Math.abs(n))).padStart(w, "0");

/**
 * The current moment as a value of `kind` (local time; `timestampTz` with the local offset), in
 * the textual forms the databases accept: `2026-10-12`, `14:03:09`, `2026-10-12 14:03:09`,
 * `2026-10-12 14:03:09+07:00`. Null for other kinds.
 */
export function nowValue(kind: ValueKind, at: Date = new Date()): Value | null {
  const date = `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}`;
  const time = `${pad(at.getHours())}:${pad(at.getMinutes())}:${pad(at.getSeconds())}`;
  switch (kind) {
    case "date":
      return { t: "date", v: date };
    case "time":
      return { t: "time", v: time };
    case "timestamp":
      return { t: "timestamp", v: `${date} ${time}` };
    case "timestampTz": {
      const offset = -at.getTimezoneOffset();
      const sign = offset < 0 ? "-" : "+";
      return {
        t: "timestampTz",
        v: `${date} ${time}${sign}${pad(offset / 60)}:${pad(offset % 60)}`,
      };
    }
    default:
      return null;
  }
}

/** Kinds where "Auto convert empty Numeric, UUID, JSON or Array to NULL" applies. */
export const AUTO_NULL_KINDS: ReadonlySet<ValueKind> = new Set<ValueKind>([
  "int",
  "bigInt",
  "decimal",
  "float",
  "uuid",
  "json",
  "array",
]);

/**
 * Whether empty input for a nullable `kind` column is staged as NULL (the Table Data preference,
 * on by default). Only an empty (or blank) text converts: the literal text `NULL` is a value like
 * any other. A NOT NULL column keeps its own error ("Enter a whole number"…) rather than a NULL
 * the server would refuse.
 */
export function emptyBecomesNull(
  col: { kind: ValueKind; nullable: boolean },
  text: string,
  enabled: boolean = getPrefs().gridAutoNullEmpty,
): boolean {
  return enabled && col.nullable && AUTO_NULL_KINDS.has(col.kind) && text.trim() === "";
}
