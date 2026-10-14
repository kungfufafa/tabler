import type { Dialect, Value } from "@/lib/ipc";
import { isTauri } from "@/lib/ipc/transport";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { literal, qualify, quoteIdent, quoteString } from "@/lib/sql/quote";
import { valueText } from "@/lib/values";
import { DEFAULT_CSV_OPTIONS, formatCsv, quoteChar, type CsvOptions } from "./csv";
import type { GridColumn } from "./types";

export type CopyFormat = "tsv" | "csv" | "json" | "insert" | "markdown" | "html" | "raw";

export interface CopyContext {
  dialect: Dialect;
  schema: string | null;
  table: string;
  /**
   * MySQL 8+ (not 5.7, not MariaDB): geographic WKT is read back with `axis-order=long-lat`, as
   * the spatial decoder writes MySQL's storage order (see `mysqlLongLat`).
   */
  mysqlLongLat?: boolean;
}

/**
 * Whether INSERTs must read geographic WKT with `axis-order=long-lat`: MySQL 8 and later (the
 * export writer's `mysql_long_lat`), judged from the server's version string.
 */
export function mysqlLongLat(driver: Dialect, serverVersion: string): boolean {
  if (driver !== "mysql" || /mariadb/i.test(serverVersion)) return false;
  const major = /^\s*(\d+)\./.exec(serverVersion)?.[1];
  return major !== undefined && Number(major) >= 8;
}

/** The WKT the spatial decoder writes: a type keyword, numbers, parentheses, commas, EMPTY… */
const WKT_TEXT = /^[A-Za-z][A-Za-z0-9 ,.()+-]*$/;
const HEX_DIGITS = /^(?:[0-9A-Fa-f]{2})*$/;

/** `SRID=4326;POINT(1 2)` → [4326, `POINT(1 2)`]; plain WKT has SRID 0; null when not (E)WKT. */
function splitStrictEwkt(text: string): { srid: number; wkt: string } | null {
  const t = text.trim();
  let srid = 0;
  let wkt = t;
  if (/^srid=/i.test(t)) {
    const m = /^srid=\s*(\d+)\s*;(.*)$/is.exec(t);
    if (!m) return null;
    srid = Number(m[1]);
    wkt = m[2]!.trim();
  }
  return WKT_TEXT.test(wkt) ? { srid, wkt } : null;
}

/** The PostGIS type a value's type names (`geometry`, `geometry(Point,4326)`, `public.geography`). */
function postgisType(ty: string): "geometry" | "geography" | null {
  const base = ty
    .trim()
    .toLowerCase()
    .split("(")[0]!
    .split(".")
    .pop()!
    .trim()
    .replace(/^"|"$/g, "");
  return base === "geometry" || base === "geography" ? base : null;
}

/**
 * A value as a SQL literal for Copy Rows As ▸ SQL Insert: `literal`, with spatial values loadable on
 * the server they came from (as the export writer's `insert_literal`): MySQL (E)WKT through
 * `ST_GeomFromText('…', srid)` (hex as `X'…'`), PostGIS EWKT or hex EWKB cast to its type
 * (`'SRID=4326;POINT(1 2)'::geometry`). Anything that is not (E)WKT stays a plain string.
 */
export function insertLiteral(dialect: Dialect, v: Value, longLat = false): string {
  if (v.t !== "other") return literal(dialect, v);
  const { ty, text } = v.v;
  if (dialect === "mysql" && ty === "geometry") {
    if (HEX_DIGITS.test(text)) return `X'${text}'`;
    const g = splitStrictEwkt(text);
    if (g) {
      const wkt = quoteString(dialect, g.wkt);
      return longLat && g.srid !== 0
        ? `ST_GeomFromText(${wkt}, ${g.srid}, 'axis-order=long-lat')`
        : `ST_GeomFromText(${wkt}, ${g.srid})`;
    }
  }
  if (dialect === "postgres") {
    const type = postgisType(ty);
    if (type) return `${quoteString(dialect, text)}::${type}`;
  }
  return literal(dialect, v);
}

