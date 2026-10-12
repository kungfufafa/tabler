import type { Value } from "@/lib/ipc";
import { editText, hasLineBreak } from "@/lib/values";
import type { GridColumn } from "../types";

/** How an editor was left: the grid moves the focus accordingly (↩ down, ⇥ right, ⇧⇥ left). */
export type EditorExit = "enter" | "tab" | "shiftTab" | "blur";

/**
 * Which editor a cell opens in:
 * - `text` — single-line input (numbers right-aligned, monospace)
 * - `multiline` — textarea panel; used whenever the text holds a line break, so none is ever lost
 * - `json` — dialog with validation, Format and Minify
 * - `choice` — bool / enum picker (with NULL for nullable columns)
 * - `date` — text input plus a calendar for date / timestamp columns
 * - `null` — not editable inline (binary values)
 */
export type EditorKind = "text" | "multiline" | "json" | "choice" | "date" | null;

export function editorFor(
  col: Pick<GridColumn, "kind" | "enumValues">,
  value: Value,
  text: string = editText(value),
): EditorKind {
  switch (col.kind) {
    case "bytes":
      return null;
    case "bool":
      return "choice";
    case "enum":
      if ((col.enumValues?.length ?? 0) > 0) return "choice";
      break;
    case "json":
      return "json";
    case "date":
    case "timestamp":
    case "timestampTz":
      return "date";
    default:
      break;
  }
  return hasLineBreak(text) ? "multiline" : "text";
}

export const NULL_CHOICE = "\u0000null";

export interface Choice {
  /** `<option>` value: the text form, or `NULL_CHOICE`. */
  key: string;
  label: string;
}

/** Options of the `choice` editor: true/false or the enum labels, plus NULL when nullable. */
export function choicesFor(col: Pick<GridColumn, "kind" | "enumValues" | "nullable">): Choice[] {
  const values = col.kind === "bool" ? ["true", "false"] : (col.enumValues ?? []);
  const out: Choice[] = values.map((v) => ({ key: v, label: v }));
  if (col.nullable) out.push({ key: NULL_CHOICE, label: "NULL" });
  return out;
}

/** The choice matching a value (`NULL_CHOICE` for NULL; null when it matches no option). */
export function choiceKeyOf(value: Value, choices: readonly Choice[]): string | null {
  const key = value.t === "null" ? NULL_CHOICE : value.t === "default" ? null : editText(value);
  return key !== null && choices.some((c) => c.key === key) ? key : null;
}
