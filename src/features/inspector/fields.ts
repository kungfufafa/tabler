import { jsonError, reformatJson } from "@/features/datagrid/editors/json";
import { emptyBecomesNull } from "@/features/datagrid/setValue";
import type { GridColumn } from "@/features/datagrid/types";
import type { Value, ValueKind } from "@/lib/ipc";
import { editText, inspectText, parseEditText, type ParseResult } from "@/lib/values";

/** What "Copy" puts on the clipboard: the raw text (JSON verbatim), or a blob's base64. */
export function copyText(v: Value): string {
  return v.t === "bytes" ? v.v.base64 : inspectText(v);
}

/**
 * What the Cell view shows: like `copyText`, but valid JSON is re-indented. Only whitespace
 * changes — no `JSON.parse` round trip — so big integers, `1.10`, duplicate keys and escapes read
 * exactly as stored (and as the grid and the JSON editor show them).
 */
export function viewText(v: Value): string {
  if (v.t === "default") return "DEFAULT";
  if (v.t === "json" && jsonError(v.v) === null) return reformatJson(v.v, 2);
  return inspectText(v);
}

/** "13 chars · 2 lines" for text-like values, null otherwise. */
export function textStats(v: Value): string | null {
  if (v.t !== "text" && v.t !== "enum" && v.t !== "json") return null;
  const chars = Array.from(v.v).length;
  const lines = v.v === "" ? 1 : v.v.split(/\r\n|\r|\n/).length;
  return `${chars.toLocaleString()} char${chars === 1 ? "" : "s"} · ${lines.toLocaleString()} line${lines === 1 ? "" : "s"}`;
}

/**
 * Kinds whose Details box is a wrapping, auto-growing text area (long values wrap instead of
 * scrolling sideways, and line breaks survive); the other kinds get a one-line input.
 */
export const TEXTAREA_KINDS: ReadonlySet<ValueKind> = new Set([
  "text",
  "enum",
  "set",
  "json",
  "array",
  "other",
  "geometry",
]);

/** Kinds where an empty box means an empty string (elsewhere it means NULL when allowed). */
const TEXT_LIKE: ReadonlySet<ValueKind> = new Set(["text", "enum", "set"]);

/** bool, and enums with known labels: picked from a list instead of typed. */
export function isChoiceColumn(col: Pick<GridColumn, "kind" | "enumValues">): boolean {
  return col.kind === "bool" || (col.kind === "enum" && (col.enumValues?.length ?? 0) > 0);
}

/**
 * The text a field's box starts from (and is compared against to know whether it was edited):
 * the inline editor's text, with valid JSON re-indented by whitespace only. NULL / DEFAULT give ""
 * (the box shows a placeholder instead).
 */
export function fieldText(v: Value): string {
  if (v.t === "json" && jsonError(v.v) === null) return reformatJson(v.v, 2);
  if (v.t === "bytes") return "";
  return editText(v);
}

/** Placeholder of an empty box: what the stored value is (NULL, DEFAULT or an empty string). */
export function placeholderOf(v: Value): string | undefined {
  if (v.t === "null") return "NULL";
  if (v.t === "default") return "DEFAULT";
  return fieldText(v) === "" ? "EMPTY" : undefined;
}

/** Placeholder of a box the user cleared: what committing it would store. */
export function clearedPlaceholder(col: Pick<GridColumn, "kind" | "nullable">): string | undefined {
  if (TEXT_LIKE.has(col.kind)) return "EMPTY";
  return col.nullable ? "NULL" : undefined;
}

/**
 * Turns a field's text into a value of the column's kind, with the inline editor's parser
 * (`parseEditText`). A cleared box in a nullable, non-text column means NULL (as pasting an empty
 * cell does) — a blank (spaces only) one too in numeric / UUID / JSON / array columns while "Auto
 * convert empty … to NULL" is on; enum labels must be one of the column's labels when known.
 */
export function parseFieldText(col: GridColumn, text: string, previous: Value): ParseResult {
  if (text === "" && col.nullable && !TEXT_LIKE.has(col.kind))
    return { ok: true, value: { t: "null" } };
  if (emptyBecomesNull(col, text)) return { ok: true, value: { t: "null" } };
  const labels = col.enumValues ?? [];
  if (col.kind === "enum" && labels.length > 0 && !labels.includes(text))
    return { ok: false, error: `Pick one of: ${labels.join(", ")}` };
  return parseEditText(col.kind, text, previous);
}

/** Lower-cased search match on the column name. */
export function matchesField(col: Pick<GridColumn, "name">, query: string): boolean {
  const q = query.trim().toLowerCase();
  return q === "" || col.name.toLowerCase().includes(q);
}
