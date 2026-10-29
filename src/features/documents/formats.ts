import type { Value } from "@/lib/ipc";

/**
 * The text formats of `JsonDocumentEditor`: a document (a row of a document store, as its fields'
 * `Value`s) written as JSON and read back. A field a document lacks is `Other{missing}`, never
 * NULL: it is left out of the text, and a field the text leaves out is missing again. A field
 * the user did not touch reads back as the very value it was (whatever the format makes of its
 * type), so saving changes only what was edited. Numbers keep their digits (no `JSON.parse`
 * rounding).
 *
 * - `PLAIN_JSON`: plain JSON (Elasticsearch's `_source`); a string or number keeps the type its
 *   field was loaded with (a timestamp stays a timestamp).
 * - `EXTENDED_JSON`: MongoDB's relaxed Extended JSON v2 (`$oid`, `$date`, `$numberLong`,
 *   `$numberDecimal`, `$uuid`, `$binary`; the rare types carry their own Extended JSON).
 * - `DYNAMODB_JSON` and `DYNAMODB_PLAIN_JSON`: DynamoDB's typed JSON (`{"S": …}`, `{"N": …}`,
 *   maps and lists as `M` / `L`, sets) and the same document as plain JSON, between which the
 *   editor toggles; numbers are never floats there.
 */

/** A document's fields by name. */
export type DocumentFields = Record<string, Value>;

/** The value of a field the document lacks (`Other{missing}`). */
export const MISSING: Value = { t: "other", v: { ty: "missing", text: "" } };

export const isMissing = (v: Value | undefined): boolean =>
  v === undefined || (v.t === "other" && v.v.ty === "missing");

export type ReadResult = { fields: DocumentFields } | { error: string };

/** A way of writing a document as text and reading it back. */
export interface DocumentFormat {
  id: string;
  /** The toggle's label ("DynamoDB JSON", "JSON"). */
  label: string;
  /** The document's text (its fields in order; missing ones left out), indented by two spaces. */
  write(fields: DocumentFields): string;
  /** The fields `text` sets; `original` gives each field's loaded type (and untouched values). */
  read(text: string, original: DocumentFields): ReadResult;
}

// ---- JSON with exact numbers ------------------------------------------------------------------

/** A parsed JSON value; numbers keep their text. */
export type JsonNode =
  | { t: "null" }
  | { t: "bool"; v: boolean }
  | { t: "number"; text: string }
  | { t: "string"; v: string }
  | { t: "array"; items: JsonNode[] }
  | { t: "object"; entries: Array<[string, JsonNode]> };

class JsonError extends Error {}

const LITERALS: ReadonlyArray<readonly [string, JsonNode]> = [
  ["true", { t: "bool", v: true }],
  ["false", { t: "bool", v: false }],
  ["null", { t: "null" }],
];

