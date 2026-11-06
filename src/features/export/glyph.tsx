import type { ReactNode } from "react";

/** A one-glyph choice (`"`, `;`, `.`): large and bold enough to read, in a cell wide enough to hit. */
export const mono = (label: string) => (
  <span className="inline-block min-w-5 text-center font-mono text-[13px] leading-none font-semibold">
    {label}
  </span>
);

/**
 * Segmented choices whose labels are characters (`,` `\n` `"…"`) shown with `mono`; worded ones
 * ("Tab", "Quote if needed") stay in the UI font.
 */
export function glyphOptions<T extends { label: string }>(
  options: readonly T[],
): Array<Omit<T, "label"> & { label: ReactNode }> {
  return options.map((o) => ({ ...o, label: /^[A-Za-z]/.test(o.label) ? o.label : mono(o.label) }));
}
