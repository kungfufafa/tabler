import type { StatementRange } from "@/features/editor/scanner";

/**
 * The console's command lines, read the way the backend reads them (`drivers/redis/cli.rs`,
 * redis-cli's `sdssplitargs`): arguments split at whitespace; `"…"` takes `\n` `\r` `\t` `\b` `\a`
 * `\xHH` and `\<any>`; `'…'` is literal except `\'`; a quote may open in the middle of a word and
 * a closing one must be followed by whitespace or the end. Arguments are bytes (the line's UTF-8,
 * `\xff` one byte). The text holds one command per line, or several separated by `;`, both only
 * outside quotes.
 */

/** Why a line cannot be read (the backend's `CliError` text). */
export type CliError = "unbalanced" | "afterQuote";

export const CLI_ERROR_TEXT: Record<CliError, string> = {
  unbalanced: "Invalid argument(s): unbalanced quotes",
  afterQuote: "Invalid argument(s): a closing quote must be followed by a space",
};

export type Argv = Uint8Array[];

const utf8 = new TextEncoder();
const strict = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

/** C's `isspace`: what separates arguments and may follow a closing quote. */
const isSpace = (b: number | undefined) =>
  b === 0x20 || b === 0x09 || b === 0x0a || b === 0x0b || b === 0x0c || b === 0x0d;

function hex(b: number | undefined): number | null {
  if (b === undefined) return null;
  if (b >= 0x30 && b <= 0x39) return b - 0x30;
  if (b >= 0x61 && b <= 0x66) return b - 0x61 + 10;
  if (b >= 0x41 && b <= 0x46) return b - 0x41 + 10;
  return null;
}

/** Splits one command line into its arguments (`cli::tokenize`). */
export function tokenize(line: string): { ok: true; argv: Argv } | { ok: false; error: CliError } {
  const p = utf8.encode(line);
  let i = 0;
  const args: Argv = [];
  for (;;) {
    while (i < p.length && isSpace(p[i])) i++;
    if (i >= p.length) return { ok: true, argv: args };
    const cur: number[] = [];
    let dq = false;
    let sq = false;
    for (;;) {
      const c = p[i];
      if (dq) {
        if (c === undefined) return { ok: false, error: "unbalanced" };
        const hi = hex(p[i + 2]);
        const lo = hex(p[i + 3]);
        if (c === 0x5c && p[i + 1] === 0x78 && hi !== null && lo !== null) {
          cur.push((hi << 4) | lo);
          i += 3;
        } else if (c === 0x5c && i + 1 < p.length) {
          i++;
          const e = p[i]!;
          cur.push(
            e === 0x6e
              ? 0x0a
              : e === 0x72
                ? 0x0d
                : e === 0x74
                  ? 0x09
                  : e === 0x62
                    ? 0x08
                    : e === 0x61
                      ? 0x07
                      : e,
          );
        } else if (c === 0x22) {
          const n = p[i + 1];
          if (n !== undefined && !isSpace(n)) return { ok: false, error: "afterQuote" };
          i++;
          break;
        } else cur.push(c);
      } else if (sq) {
        if (c === undefined) return { ok: false, error: "unbalanced" };
        if (c === 0x5c && p[i + 1] === 0x27) {
          i++;
          cur.push(0x27);
        } else if (c === 0x27) {
          const n = p[i + 1];
          if (n !== undefined && !isSpace(n)) return { ok: false, error: "afterQuote" };
          i++;
          break;
        } else cur.push(c);
      } else {
        if (c === undefined || c === 0x20 || c === 0x0a || c === 0x0d || c === 0x09) break;
        if (c === 0x22) dq = true;
        else if (c === 0x27) sq = true;
        else cur.push(c);
      }
      i++;
    }
    args.push(Uint8Array.from(cur));
  }
}

/** Bytes as text when they are UTF-8; null otherwise. */
export function utf8Text(bytes: Uint8Array): string | null {
  try {
    return strict.decode(bytes);
  } catch {
    return null;
  }
}

/** Rust's `to_ascii_uppercase`: `ß` and `ſ` stay as they are. */
export const asciiUpper = (s: string): string => s.replace(/[a-z]+/g, (m) => m.toUpperCase());

/** The command name of an argv, upper-case; null for an empty line or a name that is not text. */
export function commandName(argv: Argv): string | null {
  const first = argv[0];
  if (!first) return null;
  const name = utf8Text(first);
  return name ? asciiUpper(name) : null;
}

/** Rust's `char::is_whitespace` (the Unicode White_Space property). */
export function isWhitespace(c: string): boolean {
  const n = c.codePointAt(0) ?? -1;
  return (
    (n >= 0x09 && n <= 0x0d) ||
    n === 0x20 ||
    n === 0x85 ||
    n === 0xa0 ||
    n === 0x1680 ||
    (n >= 0x2000 && n <= 0x200a) ||
    n === 0x2028 ||
    n === 0x2029 ||
    n === 0x202f ||
    n === 0x205f ||
    n === 0x3000
  );
}

