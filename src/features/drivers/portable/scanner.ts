import type { StatementRange } from "@/features/editor/scanner";
import type { LexProfile } from "@/lib/ipc/bindings/LexProfile";
import type { Reading } from "@/lib/ipc/bindings/Reading";

/**
 * The portable scanner and splitter of the extension dialects (T-SQL, ClickHouse, Snowflake…): a
 * port of the backend's `sqlgen/dialect/lex.rs` (`scan`) and `sqlgen/portable/split.rs`, driven by
 * the same data, the dialect's `LexProfile` (`src/lib/ipc/bindings/lex/<d>.json`). So the
 * console's "Run current statement" and Safe Mode split and judge a statement the way the backend
 * does; the shared cases (`cases.ts`) run on both sides. The core dialects keep their own scanner
 * (`features/editor/scanner.ts`).
 *
 * Like the backend's, the scan works on characters (code points): token offsets are character
 * offsets. `split` gives its statements' offsets in UTF-16 code units, as the editor counts them.
 */

/** Backslashes are plain characters everywhere. */
export const PLAIN: Reading = { stringBackslash: false, identBackslash: false };

export type TokenKind =
  /** A bare word or number; a number runs on into the letters after it (`1DELETE`). */
  | "word"
  /** A quoted identifier. */
  | "ident"
  /** A string literal (with its `E`, `r` or `b` prefix) or a dollar-quoted body. */
  | "str"
  | "comment"
  | "space"
  /** Any other single character. */
  | "punct";

/** A token as a span of characters. */
export interface Token {
  kind: TokenKind;
  start: number;
  end: number;
}

/** The tokens of a text under one reading. Every character is in exactly one token. */
export interface Scan {
  chars: readonly string[];
  tokens: Token[];
  /** The reading the text was scanned under. */
  reading: Reading;
  /**
   * A server may read the text otherwise than this scan: a comment it may end elsewhere, a
   * literal opener lexers class differently, an invisible character in code (see `LexProfile`).
   * The classifier then calls the text a write, and it is never one statement.
   */
  ambiguous: boolean;
}

export const text = (scan: Scan, t: Token): string => scan.chars.slice(t.start, t.end).join("");

/** ASCII letters upper-cased only: servers match keywords in ASCII (`ſelect` is a name). */
export function asciiUpper(s: string): string {
  return s.replace(/[a-z]+/g, (m) => m.toUpperCase());
}

/** The token's text in upper case, ASCII letters only (`Scan::upper`). */
export const upper = (scan: Scan, t: Token): string => asciiUpper(text(scan, t));

/** Whether `t` is the punctuation character `c`. */
export const isPunct = (scan: Scan, t: Token, c: string): boolean =>
  t.kind === "punct" && scan.chars[t.start] === c;

/** The name a quoted identifier stands for (quotes removed, doubled quotes undone). */
export function identName(scan: Scan, t: Token): string {
  const inner = scan.chars.slice(t.start + 1, Math.max(t.end - 1, t.start + 1));
  const close = scan.chars[t.end - 1];
  let out = "";
  let i = 0;
  while (i < inner.length) {
    out += inner[i];
    i += inner[i] === close && inner[i + 1] === close ? 2 : 1;
  }
  return out;
}

/** Whether `name` (upper case) is in `list`, whose `PREFIX*` entries match by prefix. */
export function listed(list: readonly string[], name: string): boolean {
  return list.some((entry) =>
    entry.endsWith("*") ? name.startsWith(entry.slice(0, -1)) : entry === name,
  );
}

// Rust's `char` classes (Unicode properties), with an ASCII fast path.
const WHITE_SPACE = /^\p{White_Space}$/u;
const ALPHABETIC = /^\p{Alphabetic}$/u;
const ALPHANUMERIC = /^[\p{Alphabetic}\p{N}]$/u;
const CONTROL = /^\p{Cc}$/u;

const isAscii = (c: string): boolean => c.charCodeAt(0) < 0x80;
const isAsciiDigit = (c: string | undefined): boolean => c !== undefined && c >= "0" && c <= "9";
const isAsciiLetter = (c: string): boolean => (c >= "a" && c <= "z") || (c >= "A" && c <= "Z");
const isAsciiAlphanumeric = (c: string): boolean => isAsciiLetter(c) || isAsciiDigit(c);

