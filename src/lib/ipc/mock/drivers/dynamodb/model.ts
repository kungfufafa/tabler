import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { IndexMeta } from "../../../bindings/IndexMeta";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { Value } from "../../../bindings/Value";
import type { ValueKind } from "../../../bindings/ValueKind";
import {
  ATTR_TYPES,
  typeOf,
  type AttrType,
  type Item,
  type KeySchema,
  type SecondaryIndex,
  type Table,
} from "./data";

/**
 * The backend's catalog (`drivers/dynamodb/catalog.rs`) and request text over the mock's tables:
 * columns (keys, then the sampled attributes in first-seen order typed `S|N|NULL`, then index
 * keys), indexes, the comment line, the `DescribeTable` reply and the `CreateTable` request,
 * values as DynamoDB JSON, and the order items are read in.
 */

/** The grid and the console show at most this many attributes. */
export const MAX_COLUMNS = 500;

// --- values -----------------------------------------------------------------------------------

const numberText = (v: Value): string | null =>
  v.t === "int" || v.t === "float"
    ? String(v.v)
    : v.t === "bigInt" || v.t === "decimal"
      ? v.v
      : null;

/** A value as an AttributeValue (`{"S": "x"}`); a missing one is null. */
export function attrJson(v: Value): unknown {
  switch (v.t) {
    case "text":
      return { S: v.v };
    case "int":
    case "float":
    case "bigInt":
    case "decimal":
      return { N: numberText(v) };
    case "bytes":
      return { B: v.v.base64 };
    case "bool":
      return { BOOL: v.v };
    case "null":
      return { NULL: true };
    case "json": {
      const parsed: unknown = JSON.parse(v.v);
      return Array.isArray(parsed) ? { L: parsed } : { M: parsed };
    }
    case "array": {
      const type = typeOf(v);
      return {
        [type ?? "SS"]: v.v.map((x) =>
          x.t === "bytes" ? x.v.base64 : (numberText(x) ?? (x.t === "text" ? x.v : "")),
        ),
      };
    }
    default:
      return null;
  }
}

/** An item (or a key) as DynamoDB JSON. */
export function itemJson(item: Item): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(item)) {
    const a = attrJson(v);
    if (a !== null) out[k] = a;
  }
  return out;
}

/** An AttributeValue read back (the console's requests), or null when it is not one. */
export function valueOf(a: unknown): Value | null {
  if (typeof a !== "object" || a === null) return null;
  const [entry] = Object.entries(a as Record<string, unknown>);
  if (!entry || Object.keys(a).length !== 1) return null;
  const [tag, v] = entry;
  if (tag === "S" && typeof v === "string") return { t: "text", v };
  if (tag === "N" && typeof v === "string" && /^-?\d+(\.\d+)?$/.test(v))
    return /^-?\d+$/.test(v)
      ? Number.isSafeInteger(Number(v))
        ? { t: "int", v: Number(v) }
        : { t: "bigInt", v }
      : { t: "decimal", v };
  if (tag === "BOOL" && typeof v === "boolean") return { t: "bool", v };
  if (tag === "NULL" && v === true) return { t: "null" };
  if ((tag === "M" || tag === "L") && typeof v === "object" && v !== null)
    return { t: "json", v: JSON.stringify(v) };
  if (tag === "SS" && Array.isArray(v) && v.every((x) => typeof x === "string"))
    return { t: "array", v: v.map((x: string) => ({ t: "text", v: x })) };
  return null;
}

/** Whether two values are the same attribute value. */
export function same(a: Value | undefined, b: Value | undefined): boolean {
  if (a === undefined || b === undefined) return a === b;
  return JSON.stringify(attrJson(a)) === JSON.stringify(attrJson(b));
}

/** DynamoDB's order of two key values: numbers by value, strings and bytes by their text. */
export function compare(a: Value | undefined, b: Value | undefined): number {
  const na = a ? numberText(a) : null;
  const nb = b ? numberText(b) : null;
  if (na !== null && nb !== null) {
    const d = Number(na) - Number(nb);
    return d !== 0 ? Math.sign(d) : na < nb ? -1 : na > nb ? 1 : 0;
  }
  const ta = a ? JSON.stringify(attrJson(a)) : "";
  const tb = b ? JSON.stringify(attrJson(b)) : "";
  return ta < tb ? -1 : ta > tb ? 1 : 0;
}

export const keyNames = (keys: KeySchema): string[] =>
  keys.range ? [keys.hash.name, keys.range.name] : [keys.hash.name];

/** The key of an item, or null when it lacks a key attribute. */
export function keyOf(item: Item, keys: KeySchema): Item | null {
  const out: Item = {};
  for (const n of keyNames(keys)) {
    const v = item[n];
    if (v === undefined) return null;
    out[n] = v;
  }
  return out;
}

/** An item's key as text (one per item). */
export const keyText = (item: Item, keys: KeySchema): string =>
  JSON.stringify(itemJson(keyOf(item, keys) ?? {}));

