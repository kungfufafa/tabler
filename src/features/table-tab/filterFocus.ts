/** The control kinds a filter row has; ⌘↑ / ⌘↓ keep the focus on the same kind. */
export type FilterPart = "check" | "column" | "op" | "value" | "value2" | "apply";

/**
 * Focuses `part` of the filter row at `index` ("last": the bottom one) inside `bar`: the row's
 * value field when it has no such part, else its first control. False when the bar has no rows.
 */
export function focusFilterRow(
  bar: ParentNode | null | undefined,
  index: number | "last",
  part: FilterPart = "value",
): boolean {
  const rows = bar?.querySelectorAll<HTMLElement>("[data-filter-row]");
  if (!rows || rows.length === 0) return false;
  const at = index === "last" ? rows.length - 1 : Math.max(0, Math.min(index, rows.length - 1));
  const row = rows[at];
  const el =
    row?.querySelector<HTMLElement>(`[data-filter-part="${part}"]`) ??
    row?.querySelector<HTMLElement>("[data-filter-value]") ??
    row?.querySelector<HTMLElement>("input, button");
  el?.focus();
  return el !== undefined && el !== null;
}
