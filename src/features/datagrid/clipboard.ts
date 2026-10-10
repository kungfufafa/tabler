import type { Dialect, Value } from "@/lib/ipc";
import { literal, qualify, quoteIdent } from "@/lib/sql/quote";
import { valueText } from "@/lib/values";
import type { GridColumn } from "./types";

export type CopyFormat = "tsv" | "csv" | "json" | "insert" | "markdown";

export interface CopyContext {
  dialect: Dialect;
  schema: string | null;
  table: string;
}

const needsCsvQuote = (s: string) => /[",\r\n]/.test(s);
const csvCell = (s: string | null) =>
  s === null ? "" : needsCsvQuote(s) ? `"${s.replace(/"/g, '""')}"` : s;
const tsvCell = (s: string | null) =>
  s === null ? "" : /[\t\r\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;

function jsonValue(v: Value): unknown {
  switch (v.t) {
    case "null":
    case "default":
      return null;
    case "bool":
    case "int":
    case "float":
      return v.v;
    case "bigInt":
    case "decimal": {
      const n = Number(v.v);
      return Number.isSafeInteger(n) && String(n) === v.v ? n : v.v;
    }
    case "json":
      try {
        return JSON.parse(v.v);
      } catch {
        return v.v;
      }
    case "array":
      return v.v.map(jsonValue);
    case "bytes":
      return v.v.base64;
    case "other":
      return v.v.text;
    default:
      return v.v;
  }
}

/** Serializes a rectangular block of cells. `rows` must be parallel to `columns`. */
export function formatCells(
  format: CopyFormat,
  columns: GridColumn[],
  rows: Value[][],
  ctx?: Partial<CopyContext> & { headers?: boolean },
): string {
  switch (format) {
    case "tsv": {
      const lines = rows.map((r) => r.map((v) => tsvCell(valueText(v))).join("\t"));
      if (ctx?.headers) lines.unshift(columns.map((c) => c.name).join("\t"));
      return lines.join("\n");
    }
    case "csv": {
      const lines = rows.map((r) => r.map((v) => csvCell(valueText(v))).join(","));
      lines.unshift(columns.map((c) => csvCell(c.name)).join(","));
      return lines.join("\r\n");
    }
    case "json":
      return JSON.stringify(
        rows.map((r) =>
          Object.fromEntries(columns.map((c, i) => [c.name, jsonValue(r[i] ?? { t: "null" })])),
        ),
        null,
        2,
      );
    case "insert": {
      const dialect = ctx?.dialect ?? "postgres";
      const target = qualify(dialect, ctx?.schema ?? null, ctx?.table ?? "table");
      const cols = columns.map((c) => quoteIdent(dialect, c.name)).join(", ");
      return rows
        .map(
          (r) =>
            `INSERT INTO ${target} (${cols}) VALUES (${r.map((v) => literal(dialect, v)).join(", ")});`,
        )
        .join("\n");
    }
    case "markdown": {
      const esc = (s: string | null) => (s ?? "").replace(/\|/g, "\\|").replace(/\r?\n/g, " ");
      const head = `| ${columns.map((c) => esc(c.name)).join(" | ")} |`;
      const sep = `| ${columns.map(() => "---").join(" | ")} |`;
      const body = rows.map((r) => `| ${r.map((v) => esc(valueText(v))).join(" | ")} |`);
      return [head, sep, ...body].join("\n");
    }
  }
}

export async function writeClipboard(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
    await writeText(text);
  }
}
