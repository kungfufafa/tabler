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
