import type { Value, ValueKind } from "@/lib/ipc";

export function isNull(v: Value): boolean {
  return v.t === "null";
}

export function valuesEqual(a: Value, b: Value): boolean {
  if (a.t !== b.t) return false;
  if (a.t === "null" || a.t === "default") return true;
  return JSON.stringify(a.v) === JSON.stringify((b as typeof a).v);
}

/** PostgreSQL-style array literal: `{"a","b",NULL}`. */
export function pgArrayLiteral(items: Value[]): string {
  return `{${items
    .map((item) => {
      if (item.t === "null" || item.t === "default") return "NULL";
      if (item.t === "array") return pgArrayLiteral(item.v);
      const text = valueText(item) ?? "";
      return `"${text.replace(/[\\"]/g, (ch) => `\\${ch}`)}"`;
    })
    .join(",")}}`;
}

/** Textual form of a value (what the backend binds as a parameter); null for NULL/DEFAULT. */
export function valueText(v: Value): string | null {
  switch (v.t) {
    case "null":
    case "default":
      return null;
    case "bool":
      return v.v ? "true" : "false";
    case "int":
    case "float":
      return String(v.v);
    case "bytes":
      return v.v.base64;
    case "other":
      return v.v.text;
    case "array":
      return pgArrayLiteral(v.v);
    default:
      return v.v;
  }
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

export type DisplayClass =
  "null" | "default" | "bool" | "num" | "text" | "json" | "bytes" | "other" | "temporal" | "uuid";

export interface DisplayValue {
  text: string;
  cls: DisplayClass;
}

const WS = /\s+/g;

/** Single-line display form for a grid cell. */
export function displayValue(v: Value): DisplayValue {
  switch (v.t) {
    case "null":
      return { text: "NULL", cls: "null" };
    case "default":
      return { text: "DEFAULT", cls: "default" };
    case "bool":
      return { text: v.v ? "true" : "false", cls: "bool" };
    case "int":
    case "float":
      return { text: String(v.v), cls: "num" };
    case "bigInt":
    case "decimal":
      return { text: v.v, cls: "num" };
    case "bytes":
      return { text: `<blob ${formatBytes(v.v.len)}>`, cls: "bytes" };
    case "json":
      return { text: v.v.replace(WS, " "), cls: "json" };
    case "array":
      return { text: pgArrayLiteral(v.v), cls: "json" };
    case "other":
      return { text: v.v.text, cls: "other" };
    case "date":
    case "time":
    case "timestamp":
    case "timestampTz":
    case "interval":
      return { text: v.v, cls: "temporal" };
    case "uuid":
      return { text: v.v, cls: "uuid" };
    case "text":
    case "enum":
      return { text: v.v.replace(/\r?\n/g, "↵"), cls: "text" };
  }
}

export function isNumericKind(kind: ValueKind): boolean {
  return kind === "int" || kind === "bigInt" || kind === "decimal" || kind === "float";
}

/** Full multi-line form for the inspector (pretty JSON, raw text). */
export function inspectText(v: Value): string {
  if (v.t === "json") {
    try {
      return JSON.stringify(JSON.parse(v.v), null, 2);
    } catch {
      return v.v;
    }
  }
  if (v.t === "bytes")
    return `${formatBytes(v.v.len)}${v.v.truncated ? " (preview truncated)" : ""}\nbase64: ${v.v.base64.slice(0, 2000)}`;
  return displayValue(v).text === "NULL" && v.t === "null" ? "NULL" : (valueText(v) ?? "");
}
