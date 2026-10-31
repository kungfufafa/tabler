import type { StatementRange } from "@/features/editor/scanner";
import type { StatementClass } from "@/features/safety/classify";

/**
 * The console's mongosh subset as the backend reads it (`drivers/mongodb/shell.rs`, `literal.rs`
 * and `classify.rs`): the splitter, a reader of the literals, and the read/write classifier,
 * which Safe Mode asks before a statement is sent. The classifier is the backend's allow-list,
 * default-deny: a statement is a read only when it parses into a command whose name is a read
 * (an `aggregate` without `$out` or `$merge` anywhere in it, an `explain` of a read), and text it
 * cannot read is a write. Both sides run `src-tauri/tests/fixtures/mongodb/console.cases.json`.
 */

// ---------------------------------------------------------------------------------------------
// Characters, as Rust's `char` methods read them
// ---------------------------------------------------------------------------------------------

/** `char::is_whitespace` (Unicode White_Space, which has U+0085 and not U+FEFF). */
const isWhitespace = (c: string): boolean => c === "\u0085" || (c !== "﻿" && /\s/u.test(c));
const isAlphabetic = (c: string): boolean => /\p{Alphabetic}/u.test(c);
const isAlphanumeric = (c: string): boolean => /[\p{Alphabetic}\p{N}]/u.test(c);
const isIdentStart = (c: string): boolean => c === "_" || c === "$" || isAlphabetic(c);
const isIdentChar = (c: string): boolean => c === "_" || c === "$" || isAlphanumeric(c);
const isDigit = (c: string | undefined): boolean => c !== undefined && c >= "0" && c <= "9";
const isAsciiAlnum = (c: string): boolean => /^[0-9A-Za-z]$/.test(c);

/** The code point at `i` of `s` (a whole surrogate pair), or undefined at the end. */
function charAt(s: string, i: number): string | undefined {
  const cp = s.codePointAt(i);
  return cp === undefined ? undefined : String.fromCodePoint(cp);
}

// ---------------------------------------------------------------------------------------------
// Splitting (`shell::split`)
// ---------------------------------------------------------------------------------------------

/** One statement: its text and where it is (UTF-16 offsets; 1-based line). */
export interface ShellPiece {
  sql: string;
  start: number;
  end: number;
  line: number;
}

/** Whether the next code after a line break starts with `.` (a chained call on a new line). */
function continues(t: string, i: number): boolean {
  while (i < t.length) {
    const c = t[i]!;
    if (isWhitespace(c)) i += 1;
    else if (c === "/" && t[i + 1] === "/") {
      while (i < t.length && t[i] !== "\n") i += 1;
    } else if (c === "/" && t[i + 1] === "*") {
      i += 2;
      while (i < t.length && !(t[i] === "*" && t[i + 1] === "/")) i += 1;
      i += 2;
    } else return c === "." && !isDigit(t[i + 1]);
  }
  return false;
}

/** A `/` starts a regular expression where a value is expected. */
const regexMayStart = (last: string | null): boolean =>
  last === null || "(,:[{=!&|?;+-*%<>~^".includes(last);

/**
 * The statements of a console text: a statement ends at a `;` or at a line break after a
 * complete expression; a line that starts with `.` continues the one before it. Comments before
 * a statement are not part of it; empty statements are skipped.
 */
export function splitShell(text: string): ShellPiece[] {
  const t = text;
  const n = t.length;
  const out: ShellPiece[] = [];
  let start: number | null = null;
  // The last code character and where it is, for the regex heuristic and continuation.
  let lastAt = -1;
  let lastC: string | null = null;
  let depth = 0;
  let i = 0;
  const flush = (end: number) => {
    if (start !== null && end > start) {
      let line = 1;
      for (let k = 0; k < start; k++) if (t[k] === "\n") line += 1;
      out.push({ sql: t.slice(start, end), start, end, line });
    }
    start = null;
  };
  // After a string or regex scanned up to `j`: where scanning goes on (past the closing quote,
  // or at the line break that cut it).
  const closeAt = (j: number, quote: string): number => {
    lastC = quote;
    if (j < n && t[j] === quote) {
      lastAt = j;
      return j + 1;
    }
    const k = Math.min(j, n);
    lastAt = Math.max(k - 1, 0);
    return k;
  };
  while (i < n) {
    const c = t[i]!;
    const next = t[i + 1];
    if (c === "/" && next === "/") {
      while (i < n && t[i] !== "\n" && t[i] !== "\r") i += 1;
      continue;
    }
    if (c === "/" && next === "*") {
      i += 2;
      while (i < n && !(t[i] === "*" && t[i + 1] === "/")) i += 1;
      i = Math.min(i + 2, n);
      continue;
    }
    if (c === "\n" || c === "\r") {
      const complete = depth <= 0 && !(lastC !== null && ".,:=([{".includes(lastC));
      if (start !== null && lastC !== null && complete && !continues(t, i + 1)) {
        flush(lastAt + 1);
        lastC = null;
        depth = 0;
      }
      i += 1;
      continue;
    }
    if (c === ";" && depth <= 0) {
      if (lastC !== null) flush(lastAt + 1);
      start = null;
      lastC = null;
      depth = 0;
      i += 1;
      continue;
    }
    if (isWhitespace(c) || c === "﻿") {
      i += 1;
      continue;
    }
    start ??= i;
    if (c === "'" || c === '"' || c === "`") {
      let j = i + 1;
      while (j < n && t[j] !== c) {
        if (t[j] === "\\") j += 1;
        else if (c !== "`" && (t[j] === "\n" || t[j] === "\r")) break;
        j += 1;
      }
      i = closeAt(j, c);
      continue;
    }
    if (c === "/" && regexMayStart(lastC)) {
      let j = i + 1;
      let cls = false;
      while (j < n && t[j] !== "\n" && t[j] !== "\r") {
        const d = t[j];
        if (d === "\\" && t[j + 1] !== "\n" && t[j + 1] !== "\r") j += 1;
        else if (d === "[") cls = true;
        else if (d === "]") cls = false;
        else if (d === "/" && !cls) break;
        j += 1;
      }
      i = closeAt(j, "/");
      continue;
    }
    if (c === "(" || c === "[" || c === "{") depth += 1;
    else if (c === ")" || c === "]" || c === "}") depth -= 1;
    lastAt = i;
    lastC = c;
    i += 1;
  }
  if (lastC !== null) flush(lastAt + 1);
  return out;
}

/** `splitShell` as the editor's statement ranges. */
export const shellRanges = (text: string): StatementRange[] =>
  splitShell(text).map(({ start, end, line }) => ({ start, end, line }));

// ---------------------------------------------------------------------------------------------
// Literals (`literal.rs`), as much of them as the classifier needs
// ---------------------------------------------------------------------------------------------

