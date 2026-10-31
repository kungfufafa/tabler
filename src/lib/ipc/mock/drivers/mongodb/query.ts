import { parseLiteral, type Lit } from "@/features/drivers/mongodb/shell";
import type { FilterGroup } from "../../../bindings/FilterGroup";
import type { FilterNode } from "../../../bindings/FilterNode";
import type { SortSpec } from "../../../bindings/SortSpec";
import type { Value } from "../../../bindings/Value";
import { mockError } from "../../transport";
import { MISSING, type Doc } from "./data";

/**
 * What the mock server does with documents: compares values in MongoDB's type order, reads the
 * grid's filter bar as the backend's query document would match (`sqlgen/native/mongo.rs`), runs
 * the shell's query documents, sorts, and writes values as relaxed Extended JSON for the text the
 * backend shows (its previews, the console).
 */

const isMissing = (v: Value | undefined): boolean =>
  v === undefined || (v.t === "other" && v.v.ty === "missing");

/** A number the value holds (ints, doubles, longs, decimals); null for any other. */
function numberOf(v: Value): number | null {
  switch (v.t) {
    case "int":
    case "float":
      return v.v;
    case "bigInt":
    case "decimal":
      return Number(v.v);
    default:
      return null;
  }
}

/** MongoDB's order of types (null and missing first, then numbers, strings, objects…). */
function typeRank(v: Value): number {
  if (isMissing(v) || v.t === "null") return 1;
  if (numberOf(v) !== null) return 2;
  switch (v.t) {
    case "text":
      return 3;
    case "json":
      return v.v.trimStart().startsWith("[") ? 5 : 4;
    case "bytes":
    case "uuid":
      return 6;
    case "other":
      return v.v.ty === "objectId" ? 7 : 10;
    case "bool":
      return 8;
    case "timestampTz":
      return 9;
    default:
      return 10;
  }
}

/** The text a value shows (and the filter bar compares as). */
export function valueText(v: Value): string {
  switch (v.t) {
    case "null":
    case "default":
      return "null";
    case "other":
      return v.v.text;
    case "bytes":
      return v.v.base64;
    case "array":
      return JSON.stringify(v.v.map(valueText));
    default:
      return String(v.v);
  }
}

export function compareValues(a: Value, b: Value): number {
  const ra = typeRank(a);
  const rb = typeRank(b);
  if (ra !== rb) return ra < rb ? -1 : 1;
  const na = numberOf(a);
  const nb = numberOf(b);
  if (na !== null && nb !== null) return na === nb ? 0 : na < nb ? -1 : 1;
  const ta = valueText(a);
  const tb = valueText(b);
  return ta === tb ? 0 : ta < tb ? -1 : 1;
}

/** Equal as MongoDB matches: numbers by value, anything else by type and text. */
export const sameValue = (a: Value, b: Value): boolean =>
  !isMissing(a) && !isMissing(b) && compareValues(a, b) === 0;

/** A filter-bar value as the field's type, where it reads as one (the backend matches each). */
function asFieldType(cell: Value, v: Value): Value {
  if (v.t !== "text") return v;
  const n = Number(v.v);
  if (numberOf(cell) !== null && v.v.trim() !== "" && !Number.isNaN(n)) return { t: "float", v: n };
  if (cell.t === "other" || cell.t === "timestampTz" || cell.t === "uuid")
    return { ...cell, v: cell.t === "other" ? { ty: cell.v.ty, text: v.v } : v.v } as Value;
  if (cell.t === "bool" && (v.v === "true" || v.v === "false"))
    return { t: "bool", v: v.v === "true" };
  return v;
}

const escapeRegex = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** SQL LIKE's `%` and `_` as a regex (`like_pattern`). */
const likeRegex = (pattern: string, flags: string) =>
  new RegExp(
    `^${[...pattern].map((c) => (c === "%" ? ".*" : c === "_" ? "." : escapeRegex(c))).join("")}$`,
    flags,
  );