/** Parses JSON text, keeping numbers' digits; a duplicate key is an error (servers differ). */
export function parseJson(text: string): JsonNode {
  let i = 0;
  const fail = (message: string): never => {
    const before = text.slice(0, i);
    const line = before.split("\n").length;
    const column = i - before.lastIndexOf("\n");
    throw new JsonError(`${message} (line ${line}, column ${column})`);
  };
  const space = () => {
    while (i < text.length && " \t\n\r".includes(text[i]!)) i++;
  };
  const expect = (word: string) => {
    if (text.startsWith(word, i)) i += word.length;
    else fail(`Expected ${word}`);
  };
  const string = (): string => {
    i++; // the opening quote
    let out = "";
    for (;;) {
      if (i >= text.length) fail("Unterminated string");
      const c = text[i]!;
      if (c === '"') {
        i++;
        return out;
      }
      if (c === "\\") {
        const e = text[i + 1];
        const simple: Record<string, string> = {
          '"': '"',
          "\\": "\\",
          "/": "/",
          b: "\b",
          f: "\f",
          n: "\n",
          r: "\r",
          t: "\t",
        };
        if (e !== undefined && e in simple) {
          out += simple[e];
          i += 2;
        } else if (e === "u" && /^[0-9a-fA-F]{4}$/.test(text.slice(i + 2, i + 6))) {
          out += String.fromCharCode(parseInt(text.slice(i + 2, i + 6), 16));
          i += 6;
        } else fail("Invalid escape in a string");
      } else if (c < " ") fail("Control character in a string");
      else {
        out += c;
        i++;
      }
    }
  };
  const value = (): JsonNode => {
    space();
    const c = text[i];
    if (c === "{") {
      i++;
      const entries: Array<[string, JsonNode]> = [];
      const seen = new Set<string>();
      space();
      if (text[i] === "}") {
        i++;
        return { t: "object", entries };
      }
      for (;;) {
        space();
        if (text[i] !== '"') fail("Expected a field name in double quotes");
        const key = string();
        if (seen.has(key)) fail(`Duplicate field "${key}"`);
        seen.add(key);
        space();
        if (text[i] !== ":") fail("Expected :");
        i++;
        entries.push([key, value()]);
        space();
        if (text[i] === ",") i++;
        else if (text[i] === "}") {
          i++;
          return { t: "object", entries };
        } else fail("Expected , or }");
      }
    }
    if (c === "[") {
      i++;
      const items: JsonNode[] = [];
      space();
      if (text[i] === "]") {
        i++;
        return { t: "array", items };
      }
      for (;;) {
        items.push(value());
        space();
        if (text[i] === ",") i++;
        else if (text[i] === "]") {
          i++;
          return { t: "array", items };
        } else fail("Expected , or ]");
      }
    }
    if (c === '"') return { t: "string", v: string() };
    for (const [word, node] of LITERALS) {
      if (c !== word[0]) continue;
      expect(word);
      return node;
    }
    const m = /^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/.exec(text.slice(i));
    if (!m) return fail(c === undefined ? "Unexpected end of the text" : `Unexpected ${c}`);
    i += m[0].length;
    return { t: "number", text: m[0] };
  };
  const root = value();
  space();
  if (i < text.length) fail("Unexpected text after the document");
  return root;
}

/** JSON text of `node`: compact, or indented by `indent` spaces from `depth`. */
export function stringifyJson(node: JsonNode, indent: number | null = null, depth = 0): string {
  const pad = (d: number) => (indent === null ? "" : `\n${" ".repeat(indent * d)}`);
  switch (node.t) {
    case "null":
      return "null";
    case "bool":
      return String(node.v);
    case "number":
      return node.text;
    case "string":
      return JSON.stringify(node.v);
    case "array":
      if (node.items.length === 0) return "[]";
      return `[${node.items.map((n) => pad(depth + 1) + stringifyJson(n, indent, depth + 1)).join(",")}${pad(depth)}]`;
    case "object": {
      if (node.entries.length === 0) return "{}";
      const sep = indent === null ? ":" : ": ";
      const body = node.entries
        .map(
          ([k, n]) =>
            pad(depth + 1) + JSON.stringify(k) + sep + stringifyJson(n, indent, depth + 1),
        )
        .join(",");
      return `{${body}${pad(depth)}}`;
    }
  }
}

const str = (v: string): JsonNode => ({ t: "string", v });
const num = (text: string): JsonNode => ({ t: "number", text });
const obj = (...entries: Array<[string, JsonNode]>): JsonNode => ({ t: "object", entries });

/** The JSON of a `json` cell, or the text as a string when it is not JSON. */
function jsonCell(text: string): JsonNode {
  try {
    return parseJson(text);
  } catch {
    return str(text);
  }
}

/** A JavaScript number as JSON (NaN and the infinities, which JSON lacks, as strings). */
const numberNode = (v: number): JsonNode => (Number.isFinite(v) ? num(String(v)) : str(String(v)));

const INTEGER = /^-?\d+$/;

/** A number's text as the value it is: an int when JavaScript holds it exactly, else a bigInt;
 * a fraction as a float when it round-trips, else a decimal (or always a decimal when `exact`). */
function numberValue(text: string, exact: boolean): Value {
  if (INTEGER.test(text)) {
    const n = Number(text);
    return Number.isSafeInteger(n) ? { t: "int", v: n } : { t: "bigInt", v: text };
  }
  const n = Number(text);
  if (!exact && Number.isFinite(n) && String(n) === text) return { t: "float", v: n };
  return { t: "decimal", v: text };
}