/** A value of the console's text. */
export type Lit =
  | { k: "doc"; entries: [string, Lit][] }
  | { k: "arr"; items: Lit[] }
  | { k: "str"; v: string }
  | { k: "num"; v: number; int: boolean }
  | { k: "bool"; v: boolean }
  | { k: "null" }
  /** A shell helper, a regex, an Extended JSON wrapper, MinKey…: what it holds is kept. */
  | { k: "other"; name: string; inner: Lit[] };

export class ParseError extends Error {}

const MAX_DEPTH = 128;

/** Extended JSON wrappers (`literal::extended`): one key, or the two-key pairs. */
const WRAPPERS = new Set([
  "$oid",
  "$symbol",
  "$numberInt",
  "$numberLong",
  "$numberDouble",
  "$numberDecimal",
  "$binary",
  "$uuid",
  "$code",
  "$timestamp",
  "$regularExpression",
  "$date",
  "$minKey",
  "$maxKey",
  "$undefined",
  "$dbPointer",
]);
const WRAPPER_PAIRS = new Set(["$binary $type", "$type $binary", "$code $scope", "$scope $code"]);

/** The helpers `literal::helper` knows. */
const HELPERS = new Set([
  "ObjectId",
  "ObjectID",
  "ISODate",
  "Date",
  "NumberLong",
  "Long",
  "NumberInt",
  "Int32",
  "Double",
  "Number",
  "NumberDecimal",
  "Decimal128",
  "UUID",
  "BinData",
  "HexData",
  "MD5",
  "Timestamp",
  "MinKey",
  "MaxKey",
  "Code",
  "RegExp",
  "DBRef",
]);

// The BSON value a literal stands for, as far as `literal.rs` checks it: a helper or a wrapper
// with a wrong argument is an error there, so it must be one here too, or a text the backend
// refuses would classify as a read. Shapes not checked as closely as the backend's (a date
// format chrono reads but the pattern below does not…) are refused here: a write is the safe
// side.

type Bt =
  | { t: "int" | "double"; n: number }
  | { t: "str"; s: string }
  | { t: "doc"; entries: [string, Lit][] }
  | { t: "oid" | "date" | "regex" | "other" };

/** What helpers and wrappers hold, kept out of `Lit` (the mock and the tests compare it). */
const TYPED = new WeakMap<Lit, Bt>();

function typed(name: string, inner: Lit[], bt: Bt): Lit {
  const l: Lit = { k: "other", name, inner };
  TYPED.set(l, bt);
  return l;
}

function bsonOf(l: Lit): Bt {
  switch (l.k) {
    case "doc":
      return { t: "doc", entries: l.entries };
    case "str":
      return { t: "str", s: l.v };
    case "num":
      return { t: l.int ? "int" : "double", n: l.v };
    case "other":
      return TYPED.get(l) ?? { t: "other" };
    default:
      return { t: "other" };
  }
}

/** Rust's `str::trim` (Unicode White_Space; JavaScript's `trim` also takes U+FEFF). */
const rustTrim = (s: string): string => s.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, "");

/** Text Rust's `str::parse::<f64>` reads. */
const F64 = /^[+-]?(?:inf|infinity|nan|(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)$/i;

/** The double such a text reads as. */
const f64Of = (s: string): number => Number(s.replace(/^([+-]?)inf(?:inity)?$/i, "$1Infinity"));

/** Text Rust's `str::parse` reads as an integer of `bits` bits. */
function rustInt(s: string, bits: 32 | 64): number | null {
  if (!/^[+-]?\d+$/.test(s)) return null;
  const n = BigInt(s);
  const max = 2n ** BigInt(bits - 1);
  return n >= -max && n < max ? Number(n) : null;
}

/** `literal::int_of`, short of the largest longs (refused here, read there). */
const intOfBt = (b: Bt): number | null =>
  (b.t === "int" || b.t === "double") && Number.isInteger(b.n) && Math.abs(b.n) < 9.2e18
    ? b.n
    : null;

/** `literal::u32_of`. */
const u32OfBt = (b: Bt | undefined): number | null =>
  b && (b.t === "int" || b.t === "double") && Number.isInteger(b.n) && b.n >= 0 && b.n <= 0xffffffff
    ? b.n
    : null;

const getEntry = (entries: [string, Lit][], key: string): Bt | undefined => {
  const e = entries.find(([k]) => k === key);
  return e ? bsonOf(e[1]) : undefined;
};

/** `values::parse_decimal`: a double's text that bson 2.15's `Decimal128::from_str` takes. */
function decimalOk(s: string): boolean {
  const t = rustTrim(s);
  if (!F64.test(t)) return false;
  const body = t.replace(/^[+-]/, "").toLowerCase();
  if (["nan", "infinity", "inf"].includes(body)) return true;
  const at = body.indexOf("e");
  let digits = at < 0 ? body : body.slice(0, at);
  let exp = at < 0 ? 0 : (rustInt(body.slice(at + 1), 32) ?? NaN);
  const I16 = (n: number) => n >= -32768 && n <= 32767;
  if (!I16(exp)) return false;
  const dot = digits.indexOf(".");
  if (dot >= 0) {
    if (digits.length - dot - 1 > 32767) return false;
    exp -= digits.length - dot - 1;
    if (!I16(exp)) return false;
    digits = digits.slice(0, dot) + digits.slice(dot + 1);
  }
  digits = digits.replace(/^0+/, "") || "0";
  const round = (precision: number): boolean => {
    if (/[^0]/.test(digits.slice(precision))) return false;
    digits = digits.slice(0, precision);
    return true;
  };
  if (digits.length > 34) {
    exp += digits.length - 34;
    if (!round(34) || !I16(exp)) return false;
  }
  if (exp < -6176) {
    if (digits !== "0") {
      const precision = digits.length - (-6176 - exp);
      if (precision < 0 || !round(precision)) return false;
    }
    exp = -6176;
  }
  if (exp > 6111 && digits !== "0" && digits.length + exp - 6111 > 34) return false;
  return digits !== "";
}

/** `values::parse_date_millis`, for the ISO 8601 shapes it surely reads. */
function dateOk(s: string): boolean {
  const m =
    /^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2})(?::(\d{2})(?:\.\d{1,9})?)?(Z|[+-]\d{2}:\d{2})?)?$/.exec(
      rustTrim(s),
    );
  if (!m) return false;
  const [, y, mo, d, h, mi, sec, zone] = m;
  const year = Number(y);
  const month = Number(mo);
  const day = Number(d);
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][month - 1];
  if (year < 1 || days === undefined || day < 1 || day > days) return false;
  if (h !== undefined && (Number(h) > 23 || Number(mi) > 59)) return false;
  if (sec !== undefined && Number(sec) > 59) return false;
  if (zone !== undefined && zone !== "Z") {
    // An offset after minutes alone goes through `%z`, which this does not mirror.
    if (sec === undefined || Number(zone.slice(1, 3)) > 23 || Number(zone.slice(4)) > 59)
      return false;
  }
  return true;
}

