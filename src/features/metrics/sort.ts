import { toNumber } from "@/features/charts/data";
import type { Value } from "@/lib/ipc";
import { displayValue } from "@/lib/values";

const collator = new Intl.Collator("en-US", { numeric: true, sensitivity: "base" });

/**
 * Order of two cells in a data table: NULLs last (also descending), numbers by value, everything
 * else by its text (natural order: `item2` before `item10`).
 */
export function compareValues(a: Value | undefined, b: Value | undefined, desc = false): number {
  const an = !a || a.t === "null";
  const bn = !b || b.t === "null";
  if (an || bn) return an === bn ? 0 : an ? 1 : -1;
  const x = toNumber(a);
  const y = toNumber(b);
  const order =
    x !== null && y !== null ? x - y : collator.compare(displayValue(a).text, displayValue(b).text);
  return desc ? -order : order;
}
