import { fromPlain, toText } from "@/features/drivers/elasticsearch/json";
import type { Value } from "../../../bindings/Value";
import type { ValueKind } from "../../../bindings/ValueKind";

/** The mock's cells as the backend makes them (`values.rs`, `catalog::kind_of`). */

export const MISSING: Value = { t: "other", v: { ty: "missing", text: "" } };

/** The value kind of a mapping type (`catalog::kind_of`). */
export function kindOf(type: string): ValueKind {
  if (type.includes("|")) return "json";
  if (["long", "integer", "short", "byte", "token_count"].includes(type)) return "int";
  if (type === "unsigned_long") return "bigInt";
  if (["double", "float", "half_float", "scaled_float", "rank_feature"].includes(type))
    return "float";
  if (["keyword", "constant_keyword", "wildcard", "text", "match_only_text"].includes(type))
    return "text";
  if (type === "boolean") return "bool";
  if (type === "date" || type === "date_nanos") return "timestampTz";
  if (type === "ip") return "other";
  return "json";
}

/** A `_source` value as a cell (`values::to_value`). */
export function cellOf(v: unknown, type: string | null): Value {
  if (v === undefined) return MISSING;
  if (v === null) return { t: "null" };
  if (typeof v === "boolean") return { t: "bool", v };
  if (typeof v === "bigint")
    return Number.isSafeInteger(Number(v))
      ? { t: "int", v: Number(v) }
      : { t: "bigInt", v: String(v) };
  if (typeof v === "number") return Number.isInteger(v) ? { t: "int", v } : { t: "float", v };
  if (typeof v === "string") {
    if (type === "date" || type === "date_nanos") return { t: "timestampTz", v };
    if (type === "ip") return { t: "other", v: { ty: "ip", text: v } };
    return { t: "text", v };
  }
  return { t: "json", v: toText(fromPlain(v)) };
}

/** The value at a dotted path, through nested objects and literal dotted keys. */
export function at(source: Record<string, unknown>, path: string): unknown {
  if (path in source) return source[path];
  const dot = path.indexOf(".");
  for (let i = dot; i >= 0; i = path.indexOf(".", i + 1)) {
    const head = source[path.slice(0, i)];
    if (head !== null && typeof head === "object" && !Array.isArray(head)) {
      const found = at(head as Record<string, unknown>, path.slice(i + 1));
      if (found !== undefined) return found;
    }
  }
  return undefined;
}