/** Values whose JSON is a string, typed apart from text: kept when a field was loaded as one. */
type StringTyped = Extract<
  Value,
  { t: "date" | "time" | "timestamp" | "timestampTz" | "interval" | "uuid" | "enum" }
>;
const STRING_TAGS: ReadonlyArray<StringTyped["t"]> = [
  "date",
  "time",
  "timestamp",
  "timestampTz",
  "interval",
  "uuid",
  "enum",
];
const isStringTyped = (v: Value | undefined): v is StringTyped =>
  v !== undefined && STRING_TAGS.some((t) => t === v.t);

// ---- formats ----------------------------------------------------------------------------------

/** How a format writes one value and reads one back (with the field's loaded value as a hint). */
interface Codec {
  write(v: Value): JsonNode | null;
  read(node: JsonNode, hint: Value | undefined, field: string): Value;
}

function makeFormat(id: string, label: string, codec: Codec): DocumentFormat {
  return {
    id,
    label,
    write(fields) {
      const entries: Array<[string, JsonNode]> = [];
      for (const [k, v] of Object.entries(fields)) {
        if (isMissing(v)) continue;
        const node = codec.write(v);
        if (node) entries.push([k, node]);
      }
      return stringifyJson(obj(...entries), 2);
    },
    read(text, original) {
      let root: JsonNode;
      try {
        if (text.trim() === "") return { error: "Enter a document" };
        root = parseJson(text);
      } catch (e) {
        return { error: (e as Error).message };
      }
      if (root.t !== "object") return { error: "A document is a JSON object ({…})" };
      const fields: DocumentFields = {};
      try {
        for (const [k, node] of root.entries) {
          const before = original[k];
          // Untouched: the value it was, whatever the format makes of its type.
          const same =
            before !== undefined &&
            !isMissing(before) &&
            (() => {
              const written = codec.write(before);
              return written !== null && stringifyJson(written) === stringifyJson(node);
            })();
          fields[k] = same ? before : codec.read(node, before, k);
        }
      } catch (e) {
        if (e instanceof JsonError) return { error: e.message };
        throw e;
      }
      return { fields };
    },
  };
}

const bytesValue = (base64: string): Value => ({
  t: "bytes",
  v: { base64, len: Math.floor((base64.replace(/=+$/, "").length * 3) / 4), truncated: false },
});

/** Plain JSON; `exactNumbers` never reads a float, `content` writes `json` cells' content. */
function plainCodec(exactNumbers: boolean, content: (node: JsonNode) => JsonNode): Codec {
  const codec: Codec = {
    write(v) {
      switch (v.t) {
        case "null":
          return { t: "null" };
        case "bool":
          return { t: "bool", v: v.v };
        case "int":
        case "float":
          return numberNode(v.v);
        case "bigInt":
        case "decimal":
          return num(v.v);
        case "json":
          return content(jsonCell(v.v));
        case "array":
          return { t: "array", items: v.v.map((e) => codec.write(e) ?? { t: "null" }) };
        case "bytes":
          return str(v.v.base64);
        case "other":
          return isMissing(v) ? null : str(v.v.text);
        case "default":
          return null;
        default:
          return str(v.v);
      }
    },
    read(node, hint, field) {
      switch (node.t) {
        case "null":
          return { t: "null" };
        case "bool":
          return { t: "bool", v: node.v };
        case "number": {
          // A number keeps its field's numeric type: a whole one in a decimal, big integer or
          // float field stays one.
          if (hint?.t === "decimal") return { t: "decimal", v: node.text };
          const v = numberValue(node.text, exactNumbers);
          if (hint?.t === "bigInt" && v.t === "int") return { t: "bigInt", v: node.text };
          if (hint?.t === "float" && v.t === "int") return { t: "float", v: v.v };
          return v;
        }
        case "string":
          if (isStringTyped(hint)) return { ...hint, v: node.v };
          if (hint?.t === "bytes") return bytesValue(node.v);
          if (hint?.t === "other" && !isMissing(hint))
            return { t: "other", v: { ty: hint.v.ty, text: node.v } };
          return { t: "text", v: node.v };
        case "array":
          if (hint?.t === "array")
            return { t: "array", v: node.items.map((n) => codec.read(n, hint.v[0], field)) };
          return { t: "json", v: stringifyJson(node) };
        case "object":
          return { t: "json", v: stringifyJson(node) };
      }
    },
  };
  return codec;
}

