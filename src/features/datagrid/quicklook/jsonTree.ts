/**
 * A JSON document as a tree for Quick Look's Tree view. Parsed by hand so the tree shows the text
 * exactly as stored: numbers keep their literal (`9007199254740993`, `1.10`), duplicate keys stay
 * and key order is kept — `JSON.parse` would round, drop and merge them.
 */
export type JsonNode =
  | { type: "object"; entries: Array<{ key: string; value: JsonNode }> }
  | { type: "array"; items: JsonNode[] }
  | { type: "string"; value: string }
  /** `raw` is the literal as written. */
  | { type: "number"; raw: string }
  | { type: "boolean"; value: boolean }
  | { type: "null" };

/** Parses JSON text into a tree; throws a SyntaxError (like `JSON.parse`) on invalid input. */
export function parseJsonTree(text: string): JsonNode {
  let i = 0;
  const fail = (what: string): never => {
    throw new SyntaxError(`${what} at position ${i}`);
  };
  const ws = () => {
    while (i < text.length && " \t\n\r".includes(text[i]!)) i++;
  };
  const string = (): string => {
    if (text[i] !== '"') fail("Expected a string");
    let j = i + 1;
    while (j < text.length && text[j] !== '"') {
      if (text[j] === "\\") j++;
      j++;
    }
    if (j >= text.length) fail("Unterminated string");
    const literal = text.slice(i, j + 1);
    i = j + 1;
    // A lone string literal round-trips exactly through JSON.parse (escapes only).
    return JSON.parse(literal) as string;
  };
  const value = (): JsonNode => {
    ws();
    const ch = text[i];
    if (ch === "{") {
      i++;
      const entries: Array<{ key: string; value: JsonNode }> = [];
      ws();
      if (text[i] === "}") {
        i++;
        return { type: "object", entries };
      }
      for (;;) {
        ws();
        const key = string();
        ws();
        if (text[i] !== ":") fail("Expected ':'");
        i++;
        entries.push({ key, value: value() });
        ws();
        if (text[i] === ",") {
          i++;
          continue;
        }
        if (text[i] === "}") {
          i++;
          return { type: "object", entries };
        }
        fail("Expected ',' or '}'");
      }
    }
    if (ch === "[") {
      i++;
      const items: JsonNode[] = [];
      ws();
      if (text[i] === "]") {
        i++;
        return { type: "array", items };
      }
      for (;;) {
        items.push(value());
        ws();
        if (text[i] === ",") {
          i++;
          continue;
        }
        if (text[i] === "]") {
          i++;
          return { type: "array", items };
        }
        fail("Expected ',' or ']'");
      }
    }
    if (ch === '"') return { type: "string", value: string() };
    const m = /^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/.exec(text.slice(i));
    if (m && m[0] !== "" && m[0] !== "-") {
      i += m[0].length;
      return { type: "number", raw: m[0] };
    }
    for (const [word, node] of [
      ["true", { type: "boolean", value: true }],
      ["false", { type: "boolean", value: false }],
      ["null", { type: "null" }],
    ] as const) {
      if (text.startsWith(word, i)) {
        i += word.length;
        return node;
      }
    }
    return fail("Unexpected token");
  };
  const root = value();
  ws();
  if (i < text.length) fail("Unexpected text after the JSON value");
  return root;
}

/**
 * `text` with the entries of every object ordered by key (the viewer's "Sort keys"), recursively
 * and stably: equal keys (duplicates) keep their relative order, arrays keep theirs. Only the
 * entries move. Every literal stays exactly as written (big numbers, `1.10`, `\u` escapes) and so
 * does the whitespace: the gaps between entries stay where they were, so indented text stays
 * indented. Keys compare by UTF-16 code unit (the order of `jq -S`), not by locale. Throws a
 * SyntaxError (like `parseJsonTree`) on invalid JSON.
 */
export function sortJsonKeys(text: string): string {
  parseJsonTree(text); // validates, so the scan below can trust the text
  const n = text.length;
  let i = 0;
  const ws = () => {
    while (i < n && " \t\n\r".includes(text[i]!)) i++;
  };
  /** Skips the string literal at `i`; returns its decoded value. */
  const string = (): string => {
    const from = i;
    i++;
    while (text[i] !== '"') i += text[i] === "\\" ? 2 : 1;
    i++;
    return JSON.parse(text.slice(from, i)) as string;
  };
  /** The value starting at `i`, as text with its objects' entries sorted. */
  const value = (): string => {
    const start = i;
    const ch = text[i];
    if (ch === "[") {
      i++;
      const lead = i;
      ws();
      let out = `[${text.slice(lead, i)}`;
      if (text[i] === "]") {
        i++;
        return `${out}]`;
      }
      for (;;) {
        out += value();
        const gapFrom = i;
        ws();
        if (text[i] === ",") {
          i++;
          ws();
          out += text.slice(gapFrom, i);
          continue;
        }
        out += `${text.slice(gapFrom, i)}]`;
        i++;
        return out;
      }
    }
    if (ch === "{") {
      i++;
      const lead = i;
      ws();
      const open = text.slice(lead, i);
      if (text[i] === "}") {
        i++;
        return `{${open}}`;
      }
      const members: Array<{ key: string; text: string }> = [];
      /** The `  ,  ` between entry k and k + 1: it stays in its place while the entries move. */
      const gaps: string[] = [];
      let trail: string;
      for (;;) {
        const keyFrom = i;
        const key = string();
        ws();
        i++; // ':'
        ws();
        members.push({ key, text: text.slice(keyFrom, i) + value() });
        const gapFrom = i;
        ws();
        if (text[i] === ",") {
          i++;
          ws();
          gaps.push(text.slice(gapFrom, i));
          continue;
        }
        trail = text.slice(gapFrom, i);
        i++; // '}'
        break;
      }
      members.sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
      let out = `{${open}`;
      members.forEach((m, k) => {
        out += m.text + (gaps[k] ?? "");
      });
      return `${out}${trail}}`;
    }
    if (ch === '"') string();
    else while (i < n && !" \t\n\r,]}".includes(text[i]!)) i++; // a number, true, false or null
    return text.slice(start, i);
  };
  ws();
  const lead = text.slice(0, i);
  const sorted = value();
  return lead + sorted + text.slice(i);
}

/** One-line summary of a node: `{3 keys}`, `[2 items]`, or the scalar as JSON. */
export function nodeSummary(node: JsonNode): string {
  switch (node.type) {
    case "object":
      return `{${node.entries.length} ${node.entries.length === 1 ? "key" : "keys"}}`;
    case "array":
      return `[${node.items.length} ${node.items.length === 1 ? "item" : "items"}]`;
    case "string":
      return JSON.stringify(node.value);
    case "number":
      return node.raw;
    case "boolean":
      return String(node.value);
    case "null":
      return "null";
  }
}