const oidOk = (s: string): boolean => /^[0-9a-fA-F]{24}$/.test(s);

const uuidOk = (s: string): boolean =>
  /^(?:[0-9a-fA-F]{32}|[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})$/.test(
    rustTrim(s),
  );

/** The base64 crate's STANDARD engine: padded, canonical, no white space. */
function base64Ok(s: string): boolean {
  const m = /^(?:[A-Za-z0-9+/]{4})*(?:([A-Za-z0-9+/]{2})==|([A-Za-z0-9+/]{3})=)?$/.exec(s);
  if (!m) return false;
  const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  if (m[1] !== undefined) return (B64.indexOf(m[1][1]!) & 0x0f) === 0;
  if (m[2] !== undefined) return (B64.indexOf(m[2][2]!) & 0x03) === 0;
  return true;
}

const hexBytesOk = (s: string): boolean => /^(?:[0-9a-fA-F]{2})*$/.test(rustTrim(s));

const flagsOk = (f: string): boolean => /^[imsuxl]*$/.test(f);

/** `literal::binary`: base64 and a one- or two-digit hex subtype. */
const binaryOk = (b64: Bt | undefined, sub: Bt | undefined): boolean =>
  b64?.t === "str" && sub?.t === "str" && /^[0-9a-fA-F]{1,2}$/.test(sub.s) && base64Ok(b64.s);

/** `literal::extended` on a wrapper-shaped document: its value, or null for a plain document. */
function extendedBt(keys: string[], entries: [string, Lit][]): Bt | null {
  const bad = (what: string): never => {
    throw new ParseError(`invalid ${what}`);
  };
  const v = bsonOf(entries[0]![1]);
  const text = (what: string): string => (v.t === "str" ? v.s : bad(what));
  switch (keys.join(" ")) {
    case "$oid":
      return oidOk(text("$oid")) ? { t: "oid" } : bad("$oid");
    case "$symbol":
    case "$code":
      text(keys[0]!);
      return { t: "other" };
    case "$numberInt":
      return rustInt(text("$numberInt"), 32) !== null
        ? { t: "int", n: rustInt(text("$numberInt"), 32)! }
        : bad("$numberInt");
    case "$numberLong":
      return rustInt(text("$numberLong"), 64) !== null
        ? { t: "int", n: rustInt(text("$numberLong"), 64)! }
        : bad("$numberLong");
    case "$numberDouble": {
      const s = text("$numberDouble");
      return ["NaN", "-NaN", "Infinity", "-Infinity"].includes(s) || F64.test(s)
        ? { t: "double", n: f64Of(s) }
        : bad("$numberDouble");
    }
    case "$numberDecimal":
      return decimalOk(text("$numberDecimal")) ? { t: "other" } : bad("$numberDecimal");
    case "$binary":
      return v.t === "doc" &&
        v.entries.length === 2 &&
        binaryOk(getEntry(v.entries, "base64"), getEntry(v.entries, "subType"))
        ? { t: "other" }
        : bad("$binary");
    case "$binary $type":
    case "$type $binary": {
      const b64 = getEntry(entries, "$binary");
      const sub = getEntry(entries, "$type");
      if (b64?.t !== "str" || sub?.t !== "str") return null;
      return binaryOk(b64, sub) ? { t: "other" } : bad("$binary");
    }
    case "$uuid":
      return uuidOk(text("$uuid")) ? { t: "other" } : bad("$uuid");
    case "$code $scope":
    case "$scope $code":
      return getEntry(entries, "$code")?.t === "str" && getEntry(entries, "$scope")?.t === "doc"
        ? { t: "other" }
        : bad("$code with $scope");
    case "$timestamp":
      return v.t === "doc" &&
        v.entries.length === 2 &&
        u32OfBt(getEntry(v.entries, "t")) !== null &&
        u32OfBt(getEntry(v.entries, "i")) !== null
        ? { t: "other" }
        : bad("$timestamp");
    case "$regularExpression": {
      const p =
        v.t === "doc" && v.entries.length === 2 ? getEntry(v.entries, "pattern") : undefined;
      const o = v.t === "doc" ? getEntry(v.entries, "options") : undefined;
      return p?.t === "str" && o?.t === "str" && flagsOk(o.s)
        ? { t: "regex" }
        : bad("$regularExpression");
    }
    case "$date":
      if (v.t === "str") return dateOk(v.s) ? { t: "date" } : bad("$date");
      if (v.t === "int" || (v.t === "double" && intOfBt(v) !== null)) return { t: "date" };
      return bad("$date");
    case "$minKey":
    case "$maxKey":
    case "$undefined":
      return { t: "other" };
    case "$dbPointer":
      return v.t === "doc" &&
        v.entries.length === 2 &&
        getEntry(v.entries, "$ref")?.t === "str" &&
        getEntry(v.entries, "$id")?.t === "oid"
        ? { t: "other" }
        : bad("$dbPointer");
    default:
      return null;
  }
}