/**
 * Copy Rows As ▸ SQL Insert statement: rows go into multi-row INSERTs that each stay under this
 * many bytes (TablePlus splits them into ~250 KB chunks); a single larger row gets its own.
 */
export const INSERT_CHUNK_BYTES = 250_000;

/** UTF-8 length of `s` without encoding it. */
function utf8Length(s: string): number {
  let n = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    if (c < 0x80) n += 1;
    else if (c < 0x800) n += 2;
    else if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length) {
      n += 4;
      i++;
    } else n += 3;
  }
  return n;
}

/** `head` + the tuples as multi-row statements of at most `limit` bytes each. */
function chunkInserts(head: string, tuples: readonly string[], limit: number): string[] {
  const statements: string[] = [];
  const headBytes = utf8Length(head);
  let chunk: string[] = [];
  let bytes = headBytes;
  for (const tuple of tuples) {
    const size = utf8Length(tuple);
    // ",\n" goes before every tuple but the first, ";" after the last.
    if (chunk.length > 0 && bytes + 2 + size + 1 > limit) {
      statements.push(`${head}${chunk.join(",\n")};`);
      chunk = [];
      bytes = headBytes;
    }
    bytes += size + (chunk.length > 0 ? 2 : 0);
    chunk.push(tuple);
  }
  if (chunk.length > 0) statements.push(`${head}${chunk.join(",\n")};`);
  return statements;
}
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
 * the column names as a first line to TSV and CSV (the other formats always name them). `csv`:
 * the CSV options (Settings ▸ General ▸ CSV default options; the defaults when not given).
 * `insertChunkBytes`: the size cap of one INSERT statement (`INSERT_CHUNK_BYTES`).
 */
