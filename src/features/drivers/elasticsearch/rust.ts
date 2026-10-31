/**
 * Rust's string rules where JavaScript's differ, so the console mirror reads text exactly as the
 * backend does: `char::is_whitespace` (U+0085 is whitespace, U+FEFF is not), `str::trim`,
 * `split_whitespace`, `char::is_control`, and ASCII-only case folding (`to_ascii_uppercase`
 * leaves `ſ` and `K` alone, where `toUpperCase` makes them `S` and `K`).
 */

/** `char::is_whitespace`: the Unicode White_Space property. */
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

/** `char::is_control`: the Cc category. */
export function isControl(c: string): boolean {
  const n = c.codePointAt(0) ?? -1;
  return n <= 0x1f || (n >= 0x7f && n <= 0x9f);
}

/** `str::trim` (every whitespace character is in the BMP: one UTF-16 unit each). */
export function trim(s: string): string {
  let a = 0;
  let b = s.length;
  while (a < b && isWhitespace(s[a]!)) a++;
  while (b > a && isWhitespace(s[b - 1]!)) b--;
  return s.slice(a, b);
}

export function trimEnd(s: string): string {
  let b = s.length;
  while (b > 0 && isWhitespace(s[b - 1]!)) b--;
  return s.slice(0, b);
}

/** `str::split_whitespace`. */
export function splitWhitespace(s: string): string[] {
  const out: string[] = [];
  let word = "";
  for (const c of s) {
    if (isWhitespace(c)) {
      if (word !== "") out.push(word);
      word = "";
    } else word += c;
  }
  if (word !== "") out.push(word);
  return out;
}

export const asciiUpper = (s: string): string => s.replace(/[a-z]+/g, (m) => m.toUpperCase());
export const asciiLower = (s: string): string => s.replace(/[A-Z]+/g, (m) => m.toLowerCase());
export const eqIgnoreAsciiCase = (a: string, b: string): boolean =>
  a.length === b.length && asciiLower(a) === asciiLower(b);

export const isAsciiAlphabetic = (c: string): boolean => /^[A-Za-z]$/.test(c);
export const isAsciiAlphanumeric = (c: string): boolean => /^[A-Za-z0-9]$/.test(c);
export const isAscii = (c: string): boolean => (c.codePointAt(0) ?? 0) < 0x80;

/** UTF-16 offsets of a text's code points (`out[i]` is where code point `i` starts). */
export function utf16Offsets(text: string): number[] {
  const out: number[] = [];
  let i = 0;
  for (const c of text) {
    out.push(i);
    i += c.length;
  }
  out.push(i);
  return out;
}
