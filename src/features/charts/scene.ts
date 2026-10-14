import type { ChartData } from "./data";
import { formatPercent, formatTick, formatValue, linear, niceTicks } from "./scale";

/**
 * Lays a chart out as plain drawing primitives (a "scene"): the same scene renders as React SVG
 * in the results pane and as an SVG file for Copy as SVG / Export PNG. Colors are roles (`Paint`),
 * resolved to the app's CSS tokens on screen and to fixed colors in a file.
 *
 * Mark specs follow the app's chart rules: bars at most 24px thick with a 4px rounded data end and
 * a 2px gap between neighbours, 2px lines, hairline gridlines, a legend for two or more series,
 * text in text colors only.
 */

export type SeriesPaint =
  | "series-1"
  | "series-2"
  | "series-3"
  | "series-4"
  | "series-5"
  | "series-6"
  | "series-7"
  | "series-8";

export type Paint = "fg" | "muted" | "grid" | "axis" | "surface" | "other" | "hover" | SeriesPaint;

export const seriesPaint = (i: number): SeriesPaint => `series-${(i % 8) + 1}` as SeriesPaint;

export type SceneNode =
  | { t: "rect"; x: number; y: number; w: number; h: number; fill: Paint; rx?: number }
  | {
      t: "path";
      d: string;
      fill: Paint | "none";
      stroke?: Paint;
      strokeWidth?: number;
    }
  | { t: "line"; x1: number; y1: number; x2: number; y2: number; stroke: Paint }
  | {
      t: "circle";
      cx: number;
      cy: number;
      r: number;
      fill: Paint;
      stroke?: Paint;
      strokeWidth?: number;
    }
  | {
      t: "text";
      x: number;
      y: number;
      text: string;
      fill: Paint;
      anchor: "start" | "middle" | "end";
      size: number;
      weight?: number;
    };

export interface TooltipRow {
  label: string;
  value: string;
  paint: Paint;
  mark: "rect" | "line";
}

export interface TooltipData {
  title: string;
  rows: TooltipRow[];
}

export interface PieSlice {
  d: string;
  /** The slice moved out a little (hovered). */
  dLift: string;
  paint: Paint;
  /** Where its tooltip points (inside the slice). */
  ax: number;
  ay: number;
}

export interface Scene {
  width: number;
  height: number;
  title: string;
  nodes: SceneNode[];
  plot: { x: number; y: number; w: number; h: number };
  /** Bar / line: the x pixel of each point (hover snaps to the nearest). */
  xs: number[];
  /** Bar: the band each point owns; 0 for lines. */
  band: number;
  /** Line: per series, the y pixel of each point (null without a value). */
  ys: (number | null)[][];
  slices: PieSlice[];
  tooltip: (i: number) => TooltipData;
}

const PAD = 12;
const FONT = 11;
const TITLE_SIZE = 12;
const TITLE_H = 20;
const LEGEND_ROW = 18;
const X_AXIS_H = 20;
const BAR_MAX = 24;
const BAR_GAP = 2;
const RADIUS = 4;

/** Approximate text width (the layout runs without a DOM). */
const textW = (text: string, size = FONT) => text.length * size * 0.6;
const r2 = (n: number) => Math.round(n * 100) / 100;

/**
 * `text` cut to `max` characters with an ellipsis. Counts code points, so an emoji is never cut
 * in half (a lone surrogate breaks the exported SVG).
 */
export function clip(text: string, max: number): string {
  if (max <= 1) return text.length > 0 ? "…" : "";
  // UTF-16 units never undercount code points: a text this short fits.
  if (text.length <= max) return text;
  const head: string[] = [];
  for (const ch of text) {
    if (head.length === max) return `${head.slice(0, max - 1).join("")}…`;
    head.push(ch);
  }
  return text;
}

/** A text node whose `y` is the middle of the line. */
function label(
  x: number,
  y: number,
  text: string,
  anchor: "start" | "middle" | "end",
  fill: Paint = "muted",
  size = FONT,
  weight?: number,
): SceneNode {
  return {
    t: "text",
    x: r2(x),
    y: r2(y + size * 0.35),
    text,
    fill,
    anchor,
    size,
    ...(weight ? { weight } : {}),
  };
}

/** The chart's title: what is plotted against what. */
export function chartTitle(data: ChartData): string {
  const ys = data.series.map((s) => s.name).join(", ");
  return `${ys} by ${data.xName}`;
}

