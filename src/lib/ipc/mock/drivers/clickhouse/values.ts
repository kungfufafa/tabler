import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { Value } from "../../../bindings/Value";
import {
  baseType,
  decimalOf,
  display,
  enumLabels,
  integer,
  isNullable,
  kindOf,
  NULL,
} from "./data";

/** Values as ClickHouse text and back, and the errors the backend's payloads carry. */

/**
 * An exception of the server (`Code: N. DB::Exception: <message>. (<NAME>)`), as
 * `format::exception_error` makes it a `Db` error: the message with its name, the code as the
 * vendor code.
 */
export class ServerFailure extends Error {
  constructor(
    readonly code: number,
    readonly text: string,
    readonly name: string,
  ) {
    // The server ends its sentences with a period before the name.
    super(`${text.endsWith(".") ? text : `${text}.`} (${name})`);
  }
}

/** The payload of an error, as `AppError::to_payload` sends it. */
export function payload(
  code: string,
  message: string,
  extra: Partial<ErrorPayload> = {},
): ErrorPayload {
  return {
    code,
    message,
    detail: null,
    hint: null,
    sqlstate: null,
    vendorCode: null,
    position: null,
    statementIndex: null,
    data: null,
    ...extra,
  };
}

/** `exception_error`'s hints. */
function hintOf(e: ServerFailure): string | null {
  if (e.code === 164)
    return e.text.toLowerCase().includes("cannot modify")
      ? "Read-only mode forbids changing this setting: a read-only connection keeps readonly=2, and a readonly=1 profile forbids every setting."
      : "The connection is read-only.";
  if (e.code === 373)
    return "Another statement still runs in this session; wait for it or stop it.";
  return null;
}

export function dbPayload(e: ServerFailure, statementIndex: number | null): ErrorPayload {
  return payload("DB_ERROR", e.message, {
    vendorCode: e.code,
    hint: hintOf(e),
    statementIndex,
  });
}

/** `spec::single_quoted_backslash`. */
export function quoteText(s: string): string {
  return `'${s.replaceAll("\\", "\\\\").replaceAll("'", "\\'").replaceAll("\n", "\\n").replaceAll("\r", "\\r")}'`;
}

/** `spec::backticked_backslash`. */
export const backticked = (name: string): string =>
  `\`${name.replaceAll("\\", "\\\\").replaceAll("`", "\\`")}\``;

/** A value as ClickHouse SQL text, for Preview SQL and the trace. */
export function literal(v: Value): string {
  switch (v.t) {
    case "null":
      return "NULL";
    case "default":
      return "DEFAULT";
    case "bool":
      return v.v ? "true" : "false";
    case "int":
    case "float":
      return String(v.v);
    case "bigInt":
    case "decimal":
      return v.v;
    case "array":
      return `[${v.v.map(literal).join(", ")}]`;
    case "bytes":
      return `unhex('${[...atob(v.v.base64)].map((c) => c.charCodeAt(0).toString(16).padStart(2, "0")).join("")}')`;
    default:
      return quoteText(display(v));
  }
}

const cannotParse = (raw: string, type: string, what = "syntax error at begin of string") =>
  new ServerFailure(6, `Cannot parse string '${raw}' as ${type}: ${what}.`, "CANNOT_PARSE_TEXT");

const RANGES: Record<string, [bigint, bigint]> = {
  Int8: [-(2n ** 7n), 2n ** 7n - 1n],
  Int16: [-(2n ** 15n), 2n ** 15n - 1n],
  Int32: [-(2n ** 31n), 2n ** 31n - 1n],
  Int64: [-(2n ** 63n), 2n ** 63n - 1n],
  Int128: [-(2n ** 127n), 2n ** 127n - 1n],
  Int256: [-(2n ** 255n), 2n ** 255n - 1n],
  UInt8: [0n, 2n ** 8n - 1n],
  UInt16: [0n, 2n ** 16n - 1n],
  UInt32: [0n, 2n ** 32n - 1n],
  UInt64: [0n, 2n ** 64n - 1n],
  UInt128: [0n, 2n ** 128n - 1n],
  UInt256: [0n, 2n ** 256n - 1n],
};

const DATE = /^\d{4}-\d{2}-\d{2}$/;
const DATE_TIME = /^(\d{4}-\d{2}-\d{2})[ T](\d{2}:\d{2}:\d{2})(\.\d+)?$/;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** A decimal's text with the column's scale, as the server prints it. */
function scaled(raw: string, scale: number): string {
  const neg = raw.startsWith("-");
  const [whole = "0", frac = ""] = raw.replace(/^[+-]/, "").split(".");
  const digits = scale === 0 ? "" : `.${frac.padEnd(scale, "0").slice(0, scale)}`;
  return `${neg ? "-" : ""}${whole.replace(/^0+(?=\d)/, "")}${digits}`;
}