const PLAIN = plainCodec(false, (n) => n);

/** Plain JSON (Elasticsearch's `_source`, any document store without types of its own). */
export const PLAIN_JSON: DocumentFormat = makeFormat("json", "JSON", PLAIN);

/** The single `$` key of a type wrapper (`{"$oid": …}`), if `node` is one. */
function wrapper(node: JsonNode): [string, JsonNode] | null {
  if (node.t !== "object" || node.entries.length !== 1) return null;
  const entry = node.entries[0]!;
  return entry[0].startsWith("$") ? entry : null;
}

const extendedCodec: Codec = {
  write(v) {
    switch (v.t) {
      case "timestampTz":
        return obj(["$date", str(v.v)]);
      case "bigInt":
        return obj(["$numberLong", str(v.v)]);
      case "decimal":
        return obj(["$numberDecimal", str(v.v)]);
      case "uuid":
        return obj(["$uuid", str(v.v)]);
      case "bytes":
        return obj(["$binary", obj(["base64", str(v.v.base64)], ["subType", str("00")])]);
      case "other":
        if (isMissing(v)) return null;
        if (v.v.ty === "objectId") return obj(["$oid", str(v.v.text)]);
        // The rare types (regex, timestamp, MinKey…) carry their canonical Extended JSON.
        return jsonCell(v.v.text);
      default:
        return PLAIN.write(v);
    }
  },
  read(node, hint, field) {
    const w = wrapper(node);
    if (w) {
      const [key, inner] = w;
      const text = inner.t === "string" ? inner.v : null;
      if (key === "$oid" && text !== null) return { t: "other", v: { ty: "objectId", text } };
      if (key === "$date" && text !== null) return { t: "timestampTz", v: text };
      if ((key === "$numberLong" || key === "$numberInt") && text !== null && INTEGER.test(text))
        return numberValue(text, true);
      if (key === "$numberDecimal" && text !== null) return { t: "decimal", v: text };
      if (key === "$numberDouble" && text !== null) {
        const n = Number(text);
        return Number.isFinite(n)
          ? { t: "float", v: n }
          : { t: "other", v: { ty: "double", text } };
      }
      if (key === "$uuid" && text !== null) return { t: "uuid", v: text };
      if (key === "$binary" && inner.t === "object") {
        const b64 = inner.entries.find(([k]) => k === "base64")?.[1];
        const sub = inner.entries.find(([k]) => k === "subType")?.[1];
        if (b64?.t === "string" && sub?.t === "string" && /^0{0,2}$/.test(sub.v))
          return bytesValue(b64.v);
      }
      // Another type: its Extended JSON, typed as the field was where it was that type.
      const ty = hint?.t === "other" && !isMissing(hint) ? hint.v.ty : key.slice(1);
      return { t: "other", v: { ty, text: stringifyJson(node) } };
    }
    if (node.t === "number") return numberValue(node.text, hint?.t === "decimal");
    if (node.t === "array" || node.t === "object") return { t: "json", v: stringifyJson(node) };
    return PLAIN.read(node, undefined, field);
  },
};

/** MongoDB's relaxed Extended JSON v2. */
export const EXTENDED_JSON: DocumentFormat = makeFormat(
  "extendedJson",
  "Extended JSON",
  extendedCodec,
);

const DYNAMO_TYPES = ["S", "N", "B", "BOOL", "NULL", "M", "L", "SS", "NS", "BS"];

/** Whether `node` is a typed AttributeValue (`{"S": "x"}`). */
function typed(node: JsonNode): boolean {
  return (
    node.t === "object" && node.entries.length === 1 && DYNAMO_TYPES.includes(node.entries[0]![0])
  );
}

/** Plain JSON as DynamoDB JSON (the content of a map or list). */
function marshall(node: JsonNode): JsonNode {
  switch (node.t) {
    case "null":
      return obj(["NULL", { t: "bool", v: true }]);
    case "bool":
      return obj(["BOOL", node]);
    case "number":
      return obj(["N", str(node.text)]);
    case "string":
      return obj(["S", node]);
    case "array":
      return obj(["L", { t: "array", items: node.items.map(marshall) }]);
    case "object":
      return obj(["M", obj(...node.entries.map(([k, n]): [string, JsonNode] => [k, marshall(n)]))]);
  }
}

