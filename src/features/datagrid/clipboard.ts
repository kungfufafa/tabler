import type { Dialect, Value } from "@/lib/ipc";
import { isTauri } from "@/lib/ipc/transport";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { literal, qualify, quoteIdent } from "@/lib/sql/quote";
import { valueText } from "@/lib/values";
import type { GridColumn } from "./types";

export type CopyFormat = "tsv" | "csv" | "json" | "insert" | "markdown" | "html";

export interface CopyContext {
  dialect: Dialect;
  schema: string | null;
  table: string;
}

const needsCsvQuote = (s: string) => /[",\r\n]/.test(s);
const csvCell = (s: string | null) =>
  s === null ? "" : needsCsvQuote(s) ? `"${s.replace(/"/g, '""')}"` : s;
/** Quotes fields with tabs/line breaks (and a leading quote, so `parseTsv` reads them back as-is). */
const tsvCell = (s: string | null) =>
  s === null ? "" : /[\t\r\n]/.test(s) || s.startsWith('"') ? `"${s.replace(/"/g, '""')}"` : s;

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

const HTML_ESCAPES: Record<string, string> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
};
const escapeHtml = (s: string) => s.replace(/[&<>"']/g, (ch) => HTML_ESCAPES[ch]!);

/** Values written without quotes by "Copy All Column Values". */
const UNQUOTED: ReadonlySet<Value["t"]> = new Set(["int", "bigInt", "float", "decimal", "bool"]);

/**
 * "Copy All Column Values": one column's values as a comma-separated list, strings single-quoted
 * (a quote inside doubled) and numbers, booleans and NULL bare — ready for an `IN (…)` list.
 */
export function formatColumnValues(values: readonly Value[]): string {
  return values
    .map((v) => {
      const text = valueText(v);
      if (text === null) return v.t === "default" ? "DEFAULT" : "NULL";
      return UNQUOTED.has(v.t) ? text : `'${text.replace(/'/g, "''")}'`;
    })
    .join(", ");
}

/**
 * Serializes a rectangular block of cells. `rows` must be parallel to `columns`. `headers` adds
 * the column names as a first line to TSV and CSV (the other formats always name them).
 */
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
      if (ctx?.headers) lines.unshift(columns.map((c) => csvCell(c.name)).join(","));
      return lines.join("\r\n");
    }
    case "html": {
      const cell = (tag: "th" | "td", s: string | null) =>
        `<${tag}>${s === null ? "" : escapeHtml(s).replace(/\r?\n/g, "<br>")}</${tag}>`;
      const head = `<tr>${columns.map((c) => cell("th", c.name)).join("")}</tr>`;
      const body = rows.map((r) => `<tr>${r.map((v) => cell("td", valueText(v))).join("")}</tr>`);
      return `<table>\n<thead>\n${head}\n</thead>\n<tbody>\n${body.join("\n")}${body.length > 0 ? "\n" : ""}</tbody>\n</table>`;
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

/**
 * Parses tab-separated text (spreadsheet / grid copies) into rows of fields. Accepts CRLF or LF, drops
 * one trailing line break, and reads `"..."` fields with `""` escapes (which may hold tabs and line
 * breaks) — the inverse of the TSV `formatCells` writes. A field that only looks quoted (`"a"b`) is
 * kept verbatim. An empty string is one empty field: it is what copying a single NULL or empty cell
 * writes, so pasting it back must set that cell (NULL or '') rather than do nothing.
 */
export function parseTsv(text: string): string[][] {
  let src = text;
  if (src.endsWith("\r\n")) src = src.slice(0, -2);
  else if (src.endsWith("\n")) src = src.slice(0, -1);
  const rows: string[][] = [];
  let row: string[] = [];
  const n = src.length;
  let i = 0;
  for (;;) {
    let field: string | null = null;
    if (src[i] === '"') {
      // Quoted field: valid only when the closing quote is followed by a delimiter or the end.
      let j = i + 1;
      let buf = "";
      let closed = false;
      while (j < n) {
        const ch = src[j]!;
        if (ch === '"') {
          if (src[j + 1] === '"') {
            buf += '"';
            j += 2;
            continue;
          }
          closed = true;
          break;
        }
        buf += ch;
        j++;
      }
      const next = src[j + 1];
      if (closed && (next === undefined || next === "\t" || next === "\n" || next === "\r")) {
        if (next !== "\r" || src[j + 2] === "\n") {
          field = buf;
          i = j + 1;
        }
      }
    }
    if (field === null) {
      let j = i;
      while (j < n) {
        const ch = src[j];
        if (ch === "\t" || ch === "\n" || (ch === "\r" && src[j + 1] === "\n")) break;
        j++;
      }
      field = src.slice(i, j);
      i = j;
    }
    row.push(field);
    if (i >= n) break;
    if (src[i] === "\t") {
      i++;
      if (i < n) continue;
      row.push(""); // a trailing tab ends with an empty field
      break;
    }
    // Line break.
    i += src[i] === "\r" ? 2 : 1;
    rows.push(row);
    row = [];
  }
  rows.push(row);
  return rows;
}

/**
 * Reads plain text from the clipboard. Under Tauri the clipboard plugin is used first (it avoids
 * WKWebView's paste permission prompt); the browser API is the fallback, and the default in the mock.
 */
export async function readClipboardText(): Promise<string> {
  const viaBrowser = () => navigator.clipboard.readText();
  const viaTauri = async () => {
    const { readText } = await import("@tauri-apps/plugin-clipboard-manager");
    return readText();
  };
  if (!isTauri()) {
    // Plain browser (dev mock): only the async Clipboard API exists, and it may be denied.
    try {
      return await viaBrowser();
    } catch {
      throw new Error(
        `The browser blocked clipboard access. Allow clipboard permission, or paste with ${formatCombo(shortcutFor("grid.paste") ?? "Mod+V")} while the grid is focused.`,
      );
    }
  }
  try {
    return await viaTauri();
  } catch {
    return await viaBrowser();
  }
}

export async function writeClipboard(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch (err) {
    if (!isTauri()) throw err;
    const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
    await writeText(text);
  }
}