export function isWhitespace(c: string | undefined): boolean {
  if (c === undefined) return false;
  if (isAscii(c)) return c === " " || (c >= "\t" && c <= "\r");
  return WHITE_SPACE.test(c);
}

const isAlphabetic = (c: string): boolean => (isAscii(c) ? isAsciiLetter(c) : ALPHABETIC.test(c));

const isAlphanumeric = (c: string): boolean =>
  isAscii(c) ? isAsciiAlphanumeric(c) : ALPHANUMERIC.test(c);

const isControl = (c: string): boolean => CONTROL.test(c);

export const isIdentStart = (c: string | undefined): boolean =>
  c !== undefined && (isAlphabetic(c) || c === "_");

export const isWordChar = (c: string | undefined): boolean =>
  c !== undefined && (isAlphanumeric(c) || c === "_" || c === "$");

/** Default-ignorable code points (category Cf and others, Unicode 16), as the backend lists them. */
const INVISIBLE: ReadonlyArray<readonly [number, number]> = [
  [0xad, 0xad],
  [0x34f, 0x34f],
  [0x600, 0x605],
  [0x61c, 0x61c],
  [0x6dd, 0x6dd],
  [0x70f, 0x70f],
  [0x890, 0x891],
  [0x8e2, 0x8e2],
  [0x115f, 0x1160],
  [0x17b4, 0x17b5],
  [0x180b, 0x180f],
  [0x200b, 0x200f],
  [0x202a, 0x202e],
  [0x2060, 0x206f],
  [0x3164, 0x3164],
  [0xfe00, 0xfe0f],
  [0xfeff, 0xfeff],
  [0xffa0, 0xffa0],
  [0xfff0, 0xfffb],
  [0x110bd, 0x110bd],
  [0x110cd, 0x110cd],
  [0x13430, 0x1343f],
  [0x1bca0, 0x1bca3],
  [0x1d173, 0x1d17a],
  [0xe0000, 0xe0fff],
];

/**
 * Whether `c` is a format character or another default-ignorable code point: characters a text
 * shows as nothing, which servers skip, drop or keep (`is_invisible`).
 */
export function isInvisible(c: string): boolean {
  const cp = c.codePointAt(0) ?? 0;
  return INVISIBLE.some(([lo, hi]) => cp >= lo && cp <= hi);
}

/** A character a dollar-quote tag may hold for some lexer (PostgreSQL's take every non-ASCII one). */
const isTagChar = (c: string): boolean => isAsciiAlphanumeric(c) || c === "_" || !isAscii(c);

/** Line breaks some lexers end a line comment at besides LF. */
const isOtherLineBreak = (c: string): boolean =>
  c === "\r" || c === "\u000b" || c === "\u000c" || c === "\u0085" || c === " " || c === " ";

class Scanner {
  constructor(
    private readonly lex: LexProfile,
    private readonly reading: Reading,
    private readonly chars: readonly string[],
  ) {}

  private at(i: number): string | undefined {
    return this.chars[i];
  }

  token(i: number): [TokenKind, number] {
    const c = this.chars[i]!;
    const next = this.at(i + 1);
    const lex = this.lex;
    // A byte-order mark that starts the text (a file saved with one) is no part of its SQL.
    if (isWhitespace(c) || (i === 0 && c === "﻿")) {
      let j = i + 1;
      while (isWhitespace(this.at(j))) j++;
      return ["space", j];
    }
    let lineComment = false;
    if (c === "-" && next === "-") {
      const after = this.at(i + 2);
      lineComment =
        !lex.dashCommentNeedsSpace ||
        after === undefined ||
        isWhitespace(after) ||
        isControl(after);
    } else if (c === "#") lineComment = lex.hashComments;
    else if (c === "/" && next === "/") lineComment = lex.slashSlashComments;
    if (lineComment) return ["comment", this.lineEnd(i)];
    if (c === "/" && next === "*") return ["comment", this.blockCommentEnd(i)];
    if (c === "$" && lex.dollarQuotes) {
      const len = this.dollarTag(i);
      if (len !== null) return ["str", this.dollarEnd(i, len)];
    }
    if (lex.stringQuotes.includes(c))
      return ["str", this.stringEnd(i, this.reading.stringBackslash)];
    const ident = lex.identQuotes.find(([open]) => open === c);
    if (ident) return ["ident", this.quotedEnd(i, ident[1], this.reading.identBackslash)];
    if (isIdentStart(c) || isAsciiDigit(c)) {
      let j = i;
      while (isWordChar(this.at(j))) j++;
      const end = this.prefixedString(i, j);
      return end === null ? ["word", j] : ["str", end];
    }
    return ["punct", i + 1];
  }

