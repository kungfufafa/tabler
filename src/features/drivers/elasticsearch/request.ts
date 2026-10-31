import { JsonError, parseConsole, toText, type Json } from "./json";
import {
  asciiUpper,
  isAsciiAlphabetic,
  isAsciiAlphanumeric,
  isControl,
  isWhitespace,
  splitWhitespace,
  trim,
  trimEnd,
} from "./rust";

/**
 * The console's requests, mirroring the backend's `drivers/elasticsearch/console.rs` rule for
 * rule: a `METHOD /path?query` line, then an optional JSON body (one value, or one per line for
 * `_bulk` and `_msearch`), several requests per tab; and SQL statements that start with
 * `SELECT`, `SHOW`, `DESCRIBE` or `DESC`. Offsets are code points, as Rust's `chars()`.
 */

export const METHODS = ["GET", "HEAD", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"] as const;
export type Method = (typeof METHODS)[number];

export function parseMethod(word: string): Method | null {
  const up = asciiUpper(word);
  return METHODS.find((m) => m === up) ?? null;
}

export type Body = { kind: "json"; value: Json } | { kind: "ndjson"; values: Json[] };

/** Decoded query parameters in order; null for a bare flag (`?pretty`). */
export type QueryPairs = [string, string | null][];

export interface EsRequest {
  method: Method;
  /** Decoded and non-empty. */
  segments: string[];
  query: QueryPairs;
  body: Body | null;
}

export const bodyValues = (b: Body): Json[] => (b.kind === "json" ? [b.value] : b.values);

/** The value of a query parameter (the last one); a bare flag reads as "". */
export function queryValue(req: EsRequest, name: string): string | null {
  for (let i = req.query.length - 1; i >= 0; i--) {
    const [k, v] = req.query[i]!;
    if (k === name) return v ?? "";
  }
  return null;
}

const PATH_KEEP = "-._~*,:";
const QUERY_KEEP = "-._~*,";

function encode(s: string, keep: string): string {
  let out = "";
  for (const b of new TextEncoder().encode(s)) {
    const c = String.fromCharCode(b);
    out +=
      b < 0x80 && (isAsciiAlphanumeric(c) || keep.includes(c))
        ? c
        : `%${b.toString(16).toUpperCase().padStart(2, "0")}`;
  }
  return out;
}

/** Path and query, as appended to the cluster's URL (`EsRequest::target`). */
export function requestTarget(req: EsRequest): string {
  const path = req.segments.map((s) => `/${encode(s, PATH_KEEP)}`).join("") || "/";
  const q = req.query
    .map(([k, v]) => encode(k, QUERY_KEEP) + (v === null ? "" : `=${encode(v, QUERY_KEEP)}`))
    .join("&");
  return q === "" ? path : `${path}?${q}`;
}

/** The request as console text (`to_console_text`). */
export function consoleText(req: EsRequest): string {
  let out = `${req.method} ${requestTarget(req)}`;
  if (req.body) for (const v of bodyValues(req.body)) out += `\n${toText(v)}`;
  return out;
}

/** Percent-decodes `s` (no `+`); a malformed escape or bytes that are not UTF-8 throw. */
export function percentDecode(s: string): string {
  const b = new TextEncoder().encode(s);
  const out: number[] = [];
  for (let i = 0; i < b.length;) {
    if (b[i] === 0x25) {
      const hex = String.fromCharCode(b[i + 1] ?? 0, b[i + 2] ?? 0);
      if (i + 2 >= b.length || !/^[0-9a-fA-F]{2}$/.test(hex))
        throw new Error(`malformed percent escape in “${s}”`);
      out.push(parseInt(hex, 16));
      i += 3;
    } else out.push(b[i++]!);
  }
  try {
    return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(new Uint8Array(out));
  } catch {
    throw new Error(`“${s}” does not decode to UTF-8 text`);
  }
}

/** Splits a typed path (`/idx/_search?q=x`) into decoded segments and query. */
export function parseTarget(target: string): { segments: string[]; query: QueryPairs } {
  const bad = (why: string) => new Error(`path “${target}”: ${why}`);
  if (target.includes("://"))
    throw bad("give the path only; the console sends every request to this connection");
  for (const c of target)
    if (isWhitespace(c) || isControl(c) || c === "\\")
      throw bad("spaces, control characters and backslashes are not allowed");
  if (target.includes("#")) throw bad("`#` must be written %23");
  const q = target.indexOf("?");
  const path = q < 0 ? target : target.slice(0, q);
  const query = q < 0 ? null : target.slice(q + 1);
  const segments: string[] = [];
  for (const raw of path.split("/").filter((s) => s !== "")) {
    const seg = percentDecode(raw);
    if (seg.split(/[/\\]/).some((p) => p === "." || p === ".."))
      throw bad("`.` and `..` segments are not allowed (also behind %2F or %5C)");
    segments.push(seg);
  }
  const pairs: QueryPairs = [];
  if (query !== null)
    for (const part of query.split("&").filter((p) => p !== "")) {
      const eq = part.indexOf("=");
      const decode = (s: string) => percentDecode(s.replaceAll("+", "%20"));
      pairs.push(
        eq < 0 ? [decode(part), null] : [decode(part.slice(0, eq)), decode(part.slice(eq + 1))],
      );
    }
  return { segments, query: pairs };
}

/** Whether a request with these segments takes newline-delimited JSON. */
export function takesNdjson(segments: readonly string[]): boolean {
  return (
    segments.some((s) => s === "_bulk" || s === "_msearch" || s === "_fleet_msearch") ||
    segments.some((s, i) => s === "_monitoring" && segments[i + 1] === "bulk")
  );
}

/** `split_inclusive('\n')`. */
function linesInclusive(text: string): string[] {
  const out: string[] = [];
  let start = 0;
  for (let i = text.indexOf("\n"); i >= 0; i = text.indexOf("\n", start)) {
    out.push(text.slice(start, i + 1));
    start = i + 1;
  }
  if (start < text.length) out.push(text.slice(start));
  return out;
}

const isCommentLine = (t: string) => t.startsWith("#") || t.startsWith("//");

/** Parses one console request; throws an Error with the backend's reason when it is not one. */
export function parseRequest(text: string): EsRequest {
  let consumed = 0;
  let requestLine: string | null = null;
  for (const line of linesInclusive(text)) {
    consumed += line.length;
    const t = trim(line);
    if (t === "" || isCommentLine(t)) continue;
    requestLine = t;
    break;
  }
  if (requestLine === null) throw new Error("expected a request such as `GET /_search`");
  const words = splitWhitespace(requestLine);
  const methodWord = words[0] ?? "";
  const method = parseMethod(methodWord);
  if (!method)
    throw new Error(`“${methodWord}” is not a request method (GET, POST, PUT, DELETE, HEAD)`);
  const target = words[1];
  if (target === undefined) throw new Error(`${method} needs a path`);
  if (words[2] !== undefined)
    throw new Error(`unexpected “${words[2]}” after the path (the body goes on the next line)`);
  const { segments, query } = parseTarget(target);
  const bodyText = text.slice(consumed);
  let values: Json[];
  try {
    values = parseConsole(bodyText);
  } catch (e) {
    // The line counts from the request's first line, as the backend's.
    const before = text.slice(0, consumed).split("\n").length - 1;
    throw new Error(
      `request body: ${e instanceof JsonError ? e.describe(bodyText, before) : String(e)}`,
      { cause: e },
    );
  }
  let body: Body | null;
  if (values.length === 0) body = null;
  else if (takesNdjson(segments)) body = { kind: "ndjson", values };
  else if (values.length === 1) body = { kind: "json", value: values[0]! };
  else throw new Error(`${method} takes one JSON body, found ${values.length}`);
  return { method, segments, query, body };
}

/** The first word of a line (trimmed at the start), when a space, a tab or the end follows it. */
function firstWord(t: string): string | null {
  let n = 0;
  while (n < t.length && isAsciiAlphabetic(t[n]!)) n++;
  const rest = t.slice(n);
  return n > 0 && (rest === "" || rest[0] === " " || rest[0] === "\t" || rest[0] === "\r")
    ? t.slice(0, n)
    : null;
}

const isRequestLine = (t: string) => {
  const w = firstWord(t);
  return w !== null && parseMethod(w) !== null;
};

/** Inside a SQL statement still going on, a request line is a method and one path. */
function isRequestLineInSql(t: string): boolean {
  const words: string[] = [];
  for (const w of splitWhitespace(t).slice(1)) {
    if (w.startsWith("#") || w.startsWith("//")) break;
    words.push(w);
  }
  return isRequestLine(t) && words.length === 1;
}

/** The words that open a SQL statement of the console (`SQL_WORDS`). */
export const SQL_WORDS = ["SELECT", "SHOW", "DESCRIBE", "DESC"];

const isSqlLine = (t: string) => {
  const w = firstWord(t);
  return w !== null && SQL_WORDS.includes(asciiUpper(w));
};

/** The SQL of a statement whose first line opens a SQL statement, without a final `;`. */
export function sqlCommand(text: string): string | null {
  let offset = 0;
  for (const line of linesInclusive(text)) {
    const t = trim(line);
    if (t === "" || isCommentLine(t)) {
      offset += line.length;
      continue;
    }
    if (!isSqlLine(t)) return null;
    const sql = trim(text.slice(offset));
    return trimEnd(sql.endsWith(";") ? sql.slice(0, -1) : sql);
  }
  return null;
}

export type Command = { kind: "request"; req: EsRequest } | { kind: "sql"; sql: string };

/** Parses one console statement (of `split`); throws when it is neither. */
export function parseCommand(text: string): Command {
  const sql = sqlCommand(text);
  return sql !== null ? { kind: "sql", sql } : { kind: "request", req: parseRequest(text) };
}

/** A statement of `split`: its text and where it is, in code points (as the backend's). */
export interface Split {
  sql: string;
  start: number;
  end: number;
  line: number;
}

const startsWithAt = (chars: readonly string[], i: number, s: string) =>
  [...s].every((c, n) => chars[i + n] === c);

/**
 * Splits console text into statements, at lines whose first word is a method or (outside a SQL
 * statement still going on) a SQL word, outside strings and comments (`console::split`).
 */
export function split(text: string): Split[] {
  const chars = [...text];
  const starts: number[] = [];
  let i = 0;
  let lineStart = true;
  let inTriple = false;
  let inBlock = false;
  let sqlGoesOn = false;
  while (i < chars.length) {
    const c = chars[i]!;
    if (lineStart && !inTriple && !inBlock) {
      let j = i;
      while (j < chars.length && (chars[j] === " " || chars[j] === "\t")) j++;
      let lineEnd = j;
      while (lineEnd < chars.length && chars[lineEnd] !== "\n") lineEnd++;
      const line = chars.slice(j, lineEnd).join("");
      const request = sqlGoesOn ? isRequestLineInSql(line) : isRequestLine(line);
      if (request) {
        starts.push(j);
        sqlGoesOn = false;
      } else if (isSqlLine(line) && !sqlGoesOn) {
        starts.push(j);
        sqlGoesOn = true;
      }
      if (sqlGoesOn) {
        const t = trim(line);
        sqlGoesOn = !(t === "" || t.endsWith(";"));
      }
    }
    lineStart = false;
    if (inTriple) {
      if (startsWithAt(chars, i, '"""')) {
        inTriple = false;
        i += 3;
        continue;
      }
    } else if (inBlock) {
      if (startsWithAt(chars, i, "*/")) {
        inBlock = false;
        i += 2;
        continue;
      }
    } else if (startsWithAt(chars, i, '"""')) {
      inTriple = true;
      i += 3;
      continue;
    } else if (c === '"') {
      // A plain string ends at its quote or, unterminated, at the line's end.
      i++;
      while (i < chars.length && chars[i] !== '"' && chars[i] !== "\n")
        i += chars[i] === "\\" ? 2 : 1;
      if (i < chars.length && chars[i] === '"') i++;
      continue;
    } else if (c === "#" || (c === "/" && chars[i + 1] === "/")) {
      while (i < chars.length && chars[i] !== "\n") i++;
      continue;
    } else if (c === "/" && chars[i + 1] === "*") {
      inBlock = true;
      i += 2;
      continue;
    }
    if (c === "\n") lineStart = true;
    i++;
  }

  const out: Split[] = [];
  const push = (start: number, end: number) => {
    let s = start;
    while (s < end && isWhitespace(chars[s]!)) s++;
    let e = end;
    while (e > s && isWhitespace(chars[e - 1]!)) e--;
    if (s >= e) return;
    const line = chars.slice(0, s).filter((ch) => ch === "\n").length + 1;
    out.push({ sql: chars.slice(s, e).join(""), start: s, end: e, line });
  };
  const first = starts[0] ?? chars.length;
  // Leading text that is not blank or comments (`str::lines` drops a final `\r`).
  const lead = chars.slice(0, first).join("");
  if (
    lead.split("\n").some((l) => {
      const t = trim(l.endsWith("\r") ? l.slice(0, -1) : l);
      return t !== "" && !isCommentLine(t) && !t.startsWith("/*");
    })
  )
    push(0, first);
  starts.forEach((s, n) => push(s, starts[n + 1] ?? chars.length));
  return out;
}
