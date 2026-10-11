/**
 * The backend reports statement spans and error positions as *character* offsets (Unicode scalar
 * values, Rust `chars`), while CodeMirror and JS strings index UTF-16 code units. They only differ
 * when the text holds characters outside the BMP (emoji, some CJK), but then they must be mapped.
 */

function isHigh(code: number): boolean {
  return code >= 0xd800 && code <= 0xdbff;
}

function isLow(code: number): boolean {
  return code >= 0xdc00 && code <= 0xdfff;
}

/** UTF-16 index of the `charOffset`-th character of `text` (clamped to the text length). */
export function charToUtf16(text: string, charOffset: number): number {
  if (charOffset <= 0) return 0;
  let units = 0;
  let chars = 0;
  while (chars < charOffset && units < text.length) {
    const pair =
      isHigh(text.charCodeAt(units)) &&
      units + 1 < text.length &&
      isLow(text.charCodeAt(units + 1));
    units += pair ? 2 : 1;
    chars++;
  }
  return units;
}

/** Character offset of UTF-16 index `units` in `text` (clamped to the text length). */
export function utf16ToChar(text: string, units: number): number {
  const end = Math.min(units, text.length);
  let chars = 0;
  for (let i = 0; i < end; i++) {
    if (isHigh(text.charCodeAt(i)) && i + 1 < text.length && isLow(text.charCodeAt(i + 1))) i++;
    chars++;
  }
  return chars;
}

/** 1-based line and column (in characters) of UTF-16 index `units`. */
export function lineColumnAt(text: string, units: number): { line: number; column: number } {
  const end = Math.min(Math.max(0, units), text.length);
  let line = 1;
  let lineStart = 0;
  for (let i = 0; i < end; i++) {
    if (text.charCodeAt(i) === 10) {
      line++;
      lineStart = i + 1;
    }
  }
  return { line, column: utf16ToChar(text.slice(lineStart, end), end - lineStart) + 1 };
}
