import type { Value } from "@/lib/ipc";
import { displayValue } from "@/lib/values";

/** Rows of an overflow result set shown as text (the rest is summarised). */
export const PLAIN_ROWS_MAX = 50;
/** Longest cell shown (longer ones end in `…`). */
export const PLAIN_CELL_MAX = 60;

function clip(text: string): string {
  return text.length > PLAIN_CELL_MAX ? `${text.slice(0, PLAIN_CELL_MAX - 1)}…` : text;
}

/**
 * A result set as a plain-text table (psql style), for runs with more result sets than result
 * tabs: a header, a rule, up to `PLAIN_ROWS_MAX` rows and the row count.
 */
export function formatPlainResult(
  columns: readonly string[],
  rows: readonly (readonly Value[])[],
  opts: { truncated?: boolean } = {},
): string {
  const shown = rows.slice(0, PLAIN_ROWS_MAX).map((r) =>
    columns.map((_, i) => {
      const v = r[i];
      return clip(v ? displayValue(v).text : "");
    }),
  );
  const widths = columns.map((c, i) => Math.max(clip(c).length, ...shown.map((r) => r[i]!.length)));
  const line = (cells: readonly string[]) =>
    cells
      .map((c, i) => ` ${c.padEnd(widths[i]!)} `)
      .join("|")
      .trimEnd();
  const out =
    columns.length > 0
      ? [line(columns.map(clip)), widths.map((w) => "-".repeat(w + 2)).join("+")]
      : [];
  for (const r of shown) out.push(line(r));
  const n = rows.length;
  const more = n > shown.length ? `, ${(n - shown.length).toLocaleString()} not shown` : "";
  out.push(
    `(${n.toLocaleString()} ${n === 1 ? "row" : "rows"}${more}${opts.truncated ? ", row limit reached" : ""})`,
  );
  return out.join("\n");
}