/** Legend items left to right, wrapping at `width`; returns its nodes and height. */
function legend(
  items: { text: string; paint: Paint }[],
  x0: number,
  y0: number,
  width: number,
  mark: "rect" | "line",
): { nodes: SceneNode[]; height: number } {
  const nodes: SceneNode[] = [];
  let x = x0;
  let y = y0 + LEGEND_ROW / 2;
  for (const item of items) {
    const text = clip(item.text, 28);
    const w = 16 + textW(text) + 14;
    if (x > x0 && x + w > x0 + width) {
      x = x0;
      y += LEGEND_ROW;
    }
    if (mark === "rect")
      nodes.push({ t: "rect", x: r2(x), y: r2(y - 5), w: 10, h: 10, fill: item.paint, rx: 2 });
    else
      nodes.push({
        t: "path",
        d: `M${r2(x)} ${r2(y)}H${r2(x + 12)}`,
        fill: "none",
        stroke: item.paint,
        strokeWidth: 2,
      });
    nodes.push(label(x + 16, y, text, "start", "fg"));
    x += w;
  }
  return { nodes, height: y - y0 + LEGEND_ROW / 2 };
}

/** A bar from `base` to `top` (pixel y), rounded at its data end. */
export function barPath(x: number, w: number, base: number, end: number): string {
  const up = end <= base;
  const top = Math.min(base, end);
  const bottom = Math.max(base, end);
  const r = Math.max(0, Math.min(RADIUS, w / 2, bottom - top));
  const [X, W, T, B, R] = [x, w, top, bottom, r].map(r2) as [
    number,
    number,
    number,
    number,
    number,
  ];
  if (R === 0) return `M${X} ${T}H${r2(X + W)}V${B}H${X}Z`;
  return up
    ? `M${X} ${B}V${r2(T + R)}A${R} ${R} 0 0 1 ${r2(X + R)} ${T}H${r2(X + W - R)}A${R} ${R} 0 0 1 ${r2(X + W)} ${r2(T + R)}V${B}Z`
    : `M${X} ${T}V${r2(B - R)}A${R} ${R} 0 0 0 ${r2(X + R)} ${B}H${r2(X + W - R)}A${R} ${R} 0 0 0 ${r2(X + W)} ${r2(B - R)}V${T}Z`;
}

function emptyScene(width: number, height: number, title: string, text: string): Scene {
  return {
    width,
    height,
    title,
    nodes: [
      label(PAD, PAD + TITLE_H / 2, title, "start", "fg", TITLE_SIZE, 600),
      label(width / 2, height / 2, text, "middle"),
    ],
    plot: { x: PAD, y: PAD, w: width - 2 * PAD, h: height - 2 * PAD },
    xs: [],
    band: 0,
    ys: [],
    slices: [],
    tooltip: () => ({ title: "", rows: [] }),
  };
}

/** Lays out `data` in a `width` × `height` box. */
export function layoutChart(data: ChartData, width: number, height: number): Scene {
  const title = chartTitle(data);
  if (data.categories.length === 0) return emptyScene(width, height, title, "No rows to chart");
  return data.kind === "pie"
    ? layoutPie(data, width, height, title)
    : layoutXY(data, width, height, title);
}

