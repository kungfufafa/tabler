import { toNumber } from "@/features/charts/data";
import { directiveAbove } from "@/features/charts/directive";
import type { MetricsResult, Value } from "@/lib/ipc";
import { displayValue } from "@/lib/values";
import { inputValue, type BoardObject } from "./model";

/** The column named `name` (exactly, else ignoring case); -1 when there is none. */
export function columnIndex(result: MetricsResult, name: string | null): number {
  if (name === null) return result.columns.length > 0 ? 0 : -1;
  const exact = result.columns.findIndex((c) => c.name === name);
  if (exact >= 0) return exact;
  const lower = name.toLowerCase();
  return result.columns.findIndex((c) => c.name.toLowerCase() === lower);
}

/** A scoreboard's value: its column of the first row. */
export function scoreValue(
  result: MetricsResult | null | undefined,
  column: string | null,
): Value | null {
  if (!result) return null;
  const i = columnIndex(result, column);
  if (i < 0) return null;
  return result.rows[0]?.[i] ?? null;
}

/** A scoreboard's value as a number (null when it is not one). */
export function scoreNumber(
  result: MetricsResult | null | undefined,
  column: string | null,
): number | null {
  return toNumber(scoreValue(result, column) ?? undefined);
}

const compact = new Intl.NumberFormat("en-US", { notation: "compact", maximumFractionDigits: 1 });
const plain = new Intl.NumberFormat("en-US", { maximumFractionDigits: 2 });

/** A scoreboard number: grouped (1,284), compact from 10,000 on (12.9K, 4.2M). */
export function formatScore(n: number): string {
  return Math.abs(n) >= 10_000 ? compact.format(n) : plain.format(n);
}

/** The change since the previous refresh: signed, and as a percentage when it has a base. */
export function formatChange(current: number, previous: number): string {
  const diff = current - previous;
  const sign = diff > 0 ? "+" : diff < 0 ? "−" : "±";
  const abs = formatScore(Math.abs(diff));
  if (previous === 0) return `${sign}${abs}`;
  const pct = (Math.abs(diff) / Math.abs(previous)) * 100;
  return `${sign}${abs} (${sign}${pct >= 10 ? pct.toFixed(0) : pct.toFixed(1)}%)`;
}

/** One option of a dropdown: the value it sets, the text it shows. */
export interface OptionItem {
  value: Value;
  label: string;
}

/** Text that stands for a value (keys of a dropdown's options). */
export function valueKey(v: Value): string {
  return v.t === "null" ? "\u0000null" : displayValue(v).text;
}

/** A dropdown's options: its static list, or its result (value column, then label column). */
export function dropdownOptions(o: BoardObject, result: MetricsResult | null): OptionItem[] {
  if (o.optionsSource === "static")
    return o.options.map((opt) => ({ value: { t: "text", v: opt.value }, label: opt.label }));
  if (!result || result.columns.length === 0) return [];
  const seen = new Set<string>();
  const out: OptionItem[] = [];
  for (const row of result.rows) {
    const value = row[0];
    if (!value) continue;
    const key = valueKey(value);
    if (seen.has(key)) continue;
    seen.add(key);
    const label = row.length > 1 && row[1] ? displayValue(row[1]).text : displayValue(value).text;
    out.push({ value, label });
  }
  return out;
}

/**
 * The value a dropdown starts with: the option matching its default value, else the first
 * option (NULL without any).
 */
export function initialOption(o: BoardObject, options: readonly OptionItem[]): Value {
  const match = options.find((opt) => displayValue(opt.value).text === o.defaultValue);
  return (match ?? options[0])?.value ?? { t: "null" };
}

/** The variables a board's inputs and dropdowns set before anyone touched them. */
export function defaultVariables(objects: readonly BoardObject[]): Record<string, Value> {
  const out: Record<string, Value> = {};
  for (const o of objects) {
    if (!o.variable) continue;
    if (o.kind === "input") out[o.variable] = inputValue(o.inputType, o.defaultValue);
    else if (o.kind === "dropdown")
      out[o.variable] =
        o.optionsSource === "static"
          ? initialOption(o, dropdownOptions(o, null))
          : o.defaultValue !== ""
            ? { t: "text", v: o.defaultValue }
            : { t: "null" };
  }
  return out;
}

const isCommentLine = (t: string) =>
  t === "" || t.startsWith("--") || t.startsWith("#") || t.startsWith("/*") || t.startsWith("*");

/**
 * A chart's x / y columns: the object's own mapping, else a comment chart directive above its
 * query (`-- TablePlus BarChart x: month, y: revenue, cost`, spec §5.3), else none (the first
 * column against every numeric one).
 */
export function chartMapping(o: BoardObject): { x: string | null; y: string[] } {
  if (o.x !== null || o.y.length > 0) return { x: o.x, y: o.y };
  let offset = 0;
  for (const line of o.sql.split("\n")) {
    if (!isCommentLine(line.trim())) break;
    offset += line.length + 1;
  }
  const d = directiveAbove(o.sql, Math.min(offset, o.sql.length));
  return d ? { x: d.x, y: d.y } : { x: null, y: [] };
}
