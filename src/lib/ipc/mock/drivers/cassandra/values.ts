import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { Value } from "../../../bindings/Value";
import { display, kindOf, tsText } from "./data";

/** Values as CQL literals and back, and the errors the backend's payloads carry. */

/** A value as CQL text, for Preview SQL and the trace (`portable::literal::literal`). */
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
    case "bytes":
      return display(v);
    default:
      return `'${display(v).replaceAll("'", "''")}'`;
  }
}

const SAFE = BigInt(Number.MAX_SAFE_INTEGER);

/** An integer as the backend decodes one: `int` within 2^53, `bigInt` text past it. */
function integer(text: string): Value | null {
  if (!/^[+-]?\d+$/.test(text.trim())) return null;
  const n = BigInt(text.trim());
  return n > SAFE || n < -SAFE ? { t: "bigInt", v: n.toString() } : { t: "int", v: Number(n) };
}

/**
 * A staged or typed value in the column's type, as the node stores it (`encode.rs` binds by the
 * prepared marker's type); null when the text is no value of that type.
 */
export function coerce(type: string, v: Value): Value | null {
  if (v.t === "null") return v;
  const raw =
    v.t === "bytes" || v.t === "array" || v.t === "other" || v.t === "default" ? null : String(v.v);
  switch (kindOf(type)) {
    case "int":
    case "bigInt":
      return raw === null ? null : integer(raw);
    case "decimal":
      return raw !== null && /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(raw.trim())
        ? { t: "decimal", v: raw.trim() }
        : null;
    case "float": {
      const n = Number(raw);
      return raw !== null && raw.trim() !== "" && Number.isFinite(n) ? { t: "float", v: n } : null;
    }
    case "bool":
      if (v.t === "bool") return v;
      return raw === "true" || raw === "false" ? { t: "bool", v: raw === "true" } : null;
    case "uuid":
      return raw !== null && /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(raw.trim())
        ? { t: "uuid", v: raw.trim().toLowerCase() }
        : null;
    case "timestampTz": {
      if (raw === null) return null;
      const ms = Date.parse(
        raw
          .trim()
          .replace(" ", "T")
          .replace(/([+-]\d{2})(\d{2})$/, "$1:$2"),
      );
      return Number.isNaN(ms) ? null : { t: "timestampTz", v: tsText(ms) };
    }
    case "date":
      return raw !== null && /^\d{4}-\d{2}-\d{2}$/.test(raw.trim())
        ? { t: "date", v: raw.trim() }
        : null;
    case "bytes":
      if (v.t === "bytes") return v;
      if (raw === null || !/^0x([0-9a-f]{2})*$/i.test(raw.trim())) return null;
      {
        const hex = raw.trim().slice(2);
        const chars = (hex.match(/../g) ?? []).map((h) => String.fromCharCode(parseInt(h, 16)));
        return {
          t: "bytes",
          v: { base64: btoa(chars.join("")), len: chars.length, truncated: false },
        };
      }
    case "json":
      if (raw === null) return null;
      try {
        JSON.parse(raw);
        return { t: "json", v: raw };
      } catch {
        return null;
      }
    case "text":
      return raw === null ? null : { t: "text", v: raw };
    default:
      return raw === null ? null : { t: kindOf(type) === "time" ? "time" : "text", v: raw };
  }
}

/** A column's value or the server's refusal of it. */
export function cell(col: ColumnMeta, v: Value): Value {
  const out = coerce(col.dataType, v);
  if (out === null)
    throw serverFailure(
      8704,
      `Invalid ${col.dataType} constant (${display(v)}) for "${col.name}" of type ${col.dataType}`,
    );
  return out;
}

/** An error of the server: `errors.rs` maps it to `Db` with the CQL error code. */
export class ServerFailure {
  constructor(
    readonly code: number,
    readonly message: string,
  ) {}
}

export const serverFailure = (code: number, message: string) => new ServerFailure(code, message);

/** An `ErrorPayload` as `AppError::to_payload` builds one. */
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

/** A server error's payload (`AppError::Db` with the CQL code as `vendor_code`). */
export const dbPayload = (f: ServerFailure, statementIndex: number | null = null) =>
  payload("DB_ERROR", f.message, { vendorCode: f.code, statementIndex });