/** `literal::helper`: what `name(args…)` stands for, or a ParseError where the backend has one. */
function helperBt(name: string, args: Lit[]): Bt {
  const bad = (why: string): never => {
    throw new ParseError(`${name}() ${why}`);
  };
  const arity = (min: number, max: number) => {
    if (args.length < min || args.length > max) bad("takes another number of arguments");
  };
  const a = args.map(bsonOf);
  const str = (i: number): string => {
    const b = a[i];
    return b?.t === "str" ? b.s : bad("takes a string");
  };
  switch (name) {
    case "ObjectId":
    case "ObjectID":
      arity(0, 1);
      if (a[0] === undefined || a[0].t === "oid") return { t: "oid" };
      return oidOk(str(0)) ? { t: "oid" } : bad("takes a 24-digit hex string");
    case "ISODate":
    case "Date": {
      arity(0, 1);
      const x = a[0];
      if (x === undefined || x.t === "date") return { t: "date" };
      if (x.t === "str") return dateOk(x.s) ? { t: "date" } : bad("takes a date");
      return intOfBt(x) !== null ? { t: "date" } : bad("takes a date or whole milliseconds");
    }
    case "NumberLong":
    case "Long": {
      arity(1, 1);
      const x = a[0]!;
      const n = x.t === "str" ? rustInt(rustTrim(x.s), 64) : intOfBt(x);
      return n !== null ? { t: "int", n } : bad("takes an integer");
    }
    case "NumberInt":
    case "Int32": {
      arity(1, 1);
      const x = a[0]!;
      const n = x.t === "str" ? rustInt(rustTrim(x.s), 32) : intOfBt(x);
      return n !== null && n >= -(2 ** 31) && n < 2 ** 31
        ? { t: "int", n }
        : bad("takes a 32-bit integer");
    }
    case "Double":
    case "Number": {
      arity(1, 1);
      const x = a[0]!;
      if (x.t === "int" || x.t === "double") return { t: "double", n: x.n };
      if (x.t === "str" && F64.test(rustTrim(x.s))) return { t: "double", n: f64Of(rustTrim(x.s)) };
      return bad("takes a number");
    }
    case "NumberDecimal":
    case "Decimal128": {
      arity(1, 1);
      const x = a[0]!;
      // A double's shortest text always fits a Decimal128.
      if (x.t === "int" || (x.t === "double" && Number.isFinite(x.n))) return { t: "other" };
      return x.t === "str" && decimalOk(x.s) ? { t: "other" } : bad("takes a decimal string");
    }
    case "UUID":
      arity(0, 1);
      return a.length === 0 || uuidOk(str(0)) ? { t: "other" } : bad("takes a UUID");
    case "BinData":
    case "HexData": {
      arity(2, 2);
      const sub = intOfBt(a[0]!);
      if (sub === null || sub < 0 || sub > 255) bad("takes a subtype from 0 to 255");
      const ok = name === "BinData" ? base64Ok(str(1)) : hexBytesOk(str(1));
      return ok ? { t: "other" } : bad("takes valid bytes");
    }
    case "MD5":
      arity(1, 1);
      return hexBytesOk(str(0)) ? { t: "other" } : bad("takes hex");
    case "Timestamp": {
      arity(0, 2);
      const [t, i] = a;
      const ok =
        a.length === 0 ||
        (a.length === 1 &&
          t?.t === "doc" &&
          u32OfBt(getEntry(t.entries, "t")) !== null &&
          u32OfBt(getEntry(t.entries, "i")) !== null) ||
        (a.length === 2 && u32OfBt(t) !== null && u32OfBt(i) !== null);
      return ok ? { t: "other" } : bad("takes (seconds, increment) or {t, i}");
    }
    case "MinKey":
    case "MaxKey":
      arity(0, 0);
      return { t: "other" };
    case "Code":
      arity(1, 2);
      str(0);
      return a.length === 1 || a[1]!.t === "doc" ? { t: "other" } : bad("takes a scope document");
    case "RegExp": {
      arity(1, 2);
      const flags = a.length === 2 ? (a[1]!.t === "str" ? a[1]!.s : bad("takes string flags")) : "";
      const p = a[0]!;
      if (p.t === "str") return flagsOk(flags) ? { t: "regex" } : bad("takes known flags");
      return p.t === "regex" && a.length === 1 ? { t: "regex" } : bad("takes a pattern string");
    }
    case "DBRef": {
      arity(2, 3);
      const entries: [string, Lit][] = [
        ["$ref", { k: "str", v: str(0) }],
        ["$id", args[1]!],
      ];
      if (a.length === 3) entries.push(["$db", { k: "str", v: str(2) }]);
      return { t: "doc", entries };
    }
    default:
      return bad("is not a shell helper");
  }
}

/** A cursor over a text: white space and comments, names, punctuation and values. */
export class Lexer {
  pos = 0;
  constructor(readonly src: string) {}

  private rest(): string {
    return this.src.slice(this.pos);
  }

  peekRaw(): string | undefined {
    return charAt(this.src, this.pos);
  }

  private bump(): string | undefined {
    const c = this.peekRaw();
    if (c !== undefined) this.pos += c.length;
    return c;
  }

  skipTrivia(): void {
    for (;;) {
      const c = this.peekRaw();
      if (c !== undefined && (isWhitespace(c) || c === "﻿")) this.pos += c.length;
      else if (this.src.startsWith("//", this.pos)) {
        const rest = this.rest();
        const m = /[\n\r]/.exec(rest);
        this.pos += m ? m.index : rest.length;
      } else if (this.src.startsWith("/*", this.pos)) {
        const end = this.src.indexOf("*/", this.pos + 2);
        if (end < 0) throw new ParseError("unterminated comment");
        this.pos = end + 2;
      } else return;
    }
  }

  peek(): string | undefined {
    this.skipTrivia();
    return this.peekRaw();
  }

  eat(c: string): boolean {
    if (this.peek() === c) {
      this.pos += c.length;
      return true;
    }
    return false;
  }

  expect(c: string): void {
    if (!this.eat(c)) throw new ParseError(`expected \`${c}\``);
  }

  /** Only white space, comments and `;` are left. */
  end(): void {
    while (this.eat(";"));
    if (this.peek() !== undefined) throw new ParseError("expected the end of the statement");
  }

  ident(): string | null {
    this.skipTrivia();
    const first = this.peekRaw();
    if (first === undefined || !isIdentStart(first)) return null;
    let j = this.pos + first.length;
    for (;;) {
      const c = charAt(this.src, j);
      if (c === undefined || !isIdentChar(c)) break;
      j += c.length;
    }
    const name = this.src.slice(this.pos, j);
    this.pos = j;
    return name;
  }

  /** The word after `use` / `show`, up to white space, `;` or `/`. */
  word(): string {
    this.skipTrivia();
    let j = this.pos;
    for (;;) {
      const c = charAt(this.src, j);
      if (c === undefined || isWhitespace(c) || c === ";" || c === "/") break;
      j += c.length;
    }
    const w = this.src.slice(this.pos, j);
    this.pos = j;
    return w;
  }

  string(): string {
    this.skipTrivia();
    const quote = this.bump();
    if (quote !== "'" && quote !== '"' && quote !== "`") throw new ParseError("expected a string");
    let out = "";
    for (;;) {
      const c = this.bump();
      if (c === undefined) throw new ParseError("unterminated string");
      if (c === quote) return out;
      if (c === "\\") out += this.escape();
      else if ((c === "\n" || c === "\r") && quote !== "`")
        throw new ParseError("unterminated string");
      else if (c === "$" && quote === "`" && this.peekRaw() === "{")
        throw new ParseError("template expressions `${…}` are not supported");
      else out += c;
    }
  }

