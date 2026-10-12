/**
 * Backup file name patterns (TablePlus "File name … Click to change"): literal text plus tokens
 * such as `{database}` or `{year}`. Dates and times are zero-padded. A `/` in the pattern makes
 * sub-folders of the chosen folder; a `/` inside a value (a connection named "EU/Prod") does not.
 */

export type PatternToken =
  | "connection"
  | "database"
  | "tag"
  | "year"
  | "month"
  | "monthName"
  | "day"
  | "hour"
  | "minute"
  | "second";

export const PATTERN_TOKENS: ReadonlyArray<{ token: PatternToken; label: string }> = [
  { token: "connection", label: "Connection" },
  { token: "database", label: "Database" },
  { token: "tag", label: "Tag" },
  { token: "year", label: "Year" },
  { token: "month", label: "Month" },
  { token: "monthName", label: "Month name" },
  { token: "day", label: "Day" },
  { token: "hour", label: "Hour" },
  { token: "minute", label: "Minute" },
  { token: "second", label: "Second" },
];

export const DEFAULT_PATTERN = "{database}_{year}-{month}-{day}_{hour}-{minute}-{second}";

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

export interface PatternContext {
  connection: string;
  database: string;
  /** The connection's environment tag (`production`…); empty when it has none. */
  tag: string | null;
  date: Date;
}

const pad = (n: number, width = 2) => String(n).padStart(width, "0");

function tokenValue(token: PatternToken, ctx: PatternContext): string {
  const d = ctx.date;
  switch (token) {
    case "connection":
      return ctx.connection;
    case "database":
      return ctx.database;
    case "tag":
      return ctx.tag ?? "";
    case "year":
      return pad(d.getFullYear(), 4);
    case "month":
      return pad(d.getMonth() + 1);
    case "monthName":
      return MONTHS[d.getMonth()] ?? "";
    case "day":
      return pad(d.getDate());
    case "hour":
      return pad(d.getHours());
    case "minute":
      return pad(d.getMinutes());
    case "second":
      return pad(d.getSeconds());
  }
}

const TOKEN_NAMES = new Set<string>(PATTERN_TOKENS.map((t) => t.token));
const isToken = (name: string): name is PatternToken => TOKEN_NAMES.has(name);

/** A value as one file-name part: no path separators, colons or control characters. */
export function sanitizeValue(value: string): string {
  // eslint-disable-next-line no-control-regex
  return value.replace(/[/:\\\u0000-\u001f\u007f]/g, "_").trim();
}

type Part = { literal: string } | { value: string };

function parts(pattern: string, ctx: PatternContext): Part[] {
  const out: Part[] = [];
  const re = /\{([a-zA-Z]+)\}/g;
  let last = 0;
  for (let m = re.exec(pattern); m; m = re.exec(pattern)) {
    const name = m[1]!;
    if (!isToken(name)) continue; // Unknown `{…}` stays as typed.
    if (m.index > last) out.push({ literal: pattern.slice(last, m.index) });
    out.push({ value: sanitizeValue(tokenValue(name, ctx)) });
    last = m.index + m[0].length;
  }
  if (last < pattern.length) out.push({ literal: pattern.slice(last) });
  return out;
}

const SEPARATOR = /[-_ .]/;

/**
 * Renders `pattern` into a relative path without extension. An empty token also drops the
 * separator after it (or before it, at the end), so `{tag}_{database}` without a tag gives
 * `app`, not `_app`. Empty, `.` and `..` folder names are dropped; never empty (`backup`).
 */
export function renderPattern(pattern: string, ctx: PatternContext): string {
  const list = parts(pattern, ctx);
  let text = "";
  let dropNextSeparator = false;
  list.forEach((part, i) => {
    if ("value" in part) {
      if (part.value === "") {
        const next = list[i + 1];
        if (next && "literal" in next && SEPARATOR.test(next.literal[0] ?? "")) {
          dropNextSeparator = true;
        } else if (SEPARATOR.test(text.at(-1) ?? "")) {
          text = text.slice(0, -1);
        }
        return;
      }
      text += part.value;
      return;
    }
    let literal = part.literal;
    if (dropNextSeparator) {
      literal = literal.slice(1);
      dropNextSeparator = false;
    }
    text += literal;
  });
  const segments = text
    .split("/")
    .map((s) => s.trim())
    .filter((s) => s !== "" && s !== "." && s !== "..");
  return segments.length > 0 ? segments.join("/") : "backup";
}

/** The backup's path below the chosen folder: rendered pattern + extension. */
export function backupFileName(pattern: string, ctx: PatternContext, extension: string): string {
  return `${renderPattern(pattern, ctx)}${extension}`;
}

/** Inserts `{token}` into `pattern` at `[start, end)` (the input's selection). */
export function insertToken(
  pattern: string,
  token: PatternToken,
  start = pattern.length,
  end = start,
): { pattern: string; caret: number } {
  const text = `{${token}}`;
  const s = Math.max(0, Math.min(start, pattern.length));
  const e = Math.max(s, Math.min(end, pattern.length));
  return { pattern: pattern.slice(0, s) + text + pattern.slice(e), caret: s + text.length };
}