const JSON_NUMBER = /^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?$/;

/**
 * A DynamoDB number's text as a JSON number: as written where JSON takes it, else without what
 * JSON refuses (a `+`, leading zeros, a bare `.`), else as a string.
 */
function dynamoNumber(text: string): JsonNode {
  if (JSON_NUMBER.test(text)) return num(text);
  const m = /^([+-]?)0*(\d*)(?:\.(\d*))?([eE][+-]?\d+)?$/.exec(text);
  if (m) {
    const [, sign, whole, fraction, exponent] = m;
    const fixed = `${sign === "-" ? "-" : ""}${whole || "0"}${fraction ? `.${fraction}` : ""}${exponent ?? ""}`;
    if (JSON_NUMBER.test(fixed)) return num(fixed);
  }
  return str(text);
}

/** A typed AttributeValue as plain JSON (numbers keep their digits; sets become lists). */
function unmarshall(node: JsonNode): JsonNode {
  if (!typed(node) || node.t !== "object") return node;
  const [type, inner] = node.entries[0]!;
  switch (type) {
    case "S":
    case "B":
    case "BOOL":
      return inner;
    case "N":
      return inner.t === "string" ? dynamoNumber(inner.v) : inner;
    case "NULL":
      return { t: "null" };
    case "M":
      return inner.t === "object"
        ? obj(...inner.entries.map(([k, n]): [string, JsonNode] => [k, unmarshall(n)]))
        : inner;
    case "L":
      return inner.t === "array" ? { t: "array", items: inner.items.map(unmarshall) } : inner;
    case "NS":
      return inner.t === "array"
        ? { t: "array", items: inner.items.map((n) => (n.t === "string" ? dynamoNumber(n.v) : n)) }
        : inner;
    default:
      return inner; // SS, BS
  }
}

/** The content of a map or list cell as plain JSON, typed content unmarshalled. */
function plainContent(node: JsonNode): JsonNode {
  if (node.t === "object" && node.entries.length > 0 && node.entries.every(([, n]) => typed(n)))
    return obj(...node.entries.map(([k, n]): [string, JsonNode] => [k, unmarshall(n)]));
  if (node.t === "array" && node.items.length > 0 && node.items.every(typed))
    return { t: "array", items: node.items.map(unmarshall) };
  return node;
}

/** The content of a map or list cell as DynamoDB JSON. */
function typedContent(node: JsonNode): JsonNode {
  if (node.t === "object")
    return node.entries.every(([, n]) => typed(n))
      ? node
      : obj(...node.entries.map(([k, n]): [string, JsonNode] => [k, marshall(n)]));
  if (node.t === "array")
    return node.items.every(typed) ? node : { t: "array", items: node.items.map(marshall) };
  return node;
}

const isNumeric = (v: Value) =>
  v.t === "int" || v.t === "float" || v.t === "bigInt" || v.t === "decimal";
const numberText = (v: Value): string =>
  v.t === "int" || v.t === "float" ? String(v.v) : v.t === "bigInt" || v.t === "decimal" ? v.v : "";