  private escape(): string {
    const c = this.bump();
    if (c === undefined) throw new ParseError("unterminated string");
    switch (c) {
      case "n":
        return "\n";
      case "t":
        return "\t";
      case "r":
        return "\r";
      case "b":
        return "\b";
      case "f":
        return "\f";
      case "v":
        return "\v";
      case "x":
        return String.fromCodePoint(this.hex(2));
      case "u": {
        const cp = this.unicodeEscape();
        if (cp >= 0xd800 && cp < 0xdc00) {
          if (this.src.startsWith("\\u", this.pos)) {
            this.pos += 2;
            const lo = this.unicodeEscape();
            if (lo >= 0xdc00 && lo < 0xe000)
              return String.fromCodePoint(0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00));
          }
          throw new ParseError("unpaired surrogate in a \\u escape");
        }
        if (cp > 0x10ffff || (cp >= 0xdc00 && cp < 0xe000))
          throw new ParseError("invalid \\u escape");
        return String.fromCodePoint(cp);
      }
      case "0":
        return isDigit(this.peekRaw()) ? "0" : "\0";
      case "\r":
        if (this.peekRaw() === "\n") this.pos += 1;
        return "";
      case "\n":
      case " ":
      case " ":
        return "";
      default:
        return c;
    }
  }

  private unicodeEscape(): number {
    if (this.peekRaw() === "{") {
      this.pos += 1;
      const end = this.src.indexOf("}", this.pos);
      const digits = end < 0 ? "" : this.src.slice(this.pos, end);
      if (!/^[0-9a-fA-F]{1,6}$/.test(digits)) throw new ParseError("invalid \\u{…} escape");
      this.pos = end + 1;
      return parseInt(digits, 16);
    }
    return this.hex(4);
  }

  private hex(n: number): number {
    const digits = this.src.slice(this.pos, this.pos + n);
    if (digits.length !== n || !/^[0-9a-fA-F]+$/.test(digits))
      throw new ParseError("invalid escape");
    this.pos += n;
    return parseInt(digits, 16);
  }

  value(depth = 0): Lit {
    this.skipTrivia();
    if (depth > MAX_DEPTH) throw new ParseError("the value is nested too deeply");
    const c = this.peekRaw();
    if (c === "{") return this.object(depth);
    if (c === "[") {
      this.pos += 1;
      return { k: "arr", items: this.list("]", depth) };
    }
    if (c === "'" || c === '"' || c === "`") return { k: "str", v: this.string() };
    if (c === "/") return this.regex();
    if (c !== undefined && (isDigit(c) || c === "." || c === "-" || c === "+"))
      return this.number();
    if (c !== undefined && isIdentStart(c)) return this.wordValue(depth);
    throw new ParseError("expected a value");
  }

  /** Values separated by commas up to `close`; a trailing comma is allowed. */
  list(close: string, depth: number): Lit[] {
    const items: Lit[] = [];
    for (;;) {
      if (this.eat(close)) return items;
      items.push(this.value(depth + 1));
      if (!this.eat(",")) {
        this.expect(close);
        return items;
      }
    }
  }

  args(): Lit[] {
    return this.list(")", 0);
  }

  private object(depth: number): Lit {
    this.pos += 1;
    const entries: [string, Lit][] = [];
    for (;;) {
      if (this.eat("}")) break;
      const key = this.key();
      this.expect(":");
      const v = this.value(depth + 1);
      // A key given twice keeps its place and takes the later value (`Document::insert`).
      const at = entries.findIndex(([k]) => k === key);
      if (at >= 0) entries[at] = [key, v];
      else entries.push([key, v]);
      if (!this.eat(",")) {
        this.expect("}");
        break;
      }
    }
    const keys = entries.map(([k]) => k);
    const wrapper =
      (keys.length === 1 && WRAPPERS.has(keys[0]!)) ||
      (keys.length === 2 && WRAPPER_PAIRS.has(keys.join(" ")));
    const bt = wrapper ? extendedBt(keys, entries) : null;
    return bt ? typed(keys[0]!, [{ k: "doc", entries }], bt) : { k: "doc", entries };
  }

  private key(): string {
    const c = this.peek();
    if (c === "'" || c === '"' || c === "`") return this.string();
    let j = this.pos;
    const take = (ok: (c: string) => boolean) => {
      for (;;) {
        const d = charAt(this.src, j);
        if (d === undefined || !ok(d)) break;
        j += d.length;
      }
    };
    if (c !== undefined && isDigit(c)) take((d) => isAsciiAlnum(d) || d === ".");
    else if (c !== undefined && isIdentStart(c)) take((d) => isIdentChar(d) || d === ".");
    else throw new ParseError("expected a field name");
    const key = this.src.slice(this.pos, j);
    this.pos = j;
    return key;
  }

  private regex(): Lit {
    this.pos += 1;
    const from = this.pos;
    let cls = false;
    for (;;) {
      const c = this.bump();
      if (c === undefined || c === "\n" || c === "\r")
        throw new ParseError("unterminated regular expression");
      if (c === "\\") {
        const d = this.bump();
        if (d === undefined || d === "\n" || d === "\r")
          throw new ParseError("unterminated regular expression");
      } else if (c === "[") cls = true;
      else if (c === "]") cls = false;
      else if (c === "/" && !cls) break;
    }
    const pattern = this.src.slice(from, this.pos - 1);
    let j = this.pos;
    for (;;) {
      const d = charAt(this.src, j);
      if (d === undefined || !isIdentChar(d)) break;
      if (!"imsuxl".includes(d)) throw new ParseError(`unsupported regular expression flag`);
      j += d.length;
    }
    const flags = this.src.slice(this.pos, j);
    this.pos = j;
    return typed(
      "regex",
      [
        { k: "str", v: pattern },
        { k: "str", v: flags },
      ],
      { t: "regex" },
    );
  }

  private number(): Lit {
    const rest = this.rest();
    const negative = rest.startsWith("-");
    const signLen = rest.startsWith("-") || rest.startsWith("+") ? 1 : 0;
    const body = rest.slice(signLen);
    for (const [word, f] of [
      ["Infinity", Infinity],
      ["NaN", NaN],
    ] as const) {
      const after = charAt(body, word.length);
      if (body.startsWith(word) && (after === undefined || !isIdentChar(after))) {
        this.pos += signLen + word.length;
        return { k: "num", v: negative ? -f : f, int: false };
      }
    }
    let end = /^[0-9A-Za-z._]*/.exec(body)![0].length;
    while (
      /[eE]$/.test(body.slice(0, end)) &&
      /^[+-]/.test(body.slice(end)) &&
      !/^0[xX]/.test(body.slice(0, end))
    )
      end += 1 + /^[0-9A-Za-z]*/.exec(body.slice(end + 1))![0].length;
    const token = body.slice(0, end);
    this.pos += signLen + end;
    const bad = () => new ParseError(`invalid number \`${negative ? "-" : ""}${token}\``);
    if (token === "" || token.includes("_")) throw bad();
    const sign = negative ? -1 : 1;
    // A hex or `n` integer beyond 64 bits is an error (`literal::number`), not a double.
    const int64 = (digits: string): Lit => {
      const n = BigInt(digits);
      if (n > (negative ? 2n ** 63n : 2n ** 63n - 1n)) throw bad();
      return { k: "num", v: sign * Number(n), int: true };
    };
    const hex = /^0[xX]([0-9a-fA-F]+)(n?)$/.exec(token);
    if (/^0[xX]/.test(token)) {
      if (!hex) throw bad();
      return int64(`0x${hex[1]!}`);
    }
    if (token.endsWith("n")) {
      if (!/^\d+n$/.test(token)) throw bad();
      return int64(token.slice(0, -1));
    }
    if (!/[.eE]/.test(token)) {
      if (!/^\d+$/.test(token)) throw bad();
      return { k: "num", v: sign * Number(token), int: Math.abs(Number(token)) < 2 ** 63 };
    }
    const f = /^[0-9.eE+-]+$/.test(token) && /\d/.test(token) ? Number(token) : NaN;
    if (Number.isNaN(f)) throw bad();
    return { k: "num", v: sign * f, int: false };
  }

  private wordValue(depth: number): Lit {
    let name = this.ident();
    if (name === null) throw new ParseError("expected a value");
    let isNew = false;
    if (name === "new") {
      isNew = true;
      name = this.ident();
      if (name === null) throw new ParseError("expected a constructor name after `new`");
    }
    if (this.eat("(")) {
      const args = this.list(")", depth);
      if (!HELPERS.has(name)) throw new ParseError(`${name}() is not a shell helper`);
      return typed(name, args, helperBt(name, args));
    }
    if (isNew) throw new ParseError(`expected \`(\` after \`new ${name}\``);
    switch (name) {
      case "true":
        return { k: "bool", v: true };
      case "false":
        return { k: "bool", v: false };
      case "null":
      case "undefined":
        return { k: "null" };
      case "NaN":
        return { k: "num", v: NaN, int: false };
      case "Infinity":
        return { k: "num", v: Infinity, int: false };
      case "MinKey":
      case "MaxKey":
        return typed(name, [], { t: "other" });
      default:
        throw new ParseError(`unknown name \`${name}\``);
    }
  }
}