// --- the catalog ------------------------------------------------------------------------------

const KIND: Record<AttrType, ValueKind> = {
  S: "text",
  N: "decimal",
  B: "bytes",
  BOOL: "bool",
  NULL: "other",
  M: "json",
  L: "json",
  SS: "array",
  NS: "array",
  BS: "array",
};

const label = (types: Set<AttrType>): string => ATTR_TYPES.filter((t) => types.has(t)).join("|");

function kindOf(types: Set<AttrType>): ValueKind {
  const real = [...types].filter((t) => t !== "NULL");
  return real.length === 1 ? KIND[real[0]!] : "other";
}

function column(name: string, types: Set<AttrType>): ColumnMeta {
  const elem = types.has("SS") ? "S" : types.has("NS") ? "N" : types.has("BS") ? "B" : null;
  return {
    name,
    ordinal: 0,
    dataType: label(types),
    kind: kindOf(types),
    nullable: true,
    defaultExpr: null,
    isPk: false,
    pkOrdinal: null,
    isAutoIncrement: false,
    isGenerated: false,
    isUnsigned: false,
    comment: null,
    enumValues: null,
    charMaxLen: null,
    numericPrecision: null,
    numericScale: null,
    collation: null,
    arrayElemType: elem,
    extra: null,
    baseType: null,
  };
}

/** The attributes of items in first-seen order, with the types seen. */
export function sampled(items: readonly Item[]): [string, Set<AttrType>][] {
  const out = new Map<string, Set<AttrType>>();
  for (const item of items)
    for (const [name, v] of Object.entries(item)) {
      const t = typeOf(v);
      if (t === null) continue;
      const types = out.get(name) ?? new Set<AttrType>();
      types.add(t);
      out.set(name, types);
    }
  return [...out];
}

/** All the attribute definitions of a table: its keys' and its indexes'. */
function definitions(t: Table): [string, AttrType][] {
  const out = new Map<string, AttrType>();
  for (const k of [t.keys, ...t.global.map((i) => i.keys), ...t.local.map((i) => i.keys)])
    for (const a of [k.hash, k.range]) if (a && !out.has(a.name)) out.set(a.name, a.type);
  return [...out];
}

/** A table's columns: its keys, the sampled attributes, then index keys the sample lacked. */
export function columns(t: Table, sample: readonly Item[]): ColumnMeta[] {
  const out: ColumnMeta[] = [];
  const keys: [string, AttrType, number, string, string][] = [
    [t.keys.hash.name, t.keys.hash.type, 1, "partition key", "HASH"],
  ];
  if (t.keys.range) keys.push([t.keys.range.name, t.keys.range.type, 2, "sort key", "RANGE"]);
  for (const [name, type, pk, role, keyType] of keys)
    out.push({
      ...column(name, new Set([type])),
      nullable: false,
      isPk: true,
      pkOrdinal: pk,
      comment: role,
      extra: keyType,
    });
  const names = new Set(out.map((c) => c.name));
  for (const [name, types] of sampled(sample)) {
    if (out.length >= MAX_COLUMNS) break;
    if (!names.has(name)) {
      names.add(name);
      out.push(column(name, types));
    }
  }
  for (const [name, type] of definitions(t))
    if (!names.has(name)) {
      names.add(name);
      out.push({ ...column(name, new Set([type])), comment: "index key" });
    }
  return out.map((c, i) => ({ ...c, ordinal: i + 1 }));
}

/** Adds the attributes a page's items have that the columns lack, and types not seen yet. */
export function mergeColumns(cols: ColumnMeta[], items: readonly Item[]): ColumnMeta[] {
  const out = cols.map((c) => ({ ...c }));
  for (const [name, types] of sampled(items)) {
    const c = out.find((x) => x.name === name);
    if (c && !c.isPk) {
      const all = new Set([...(c.dataType.split("|") as AttrType[]), ...types]);
      c.dataType = label(all);
      c.kind = kindOf(all);
    } else if (!c && out.length < MAX_COLUMNS)
      out.push({ ...column(name, types), ordinal: out.length + 1 });
  }
  return out;
}

function indexMeta(i: SecondaryIndex, method: "GSI" | "LSI"): IndexMeta {
  return {
    name: i.name,
    columns: keyNames(i.keys),
    unique: false,
    primary: false,
    method,
    predicate: null,
    definition: `${i.projection} projection: ${JSON.stringify(projection(i))}`,
    comment: "ACTIVE",
    include: i.nonKey,
  };
}

export function indexes(t: Table): IndexMeta[] {
  return [
    {
      name: "PRIMARY",
      columns: keyNames(t.keys),
      unique: true,
      primary: true,
      method: "primary key",
      predicate: null,
      definition: null,
      comment: null,
      include: [],
    },
    ...t.global.map((i) => indexMeta(i, "GSI")),
    ...t.local.map((i) => indexMeta(i, "LSI")),
  ];
}

