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

/**
 * Full multi-line form for the inspector: the raw text, with JSON exactly as stored. Never pretty-print
 * JSON with a `JSON.parse` round trip — it rounds integers beyond 2^53, drops `1.10`'s trailing zero,
 * collapses duplicate keys and rewrites `\u` escapes. Re-indent by whitespace only
 * (`reformatJson` in features/datagrid/editors/json.ts) instead.
 */
export function inspectText(v: Value): string {
  if (v.t === "json") return v.v;
  if (v.t === "bytes")
    return `${formatBytes(v.v.len)}${v.v.truncated ? " (preview truncated)" : ""}\nbase64: ${v.v.base64.slice(0, 2000)}`;
  return displayValue(v).text === "NULL" && v.t === "null" ? "NULL" : (valueText(v) ?? "");
}

/** Raw text shown in an inline editor (NULL/DEFAULT become empty). */
export function editText(v: Value): string {
  if (v.t === "null" || v.t === "default") return "";
  if (v.t === "bool") return v.v ? "true" : "false";
  return valueText(v) ?? "";
}

export type ParseResult = { ok: true; value: Value } | { ok: false; error: string };

const INT_RE = /^[+-]?\d+$/;
const DEC_RE = /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/;

/** Turns editor text into a typed value; never uses `Number()` for integers/decimals. */
export function parseEditText(kind: ValueKind, text: string, previous?: Value): ParseResult {
  const t = text.trim();
  switch (kind) {
    case "int":
    case "bigInt": {
      if (!INT_RE.test(t)) return { ok: false, error: "Enter a whole number" };
      const n = Number(t);
      return {
        ok: true,
        value: Number.isSafeInteger(n)
          ? { t: "int", v: n }
          : { t: "bigInt", v: t.replace(/^\+/, "") },
      };
    }
    case "decimal":
      return DEC_RE.test(t)
        ? { ok: true, value: { t: "decimal", v: t } }
        : { ok: false, error: "Enter a number" };
    case "float": {
      if (!DEC_RE.test(t) && !/^[+-]?(inf|infinity|nan)$/i.test(t))
        return { ok: false, error: "Enter a number" };
      const n = Number(t);
      return Number.isFinite(n)
        ? { ok: true, value: { t: "float", v: n } }
        : { ok: true, value: { t: "other", v: { ty: "float", text: t } } };
    }
    case "bool": {
      if (/^(true|t|1|yes|y|on)$/i.test(t)) return { ok: true, value: { t: "bool", v: true } };
      if (/^(false|f|0|no|n|off)$/i.test(t)) return { ok: true, value: { t: "bool", v: false } };
      return { ok: false, error: "Enter true or false" };
    }
    case "json": {
      try {
        JSON.parse(text);
        return { ok: true, value: { t: "json", v: text } };
      } catch (e) {
        return { ok: false, error: `Invalid JSON: ${(e as Error).message}` };
      }
    }
    case "uuid":
      return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(t)
        ? { ok: true, value: { t: "uuid", v: t } }
        : { ok: false, error: "Enter a UUID" };
    case "enum":
      return { ok: true, value: { t: "enum", v: text } };
    case "date":
      return { ok: true, value: { t: "date", v: t } };
    case "time":
      return { ok: true, value: { t: "time", v: t } };
    case "timestamp":
      return { ok: true, value: { t: "timestamp", v: t } };
    case "timestampTz":
      return { ok: true, value: { t: "timestampTz", v: t } };
    case "interval":
      return { ok: true, value: { t: "interval", v: t } };
    case "array":
      return {
        ok: true,
        value: { t: "other", v: { ty: previous?.t === "other" ? previous.v.ty : "array", text } },
      };
    case "bytes":
      return { ok: false, error: "Binary values cannot be edited inline" };
    case "bit":
    case "geometry":
    case "other":
      return {
        ok: true,
        value: { t: "other", v: { ty: previous?.t === "other" ? previous.v.ty : kind, text } },
      };
    default:
      return { ok: true, value: { t: "text", v: text } };
  }
}

/** True when the text spans several lines (needs the multi-line editor). */
export function hasLineBreak(text: string): boolean {
  return /[\r\n]/.test(text);
}

const DATE_PREFIX = /^(\d{4}-\d{2}-\d{2})([\s\S]*)$/;

/**
 * Splits a date/timestamp text into its leading `YYYY-MM-DD` part and the rest (time, offset…):
 * `"2024-03-05 12:00:00+07"` → `{ date: "2024-03-05", rest: " 12:00:00+07" }`. `date` is null when
 * the text does not start with a date.
 */
export function splitDatePart(text: string): { date: string | null; rest: string } {
  const m = DATE_PREFIX.exec(text.trim());
  return m ? { date: m[1]!, rest: m[2]! } : { date: null, rest: "" };
}

/** Replaces the date part of `text` with `date` (YYYY-MM-DD), keeping any time/offset part. */
export function mergeDatePart(text: string, date: string): string {
  const { date: old, rest } = splitDatePart(text);
  return old === null ? date : `${date}${rest}`;
}
