/**
 * JSON as the console takes it, mirroring the backend's `drivers/elasticsearch/json.rs`
 * (`parse_console`): numbers keep their text, objects their key order, and a body may hold `#`,
 * `//` and `/* *\/` comments between tokens and Kibana's `"""…"""` strings. Malformed text is an
 * error, never a guess: the classifier calls a body it cannot read a write.
 */

export type Json =
  | { k: "null" }
  | { k: "bool"; v: boolean }
  | { k: "num"; v: string }
  | { k: "str"; v: string }
  | { k: "arr"; v: Json[] }
  | { k: "obj"; v: [string, Json][] };

/** Nesting deeper than this is refused, as the backend's `MAX_DEPTH`. */
export const MAX_DEPTH = 512;

export class JsonError extends Error {
  constructor(
    message: string,
    /** UTF-16 offset into the parsed text. */
    readonly pos: number,
  ) {
    super(message);
  }

  /** The message with its 1-based line (`lines` more) and column in `text`. */
  describe(text: string, lines = 0): string {
    const upto = text.slice(0, this.pos);
    const line = upto.split("\n").length + lines;
    const col = [...(upto.split("\n").at(-1) ?? "")].length + 1;
    return `${this.message} (line ${line}, column ${col})`;
  }
}

/** Whether `s` is a JSON number literal (`is_number`). */
export function isNumber(s: string): boolean {
  return /^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?$/.test(s);
}

class Parser {
  i = 0;
  constructor(
    readonly text: string,
    readonly console: boolean,
  ) {}

  err(message: string): JsonError {
    return new JsonError(message, this.i);
  }

  skipSpace(): void {
    const t = this.text;
    for (;;) {
      const c = t[this.i];
      if (c === " " || c === "\t" || c === "\n" || c === "\r") this.i++;
      else if (this.console && (c === "#" || (c === "/" && t[this.i + 1] === "/"))) {
        while (this.i < t.length && t[this.i++] !== "\n");
      } else if (this.console && c === "/" && t[this.i + 1] === "*") {
        const end = t.indexOf("*/", this.i + 2);
        if (end < 0) throw this.err("unterminated /* comment");
        this.i = end + 2;
      } else return;
    }
  }

  value(depth: number): Json {
    if (depth > MAX_DEPTH) throw this.err("JSON nested too deeply");
    this.skipSpace();
    const c = this.text[this.i];
    if (c === undefined) throw this.err("expected a JSON value");
    if (c === "{") return this.object(depth);
    if (c === "[") return this.array(depth);
    if (c === '"') return { k: "str", v: this.string() };
    if (c === "t") return this.word("true", { k: "bool", v: true });
    if (c === "f") return this.word("false", { k: "bool", v: false });
    if (c === "n") return this.word("null", { k: "null" });
    if (c === "-" || (c >= "0" && c <= "9")) return this.number();
    throw this.err("expected a JSON value");
  }

  word(w: string, v: Json): Json {
    if (!this.text.startsWith(w, this.i)) throw this.err("expected a JSON value");
    this.i += w.length;
    return v;
  }

  number(): Json {
    const start = this.i;
    while (/[-+.eE0-9]/.test(this.text[this.i] ?? "")) this.i++;
    const text = this.text.slice(start, this.i);
    if (isNumber(text)) return { k: "num", v: text };
    this.i = start;
    throw this.err("malformed number");
  }

  object(depth: number): Json {
    this.i++;
    const pairs: [string, Json][] = [];
    this.skipSpace();
    if (this.text[this.i] === "}") {
      this.i++;
      return { k: "obj", v: pairs };
    }
    for (;;) {
      this.skipSpace();
      if (this.text[this.i] !== '"') throw this.err("expected a quoted key");
      const key = this.string();
      this.skipSpace();
      if (this.text[this.i] !== ":") throw this.err("expected `:` after the key");
      this.i++;
      pairs.push([key, this.value(depth + 1)]);
      this.skipSpace();
      const c = this.text[this.i];
      if (c === ",") this.i++;
      else if (c === "}") {
        this.i++;
        return { k: "obj", v: pairs };
      } else throw this.err("expected `,` or `}`");
    }
  }

  array(depth: number): Json {
    this.i++;
    const items: Json[] = [];
    this.skipSpace();
    if (this.text[this.i] === "]") {
      this.i++;
      return { k: "arr", v: items };
    }
    for (;;) {
      items.push(this.value(depth + 1));
      this.skipSpace();
      const c = this.text[this.i];
      if (c === ",") this.i++;
      else if (c === "]") {
        this.i++;
        return { k: "arr", v: items };
      } else throw this.err("expected `,` or `]`");
    }
  }