/** About how many bytes the table takes (DynamoDB's `TableSizeBytes`, refreshed with the count). */
const sizeOf = (t: Table) => t.itemCount * 120;

/** Status, billing, TTL, stream, protection and size in a line (`catalog::comment`). */
export function comment(t: Table): string {
  const parts = ["ACTIVE", t.billing === "PROVISIONED" ? "provisioned" : "on-demand"];
  if (t.ttl) parts.push(`TTL on ${t.ttl}`);
  if (t.stream) parts.push(`stream ${t.stream}`);
  if (t.deletionProtection) parts.push("deletion protection");
  parts.push(`about ${sizeOf(t)} bytes`);
  return parts.join(" · ");
}

function keySchemaJson(k: KeySchema) {
  const out = [{ AttributeName: k.hash.name, KeyType: "HASH" }];
  if (k.range) out.push({ AttributeName: k.range.name, KeyType: "RANGE" });
  return out;
}

function projection(i: SecondaryIndex) {
  return i.projection === "INCLUDE"
    ? { ProjectionType: "INCLUDE", NonKeyAttributes: i.nonKey }
    : { ProjectionType: i.projection };
}

const THROUGHPUT = { ReadCapacityUnits: 5, WriteCapacityUnits: 5 };

/** The `Table` of a `DescribeTable` reply. */
export function describeTable(t: Table, region: string): Record<string, unknown> {
  const provisioned = t.billing === "PROVISIONED";
  const out: Record<string, unknown> = {
    TableName: t.name,
    TableStatus: "ACTIVE",
    TableArn: `arn:aws:dynamodb:${region}:123456789012:table/${t.name}`,
    CreationDateTime: 1_767_225_600,
    AttributeDefinitions: definitions(t).map(([n, ty]) => ({
      AttributeName: n,
      AttributeType: ty,
    })),
    KeySchema: keySchemaJson(t.keys),
    ItemCount: t.itemCount,
    TableSizeBytes: sizeOf(t),
    BillingModeSummary: { BillingMode: t.billing },
    ProvisionedThroughput: provisioned
      ? THROUGHPUT
      : { ReadCapacityUnits: 0, WriteCapacityUnits: 0 },
    DeletionProtectionEnabled: t.deletionProtection,
  };
  if (t.global.length > 0)
    out.GlobalSecondaryIndexes = t.global.map((i) => ({
      IndexName: i.name,
      KeySchema: keySchemaJson(i.keys),
      Projection: projection(i),
      IndexStatus: "ACTIVE",
      ...(provisioned ? { ProvisionedThroughput: THROUGHPUT } : {}),
    }));
  if (t.local.length > 0)
    out.LocalSecondaryIndexes = t.local.map((i) => ({
      IndexName: i.name,
      KeySchema: keySchemaJson(i.keys),
      Projection: projection(i),
    }));
  if (t.stream) out.StreamSpecification = { StreamEnabled: true, StreamViewType: t.stream };
  return out;
}

/** The text of a request: `Action` and its body as indented JSON (`request_text`). */
export const requestText = (action: string, body: unknown): string =>
  `${action} ${JSON.stringify(body, null, 2)}`;

/** The table as the `CreateTable` request that makes one like it (`catalog::definition`). */
export function definition(t: Table): string {
  const provisioned = t.billing === "PROVISIONED";
  const body: Record<string, unknown> = {
    TableName: t.name,
    AttributeDefinitions: definitions(t).map(([n, ty]) => ({
      AttributeName: n,
      AttributeType: ty,
    })),
    KeySchema: keySchemaJson(t.keys),
    BillingMode: t.billing,
  };
  if (provisioned) body.ProvisionedThroughput = THROUGHPUT;
  if (t.global.length > 0)
    body.GlobalSecondaryIndexes = t.global.map((i) => ({
      IndexName: i.name,
      KeySchema: keySchemaJson(i.keys),
      Projection: projection(i),
      ...(provisioned ? { ProvisionedThroughput: THROUGHPUT } : {}),
    }));
  if (t.local.length > 0)
    body.LocalSecondaryIndexes = t.local.map((i) => ({
      IndexName: i.name,
      KeySchema: keySchemaJson(i.keys),
      Projection: projection(i),
    }));
  if (t.stream) body.StreamSpecification = { StreamEnabled: true, StreamViewType: t.stream };
  if (t.deletionProtection) body.DeletionProtectionEnabled = true;
  return requestText("CreateTable", body);
}

/** A table's structure as the backend's `table_meta` gives it. */
export function tableMeta(t: Table, region: string, sampleSize: number): TableMeta {
  return {
    table: { schema: region, name: t.name },
    kind: "table",
    columns: columns(t, t.items.slice(0, sampleSize)),
    primaryKey: keyNames(t.keys),
    indexes: indexes(t),
    foreignKeys: [],
    triggers: [],
    comment: comment(t),
    rowEstimate: t.itemCount,
    ddl: definition(t),
    checks: [],
    options: null,
  };
}