/** An element of an array as its element type reads it. */
function element(type: string, v: unknown): Value {
  if (v === null) return NULL;
  if (typeof v === "string") return coerce(type, { t: "text", v });
  if (typeof v === "number") return coerce(type, { t: "text", v: String(v) });
  if (typeof v === "boolean") return coerce(type, { t: "bool", v });
  return coerce(type, { t: "text", v: JSON.stringify(v) });
}

/**
 * A staged or typed value as the server stores it in a column of `type` (the backend binds each
 * value as `{pN:Type}`, which the server parses as that type); a `ServerFailure` when the text is
 * no value of the type.
 */
export function coerce(type: string, v: Value): Value {
  if (v.t === "null") {
    if (isNullable(type)) return NULL;
    throw new ServerFailure(
      349,
      `Cannot convert NULL value to non-Nullable type ${type}`,
      "CANNOT_INSERT_NULL_IN_ORDINARY_COLUMN",
    );
  }
  const base = baseType(type);
  const raw = v.t === "array" ? null : display(v);
  switch (kindOf(type)) {
    case "int":
    case "bigInt": {
      const r = (raw ?? "").trim();
      if (!/^[+-]?\d+$/.test(r)) throw cannotParse(raw ?? "", base);
      const n = BigInt(r);
      const range = RANGES[base];
      if (range && (n < range[0] || n > range[1]))
        throw new ServerFailure(
          321,
          `Value ${r} is out of the range of ${base}`,
          "VALUE_IS_OUT_OF_RANGE_OF_DATA_TYPE",
        );
      return integer(n);
    }
    case "float": {
      const n = Number(raw);
      if (raw === null || raw.trim() === "" || Number.isNaN(n)) throw cannotParse(raw ?? "", base);
      return { t: "float", v: n };
    }
    case "decimal": {
      const r = (raw ?? "").trim();
      if (!/^[+-]?(\d+\.?\d*|\.\d+)$/.test(r)) throw cannotParse(raw ?? "", base);
      return { t: "decimal", v: scaled(r, decimalOf(type)?.[1] ?? 0) };
    }
    case "bool": {
      const r = (raw ?? "").trim().toLowerCase();
      if (["true", "1"].includes(r)) return { t: "bool", v: true };
      if (["false", "0"].includes(r)) return { t: "bool", v: false };
      throw cannotParse(raw ?? "", base);
    }
    case "text": {
      const fixed = /^FixedString\((\d+)\)$/.exec(base);
      if (fixed && new TextEncoder().encode(raw ?? "").length > Number(fixed[1]))
        throw new ServerFailure(131, `Too large value for ${base}`, "TOO_LARGE_STRING_SIZE");
      return { t: "text", v: raw ?? "" };
    }
    case "uuid":
      if (!UUID.test((raw ?? "").trim())) throw cannotParse(raw ?? "", base);
      return { t: "uuid", v: (raw ?? "").trim().toLowerCase() };
    case "date":
      if (!DATE.test((raw ?? "").trim())) throw cannotParse(raw ?? "", base, "cannot parse Date");
      return { t: "date", v: (raw ?? "").trim() };
    case "timestamp": {
      const m = DATE_TIME.exec((raw ?? "").trim());
      if (!m) throw cannotParse(raw ?? "", base, "cannot parse DateTime");
      const precision = /^DateTime64\((\d+)/.exec(base)?.[1];
      const frac = precision
        ? `.${(m[3] ?? ".").slice(1).padEnd(Number(precision), "0").slice(0, Number(precision))}`
        : "";
      return { t: "timestamp", v: `${m[1]} ${m[2]}${precision === "0" ? "" : frac}` };
    }
    case "time":
      if (!/^\d{1,3}:\d{2}:\d{2}(\.\d+)?$/.test((raw ?? "").trim()))
        throw cannotParse(raw ?? "", base);
      return { t: "time", v: (raw ?? "").trim() };
    case "enum": {
      const labels = enumLabels(type) ?? [];
      if (!labels.includes(raw ?? ""))
        throw new ServerFailure(36, `Unknown element '${raw}' for enum`, "BAD_ARGUMENTS");
      return { t: "enum", v: raw! };
    }
    case "array": {
      const inner = /^Array\((.*)\)$/s.exec(base)?.[1] ?? "String";
      if (v.t === "array") return { t: "array", v: v.v.map((x) => coerce(inner, x)) };
      let items: unknown;
      try {
        items = JSON.parse((raw ?? "").replaceAll("'", '"'));
      } catch {
        throw cannotParse(raw ?? "", base);
      }
      if (!Array.isArray(items)) throw cannotParse(raw ?? "", base);
      return { t: "array", v: items.map((x) => element(inner, x)) };
    }
    case "json": {
      try {
        JSON.parse(raw ?? "");
      } catch {
        throw cannotParse(raw ?? "", base);
      }
      return { t: "json", v: raw! };
    }
    default:
      return { t: "other", v: { ty: base, text: raw ?? "" } };
  }
}
