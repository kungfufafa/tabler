import type { Dialect, Value } from "@/lib/ipc";
import { valueText } from "@/lib/values";

export function quoteIdent(dialect: Dialect, ident: string): string {
  return dialect === "mysql"
    ? `\`${ident.replace(/`/g, "``")}\``
    : `"${ident.replace(/"/g, '""')}"`;
}

export function qualify(dialect: Dialect, schema: string | null | undefined, name: string): string {
  return schema
    ? `${quoteIdent(dialect, schema)}.${quoteIdent(dialect, name)}`
    : quoteIdent(dialect, name);
}

export function quoteString(dialect: Dialect, s: string): string {
  if (dialect === "mysql")
    return `'${s.replace(/\\/g, "\\\\").replace(/'/g, "\\'").replace(/\0/g, "\\0")}'`;
  return `'${s.replace(/'/g, "''")}'`;
}

function hexOf(base64: string): string {
  const bin = atob(base64);
  let hex = "";
  for (let i = 0; i < bin.length; i++) hex += bin.charCodeAt(i).toString(16).padStart(2, "0");
  return hex;
}

/** SQL literal for display / copy-as-INSERT. */
export function literal(dialect: Dialect, v: Value): string {
  switch (v.t) {
    case "null":
      return "NULL";
    case "default":
      return "DEFAULT";
    case "bool":
      return dialect === "postgres" ? (v.v ? "TRUE" : "FALSE") : v.v ? "1" : "0";
    case "int":
    case "float":
      return String(v.v);
    case "bigInt":
    case "decimal":
      return v.v;
    case "bytes": {
      const hex = hexOf(v.v.base64);
      return dialect === "postgres" ? `'\\x${hex}'::bytea` : `X'${hex}'`;
    }
    default:
      return quoteString(dialect, valueText(v) ?? "");
  }
}