function conditionMatches(cell: Value, op: FilterNode & { kind: "condition" }): boolean {
  const [first, second] = op.values;
  const eq = (v: Value | undefined) => v !== undefined && sameValue(cell, asFieldType(cell, v));
  const cmp = (v: Value | undefined) =>
    v === undefined || isMissing(cell) || typeRank(cell) !== typeRank(asFieldType(cell, v))
      ? null
      : compareValues(cell, asFieldType(cell, v));
  const str = cell.t === "text" ? cell.v : null;
  const re = (pattern: string, insensitive: boolean) =>
    str !== null && new RegExp(pattern, insensitive ? "is" : "s").test(str);
  const lit = first ? escapeRegex(valueText(first)) : "";
  switch (op.op) {
    case "eq":
      return eq(first);
    case "ne":
      return !eq(first);
    case "gt":
      return (cmp(first) ?? 0) > 0 && cmp(first) !== null;
    case "lt":
      return (cmp(first) ?? 0) < 0 && cmp(first) !== null;
    case "ge":
      return cmp(first) !== null && cmp(first)! >= 0;
    case "le":
      return cmp(first) !== null && cmp(first)! <= 0;
    case "in":
      return op.values.some(eq);
    case "notIn":
      return !op.values.some(eq);
    case "isNull":
      return isMissing(cell) || cell.t === "null";
    case "isNotNull":
      return !isMissing(cell) && cell.t !== "null";
    case "between":
    case "notBetween": {
      const inside =
        cmp(first) !== null && cmp(first)! >= 0 && cmp(second) !== null && cmp(second)! <= 0;
      return op.op === "between" ? inside : !inside;
    }
    case "like":
    case "ilike":
      return (
        str !== null &&
        likeRegex(first ? valueText(first) : "", op.op === "ilike" ? "is" : "s").test(str)
      );
    case "notLike":
      return !(str !== null && likeRegex(first ? valueText(first) : "", "s").test(str));
    case "contains":
      return re(lit, false);
    case "containsCase":
      return re(lit, true);
    case "notContains":
      return !re(lit, false);
    case "notContainsCase":
      return !re(lit, true);
    case "hasPrefix":
      return re(`^${lit}`, false);
    case "hasPrefixCase":
      return re(`^${lit}`, true);
    case "hasSuffix":
      return re(`${lit}$`, false);
    case "hasSuffixCase":
      return re(`${lit}$`, true);
    default:
      return mockError("UNSUPPORTED", `unsupported: the filter operator ${op.op}`);
  }
}

/** Whether `doc` passes the filter bar. */
export function filterMatches(doc: Doc, group: FilterGroup | null): boolean {
  if (!group || group.items.length === 0) return true;
  const each = (node: FilterNode): boolean => {
    switch (node.kind) {
      case "condition":
        return conditionMatches(doc[node.column] ?? MISSING, node);
      case "group":
        return filterMatches(doc, node);
      case "raw":
        return queryMatches(doc, rawFilter(node.sql));
      case "anyColumn":
        return Object.values(doc).some((cell) =>
          conditionMatches(cell, {
            kind: "condition",
            column: "",
            op: node.op,
            values: node.values,
          }),
        );
    }
  };
  return group.conjunction === "or" ? group.items.some(each) : group.items.every(each);
}

/** The raw filter: a query document in shell syntax, its braces optional (`{a: 1}` or `a: 1`). */
export function rawFilter(text: string): Extract<Lit, { k: "doc" }> {
  const t = text.trim();
  let v: Lit;
  try {
    v = parseLiteral(t.startsWith("{") ? t : `{${t}}`);
  } catch (e) {
    return mockError("INVALID_INPUT", `MongoDB filter: ${e instanceof Error ? e.message : "?"}`);
  }
  return v.k === "doc" ? v : mockError("INVALID_INPUT", "MongoDB filter: a query document");
}

// ---------------------------------------------------------------------------------------------
// The shell's values and query documents
// ---------------------------------------------------------------------------------------------