/** One command of the text: its text, and where it is in code points (the backend's offsets). */
export interface ConsoleCommand {
  sql: string;
  start: number;
  end: number;
  line: number;
}

/**
 * Splits editor text into commands at line breaks and `;` outside quotes, each trimmed, blank
 * ones skipped (`cli::split_console`). Offsets count code points, as the backend's do.
 */
export function splitConsole(text: string): ConsoleCommand[] {
  const chars = Array.from(text);
  const out: ConsoleCommand[] = [];
  let q: "no" | "double" | "single" = "no";
  let start = 0;
  const flush = (from: number, to: number) => {
    let s = from;
    let e = to;
    while (s < e && isWhitespace(chars[s]!)) s++;
    while (e > s && isWhitespace(chars[e - 1]!)) e--;
    if (s < e) {
      let line = 1;
      for (let k = 0; k < s; k++) if (chars[k] === "\n") line++;
      out.push({ sql: chars.slice(s, e).join(""), start: s, end: e, line });
    }
  };
  for (let i = 0; i < chars.length; i++) {
    const c = chars[i];
    if (q === "double") {
      if (c === "\\") i++;
      else if (c === '"') q = "no";
    } else if (q === "single") {
      if (c === "\\" && chars[i + 1] === "'") i++;
      else if (c === "'") q = "no";
    } else if (c === '"') q = "double";
    else if (c === "'") q = "single";
    else if (c === "\n" || c === ";") {
      flush(start, i);
      start = i + 1;
    }
  }
  flush(start, Math.max(chars.length, start));
  return out;
}

/** The commands of `text` as the editor's ranges (UTF-16 offsets, 1-based lines). */
export function consoleRanges(text: string): StatementRange[] {
  const at: number[] = [];
  let i = 0;
  for (const c of text) {
    at.push(i);
    i += c.length;
  }
  at.push(i);
  return splitConsole(text).map((s) => ({ start: at[s.start]!, end: at[s.end]!, line: s.line }));
}

/** Rust's `char::is_control` (the Cc category). */
const isControl = (n: number) => n <= 0x1f || (n >= 0x7f && n <= 0x9f);

const hex2 = (b: number) => `\\x${b.toString(16).padStart(2, "0")}`;

/**
 * One argument as redis-cli reads it back to the same bytes (`sqlgen::native::redis::quote_arg`):
 * bare when it is plain printable text, otherwise double-quoted with escapes.
 */
export function quoteArg(arg: Uint8Array | string): string {
  const bytes = typeof arg === "string" ? utf8.encode(arg) : arg;
  const text = utf8Text(bytes);
  if (
    bytes.length > 0 &&
    text !== null &&
    Array.from(text).every((c) => {
      const n = c.codePointAt(0)!;
      return !isControl(n) && !isWhitespace(c) && !` "';`.includes(c);
    })
  )
    return text;
  let out = '"';
  // Valid UTF-8 runs as characters, anything else byte by byte.
  let i = 0;
  while (i < bytes.length) {
    const len = utf8Length(bytes, i);
    if (len === 0) {
      out += hex2(bytes[i]!);
      i++;
      continue;
    }
    const c = strict.decode(bytes.subarray(i, i + len));
    const n = c.codePointAt(0)!;
    if (c === '"') out += '\\"';
    else if (c === "\\") out += "\\\\";
    else if (c === "\n") out += "\\n";
    else if (c === "\r") out += "\\r";
    else if (c === "\t") out += "\\t";
    else if (c === "\u0007") out += "\\a";
    else if (c === "\b") out += "\\b";
    else if (isControl(n)) for (const b of bytes.subarray(i, i + len)) out += hex2(b);
    else out += c;
    i += len;
  }
  return `${out}"`;
}

/** The length of the valid UTF-8 character at `i`; 0 when the bytes there are not one. */
function utf8Length(b: Uint8Array, i: number): number {
  const c = b[i]!;
  const len =
    c < 0x80
      ? 1
      : c >= 0xc2 && c <= 0xdf
        ? 2
        : c >= 0xe0 && c <= 0xef
          ? 3
          : c >= 0xf0 && c <= 0xf4
            ? 4
            : 0;
  if (len <= 1) return len;
  if (i + len > b.length) return 0;
  try {
    strict.decode(b.subarray(i, i + len));
    return len;
  } catch {
    return 0;
  }
}

/** An argv as a redis-cli line (`argv_text`). */
export const argvText = (argv: readonly (Uint8Array | string)[]): string =>
  argv.map(quoteArg).join(" ");
