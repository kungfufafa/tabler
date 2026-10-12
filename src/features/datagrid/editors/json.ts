/** Why `text` is not valid JSON (with line and column when the engine reports a position), or null. */
export function jsonError(text: string): string | null {
  if (text.trim() === "") return "Enter a JSON value";
  try {
    JSON.parse(text);
    return null;
  } catch (e) {
    const message = (e as Error).message;
    if (/\bline \d+/i.test(message)) return message;
    const pos = /position (\d+)/i.exec(message);
    if (!pos) return message;
    const offset = Number(pos[1]);
    const before = text.slice(0, offset);
    const line = before.split("\n").length;
    const column = offset - before.lastIndexOf("\n");
    return `${message} (line ${line}, column ${column})`;
  }
}

/**
 * Re-indents (or, with `indent` null, minifies) VALID JSON text by rewriting only the whitespace
 * between tokens — numbers, key order and duplicate keys stay exactly as written (no `JSON.parse`
 * round trip, which would round big integers).
 */
export function reformatJson(text: string, indent: number | null): string {
  const pad = (depth: number) => (indent === null ? "" : `\n${" ".repeat(indent * depth)}`);
  let out = "";
  let depth = 0;
  let i = 0;
  const n = text.length;
  while (i < n) {
    const ch = text[i]!;
    if (ch === '"') {
      let j = i + 1;
      while (j < n && text[j] !== '"') j += text[j] === "\\" ? 2 : 1;
      out += text.slice(i, j + 1);
      i = j + 1;
      continue;
    }
    if (ch === " " || ch === "\t" || ch === "\n" || ch === "\r") {
      i++;
      continue;
    }
    if (ch === "{" || ch === "[") {
      // Keep empty containers compact.
      let k = i + 1;
      while (k < n && /\s/.test(text[k]!)) k++;
      const close = ch === "{" ? "}" : "]";
      if (text[k] === close) {
        out += ch + close;
        i = k + 1;
        continue;
      }
      depth++;
      out += ch + pad(depth);
      i++;
      continue;
    }
    if (ch === "}" || ch === "]") {
      depth = Math.max(0, depth - 1);
      out += pad(depth) + ch;
      i++;
      continue;
    }
    if (ch === ",") {
      out += "," + pad(depth);
      i++;
      continue;
    }
    if (ch === ":") {
      out += indent === null ? ":" : ": ";
      i++;
      continue;
    }
    out += ch;
    i++;
  }
  return out;
}