function layoutXY(data: ChartData, width: number, height: number, title: string): Scene {
  const bar = data.kind === "bar";
  const nodes: SceneNode[] = [
    label(
      PAD,
      PAD + TITLE_H / 2,
      clip(title, Math.floor((width - 2 * PAD) / (TITLE_SIZE * 0.6))),
      "start",
      "fg",
      TITLE_SIZE,
      600,
    ),
  ];
  let top = PAD + TITLE_H;
  if (data.series.length > 1) {
    const l = legend(
      data.series.map((s, i) => ({ text: s.name, paint: seriesPaint(i) })),
      PAD,
      top,
      width - 2 * PAD,
      bar ? "rect" : "line",
    );
    nodes.push(...l.nodes);
    top += l.height + 4;
  }

  // y scale over every series (bars always include their 0 baseline).
  let lo = Infinity;
  let hi = -Infinity;
  for (const s of data.series)
    for (const v of s.values) {
      if (v === null) continue;
      lo = Math.min(lo, v);
      hi = Math.max(hi, v);
    }
  if (!Number.isFinite(lo)) return emptyScene(width, height, title, "No numeric values to chart");
  if (bar) {
    lo = Math.min(lo, 0);
    hi = Math.max(hi, 0);
  }
  const plotH0 = height - top - PAD - X_AXIS_H;
  const yt = niceTicks(lo, hi, Math.max(2, Math.min(6, Math.floor(plotH0 / 40) + 1)));
  const tickText = yt.ticks.map((t) => formatTick(t, yt.step));
  const axisW = Math.max(...tickText.map((t) => textW(t))) + 8;
  const plot = {
    x: PAD + axisW,
    y: top + 4,
    w: Math.max(20, width - PAD * 2 - axisW),
    h: Math.max(20, plotH0 - 4),
  };
  const y = linear(yt.min, yt.max, plot.y + plot.h, plot.y);

  // Gridlines and y ticks (hairlines; the 0 line is the baseline).
  yt.ticks.forEach((t, i) => {
    const py = r2(y(t));
    nodes.push({
      t: "line",
      x1: plot.x,
      y1: py,
      x2: r2(plot.x + plot.w),
      y2: py,
      stroke: t === 0 ? "axis" : "grid",
    });
    nodes.push(label(plot.x - 6, py, tickText[i]!, "end"));
  });

  const n = data.categories.length;
  let xs: number[];
  let band = 0;
  const xLabels: SceneNode[] = [];
  if (bar || data.xValues === null) {
    if (bar) {
      band = plot.w / n;
      xs = data.categories.map((_, i) => plot.x + band * (i + 0.5));
    } else {
      xs = data.categories.map((_, i) =>
        n === 1 ? plot.x + plot.w / 2 : plot.x + (plot.w * i) / (n - 1),
      );
    }
    // Category labels, thinned to fit.
    const spacing = bar ? band : n > 1 ? plot.w / (n - 1) : plot.w;
    const longest = Math.min(24, Math.max(...data.categories.map((c) => c.length)));
    const stride = Math.max(1, Math.ceil((longest * FONT * 0.6 + 10) / spacing));
    const maxChars = Math.max(1, Math.floor((spacing * stride - 8) / (FONT * 0.6)));
    for (let i = 0; i < n; i += stride) {
      // Line charts run edge to edge: the outer labels hang inwards.
      const anchor = bar || n === 1 ? "middle" : i === 0 ? "start" : i === n - 1 ? "end" : "middle";
      xLabels.push(
        label(
          xs[i]!,
          plot.y + plot.h + X_AXIS_H / 2 + 2,
          clip(data.categories[i]!, Math.min(24, maxChars)),
          anchor,
        ),
      );
    }
  } else {
    const xv = data.xValues;
    const xt = niceTicks(Math.min(...xv), Math.max(...xv), Math.max(2, Math.floor(plot.w / 80)));
    const x = linear(xt.min, xt.max, plot.x, plot.x + plot.w);
    xs = xv.map(x);
    xt.ticks.forEach((t, i) => {
      const anchor = i === 0 ? "start" : i === xt.ticks.length - 1 ? "end" : "middle";
      xLabels.push(label(x(t), plot.y + plot.h + X_AXIS_H / 2 + 2, formatTick(t, xt.step), anchor));
    });
  }
  nodes.push(...xLabels);

  const ys: (number | null)[][] = [];
  if (bar) {
    const k = data.series.length;
    const inner = band * 0.8;
    let gap = k > 1 ? BAR_GAP : 0;
    let barW = Math.min(BAR_MAX, (inner - gap * (k - 1)) / k);
    if (barW < 2) {
      gap = 0;
      barW = Math.max(0.5, inner / k);
    }
    const groupW = k * barW + (k - 1) * gap;
    const base = y(0);
    data.series.forEach((s, j) => {
      s.values.forEach((v, i) => {
        if (v === null || v === 0) return;
        const x = xs[i]! - groupW / 2 + j * (barW + gap);
        nodes.push({ t: "path", d: barPath(x, barW, base, y(v)), fill: seriesPaint(j) });
      });
    });
  } else {
    data.series.forEach((s, j) => {
      const py = s.values.map((v) => (v === null ? null : r2(y(v))));
      ys.push(py);
      let d = "";
      let open = false;
      py.forEach((v, i) => {
        if (v === null) {
          open = false;
          return;
        }
        d += `${open ? "L" : "M"}${r2(xs[i]!)} ${v}`;
        open = true;
      });
      if (d) nodes.push({ t: "path", d, fill: "none", stroke: seriesPaint(j), strokeWidth: 2 });
      // A point with no neighbour draws no line: mark it.
      py.forEach((v, i) => {
        if (v === null || (py[i - 1] ?? null) !== null || (py[i + 1] ?? null) !== null) return;
        nodes.push({
          t: "circle",
          cx: r2(xs[i]!),
          cy: v,
          r: 4,
          fill: seriesPaint(j),
          stroke: "surface",
          strokeWidth: 2,
        });
      });
    });
  }

  const tooltip = (i: number): TooltipData => ({
    title: data.xValues && !bar ? formatValue(data.xValues[i] ?? 0) : (data.categories[i] ?? ""),
    rows: data.series.map((s, j) => {
      const v = s.values[i] ?? null;
      return {
        label: s.name,
        value: v === null ? "NULL" : formatValue(v),
        paint: seriesPaint(j),
        mark: bar ? "rect" : "line",
      };
    }),
  });
  return { width, height, title, nodes, plot, xs: xs.map(r2), band, ys, slices: [], tooltip };
}