/** Extended JSON text of a literal (for Json cells). */
function litJson(v: Lit): unknown {
  switch (v.k) {
    case "doc":
      return Object.fromEntries(v.entries.map(([k, x]) => [k, litJson(x)]));
    case "arr":
      return v.items.map(litJson);
    case "str":
    case "num":
    case "bool":
      return v.v;
    case "null":
      return null;
    case "other": {
      const value = litValue(v);
      return value.t === "other" && value.v.ty === "objectId"
        ? { $oid: value.v.text }
        : value.t === "timestampTz"
          ? { $date: value.v }
          : valueText(value);
    }
  }
}

/** A literal of the shell as the value a document holds. */
export function litValue(v: Lit): Value {
  switch (v.k) {
    case "str":
      return { t: "text", v: v.v };
    case "num":
      return v.int && Number.isSafeInteger(v.v) ? { t: "int", v: v.v } : { t: "float", v: v.v };
    case "bool":
      return { t: "bool", v: v.v };
    case "null":
      return { t: "null" };
    case "doc":
    case "arr":
      return { t: "json", v: JSON.stringify(litJson(v)) };
    case "other": {
      const arg = v.inner[0];
      const s = arg?.k === "str" ? arg.v : arg?.k === "num" ? String(arg.v) : null;
      switch (v.name) {
        case "ObjectId":
        case "ObjectID":
          return {
            t: "other",
            v: {
              ty: "objectId",
              text:
                s ??
                Math.floor(Date.now() / 1000)
                  .toString(16)
                  .padEnd(24, "0"),
            },
          };
        case "$oid":
          return { t: "other", v: { ty: "objectId", text: wrapped(v) } };
        case "ISODate":
        case "Date":
          return { t: "timestampTz", v: new Date(s ?? Date.now()).toISOString() };
        case "$date":
          return { t: "timestampTz", v: new Date(wrapped(v)).toISOString() };
        case "NumberLong":
        case "Long":
          return Number.isSafeInteger(Number(s))
            ? { t: "int", v: Number(s) }
            : { t: "bigInt", v: s ?? "0" };
        case "NumberInt":
        case "Int32":
          return { t: "int", v: Number(s ?? 0) };
        case "NumberDecimal":
        case "Decimal128":
          return { t: "decimal", v: s ?? "0" };
        case "UUID":
          return { t: "uuid", v: s ?? crypto.randomUUID() };
        default:
          return {
            t: "other",
            v: {
              ty: v.name.replace(/^\$/, ""),
              text: JSON.stringify(litJson(v.inner[0] ?? { k: "null" })),
            },
          };
      }
    }
  }
}

/** The text inside a one-key Extended JSON wrapper (`{$oid: "…"}`). */
function wrapped(v: Extract<Lit, { k: "other" }>): string {
  const doc = v.inner[0];
  const inner = doc?.k === "doc" ? doc.entries[0]?.[1] : undefined;
  return inner?.k === "str" ? inner.v : "";
}

function regexOf(v: Lit | undefined): RegExp | null {
  if (v?.k === "other" && v.name === "regex") {
    const [p, f] = v.inner;
    return new RegExp(
      p?.k === "str" ? p.v : "",
      (f?.k === "str" ? f.v : "").replace(/[^ims]/g, ""),
    );
  }
  return null;
}

/** Whether `doc` matches a query document (the operators the mock knows; others are refused). */
export function queryMatches(doc: Doc, query: Extract<Lit, { k: "doc" }>): boolean {
  return query.entries.every(([key, cond]) => {
    if (key === "$and" || key === "$or" || key === "$nor") {
      const parts = cond.k === "arr" ? cond.items.filter((c) => c.k === "doc") : [];
      const hits = parts.map((p) => queryMatches(doc, p as Extract<Lit, { k: "doc" }>));
      return key === "$and"
        ? hits.every(Boolean)
        : key === "$or"
          ? hits.some(Boolean)
          : !hits.some(Boolean);
    }
    if (key.startsWith("$"))
      return mockError("UNSUPPORTED", `unsupported: the mock does not run ${key}`);
    const cell = doc[key] ?? MISSING;
    const isOps =
      cond.k === "doc" && cond.entries.length > 0 && cond.entries.every(([k]) => k.startsWith("$"));
    if (!isOps) return fieldEquals(cell, cond);
    return (cond as Extract<Lit, { k: "doc" }>).entries.every(([op, arg]) =>
      operatorMatches(cell, op, arg),
    );
  });
}

