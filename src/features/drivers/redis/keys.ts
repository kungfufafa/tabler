import type { Value } from "@/lib/ipc";
import { valueText } from "@/lib/values";

/** The `TYPE` name of a RedisJSON document (`sqlgen::native::redis::TYPE_JSON`). */
export const TYPE_JSON = "ReJSON-RL";

/** The `dbN` of a table as its index (the sidebar lists databases as tables). */
export function dbOf(table: { name: string }): number {
  const m = /^db(\d+)$/.exec(table.name);
  return m ? Number(m[1]) : 0;
}

/** A cell as the editor's text; null for a value only previewed or not text. */
export function editableText(v: Value | undefined): string | null {
  if (!v) return null;
  if (v.t === "bytes") return null;
  if (v.t === "null") return "";
  return valueText(v);
}