/** A pie slice from angle `a0` to `a1` (radians, 0 = 12 o'clock, clockwise). */
function slicePath(cx: number, cy: number, r: number, a0: number, a1: number): string {
  const pt = (a: number) => `${r2(cx + r * Math.sin(a))} ${r2(cy - r * Math.cos(a))}`;
  if (a1 - a0 >= Math.PI * 2 - 1e-9)
    return `M${r2(cx)} ${r2(cy - r)}A${r2(r)} ${r2(r)} 0 1 1 ${r2(cx)} ${r2(cy + r)}A${r2(r)} ${r2(r)} 0 1 1 ${r2(cx)} ${r2(cy - r)}Z`;
  const large = a1 - a0 > Math.PI ? 1 : 0;
  return `M${r2(cx)} ${r2(cy)}L${pt(a0)}A${r2(r)} ${r2(r)} 0 ${large} 1 ${pt(a1)}Z`;
}

function layoutPie(data: ChartData, width: number, height: number, title: string): Scene {
  const nodes: SceneNode[] = [
    label(
      PAD,
      PAD + TITLE_H / 2,
      clip(title, Math.floor((width - 2 * PAD) / (TITLE_SIZE * 0.6))),
      "start",
      "fg",
      TITLE_SIZE,
      600,
    ),
  ];
  const values = data.series[0]?.values.map((v) => v ?? 0) ?? [];
  const total = values.reduce((a, b) => a + b, 0);
  const paints = values.map((_, i) =>
    data.folded && i === values.length - 1 ? ("other" as const) : seriesPaint(i),
  );
  const top = PAD + TITLE_H + 4;
  const area = { x: PAD, y: top, w: width - 2 * PAD, h: height - top - PAD };
  const side = area.w >= 360;
  const legendW = side ? Math.min(240, area.w * 0.42) : area.w;
  const legendH = side ? 0 : values.length * LEGEND_ROW + 8;
  const pieW = side ? area.w - legendW - 16 : area.w;
  const pieH = area.h - legendH;
  const r = Math.max(8, Math.min(pieW, pieH) / 2 - 4);
  const cx = area.x + pieW / 2;
  const cy = area.y + pieH / 2;

  const slices: PieSlice[] = [];
  let a = 0;
  values.forEach((v, i) => {
    const a1 = total > 0 ? a + (v / total) * Math.PI * 2 : a;
    const mid = (a + a1) / 2;
    const dx = Math.sin(mid) * 4;
    const dy = -Math.cos(mid) * 4;
    const d = slicePath(cx, cy, r, a, a1);
    slices.push({
      d,
      dLift: slicePath(cx + dx, cy + dy, r, a, a1),
      paint: paints[i]!,
      ax: r2(cx + Math.sin(mid) * r * 0.6),
      ay: r2(cy - Math.cos(mid) * r * 0.6),
    });
    nodes.push({
      t: "path",
      d,
      fill: paints[i]!,
      // The 2px surface gap between neighbouring slices.
      ...(values.length > 1 ? { stroke: "surface" as const, strokeWidth: 2 } : {}),
    });
    a = a1;
  });

  // Legend: swatch, label, share.
  const lx = side ? area.x + pieW + 16 : area.x;
  const ly0 = side ? cy - (values.length * LEGEND_ROW) / 2 : area.y + pieH + 8;
  const pctW = 48;
  const maxChars = Math.max(4, Math.floor((legendW - 16 - pctW) / (FONT * 0.6)));
  values.forEach((v, i) => {
    const ly = ly0 + i * LEGEND_ROW + LEGEND_ROW / 2;
    nodes.push({ t: "rect", x: r2(lx), y: r2(ly - 5), w: 10, h: 10, fill: paints[i]!, rx: 2 });
    nodes.push(label(lx + 16, ly, clip(data.categories[i] ?? "", maxChars), "start", "fg"));
    nodes.push(label(lx + legendW, ly, formatPercent(v, total), "end"));
  });

  const tooltip = (i: number): TooltipData => {
    const v = values[i] ?? 0;
    return {
      title: data.categories[i] ?? "",
      rows: [
        {
          label: data.series[0]?.name ?? "",
          value: `${formatValue(v)} (${formatPercent(v, total)})`,
          paint: paints[i] ?? "other",
          mark: "rect",
        },
      ],
    };
  };
  return {
    width,
    height,
    title,
    nodes,
    plot: { x: cx - r, y: cy - r, w: 2 * r, h: 2 * r },
    xs: [],
    band: 0,
    ys: [],
    slices,
    tooltip,
  };
}

/** The point nearest to pixel `x` (`xs` ascending), or null without points. */
export function nearestIndex(xs: readonly number[], x: number): number | null {
  if (xs.length === 0) return null;
  let lo = 0;
  let hi = xs.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (xs[mid]! < x) lo = mid + 1;
    else hi = mid;
  }
  if (lo > 0 && Math.abs(xs[lo - 1]! - x) <= Math.abs(xs[lo]! - x)) return lo - 1;
  return lo;
}