  /** A string whose prefix is the word `i..j` (`E'…'`, `r'…'`, `b'…'`), when the prefix changes
   * how it reads; its end. */
  private prefixedString(i: number, j: number): number | null {
    const quote = this.at(j);
    if (quote === undefined || !this.lex.stringQuotes.includes(quote)) return null;
    const prefix = this.chars.slice(i, j).join("").toUpperCase();
    const lex = this.lex;
    if (prefix === "E" && lex.eStrings && quote === "'") return this.quotedEnd(j, quote, true);
    // No escapes, but a backslash and the character after it stay together.
    if ((prefix === "R" || prefix === "RB" || prefix === "BR") && lex.rawPrefixes)
      return this.stringEnd(j, true);
    if (prefix === "B" && lex.bytePrefixes) return this.stringEnd(j, this.reading.stringBackslash);
    return null;
  }

  /** End of a line comment for splitting: before the LF, or before a lone CR. */
  private lineEnd(i: number): number {
    let j = i;
    while (j < this.chars.length && this.chars[j] !== "\n" && this.chars[j] !== "\r") j++;
    return j;
  }

  /** Whether a server may end the comment `i..end` elsewhere (`Scan.ambiguous`). */
  commentMayEndElsewhere(i: number, end: number): boolean {
    const chars = this.chars;
    if (chars[i] === "/" && this.at(i + 1) === "*") {
      for (let j = i + 2; j + 1 < end; j++)
        if (chars[j] === "/" && chars[j + 1] === "*") return true;
      return false;
    }
    // A line comment, up to the LF: a CR right before the LF ends the same line either way.
    for (let j = i; j < chars.length && chars[j] !== "\n"; j++) {
      const c = chars[j]!;
      if (isOtherLineBreak(c) && !(c === "\r" && this.at(j + 1) === "\n")) return true;
    }
    return false;
  }

  /**
   * Whether a lexer may open a literal elsewhere than this scan did around the token `i..end` of
   * `kind`: a `$` that may open a body next to a character lexers class differently, a body that
   * never closes, or an `E'…'` string whose prefix a lexer may read otherwise.
   */
  literalMayOpenElsewhere(kind: TokenKind, i: number, end: number): boolean {
    const chars = this.chars;
    const afterNonAscii = (j: number) => j > 0 && !isAscii(chars[j - 1]!);
    const number = kind === "word" && isAsciiDigit(chars[i]);
    if (this.lex.dollarQuotes && kind === "str" && chars[i] === "$") {
      const len = this.dollarTag(i) ?? 1;
      let closed = end - i >= 2 * len;
      for (let k = 0; closed && k < len; k++)
        if (chars[end - len + k] !== chars[i + k]) closed = false;
      if (!closed) return true;
    }
    if (this.lex.dollarQuotes) {
      // The `$`s of code: in words and punctuation, and the opener of a body.
      const [from, to] =
        kind === "word" || kind === "punct"
          ? [i, end]
          : kind === "str" && chars[i] === "$"
            ? [i, i + 1]
            : [i, i];
      for (let j = from; j < to; j++) {
        if (chars[j] !== "$") continue;
        let k = j + 1;
        let nonAscii = false;
        while (k < chars.length && isTagChar(chars[k]!)) {
          if (!isAscii(chars[k]!)) nonAscii = true;
          k++;
        }
        const opens = chars[k] === "$";
        if (opens && (number || afterNonAscii(j) || nonAscii)) return true;
      }
    }
    if (this.lex.eStrings) {
      const eString = kind === "str" && (chars[i] === "E" || chars[i] === "e");
      const gluedE =
        number && (chars[end - 1] === "E" || chars[end - 1] === "e") && this.at(end) === "'";
      if ((eString && afterNonAscii(i)) || gluedE) return true;
    }
    return false;
  }

