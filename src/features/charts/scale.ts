/** Axis scales for the result charts: nice ticks and number formatting. */

export interface Ticks {
  /** The axis domain, widened to whole steps. */
  min: number;
  max: number;
  step: number;
  ticks: number[];
}

/** 1, 2, 5 or 10 times a power of ten, close to `x` (rounded, or the next one up). */
function niceNum(x: number, round: boolean): number {
  const exp = Math.floor(Math.log10(x));
  const f = x / 10 ** exp;
  const nice = round
    ? f < 1.5
      ? 1
      : f < 3
        ? 2
        : f < 7
          ? 5
          : 10
    : f <= 1
      ? 1
      : f <= 2
        ? 2
        : f <= 5
          ? 5
          : 10;
  return nice * 10 ** exp;
}

/** At least this many ticks are allowed before the guard against runaway tick lists stops. */
const MAX_TICKS = 50;

/**
 * Values this close next to their size are one value: AVG / SUM results that differ by rounding
 * noise only (0.1 vs 0.10000000000000002). Below it a step would be too small to move a float.
 */
const SAME_VALUE = 1e-12;

/** Slack for snapping the domain to whole steps, so rounding noise does not add a step. */
const SNAP = 1e-9;

/** Decimal places a multiple of `step` needs (0.02 → 2, 0.5 → 1, 20 → 0). */
const stepDecimals = (step: number) => Math.max(0, -Math.floor(Math.log10(step)));

/**
 * `v` rounded to `decimals` places: removes floating-point noise (0.30000000000000004 → 0.3)
 * while keeping every digit the step can tell apart (1700000000001 stays distinct).
 */
const clean = (v: number, decimals: number) =>
  Number.parseFloat(v.toFixed(Math.min(100, decimals)));

/** About `count` round ticks covering `[min, max]`. */
export function niceTicks(min: number, max: number, count = 5): Ticks {
  let lo = Number.isFinite(min) ? min : 0;
  let hi = Number.isFinite(max) ? max : 0;
  if (lo > hi) [lo, hi] = [hi, lo];
  if (hi - lo <= Math.max(Math.abs(lo), Math.abs(hi)) * SAME_VALUE) {
    // One value: an axis from 0 to it (or around it when it is 0).
    if (lo === 0 && hi === 0) hi = 1;
    else if (lo > 0) lo = 0;
    else hi = 0;
  }
  const span = hi - lo;
  const step = niceNum(niceNum(span, false) / Math.max(1, count - 1), true);
  const start = Math.floor(lo / step + SNAP) * step;
  const end = Math.ceil(hi / step - SNAP) * step;
  const n = Math.round((end - start) / step);
  if (!(step > 0) || ![step, start, end, n].every(Number.isFinite)) {
    // Past what a nice scale can describe (a span beyond Number.MAX_VALUE, subnormal steps).
    return { min: lo, max: hi, step: span, ticks: [lo, hi] };
  }
  // Ticks by index: adding `step` to a large `start` could leave it unchanged forever.
  const d = stepDecimals(step);
  const ticks: number[] = [];
  const last = Math.min(n, Math.max(MAX_TICKS, 4 * count));
  for (let i = 0; i <= last; i++) ticks.push(clean(start + i * step, d));
  return { min: clean(start, d), max: clean(end, d), step, ticks };
}

const COMPACT = new Intl.NumberFormat("en-US", { notation: "compact", maximumFractionDigits: 1 });

/** Decimals a step needs (0.25 → 2, 0.5 → 1, 5 → 0). */
function decimalsOf(step: number): number {
  let d = 0;
  while (d < 10 && Math.abs(Math.round(step * 10 ** d) - step * 10 ** d) > 1e-6) d++;
  return d;
}

/** An axis tick: thousands grouped, as many decimals as the step needs, compact past a million. */
export function formatTick(v: number, step: number): string {
  if (Math.abs(v) >= 1e6) return COMPACT.format(v);
  const digits = Number.isInteger(step) ? 0 : decimalsOf(step);
  return v.toLocaleString("en-US", {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
}

/** A value in a tooltip or legend: grouped, up to 2 decimals (4 below 1). */
export function formatValue(v: number): string {
  return v.toLocaleString("en-US", { maximumFractionDigits: Math.abs(v) < 1 ? 4 : 2 });
}

/** `part / total` as a percentage with one decimal ("42.1%"). */
export function formatPercent(part: number, total: number): string {
  if (total <= 0) return "0%";
  return `${((part / total) * 100).toFixed(1)}%`;
}

/** Maps `[d0, d1]` onto `[r0, r1]`. */
export function linear(d0: number, d1: number, r0: number, r1: number): (v: number) => number {
  const span = d1 - d0 || 1;
  return (v) => r0 + ((v - d0) / span) * (r1 - r0);
}