/** A whole number (`literal::int_of`). */
const intOf = (v: Lit | undefined): number | null =>
  v?.k === "num" && Number.isInteger(v.v) && Math.abs(v.v) < 9.2e18 ? v.v : null;

/** Whether any key inside `v` is one of `keys` (inside helpers and wrappers too). */
function hasKey(v: Lit, keys: readonly string[], depth = 0): boolean {
  if (depth > 200) return true;
  switch (v.k) {
    case "doc":
      return v.entries.some(([k, x]) => keys.includes(k) || hasKey(x, keys, depth + 1));
    case "arr":
      return v.items.some((x) => hasKey(x, keys, depth + 1));
    case "other":
      return v.inner.some((x) => hasKey(x, keys, depth + 1));
    default:
      return false;
  }
}

// ---------------------------------------------------------------------------------------------
// Statements (`shell::parse`), as the commands they run
// ---------------------------------------------------------------------------------------------

/** The command a statement runs: its name, what it carries, and the command it explains. */
interface Cmd {
  name: string;
  /** The values in it (an aggregate is a write when one holds `$out` or `$merge`). */
  scan: Lit[];
  inner?: Cmd | null;
  /** Its reply is a cursor (`Output::Cursor`), which `.toArray()` / `.pretty()` may follow. */
  cursor?: boolean;
}

/**
 * A statement: one command, `use`, or a write (a method that writes, `bulkWrite`, one the console
 * refuses or does not know): nothing after it can make it a read, so it is not read further.
 */
type Stmt = { k: "run"; cmd: Cmd } | { k: "write" } | { k: "use" };

const cmd = (name: string, scan: Lit[] = [], cursor = false): Stmt => ({
  k: "run",
  cmd: { name, scan, cursor },
});

/** A command document (`db.runCommand({…})`) as its name (the first key) and contents. */
function cmdOfDoc(d: Extract<Lit, { k: "doc" }>): Cmd {
  const [first] = d.entries;
  if (!first) return { name: "", scan: [d] };
  const [name, body] = first;
  return { name, scan: [d], inner: name === "explain" && body.k === "doc" ? cmdOfDoc(body) : null };
}

const FIND_OPTIONS = [
  "projection",
  "sort",
  "skip",
  "limit",
  "batchSize",
  "hint",
  "collation",
  "maxTimeMS",
  "comment",
  "min",
  "max",
  "allowDiskUse",
  "returnKey",
  "showRecordId",
  "noCursorTimeout",
  "let",
  "readConcern",
  "singleBatch",
];
const CURSOR_METHODS = new Set([
  "sort",
  "project",
  "projection",
  "collation",
  "min",
  "max",
  "skip",
  "batchSize",
  "maxTimeMS",
  "limit",
  "comment",
  "hint",
  "allowDiskUse",
  "noCursorTimeout",
  "returnKey",
  "showRecordId",
  "readConcern",
  "toArray",
  "pretty",
  "count",
  "size",
  "explain",
]);
const AGGREGATE_OPTIONS = [
  "allowDiskUse",
  "maxTimeMS",
  "collation",
  "hint",
  "comment",
  "let",
  "batchSize",
  "explain",
  "bypassDocumentValidation",
  "readConcern",
  "writeConcern",
  "cursor",
];
const VERBOSITIES = ["queryPlanner", "executionStats", "allPlansExecution"];

/** The arguments of one call. */
class Args {
  constructor(
    readonly method: string,
    readonly items: Lit[],
  ) {}
  err(what: string): never {
    throw new ParseError(`${this.method}(): ${what}`);
  }
  atMost(max: number): void {
    if (this.items.length > max) this.err(`takes at most ${max} arguments`);
  }
  atLeast(min: number): void {
    if (this.items.length < min) this.err(`takes ${min} arguments`);
  }
  /** The argument, unless it is absent, null or undefined. */
  get(i: number): Lit | undefined {
    const v = this.items[i];
    return v?.k === "null" ? undefined : v;
  }
  doc(i: number): Extract<Lit, { k: "doc" }> {
    const v = this.get(i);
    return v?.k === "doc" ? v : this.err("takes a document");
  }
  optDoc(i: number): Extract<Lit, { k: "doc" }> | null {
    const v = this.get(i);
    if (v === undefined) return null;
    return v.k === "doc" ? v : this.err("takes a document");
  }
  /** An options document, with only the `allowed` keys when they are given. */
  options(i: number, allowed?: readonly string[]): Extract<Lit, { k: "doc" }> {
    const o = this.optDoc(i) ?? { k: "doc", entries: [] };
    if (allowed) known(this, o, allowed);
    return o;
  }
  string(i: number): string {
    const v = this.get(i);
    return v?.k === "str" ? v.v : this.err("takes a string");
  }
  int(i: number): number {
    return intOf(this.get(i)) ?? this.err("takes a whole number");
  }
  optBool(i: number): boolean | null {
    const v = this.get(i);
    if (v === undefined) return null;
    return v.k === "bool" ? v.v : this.err("takes true or false");
  }
  array(i: number): Lit[] {
    const v = this.get(i);
    return v?.k === "arr" ? v.items : this.err("takes an array");
  }
  /** `explain()`'s verbosity. */
  verbosity(i: number): void {
    const v = this.get(i);
    if (v === undefined || v.k === "bool") return;
    if (v.k === "str" && VERBOSITIES.includes(v.v)) return;
    this.err("takes a verbosity");
  }
}

