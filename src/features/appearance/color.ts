/**
 * Colour maths for the appearance feature: the two notations themes may use (`#rrggbb[aa]` and
 * `oklch(L C H [/ A])`), conversion to sRGB, alpha compositing and WCAG 2 contrast. Pure, so the
 * theme validator, the colour fields and the contrast tests all share it.
 */

/** sRGB channels and alpha, each 0–1. */
export interface Rgba {
  r: number;
  g: number;
  b: number;
  a: number;
}

const HEX = /^#([0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i;
const NUM = String.raw`[+-]?(?:\d+\.?\d*|\.\d+)`;
const OKLCH = new RegExp(
  String.raw`^oklch\(\s*(${NUM}%?)\s+(${NUM}%?)\s+(${NUM})(?:deg)?\s*(?:/\s*(${NUM}%?)\s*)?\)$`,
  "i",
);

/** Longest colour string a theme may hold. */
export const MAX_COLOR_LENGTH = 64;

const clamp01 = (n: number) => Math.min(1, Math.max(0, n));

/** `0.5` or `50%` → 0.5 (the scale is 1 for plain numbers and 100 for percentages). */
function ratio(text: string, percentScale = 1): number {
  return text.endsWith("%")
    ? Number.parseFloat(text) / 100
    : Number.parseFloat(text) / percentScale;
}

function parseHex(value: string): Rgba | null {
  const m = HEX.exec(value);
  if (!m) return null;
  let h = m[1]!;
  if (h.length <= 4) h = [...h].map((c) => c + c).join("");
  const byte = (i: number) => Number.parseInt(h.slice(i, i + 2), 16) / 255;
  return { r: byte(0), g: byte(2), b: byte(4), a: h.length === 8 ? byte(6) : 1 };
}

const toSrgb = (c: number) => (c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055);

/** Linear sRGB of an OKLab colour; components may lie outside 0–1 (out of gamut). */
function oklabToLinear(L: number, a: number, b: number): [number, number, number] {
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
}

const inGamut = (rgb: readonly number[]) => rgb.every((c) => c >= -0.0005 && c <= 1.0005);

/** OKLCH → sRGB; an out-of-gamut colour keeps its lightness and hue and loses chroma. */
function oklchToRgb(L: number, C: number, H: number): [number, number, number] {
  const h = (H * Math.PI) / 180;
  const at = (chroma: number) => oklabToLinear(L, chroma * Math.cos(h), chroma * Math.sin(h));
  let lin = at(C);
  if (!inGamut(lin)) {
    let lo = 0;
    let hi = C;
    for (let i = 0; i < 24; i++) {
      const mid = (lo + hi) / 2;
      if (inGamut(at(mid))) lo = mid;
      else hi = mid;
    }
    lin = at(lo);
  }
  const [r, g, b] = lin.map((c) => toSrgb(clamp01(c)));
  return [r!, g!, b!];
}

function parseOklch(value: string): Rgba | null {
  const m = OKLCH.exec(value);
  if (!m) return null;
  const L = clamp01(ratio(m[1]!));
  // Chroma: 100% is 0.4 in CSS Color 4.
  const C = Math.max(0, ratio(m[2]!, 1) * (m[2]!.endsWith("%") ? 0.4 : 1));
  const H = Number.parseFloat(m[3]!);
  const a = m[4] === undefined ? 1 : clamp01(ratio(m[4]));
  if (![L, C, H, a].every(Number.isFinite)) return null;
  const [r, g, b] = oklchToRgb(L, C, H);
  return { r, g, b, a };
}

/** Parses `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` and `oklch(L C H [/ A])`; null for anything else. */
export function parseColor(value: string): Rgba | null {
  const v = value.trim();
  if (v.length === 0 || v.length > MAX_COLOR_LENGTH) return null;
  return v.startsWith("#") ? parseHex(v) : parseOklch(v);
}

/** Whether a theme may hold `value` as a colour (the notations `parseColor` reads). */
export function isThemeColor(value: unknown): value is string {
  return typeof value === "string" && parseColor(value) !== null;
}

const hex2 = (c: number) =>
  Math.round(clamp01(c) * 255)
    .toString(16)
    .padStart(2, "0");

/** `#rrggbb`, or `#rrggbbaa` when `rgba` is translucent. */
export function toHex({ r, g, b, a }: Rgba): string {
  return `#${hex2(r)}${hex2(g)}${hex2(b)}${a < 0.9995 ? hex2(a) : ""}`;
}

/** Any theme colour as `#rrggbb[aa]` (`fallback` when it does not parse). */
export function colorToHex(value: string, fallback = "#000000"): string {
  const c = parseColor(value);
  return c ? toHex(c) : fallback;
}

/** `#rrggbb` only (what a native colour input holds): the alpha is dropped. */
export function colorToOpaqueHex(value: string, fallback = "#000000"): string {
  const c = parseColor(value);
  return c ? toHex({ ...c, a: 1 }) : fallback;
}

/** `top` painted over the opaque colour `bottom`. */
export function composite(top: Rgba, bottom: Rgba): Rgba {
  const a = top.a;
  return {
    r: top.r * a + bottom.r * (1 - a),
    g: top.g * a + bottom.g * (1 - a),
    b: top.b * a + bottom.b * (1 - a),
    a: 1,
  };
}

/** WCAG 2 relative luminance of an opaque colour. */
export function luminance({ r, g, b }: Rgba): number {
  const lin = (c: number) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

/**
 * WCAG 2 contrast ratio (1–21) of text `fg` on `bg`. A translucent `fg` is composited over `bg`;
 * `bg` must be opaque (composite it over the surface first).
 */
export function contrastRatio(fg: Rgba, bg: Rgba): number {
  const top = luminance(fg.a < 1 ? composite(fg, bg) : fg);
  const bottom = luminance(bg);
  const [hi, lo] = top >= bottom ? [top, bottom] : [bottom, top];
  return (hi + 0.05) / (lo + 0.05);
}

/** sRGB → OKLab, for perceptual distances. */
function toOklab({ r, g, b }: Rgba): [number, number, number] {
  const lin = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
  const [lr, lg, lb] = [lin(r), lin(g), lin(b)];
  const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb);
  const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb);
  const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb);
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ];
}

/** Perceptual distance of two opaque colours: OKLab distance × 100 (1 is about a just-noticeable step). */
export function colorDistance(a: Rgba, b: Rgba): number {
  const [l1, a1, b1] = toOklab(a);
  const [l2, a2, b2] = toOklab(b);
  return Math.hypot(l1 - l2, a1 - a2, b1 - b2) * 100;
}
