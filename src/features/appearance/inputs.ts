import { parseColor, toHex } from "./color";

/** `#abc`, `abc`, `#aabbcc`, `aabbccdd` → a `#rrggbb[aa]` value; null when it is none of those. */
export function normalizeHexInput(text: string): string | null {
  const t = text.trim();
  const hex = t.startsWith("#") ? t : `#${t}`;
  const c = /^#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i.test(hex) ? parseColor(hex) : null;
  return c ? toHex(c) : null;
}

/** Parses typed text as a number clamped to [min, max], rounded to `step`; null when it is not one. */
export function parseDecimal(text: string, min: number, max: number, step: number): number | null {
  const n = Number.parseFloat(text.replace(",", "."));
  if (!Number.isFinite(n)) return null;
  const rounded = Math.round(n / step) * step;
  return Math.min(max, Math.max(min, Number(rounded.toFixed(4))));
}
