import { exprValue } from "@/features/staging/expr";
import type { Value, ValueKind } from "@/lib/ipc";
import { getPrefs } from "@/stores/prefs";

/**
 * Set Value ▸ NOW() (TablePlus): the cell is staged as the server's own `NOW()`, sent as the SQL
 * expression (see `features/staging/expr.ts`), not as this computer's clock.
 */
export const NOW_LABEL = "NOW()";

/** Kinds Set Value ▸ NOW() applies to. */
export const NOW_KINDS: ReadonlySet<ValueKind> = new Set<ValueKind>([
  "date",
  "time",
  "timestamp",
  "timestampTz",
]);

/**
 * The staged value of NOW() in a column of `kind` (the server's `NOW()`; SQLite gets the
 * `CURRENT_*` keyword of the column's kind), null for kinds other than date / time ones.
 */
export function nowValue(kind: ValueKind): Value | null {
  return NOW_KINDS.has(kind) ? exprValue("now") : null;
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
