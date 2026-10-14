import type { ResultColumn, Value } from "@/lib/ipc";
import { displayValue, isNumericKind } from "@/lib/values";
import type { ChartDirective, ChartKind } from "./directive";

/** Points drawn per series; longer results are sampled evenly (first and last kept). */
export const MAX_POINTS = 5_000;
/** Series per chart: the categorical palette's slots (never cycled). */
export const MAX_SERIES = 8;
/** Pie slices: the largest ones, then one "Other". */
export const MAX_SLICES = 8;

export interface ChartSeries {
  name: string;
  values: (number | null)[];
}

export interface ChartData {
  kind: ChartKind;
  xName: string;
  /** The x value of each point, as the grid shows it. */
  categories: string[];
  /** Numeric x of each point (a line chart over a numeric column), else null: points in order. */
  xValues: number[] | null;
  series: ChartSeries[];
  /** Pie: the last slice is the "Other" fold of the smaller ones. */
  folded: boolean;
  /** What the chart leaves out, said under it (sampling, extra columns…). */
  notes: string[];
}

export type ChartBuild = { ok: true; data: ChartData } | { ok: false; error: string };

/** A value as a number for plotting; null when it is not one (NULL, text…). */
export function toNumber(v: Value | undefined): number | null {
  if (!v) return null;
  switch (v.t) {
    case "int":
    case "float":
      return Number.isFinite(v.v) ? v.v : null;
    case "bigInt":
    case "decimal": {
      const n = Number(v.v);
      return Number.isFinite(n) ? n : null;
    }
    case "text": {
      const t = v.v.trim();
      if (!/^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(t)) return null;
      return Number(t);
    }
    default:
      return null;
  }
}

const categoryText = (v: Value | undefined) => (v ? displayValue(v).text : "");

/** The column named `name` (exactly, else ignoring case). */
function findColumn(columns: readonly ResultColumn[], name: string): number {
  const exact = columns.findIndex((c) => c.name === name);
  if (exact >= 0) return exact;
  const lower = name.toLowerCase();
  return columns.findIndex((c) => c.name.toLowerCase() === lower);
}

/** Row indexes to draw: all of them, or an even sample of `max` (first and last included). */
export function sampleIndexes(n: number, max = MAX_POINTS): number[] {
  if (n <= max) return Array.from({ length: n }, (_, i) => i);
  const out: number[] = [];
  const step = (n - 1) / (max - 1);
  for (let i = 0; i < max; i++) out.push(Math.round(i * step));
  return out;
}

const plural = (n: number, word: string) =>
  `${n.toLocaleString("en-US")} ${word}${n === 1 ? "" : "s"}`;

/** What the directive draws from this result, or why it cannot. */
export function buildChartData(
  directive: ChartDirective,
  columns: readonly ResultColumn[],
  rows: readonly Value[][],
): ChartBuild {
  if (columns.length === 0) return { ok: false, error: "The result has no columns." };
  const notes: string[] = [];
  const xi = directive.x === null ? 0 : findColumn(columns, directive.x);
  if (xi < 0) return { ok: false, error: `The result has no column "${directive.x}".` };
  let yi: number[];
  if (directive.y.length > 0) {
    yi = [];
    for (const name of directive.y) {
      const i = findColumn(columns, name);
      if (i < 0) return { ok: false, error: `The result has no column "${name}".` };
      yi.push(i);
    }
  } else {
    yi = columns.flatMap((c, i) => (i !== xi && isNumericKind(c.kind) ? [i] : []));
    if (yi.length === 0)
      return {
        ok: false,
        error: "The result has no numeric column to plot. Name one with y: (e.g. y: total).",
      };
  }
  const limit = directive.kind === "pie" ? 1 : MAX_SERIES;
  if (yi.length > limit) {
    notes.push(
      directive.kind === "pie"
        ? `A pie chart shows one column: ${columns[yi[0]!]!.name}.`
        : `Only the first ${MAX_SERIES} y columns are drawn.`,
    );
    yi = yi.slice(0, limit);
  }
  const xName = columns[xi]!.name;
  if (directive.kind === "pie")
    return { ok: true, data: pieData(xi, yi[0]!, columns, rows, notes) };

  let picked = sampleIndexes(rows.length);
  if (picked.length < rows.length)
    notes.push(
      `Showing ${plural(picked.length, "point")} of ${rows.length.toLocaleString("en-US")} (sampled).`,
    );
  let xValues: number[] | null = null;
  if (directive.kind === "line" && isNumericKind(columns[xi]!.kind)) {
    const xs = picked.map((r) => toNumber(rows[r]?.[xi]));
    if (xs.every((v): v is number => v !== null)) {
      // A numeric x axis: points left to right whatever the row order.
      const order = picked.map((_, i) => i).sort((a, b) => xs[a]! - xs[b]! || a - b);
      picked = order.map((i) => picked[i]!);
      xValues = order.map((i) => xs[i]!);
    }
  }
  const categories = picked.map((r) => categoryText(rows[r]?.[xi]));
  const series = yi.map((c) => ({
    name: columns[c]!.name,
    values: picked.map((r) => toNumber(rows[r]?.[c])),
  }));
  return {
    ok: true,
    data: { kind: directive.kind, xName, categories, xValues, series, folded: false, notes },
  };
}

function pieData(
  xi: number,
  yi: number,
  columns: readonly ResultColumn[],
  rows: readonly Value[][],
  notes: string[],
): ChartData {
  const slices: { label: string; value: number }[] = [];
  let skipped = 0;
  for (const row of rows) {
    const value = toNumber(row[yi]);
    if (value === null || value <= 0) {
      skipped++;
      continue;
    }
    slices.push({ label: categoryText(row[xi]), value });
  }
  if (skipped > 0) notes.push(`${plural(skipped, "row")} without a positive value left out.`);
  slices.sort((a, b) => b.value - a.value);
  let folded = false;
  if (slices.length > MAX_SLICES) {
    const rest = slices.splice(MAX_SLICES - 1);
    slices.push({ label: "Other", value: rest.reduce((s, x) => s + x.value, 0) });
    folded = true;
    notes.push(`The ${plural(rest.length, "smallest slice")} are summed up in Other.`);
  }
  return {
    kind: "pie",
    xName: columns[xi]!.name,
    categories: slices.map((s) => s.label),
    xValues: null,
    series: [{ name: columns[yi]!.name, values: slices.map((s) => s.value) }],
    folded,
    notes,
  };
}