const dynamoCodec: Codec = {
  write(v) {
    switch (v.t) {
      case "null":
        return obj(["NULL", { t: "bool", v: true }]);
      case "bool":
        return obj(["BOOL", { t: "bool", v: v.v }]);
      case "int":
      case "float":
      case "bigInt":
      case "decimal":
        return obj(["N", str(numberText(v))]);
      case "bytes":
        return obj(["B", str(v.v.base64)]);
      case "json": {
        const content = typedContent(jsonCell(v.v));
        if (content.t === "array") return obj(["L", content]);
        return content.t === "object" ? obj(["M", content]) : marshall(content);
      }
      case "array": {
        const items = v.v;
        if (items.length > 0 && items.every(isNumeric))
          return obj(["NS", { t: "array", items: items.map((e) => str(numberText(e))) }]);
        if (items.length > 0 && items.every((e) => e.t === "bytes"))
          return obj([
            "BS",
            { t: "array", items: items.map((e) => str(e.t === "bytes" ? e.v.base64 : "")) },
          ]);
        return obj([
          "SS",
          { t: "array", items: items.map((e) => str(e.t === "text" ? e.v : valueText(e))) },
        ]);
      }
      case "other":
        return isMissing(v) ? null : obj(["S", str(v.v.text)]);
      case "default":
        return null;
      default:
        return obj(["S", str(v.v)]);
    }
  },
  read(node, _hint, field) {
    const fail = (): never => {
      throw new JsonError(
        `Field "${field}" is not DynamoDB JSON: write it as {"S": …}, {"N": …}, {"M": …}, {"L": …}…`,
      );
    };
    if (!typed(node) || node.t !== "object") return fail();
    const [type, inner] = node.entries[0]!;
    const strings = (): string[] =>
      inner.t === "array" && inner.items.every((n) => n.t === "string")
        ? inner.items.map((n) => (n.t === "string" ? n.v : ""))
        : fail();
    switch (type) {
      case "S":
        return inner.t === "string" ? { t: "text", v: inner.v } : fail();
      case "N":
        return inner.t === "string" && /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(inner.v)
          ? numberValue(inner.v, true)
          : fail();
      case "B":
        return inner.t === "string" ? bytesValue(inner.v) : fail();
      case "BOOL":
        return inner.t === "bool" ? { t: "bool", v: inner.v } : fail();
      case "NULL":
        return { t: "null" };
      case "M":
        return inner.t === "object" ? { t: "json", v: stringifyJson(inner) } : fail();
      case "L":
        return inner.t === "array" ? { t: "json", v: stringifyJson(inner) } : fail();
      case "SS":
        return { t: "array", v: strings().map((v) => ({ t: "text", v })) };
      case "NS":
        return { t: "array", v: strings().map((v) => numberValue(v, true)) };
      default:
        return { t: "array", v: strings().map(bytesValue) };
    }
  },
};

const valueText = (v: Value): string =>
  v.t === "null" || v.t === "default"
    ? ""
    : v.t === "other"
      ? v.v.text
      : v.t === "bytes"
        ? v.v.base64
        : v.t === "array"
          ? JSON.stringify(v.v.map(valueText))
          : String(v.v);

/** DynamoDB's typed JSON. */
export const DYNAMODB_JSON: DocumentFormat = makeFormat(
  "dynamodbJson",
  "DynamoDB JSON",
  dynamoCodec,
);

/** A DynamoDB item as plain JSON: maps and lists unmarshalled, numbers exact, sets as lists. */
export const DYNAMODB_PLAIN_JSON: DocumentFormat = makeFormat(
  "dynamodbPlain",
  "JSON",
  plainCodec(true, plainContent),
);

// ---- changes ----------------------------------------------------------------------------------

/** Whether two values are the same (JSON compared without its whitespace). */
export function sameValue(a: Value, b: Value): boolean {
  if (isMissing(a) || isMissing(b)) return isMissing(a) && isMissing(b);
  if (a.t === "json" && b.t === "json")
    return stringifyJson(jsonCell(a.v)) === stringifyJson(jsonCell(b.v));
  return JSON.stringify(a) === JSON.stringify(b);
}

export type DocumentChanges = { changes: DocumentFields } | { error: string };

/**
 * The fields to stage for an edited document: those whose value changed, a field the edit
 * removed as missing (`Other{missing}`: the lane unsets it). `fields` are the document's known
 * fields (its columns), so one the document lacked and the edit still lacks is not a change.
 */
export function documentChanges(
  original: DocumentFields,
  edited: DocumentFields,
  options: { readOnlyFields?: readonly string[]; canRemoveFields?: boolean } = {},
): DocumentChanges {
  const changes: DocumentFields = {};
  const names = new Set([...Object.keys(original), ...Object.keys(edited)]);
  for (const name of names) {
    const before = original[name] ?? MISSING;
    const after = edited[name] ?? MISSING;
    if (sameValue(before, after)) continue;
    if (options.readOnlyFields?.includes(name)) return { error: `"${name}" cannot be changed` };
    if (isMissing(after) && !options.canRemoveFields)
      return {
        error: `Removing a field ("${name}") is not supported here; set it to null instead`,
      };
    changes[name] = after;
  }
  return { changes };
}