  string(): string {
    const t = this.text;
    const start = this.i;
    if (this.console && t.startsWith('"""', this.i)) {
      const end = t.indexOf('"""', this.i + 3);
      if (end < 0) throw this.err('unterminated """ string');
      const s = t.slice(this.i + 3, end);
      this.i = end + 3;
      return s;
    }
    this.i++;
    let out = "";
    for (;;) {
      let run = this.i;
      while (run < t.length && t[run] !== '"' && t[run] !== "\\") run++;
      out += t.slice(this.i, run);
      this.i = run;
      const c = t[this.i];
      if (c === undefined) {
        this.i = start;
        throw this.err("unterminated string");
      }
      if (c === '"') {
        this.i++;
        return out;
      }
      this.i++;
      const esc = t[this.i];
      this.i++;
      switch (esc) {
        case '"':
          out += '"';
          break;
        case "\\":
          out += "\\";
          break;
        case "/":
          out += "/";
          break;
        case "b":
          out += "\b";
          break;
        case "f":
          out += "\f";
          break;
        case "n":
          out += "\n";
          break;
        case "r":
          out += "\r";
          break;
        case "t":
          out += "\t";
          break;
        case "u": {
          const hi = this.hex4();
          let ch: string | null;
          if (hi >= 0xd800 && hi < 0xdc00 && t.startsWith("\\u", this.i)) {
            const save = this.i;
            this.i += 2;
            const lo = this.hex4();
            if (lo >= 0xdc00 && lo < 0xe000)
              ch = String.fromCodePoint(0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00));
            else {
              this.i = save;
              ch = null;
            }
          } else ch = hi >= 0xd800 && hi < 0xe000 ? null : String.fromCodePoint(hi);
          // A lone surrogate has no character: keep the text readable.
          out += ch ?? "�";
          break;
        }
        default:
          this.i--;
          throw this.err("invalid escape in a string");
      }
    }
  }

  hex4(): number {
    const digits = this.text.slice(this.i, this.i + 4);
    if (!/^[0-9a-fA-F]{4}$/.test(digits)) throw this.err("invalid \\u escape");
    this.i += 4;
    return parseInt(digits, 16);
  }
}

/** Every value of a console body, in order (one per line for `_bulk` and `_msearch`). */
export function parseConsole(text: string): Json[] {
  const p = new Parser(text, true);
  const out: Json[] = [];
  for (;;) {
    p.skipSpace();
    if (p.i >= text.length) return out;
    out.push(p.value(0));
  }
}

/** Strict JSON: one value, surrounded by whitespace only. */
export function parseStrict(text: string): Json {
  const p = new Parser(text, false);
  const v = p.value(0);
  p.skipSpace();
  if (p.i < text.length) throw p.err("unexpected text after the JSON value");
  return v;
}

/** The value of `key` in an object (the first, if repeated). */
export function get(v: Json | undefined, key: string): Json | undefined {
  return v?.k === "obj" ? v.v.find(([k]) => k === key)?.[1] : undefined;
}

export const asStr = (v: Json | undefined): string | null => (v?.k === "str" ? v.v : null);

// `JSON.stringify` escapes as the backend's `write_str` does (`\uXXXX` for other controls).
const quote = (s: string): string => JSON.stringify(s);

/** Compact JSON text (`to_text`), or indented by two spaces (`to_pretty`). */
export function toText(v: Json, indent: number | null = null, level = 0): string {
  const nl = (l: number) => (indent === null ? "" : `\n${" ".repeat(indent * l)}`);
  switch (v.k) {
    case "null":
      return "null";
    case "bool":
      return v.v ? "true" : "false";
    case "num":
      return isNumber(v.v) ? v.v : quote(v.v);
    case "str":
      return quote(v.v);
    case "arr":
      if (v.v.length === 0) return "[]";
      return `[${v.v.map((x) => nl(level + 1) + toText(x, indent, level + 1)).join(",")}${nl(level)}]`;
    case "obj":
      if (v.v.length === 0) return "{}";
      return `{${v.v
        .map(
          ([k, x]) =>
            `${nl(level + 1)}${quote(k)}:${indent === null ? "" : " "}${toText(x, indent, level + 1)}`,
        )
        .join(",")}${nl(level)}}`;
  }
}

/** A plain JavaScript value as `Json` (the mock's documents; numbers as JavaScript writes them). */
export function fromPlain(v: unknown): Json {
  if (v === null || v === undefined) return { k: "null" };
  if (typeof v === "boolean") return { k: "bool", v };
  if (typeof v === "number" || typeof v === "bigint") return { k: "num", v: String(v) };
  if (typeof v === "string") return { k: "str", v };
  if (Array.isArray(v)) return { k: "arr", v: v.map(fromPlain) };
  return { k: "obj", v: Object.entries(v as object).map(([k, x]) => [k, fromPlain(x)]) };
}