export function formatCells(
  format: CopyFormat,
  columns: GridColumn[],
  rows: Value[][],
  ctx?: Partial<CopyContext> & { headers?: boolean; csv?: CsvOptions; insertChunkBytes?: number },
): string {
  switch (format) {
    case "tsv": {
      const lines = rows.map((r) => r.map((v) => tsvCell(valueText(v))).join("\t"));
      if (ctx?.headers) lines.unshift(columns.map((c) => c.name).join("\t"));
      return lines.join("\n");
    }
    case "raw":
      // Copy as Raw: the values exactly as stored, tab-separated, nothing quoted or escaped.
      return rows.map((r) => r.map((v) => valueText(v) ?? "").join("\t")).join("\n");
    case "csv":
      return formatCsv(
        ctx?.headers ? columns.map((c) => c.name) : null,
        rows,
        ctx?.csv ?? DEFAULT_CSV_OPTIONS,
      );
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
      const tuples = rows.map(
        (r) => `(${r.map((v) => insertLiteral(dialect, v, ctx?.mysqlLongLat)).join(", ")})`,
      );
      const head = `INSERT INTO ${target} (${cols}) VALUES\n`;
      return chunkInserts(head, tuples, ctx?.insertChunkBytes ?? INSERT_CHUNK_BYTES).join("\n");
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
 * ⌘C writes plain TSV, where NULL and an empty string both read as an empty field. Which copied
 * fields were NULL (or DEFAULT) is kept aside, out of the text, keyed by the text's hash, so ⌘V of
 * that same text in Tabler (any window: localStorage is shared) restores NULL vs ''. Text copied
 * elsewhere, or changed since, has no markers and pastes as before.
 *
 * The same entry records whether the copy was one column whose last row is empty: its text ends
 * with a line break that `parseTsv` takes for the terminator, so the last row would be lost
 * (`parseTsvPaste` gives it back).
 */
export const COPY_MARKERS_KEY = "tabler.clipboard.markers";
/** Most marked fields remembered for one copy. */
const MAX_MARKERS = 200_000;

export type CopyMarker = "null" | "default";

/** FNV-1a over the UTF-16 code units, with the length: enough to recognise our own copy. */
function textHash(text: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return `${text.length}:${(h >>> 0).toString(16)}`;
}

interface RememberedCopy {
  hash: string;
  marks: Array<[number, number, CopyMarker]>;
  /** One column wide, two rows or more, and the last row renders as an empty field. */
  trailingEmptyRow?: boolean;
}

let lastMarkers: RememberedCopy | null = null;

/** Remembers which fields of copied `text` (rows × fields, as `rows`) were NULL / DEFAULT. */
export function rememberCopy(text: string, rows: readonly (readonly Value[])[]): void {
  const marks: Array<[number, number, CopyMarker]> = [];
  rows.forEach((row, r) =>
    row.forEach((v, c) => {
      if ((v.t === "null" || v.t === "default") && marks.length < MAX_MARKERS)
        marks.push([r, c, v.t]);
    }),
  );
  const last = rows[rows.length - 1];
  // A single row's text has no line break: nothing is lost there.
  const trailingEmptyRow =
    rows.length > 1 &&
    rows.every((r) => r.length === 1) &&
    last !== undefined &&
    tsvCell(valueText(last[0]!)) === "";
  lastMarkers = { hash: textHash(text), marks, trailingEmptyRow };
  try {
    if (marks.length === 0 && !trailingEmptyRow)
      globalThis.localStorage?.removeItem(COPY_MARKERS_KEY);
    else globalThis.localStorage?.setItem(COPY_MARKERS_KEY, JSON.stringify(lastMarkers));
  } catch {
    // Storage unavailable or full: this window still knows.
  }
}

/** What Tabler remembers of `text` when it copied it (this window or another), else null. */
function rememberedCopy(text: string): RememberedCopy | null {
  const hash = textHash(text);
  if (lastMarkers?.hash === hash) return lastMarkers;
  try {
    const raw = globalThis.localStorage?.getItem(COPY_MARKERS_KEY);
    const parsed = raw ? (JSON.parse(raw) as RememberedCopy | null) : null;
    if (parsed?.hash === hash && Array.isArray(parsed.marks)) return parsed;
  } catch {
    // Unreadable: nothing remembered.
  }
  return null;
}

/** The NULL / DEFAULT fields of `text` when Tabler copied it (`"r:c"` → marker), else null. */
export function copiedMarkers(text: string): Map<string, CopyMarker> | null {
  const entry = rememberedCopy(text);
  if (!entry || entry.marks.length === 0) return null;
  return new Map(entry.marks.map(([r, c, m]) => [`${r}:${c}`, m]));
}

/**
 * `parseTsv` of pasted text, for ⌘V in the grid. When the text is what Tabler copied last and that
 * was one column ending in a NULL or empty cell, the empty last row is given back: its text ends
 * with a line break that `parseTsv` reads as the terminator (`[x, y, NULL]` copies as `x\ny\n`).
 * Text from elsewhere parses as `parseTsv` does.
 */
export function parseTsvPaste(text: string): string[][] {
  const rows = parseTsv(text);
  if (rememberedCopy(text)?.trailingEmptyRow) rows.push([""]);
  return rows;
}

/**
 * Parses delimiter-separated text (a CSV file, Copy as CSV) into rows of fields, RFC 4180 style: a
 * field in `quote` characters may hold the delimiter, line breaks and the quote itself (doubled).
 * Accepts CRLF, LF or CR line breaks and drops one trailing line break. As `parseTsv` does, a
 * field that only looks quoted (`"a"b`) is kept verbatim, an empty string is one empty field and a
 * trailing delimiter ends with an empty field.
 */
export function parseCsvPaste(text: string, delimiter = ",", quote: '"' | "'" = '"'): string[][] {
  let src = text;
  if (src.endsWith("\r\n")) src = src.slice(0, -2);
  else if (src.endsWith("\n") || src.endsWith("\r")) src = src.slice(0, -1);
  const rows: string[][] = [];
  let row: string[] = [];
  const n = src.length;
  /** The length of the line break at `at` (0 when there is none). */
  const breakAt = (at: number) =>
    src[at] === "\r" ? (src[at + 1] === "\n" ? 2 : 1) : src[at] === "\n" ? 1 : 0;
  let i = 0;
  for (;;) {
    let field: string | null = null;
    if (src[i] === quote) {
      // Quoted field: valid only when the closing quote is followed by a delimiter or a line end.
      let j = i + 1;
      let buf = "";
      let closed = false;
      while (j < n) {
        const ch = src[j]!;
        if (ch === quote) {
          if (src[j + 1] === quote) {
            buf += quote;
            j += 2;
            continue;
          }
          closed = true;
          break;
        }
        buf += ch;
        j++;
      }
      if (closed && (j + 1 >= n || src[j + 1] === delimiter || breakAt(j + 1) > 0)) {
        field = buf;
        i = j + 1;
      }
    }
    if (field === null) {
      let j = i;
      while (j < n && src[j] !== delimiter && breakAt(j) === 0) j++;
      field = src.slice(i, j);
      i = j;
    }
    row.push(field);
    if (i >= n) break;
    if (src[i] === delimiter) {
      i++;
      if (i < n) continue;
      row.push(""); // a trailing delimiter ends with an empty field
      break;
    }
    // Line break.
    i += breakAt(i);
    rows.push(row);
    row = [];
  }
  rows.push(row);
  return rows;
}

/**
 * Whether `block` (a parse of pasted text) is CSV rather than one value: every line has the same
 * number of fields, at least two, and either there are several lines or the single line has one
 * field per visible column. A lone line also must not read as prose (`Smith, John`: a field with a
 * space next to the delimiter), which a cell holds as one value.
 */
function looksLikeCsv(block: readonly (readonly string[])[], visibleColumns: number): boolean {
  const width = block[0]?.length ?? 0;
  if (width < 2 || !block.every((fields) => fields.length === width)) return false;
  if (block.length >= 2) return true;
  return width === visibleColumns && block[0]!.every((field) => field === field.trim());
}

/**
 * Pasted comma / semicolon separated text as a block of fields (TablePlus pastes CSV straight into
 * the grid), or null when it is not CSV: it has a tab (that is TSV), Tabler copied it itself as
 * plain text, or no delimiter splits it as `looksLikeCsv` asks. The Table Data CSV default options'
 * delimiter and quote are tried first, then `,` / `;` and `"`; among the splits that fit, the one
 * with one field per visible column wins (`1,5;2,5` is two columns with `;`), else the first.
 * The first line is data, not a header.
 */
export function sniffCsvPaste(
  text: string,
  visibleColumns: number,
  options: Pick<CsvOptions, "delimiter" | "quote"> = DEFAULT_CSV_OPTIONS,
): string[][] | null {
  if (text.includes("\t") || !/[,;]/.test(text) || rememberedCopy(text)) return null;
  const delimiters = [...new Set([options.delimiter, ",", ";"])].filter((d) => d !== "\t");
  const quotes = [...new Set([quoteChar(options.quote), '"' as const])];
  let first: string[][] | null = null;
  for (const delimiter of delimiters) {
    for (const quote of quotes) {
      const block = parseCsvPaste(text, delimiter, quote);
      if (!looksLikeCsv(block, visibleColumns)) continue;
      if (block[0]!.length === visibleColumns) return block;
      first ??= block;
    }
  }
  return first;
}

/** Test-only: forget the remembered copy. */
export function resetCopyMarkersForTests(): void {
  lastMarkers = null;
  try {
    globalThis.localStorage?.removeItem(COPY_MARKERS_KEY);
  } catch {
    // ignore
  }
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