/** Refuses an option the method does not know (`known`). */
function known(a: Args, o: Extract<Lit, { k: "doc" }>, allowed: readonly string[]): void {
  const unknown = o.entries.find(([k]) => !allowed.includes(k));
  if (unknown) a.err(`unknown option \`${unknown[0]}\``);
}

/** A database name the server accepts (`check_db_name`). */
function checkDbName(name: string): void {
  const bytes = new TextEncoder().encode(name).length;
  const bad =
    name === "" ||
    bytes > 64 ||
    [...name].some((c) => isWhitespace(c) || '/\\."$*<>:|?\0'.includes(c));
  if (bad) throw new ParseError(`invalid database name “${name}”`);
}

/** The parser of one statement. Methods that write end it: its kind is known. */
class Parser {
  readonly lx: Lexer;
  constructor(text: string) {
    this.lx = new Lexer(text);
  }

  statement(): Stmt {
    const word = this.lx.ident();
    switch (word) {
      case "use":
        checkDbName(this.lx.word());
        return { k: "use" };
      case "show": {
        const what = this.lx.word();
        const name = {
          dbs: "listDatabases",
          databases: "listDatabases",
          collections: "listCollections",
          tables: "listCollections",
          users: "usersInfo",
          roles: "rolesInfo",
        }[what];
        if (!name) throw new ParseError("show takes dbs, collections, users or roles");
        return cmd(name);
      }
      case "db":
        return this.dbExpr();
      case "rs":
        return this.rs();
      default:
        throw new ParseError("the console runs db.…, use and show");
    }
  }

  private name(): string {
    const n = this.lx.ident();
    if (n === null) throw new ParseError("expected a name");
    return n;
  }

  private dot(): void {
    if (!this.lx.eat(".")) throw new ParseError("expected `.`");
  }

  private call(method: string): Args {
    this.lx.expect("(");
    return new Args(method, this.lx.args());
  }

  /** `.pretty()` and `.toArray()` after a call. */
  private tail(): void {
    while (this.lx.eat(".")) {
      const m = this.name();
      if (m !== "pretty" && m !== "toArray") throw new ParseError(`unexpected call \`.${m}()\``);
      this.call(m).atMost(0);
    }
  }

  /** `.pretty()` only. */
  private tailPretty(): void {
    if (this.lx.eat(".")) {
      const m = this.name();
      if (m !== "pretty") throw new ParseError(`unexpected call \`.${m}()\``);
      this.call(m).atMost(0);
    }
  }

  private dbExpr(): Stmt {
    for (;;) {
      if (this.lx.eat("[")) {
        const name = this.lx.string();
        this.lx.expect("]");
        return this.collection(name);
      }
      this.dot();
      const name = this.name();
      if (this.lx.peek() === "(") {
        const a = this.call(name);
        if (name === "getSiblingDB") {
          a.atLeast(1);
          a.atMost(1);
          checkDbName(a.string(0));
          continue;
        }
        if (name === "getCollection") {
          a.atLeast(1);
          a.atMost(1);
          return this.collection(a.string(0));
        }
        const st = this.dbMethod(a);
        this.tail();
        return st;
      }
      // A collection: its name runs up to the method (`db.system.profile.find()`).
      for (;;) {
        if (this.lx.peek() !== ".") throw new ParseError("expected a method call");
        this.dot();
        const part = this.name();
        if (this.lx.peek() === "(") return this.collectionMethod(part);
      }
    }
  }

  private collection(name: string): Stmt {
    if (name === "") throw new ParseError("expected a collection name");
    this.dot();
    return this.collectionMethod(this.name());
  }

  private rs(): Stmt {
    this.dot();
    const m = this.name();
    this.call(m).atMost(0);
    const name = {
      status: "replSetGetStatus",
      conf: "replSetGetConfig",
      config: "replSetGetConfig",
      hello: "hello",
      isMaster: "hello",
    }[m];
    if (!name) throw new ParseError(`rs.${m}() is not supported in the console`);
    this.tail();
    return cmd(name);
  }

  private dbMethod(a: Args): Stmt {
    switch (a.method) {
      case "runCommand":
      case "adminCommand": {
        a.atLeast(1);
        a.atMost(2);
        const c = a.get(0);
        if (c?.k === "doc" && c.entries.length > 0) return { k: "run", cmd: cmdOfDoc(c) };
        if (c?.k === "str" && c.v !== "") return cmd(c.v);
        return a.err("takes a command document");
      }
      case "getCollectionNames":
        a.atMost(0);
        return cmd("listCollections");
      case "getCollectionInfos":
        a.atMost(2);
        a.optDoc(0);
        a.options(1, ["nameOnly", "authorizedCollections"]);
        return cmd("listCollections");
      case "stats": {
        a.atMost(1);
        const o = a.get(0);
        if (o?.k === "doc") a.options(0, ["scale", "freeStorage"]);
        else if (o !== undefined && intOf(o) === null) a.err("takes a scale");
        return cmd("dbStats");
      }
      case "serverStatus":
        a.atMost(1);
        a.options(0);
        return cmd("serverStatus");
      case "hostInfo":
        a.atMost(0);
        return cmd("hostInfo");
      case "version":
        a.atMost(0);
        return cmd("buildInfo");
      case "hello":
      case "isMaster":
        a.atMost(0);
        return cmd("hello");
      case "listCommands":
        a.atMost(0);
        return cmd("listCommands");
      case "currentOp": {
        a.atMost(1);
        const o = a.get(0);
        if (o !== undefined && o.k !== "bool" && o.k !== "doc")
          a.err("takes true or a filter document");
        return cmd("currentOp");
      }
      case "getUsers":
        a.atMost(1);
        a.options(0);
        return cmd("usersInfo");
      case "getRoles":
        a.atMost(1);
        a.options(0);
        return cmd("rolesInfo");
      case "aggregate": {
        a.atLeast(1);
        a.atMost(2);
        const pipeline = a.array(0);
        const options = a.options(1);
        return cmd("aggregate", [...pipeline, options], !explainsItself(options));
      }
      default:
        // createCollection, createView, dropDatabase, killOp, and anything unknown.
        return { k: "write" };
    }
  }

  private collectionMethod(method: string): Stmt {
    if (method === "explain") return this.explain();
    const a = this.call(method);
    const st = this.collectionCall(a);
    if (st.k === "run" && st.cmd.cursor) this.tail();
    else this.tailPretty();
    return st;
  }

  /** `db.c.explain(verbosity).<method>(…)`. */
  private explain(): Stmt {
    const a = this.call("explain");
    a.atMost(1);
    a.verbosity(0);
    this.dot();
    const method = this.name();
    if (method === "explain") throw new ParseError("explain() of explain()");
    const inner = this.collectionMethod(method);
    if (inner.k !== "run") throw new ParseError(`${method}() cannot be explained`);
    return { k: "run", cmd: { name: "explain", scan: [], inner: inner.cmd } };
  }