  private blockCommentEnd(i: number): number {
    const chars = this.chars;
    let depth = 1;
    let j = i + 2;
    while (j < chars.length) {
      const next = this.at(j + 1);
      if (this.lex.nestedBlockComments && chars[j] === "/" && next === "*") {
        depth++;
        j += 2;
      } else if (chars[j] === "*" && next === "/") {
        depth--;
        j += 2;
        if (depth === 0) return j;
      } else j++;
    }
    return chars.length;
  }

  /** A string at `i`, triple-quoted where the dialect has those. */
  private stringEnd(i: number, backslash: boolean): number {
    const chars = this.chars;
    const q = chars[i]!;
    if (this.lex.tripleQuotes && this.at(i + 1) === q && this.at(i + 2) === q) {
      let j = i + 3;
      while (j < chars.length) {
        if (backslash && chars[j] === "\\") j += 2;
        else if (chars[j] === q && this.at(j + 1) === q && this.at(j + 2) === q) return j + 3;
        else j++;
      }
      return chars.length;
    }
    return this.quotedEnd(i, q, backslash);
  }

  /** End of the run quoted from `i` up to `close` (doubled: escaped; and after a backslash when
   * `backslash` is set). */
  private quotedEnd(i: number, close: string, backslash: boolean): number {
    const chars = this.chars;
    let j = i + 1;
    while (j < chars.length) {
      const c = chars[j];
      if (backslash && c === "\\") j += 2;
      else if (c === close) {
        if (this.at(j + 1) === close) j += 2;
        else return j + 1;
      } else j++;
    }
    return chars.length;
  }

  /** Length of the `$$` / `$tag$` opener at `i`. A `$` after a word character (`a$b`) or before a
   * digit (`$1`) opens nothing, and a tag opens nothing without `dollarTags`. */
  private dollarTag(i: number): number | null {
    if (i > 0 && isWordChar(this.chars[i - 1])) return null;
    let j = i + 1;
    const first = this.at(j);
    if (first === "$") return 2;
    if (!(this.lex.dollarTags && isIdentStart(first))) return null;
    j++;
    for (let c = this.at(j); c !== undefined; c = this.at(++j)) {
      if (c === "$") return j + 1 - i;
      if (!(isAlphanumeric(c) || c === "_")) return null;
    }
    return null;
  }

  private dollarEnd(i: number, len: number): number {
    const chars = this.chars;
    for (let j = i + len; j + len <= chars.length; j++) {
      let same = true;
      for (let k = 0; same && k < len; k++) if (chars[j + k] !== chars[i + k]) same = false;
      if (same) return j + len;
    }
    return chars.length;
  }
}

/**
 * Whether ClickHouse may read a heredoc at a `$` of code where the scan opened none: it takes the
 * text from a `$` that starts a token up to the next `$` as the delimiter whatever it holds, and
 * reads a heredoc when that delimiter comes again.
 */
function heredocElsewhere(chars: readonly string[], tokens: readonly Token[]): boolean {
  // A `$` starts a token unless a word goes on through it (`a$b`); after a number it does.
  const starts = (p: number, number: boolean) => {
    if (number || p === 0) return true;
    const c = chars[p - 1]!;
    return !(isAsciiAlphanumeric(c) || c === "_" || c === "$");
  };
  const candidates: number[] = [];
  for (const t of tokens) {
    if (t.kind !== "word" && t.kind !== "punct") continue;
    const number = t.kind === "word" && isAsciiDigit(chars[t.start]);
    for (let p = t.start; p < t.end; p++)
      if (chars[p] === "$" && starts(p, number)) candidates.push(p);
  }
  if (candidates.length === 0) return false;
  const dollars: number[] = [];
  chars.forEach((c, p) => {
    if (c === "$") dollars.push(p);
  });
  // The delimiter at each `$` runs to the next one, so the delimiter of `p` comes again at `r`
  // exactly when `r`'s own delimiter is the same text: the last `$` each delimiter starts at.
  const delimiter = (from: number, to: number) => chars.slice(from, to + 1).join("");
  const last = new Map<string, number>();
  for (let k = 0; k + 1 < dollars.length; k++)
    last.set(delimiter(dollars[k]!, dollars[k + 1]!), dollars[k]!);
  return candidates.some((p) => {
    // The first `$` after `p` (binary search: a long text may hold many).
    let lo = 0;
    let hi = dollars.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (dollars[mid]! <= p) lo = mid + 1;
      else hi = mid;
    }
    const q = dollars[lo];
    if (q === undefined) return false;
    const r = last.get(delimiter(p, q));
    return r !== undefined && r > q;
  });
}

