import { exprValue, parseServerExpr } from "@/features/staging/expr";
import type { Value } from "@/lib/ipc";
import { parseEditText, type ParseResult } from "@/lib/values";
import { NOW_KINDS } from "./setValue";
import type { GridColumn } from "./types";

/**
 * Typed in a date / time cell, these mean the server's own clock (TablePlus accepts NOW() there):
 * the cell is staged as that SQL expression (`features/staging/expr.ts`), not as this computer's
 * time. A typed precision (`NOW(6)`, `CURRENT_TIMESTAMP(3)`) stays part of the expression.
 */
export function isNowText(text: string): boolean {
  return parseServerExpr(text) !== null;
}

/** A blob's bytes as hex (`48656c6c6f`), what the inline editor and Quick Look edit (see `values.ts`). */
export { bytesHex } from "@/lib/values";

/**
 * Hex text as a blob value: an optional `\x` / `0x` prefix, any whitespace between digits, an
 * even number of hex digits.
 */
export function parseHexBytes(text: string): ParseResult {
  const digits = text
    .trim()
    .replace(/^(\\x|0x)/i, "")
    .replace(/\s+/g, "");
  if (!/^(?:[0-9a-fA-F]{2})*$/.test(digits))
    return { ok: false, error: "Enter hex bytes (pairs of 0-9 / a-f)" };
  let bin = "";
  for (let i = 0; i < digits.length; i += 2)
    bin += String.fromCharCode(parseInt(digits.slice(i, i + 2), 16));
  return {
    ok: true,
    value: { t: "bytes", v: { base64: btoa(bin), len: bin.length, truncated: false } },
  };
}

/**
 * Editor text as a value of `col`'s kind: `parseEditText`, plus NOW() / NOW(6) / CURRENT_TIMESTAMP
 * in date and time columns and hex in binary ones (an emptied nullable blob is NULL).
 */
export function parseCellText(
  col: Pick<GridColumn, "kind" | "nullable">,
  text: string,
  previous?: Value,
): ParseResult {
  if (NOW_KINDS.has(col.kind)) {
    const expr = parseServerExpr(text);
    if (expr) return { ok: true, value: exprValue(expr) };
  }
  if (col.kind === "bytes") {
    if (text.trim() === "" && col.nullable) return { ok: true, value: { t: "null" } };
    return parseHexBytes(text);
  }
  return parseEditText(col.kind, text, previous);
}

/** Whether a blob value is held in full (only then can it be edited as hex). */
export function wholeBytes(v: Value): boolean {
  return v.t !== "bytes" || !v.v.truncated;
}