function fieldEquals(cell: Value, cond: Lit): boolean {
  const re = regexOf(cond);
  if (re) return cell.t === "text" && re.test(cell.v);
  if (cond.k === "null") return isMissing(cell) || cell.t === "null";
  return sameValue(cell, litValue(cond));
}

function operatorMatches(cell: Value, op: string, arg: Lit): boolean {
  const cmp = () => {
    const v = litValue(arg);
    return isMissing(cell) || typeRank(cell) !== typeRank(v) ? null : compareValues(cell, v);
  };
  switch (op) {
    case "$eq":
      return fieldEquals(cell, arg);
    case "$ne":
      return !fieldEquals(cell, arg);
    case "$gt":
      return (cmp() ?? -Infinity) > 0;
    case "$gte":
      return (cmp() ?? -Infinity) >= 0;
    case "$lt":
      return (cmp() ?? Infinity) < 0;
    case "$lte":
      return (cmp() ?? Infinity) <= 0;
    case "$in":
      return arg.k === "arr" && arg.items.some((x) => fieldEquals(cell, x));
    case "$nin":
      return !(arg.k === "arr" && arg.items.some((x) => fieldEquals(cell, x)));
    case "$exists":
      return (arg.k === "bool" && !arg.v) === isMissing(cell);
    case "$regex": {
      const re = regexOf(arg) ?? (arg.k === "str" ? new RegExp(arg.v) : null);
      return re !== null && cell.t === "text" && re.test(cell.v);
    }
    case "$options":
      return true;
    case "$not":
      return !(arg.k === "doc"
        ? arg.entries.every(([o, a]) => operatorMatches(cell, o, a))
        : fieldEquals(cell, arg));
    default:
      return mockError("UNSUPPORTED", `unsupported: the mock does not run ${op}`);
  }
}

/** Sorted by the specs, `_id` breaking ties (as the backend's `find`). */
export function sortDocs(docs: Doc[], sort: readonly SortSpec[]): Doc[] {
  const keys: [string, number][] = [
    ...sort.map((s): [string, number] => [s.column, s.dir === "desc" ? -1 : 1]),
    ["_id", 1],
  ];
  return docs.slice().sort((a, b) => {
    for (const [k, dir] of keys) {
      const c = compareValues(a[k] ?? MISSING, b[k] ?? MISSING);
      if (c !== 0) return c * dir;
    }
    return 0;
  });
}

// ---------------------------------------------------------------------------------------------
// Relaxed Extended JSON
// ---------------------------------------------------------------------------------------------

/** A value as relaxed Extended JSON text (`doc_json`). */
export function valueJson(v: Value): string {
  switch (v.t) {
    case "null":
    case "default":
      return "null";
    case "int":
      return String(v.v);
    case "float":
      return Number.isInteger(v.v) ? `${v.v}.0` : String(v.v);
    case "bigInt":
      return `{"$numberLong": ${JSON.stringify(v.v)}}`;
    case "decimal":
      return `{"$numberDecimal": ${JSON.stringify(v.v)}}`;
    case "bool":
      return String(v.v);
    case "timestampTz":
      return `{"$date": ${JSON.stringify(v.v)}}`;
    case "uuid":
      return `{"$uuid": ${JSON.stringify(v.v)}}`;
    case "json":
      return v.v;
    case "other":
      return v.v.ty === "objectId" ? `{"$oid": ${JSON.stringify(v.v.text)}}` : v.v.text;
    default:
      return JSON.stringify(valueText(v));
  }
}

/** A document (or any key-value list) as relaxed Extended JSON text. */
export const docJson = (entries: readonly (readonly [string, string])[]): string =>
  `{${entries.map(([k, v]) => `${JSON.stringify(k)}: ${v}`).join(", ")}}`;