/**
 * Splits `sql` into tokens as `lex` reads it under `reading`. An unterminated string, quoted name
 * or comment runs to the end of the text.
 */
export function scan(lex: LexProfile, reading: Reading, sql: string): Scan {
  const chars = Array.from(sql);
  const s = new Scanner(lex, reading, chars);
  const tokens: Token[] = [];
  let ambiguous = false;
  let i = 0;
  while (i < chars.length) {
    const [kind, to] = s.token(i);
    const end = Math.min(Math.max(to, i + 1), chars.length);
    if (kind === "comment") {
      if (s.commentMayEndElsewhere(i, end)) ambiguous = true;
    } else if (
      (kind === "word" || kind === "punct") &&
      chars.slice(i, end).some((c) => isInvisible(c))
    ) {
      ambiguous = true;
    } else if (s.literalMayOpenElsewhere(kind, i, end)) {
      ambiguous = true;
    }
    tokens.push({ kind, start: i, end });
    i = end;
  }
  if (lex.dollarTags && !ambiguous) ambiguous = heredocElsewhere(chars, tokens);
  return { chars, tokens, reading, ambiguous };
}

/** The reading splitting uses: the dialect's first. */
export function firstReading(lex: LexProfile): Reading {
  return lex.readings[0] ?? PLAIN;
}

/** Words after `BEGIN` that start a transaction rather than a block. */
const TRANSACTION_WORDS = [
  "TRANSACTION",
  "TRAN",
  "WORK",
  "DISTRIBUTED",
  "DEFERRED",
  "IMMEDIATE",
  "EXCLUSIVE",
  "NAME",
  "ISOLATION",
  "READ",
];

/** Words after `END` that close a construct whose opener the block depth does not count. */
const END_OF_CONSTRUCT = ["IF", "LOOP", "WHILE", "REPEAT", "FOR"];

/** A statement's tokens: `start..end` of `Scan.tokens`. */
export interface TokenRange {
  start: number;
  end: number;
}

const isCode = (t: Token) => t.kind !== "space" && t.kind !== "comment";

/** The next token after `i` that is neither space nor a comment. */
function nextCode(scan: Scan, i: number): number | null {
  for (let j = i + 1; j < scan.tokens.length; j++) if (isCode(scan.tokens[j]!)) return j;
  return null;
}

function previousCode(scan: Scan, i: number): number | null {
  for (let j = i - 1; j >= 0; j--) if (isCode(scan.tokens[j]!)) return j;
  return null;
}

/**
 * The token ranges of the statements of `scan`: from the first token that is neither space nor a
 * comment to the last one before the terminator, trailing space left out. A `;` ends a statement
 * except inside a `BEGIN … END` block (`beginEndBodies`); with a batch separator (T-SQL's `GO` on a
 * line of its own) only the separator does.
 */