  private collectionCall(a: Args): Stmt {
    switch (a.method) {
      case "find": {
        a.atMost(3);
        a.optDoc(0);
        a.options(2, FIND_OPTIONS);
        a.optDoc(1);
        return this.findChain();
      }
      case "findOne":
        a.atMost(3);
        a.optDoc(0);
        a.options(2, FIND_OPTIONS);
        a.optDoc(1);
        return cmd("find");
      case "countDocuments": {
        a.atMost(2);
        const filter = a.optDoc(0);
        const options = a.options(1, [
          "skip",
          "limit",
          "hint",
          "maxTimeMS",
          "collation",
          "comment",
        ]);
        return cmd("aggregate", [...(filter ? [filter] : []), options]);
      }
      case "estimatedDocumentCount":
        a.atMost(1);
        a.options(0, ["maxTimeMS", "comment"]);
        return cmd("count");
      case "count":
        a.atMost(2);
        a.optDoc(0);
        a.options(1, ["skip", "limit", "hint", "maxTimeMS", "collation", "comment", "readConcern"]);
        return cmd("count");
      case "distinct":
        a.atLeast(1);
        a.atMost(3);
        a.string(0);
        a.optDoc(1);
        a.options(2, ["collation", "maxTimeMS", "comment", "hint"]);
        return cmd("distinct");
      case "aggregate": {
        // `aggregate([stages], {options})`, or the old `aggregate(stage, stage, …)`.
        const first = a.items[0];
        let pipeline: Lit[] = [];
        let options: Extract<Lit, { k: "doc" }> = { k: "doc", entries: [] };
        if (first?.k === "arr") {
          a.atMost(2);
          pipeline = first.items;
          options = a.options(1);
        } else if (first !== undefined) {
          if (!a.items.every((s) => s.k === "doc")) a.err("takes a pipeline array");
          pipeline = a.items;
        }
        known(a, options, AGGREGATE_OPTIONS);
        const st = cmd("aggregate", [...pipeline, options], !explainsItself(options));
        return this.aggregateChain(st);
      }
      case "getIndexes":
      case "getIndexSpecs":
        a.atMost(0);
        return cmd("listIndexes", [], true);
      case "stats":
        a.atMost(1);
        return cmd("aggregate");
      default:
        // Every other method writes, or is refused (watch, mapReduce…), or is unknown.
        return { k: "write" };
    }
  }

  /** The cursor chain of `find`: modifiers, then the end (`count()`, `explain()`…). */
  private findChain(): Stmt {
    while (this.lx.eat(".")) {
      const m = this.name();
      if (!CURSOR_METHODS.has(m)) throw new ParseError(`${m}(): is not a cursor method`);
      const a = this.call(m);
      switch (m) {
        case "sort":
        case "project":
        case "projection":
        case "collation":
        case "min":
        case "max":
          a.atLeast(1);
          a.atMost(1);
          a.doc(0);
          break;
        case "skip":
        case "batchSize":
        case "maxTimeMS":
          a.atLeast(1);
          a.atMost(1);
          if (a.int(0) < 0) a.err("takes a number of 0 or more");
          break;
        case "limit":
          a.atLeast(1);
          a.atMost(1);
          a.int(0);
          break;
        case "comment":
        case "hint": {
          a.atLeast(1);
          a.atMost(1);
          const v = a.get(0);
          if (v?.k !== "str" && v?.k !== "doc") a.err("takes a string or a document");
          break;
        }
        case "allowDiskUse":
        case "noCursorTimeout":
        case "returnKey":
        case "showRecordId":
          a.atMost(1);
          a.optBool(0);
          break;
        case "readConcern":
          a.atLeast(1);
          a.atMost(1);
          a.string(0);
          break;
        case "toArray":
        case "pretty":
          a.atMost(0);
          break;
        case "count":
        case "size":
          a.atMost(1);
          if (m === "count") a.optBool(0);
          this.tailPretty();
          return cmd("count");
        case "explain":
          a.atMost(1);
          a.verbosity(0);
          this.tailPretty();
          return {
            k: "run",
            cmd: { name: "explain", scan: [], inner: { name: "find", scan: [] } },
          };
      }
    }
    return cmd("find", [], true);
  }

  /** `aggregate(…).explain()`. */
  private aggregateChain(st: Stmt): Stmt {
    const save = this.lx.pos;
    if (!this.lx.eat(".")) return st;
    const m = this.name();
    if (m !== "explain" || st.k !== "run") {
      this.lx.pos = save;
      return st;
    }
    const a = this.call(m);
    a.atMost(1);
    a.verbosity(0);
    return { k: "run", cmd: { name: "explain", scan: [], inner: st.cmd } };
  }
}

/** An aggregate whose options ask for its plan (`explain: true`) answers no cursor. */
const explainsItself = (options: Extract<Lit, { k: "doc" }>): boolean =>
  options.entries.some(([k, v]) => k === "explain" && v.k === "bool" && v.v);

// ---------------------------------------------------------------------------------------------
// Read or write (`classify.rs`)
// ---------------------------------------------------------------------------------------------

/** Commands that never change data or server state (aggregate and explain are checked apart). */
const READS = new Set([
  "find",
  "count",
  "distinct",
  "listCollections",
  "listIndexes",
  "listDatabases",
  "collStats",
  "dbStats",
  "hello",
  "isMaster",
  "ismaster",
  "buildInfo",
  "buildinfo",
  "connectionStatus",
  "ping",
  "serverStatus",
  "hostInfo",
  "usersInfo",
  "rolesInfo",
  "listCommands",
  "currentOp",
  "replSetGetStatus",
  "replSetGetConfig",
]);
const EXPLAINABLE_READS = ["find", "aggregate", "count", "distinct"];
const WRITE_STAGES = ["$out", "$merge"];

function commandReads(c: Cmd): boolean {
  if (c.name === "aggregate") return !c.scan.some((v) => hasKey(v, WRITE_STAGES));
  if (c.name === "explain")
    return !!c.inner && EXPLAINABLE_READS.includes(c.inner.name) && commandReads(c.inner);
  return READS.has(c.name);
}

/** One statement as the splitter gives it. */
export function classifyStatement(text: string): StatementClass {
  try {
    const p = new Parser(text);
    const st = p.statement();
    if (st.k === "write") return "write";
    p.lx.end();
    return st.k === "use" || commandReads(st.cmd) ? "read" : "write";
  } catch {
    return "write";
  }
}

/** A whole console text: a read only when it has statements and every one reads. */
export function classifyShell(text: string): StatementClass {
  const pieces = splitShell(text);
  if (pieces.length === 0) return "write";
  return pieces.every((p) => classifyStatement(p.sql) === "read") ? "read" : "write";
}

/** One value of the console's syntax (a filter, a document), as the mock reads it. */
export function parseLiteral(text: string): Lit {
  const lx = new Lexer(text);
  const v = lx.value();
  if (lx.peek() !== undefined) throw new ParseError("expected the end of the value");
  return v;
}