export function statements(lex: LexProfile, scan: Scan): TokenRange[] {
  const toks = scan.tokens;
  const out: TokenRange[] = [];
  let start: number | null = null;
  let depth = 0;
  let i = 0;
  const finish = (end: number) => {
    if (start === null) return;
    let e = end;
    while (e > start && toks[e - 1]!.kind === "space") e--;
    if (e > start) out.push({ start, end: e });
    start = null;
  };
  while (i < toks.length) {
    const t = toks[i]!;
    if (!isCode(t)) {
      i++;
      continue;
    }
    if (isPunct(scan, t, ";") && lex.batchSeparator === null && depth === 0) {
      finish(i);
      i++;
      continue;
    }
    if (t.kind === "word") {
      const next = lex.batchSeparator === null ? null : separatorLine(scan, i, lex.batchSeparator);
      if (next !== null) {
        finish(i);
        depth = 0;
        i = next;
        continue;
      }
      if (lex.beginEndBodies) depth = blockDepth(scan, i, depth);
    }
    // A `;` with nothing before it starts no statement.
    if (start === null && !isPunct(scan, t, ";")) start = i;
    i++;
  }
  finish(toks.length);
  return out;
}

/**
 * When the word at `i` is the batch separator on a line of its own (blanks, an optional repeat
 * count and an optional `--` comment around it), the index of the first token after that line.
 */
function separatorLine(scan: Scan, i: number, separator: string): number | null {
  const t = scan.tokens[i]!;
  if (asciiUpper(text(scan, t)) !== asciiUpper(separator)) return null;
  const chars = scan.chars;
  for (let j = t.start; j > 0; j--) {
    const c = chars[j - 1];
    if (c === "\n") break;
    if (c !== " " && c !== "\t") return null;
  }
  const blanks = (from: number) => {
    let k = from;
    while (chars[k] === " " || chars[k] === "\t") k++;
    return k;
  };
  let k = blanks(t.end);
  while (isAsciiDigit(chars[k])) k++;
  k = blanks(k);
  if (chars[k] === "-" && chars[k + 1] === "-")
    while (k < chars.length && chars[k] !== "\n" && chars[k] !== "\r") k++;
  if (chars[k] === "\r") k++;
  if (k < chars.length) {
    if (chars[k] !== "\n") return null;
    k++;
  }
  const after = scan.tokens.findIndex((tok) => tok.start >= k);
  return after < 0 ? scan.tokens.length : after;
}

/** The `BEGIN … END` depth after the word at `i`. */
function blockDepth(scan: Scan, i: number, depth: number): number {
  const prev = previousCode(scan, i);
  if (prev !== null && isPunct(scan, scan.tokens[prev]!, ".")) return depth; // a qualified name: `t.end`
  const word = upper(scan, scan.tokens[i]!);
  if (word === "BEGIN") {
    const n = nextCode(scan, i);
    const next = n === null ? null : scan.tokens[n]!;
    const opens =
      next !== null &&
      !isPunct(scan, next, ";") &&
      !(next.kind === "word" && TRANSACTION_WORDS.includes(upper(scan, next)));
    return depth + (opens ? 1 : 0);
  }
  if (word === "CASE" && depth > 0) return depth + 1;
  if (word === "END" && depth > 0) {
    const n = nextCode(scan, i);
    const closesConstruct =
      n !== null &&
      scan.tokens[n]!.kind === "word" &&
      END_OF_CONSTRUCT.includes(upper(scan, scan.tokens[n]!));
    return closesConstruct ? depth : depth - 1;
  }
  return depth;
}

/** A statement of the editor's text: its SQL and where it is (UTF-16 offsets, 1-based line). */
export interface SplitStatement extends StatementRange {
  sql: string;
}

/**
 * Splits editor text into statements. Leading comments are not part of a statement, and empty or
 * comment-only statements are skipped, as in the core splitter.
 */
export function split(lex: LexProfile, sql: string): SplitStatement[] {
  const s = scan(lex, firstReading(lex), sql);
  // Statements come in order, so offsets and lines go on from the previous one's.
  let at = 0;
  let utf16 = 0;
  let line = 1;
  const advance = (to: number) => {
    for (; at < to; at++) {
      const c = s.chars[at]!;
      utf16 += c.length;
      if (c === "\n") line++;
    }
    return utf16;
  };
  return statements(lex, s).map((r) => {
    const startChar = s.tokens[r.start]!.start;
    const endChar = s.tokens[r.end - 1]!.end;
    const start = advance(startChar);
    const startLine = line;
    const end = advance(endChar);
    return { sql: s.chars.slice(startChar, endChar).join(""), start, end, line: startLine };
  });
}
