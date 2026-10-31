import type { Value } from "../../../bindings/Value";

/**
 * The mock MongoDB server's databases: documents as the backend hands them over (`values.rs`:
 * ObjectId `Other{objectId}`, dates `TimestampTz` text, Decimal128 `Decimal`, longs past 2^53
 * `BigInt`, nested values `Json`), fields some documents lack, and a column that mixes types.
 * Shared by every mock session (one server), and reset by tests.
 */

export type Doc = Record<string, Value>;

export interface IndexSpec {
  name: string;
  keys: [string, number | string][];
  unique?: boolean;
  expireAfterSeconds?: number;
}

export interface Collection {
  name: string;
  kind: "collection" | "view" | "timeseries";
  docs: Doc[];
  indexes: IndexSpec[];
  /** The validator as relaxed Extended JSON (`collMod`'s). */
  validator?: string;
  viewOn?: string;
  pipeline?: string;
}

export type Database = Map<string, Collection>;

export const text = (v: string): Value => ({ t: "text", v });
export const int = (v: number): Value => ({ t: "int", v });
export const float = (v: number): Value => ({ t: "float", v });
export const json = (v: unknown): Value => ({ t: "json", v: JSON.stringify(v) });
export const date = (v: string): Value => ({ t: "timestampTz", v });
export const oid = (hex: string): Value => ({ t: "other", v: { ty: "objectId", text: hex } });
export const NULL: Value = { t: "null" };
export const MISSING: Value = { t: "other", v: { ty: "missing", text: "" } };

/** A 24-digit ObjectId of `n` under a prefix of four hex digits. */
export const oidOf = (prefix: string, n: number): string =>
  `${prefix}${n.toString(16).padStart(20, "0")}`;

/** The `$type` alias of a value (`values::type_label`), as the sample reads it. */
export function typeLabel(v: Value): string {
  switch (v.t) {
    case "int":
      return "int";
    case "bigInt":
      return "long";
    case "float":
      return "double";
    case "decimal":
      return "decimal";
    case "text":
      return "string";
    case "bool":
      return "bool";
    case "null":
      return "null";
    case "timestampTz":
      return "date";
    case "uuid":
      return "uuid";
    case "bytes":
      return "binData";
    case "json":
      return v.v.trimStart().startsWith("[") ? "array" : "object";
    case "other":
      return v.v.ty;
    default:
      return v.t;
  }
}

const STATUSES = ["new", "paid", "shipped", "cancelled"];
const day = (n: number) =>
  new Date(Date.UTC(2026, 8, 1) + n * 86_400_000 + (n % 7) * 3_600_000).toISOString();

function shop(): Database {
  const customers: Doc[] = [
    "Ada Lovelace",
    "Grace Hopper",
    "Linus Torvalds",
    "Margaret Hamilton",
    "Ken Thompson",
    "Barbara Liskov",
    "Dennis Ritchie",
    "Frances Allen",
    "Edsger Dijkstra",
    "Radia Perlman",
    "Donald Knuth",
    "Katherine Johnson",
  ].map((name, i) => {
    const doc: Doc = {
      _id: oid(oidOf("6720", i + 1)),
      name: text(name),
      email: text(`${name.split(" ")[0]!.toLowerCase()}@example.com`),
      tags: json(i % 3 === 0 ? ["vip", "early"] : i % 3 === 1 ? ["newsletter"] : []),
      address: json({ city: ["Jakarta", "Bandung", "Surabaya"][i % 3], zip: `1${i}000` }),
      vip: { t: "bool", v: i % 4 === 0 },
      signedUp: date(day(-i * 9)),
    };
    // Old accounts carry the id of the system they came from.
    if (i < 4) doc.legacyId = int(1000 + i);
    return doc;
  });
  const orders: Doc[] = Array.from({ length: 60 }, (_, i) => {
    const doc: Doc = {
      _id: oid(oidOf("6730", i + 1)),
      customer: customers[i % customers.length]!._id!,
      // Imported orders kept whole numbers: the column mixes doubles and ints.
      total: i % 6 === 5 ? int(10 * (i + 1)) : float(Math.round((i + 1) * 1999) / 100),
      items: json([{ sku: `sku-00${(i % 5) + 1}`, qty: (i % 3) + 1 }]),
      createdAt: date(day(i)),
    };
    if (i % 7 !== 6) doc.status = text(STATUSES[i % STATUSES.length]!);
    if (i % 10 === 3) doc.note = NULL;
    if (i === 0) doc.trace = { t: "bigInt", v: "9007199254740993" };
    if (i % 4 === 0) doc.discount = { t: "decimal", v: "0.10" };
    return doc;
  });
  const products: Doc[] = ["Keyboard", "Mouse", "Monitor", "Dock", "Cable"].map((name, i) => ({
    _id: text(`sku-00${i + 1}`),
    name: text(name),
    price: { t: "decimal", v: ["49.90", "19.50", "199.00", "89.99", "4.75"][i]! },
    stock: int([12, 0, 4, 31, 250][i]!),
    batch: { t: "uuid", v: `0f8fad5b-d9cb-469f-a165-70867728950${i}` },
  }));
  return new Map<string, Collection>([
    [
      "customers",
      {
        name: "customers",
        kind: "collection",
        docs: customers,
        indexes: [
          { name: "_id_", keys: [["_id", 1]] },
          { name: "email_1", keys: [["email", 1]], unique: true },
        ],
      },
    ],
    [
      "orders",
      {
        name: "orders",
        kind: "collection",
        docs: orders,
        indexes: [
          { name: "_id_", keys: [["_id", 1]] },
          {
            name: "customer_1_createdAt_-1",
            keys: [
              ["customer", 1],
              ["createdAt", -1],
            ],
          },
        ],
        // A total below zero fails validation: the mock story's failing change.
        validator:
          '{"$jsonSchema": {"required": ["total"], "properties": {"total": {"bsonType": ["double", "int", "decimal"], "minimum": 0}}}}',
      },
    ],
    [
      "products",
      {
        name: "products",
        kind: "collection",
        docs: products,
        indexes: [{ name: "_id_", keys: [["_id", 1]] }],
      },
    ],
    [
      "paid_orders",
      {
        name: "paid_orders",
        kind: "view",
        docs: [],
        indexes: [],
        viewOn: "orders",
        pipeline: '[{"$match": {"status": "paid"}}]',
      },
    ],
    [
      "system.views",
      {
        name: "system.views",
        kind: "collection",
        docs: [
          {
            _id: text("shop.paid_orders"),
            viewOn: text("orders"),
            pipeline: json([{ $match: { status: "paid" } }]),
          },
        ],
        indexes: [{ name: "_id_", keys: [["_id", 1]] }],
      },
    ],
  ]);
}

function analytics(): Database {
  const types = ["page_view", "click", "signup", "purchase"];
  const events: Doc[] = Array.from({ length: 250 }, (_, i) => ({
    _id: oid(oidOf("6740", i + 1)),
    type: text(types[i % types.length]!),
    at: date(new Date(Date.UTC(2026, 9, 1) + i * 600_000).toISOString()),
    payload: json({ path: `/p/${i % 13}`, ms: (i * 37) % 900 }),
  }));
  return new Map<string, Collection>([
    [
      "events",
      {
        name: "events",
        kind: "timeseries",
        docs: events,
        indexes: [{ name: "type_1", keys: [["type", 1]] }],
      },
    ],
  ]);
}

/** The server's databases; `admin`, `config` and `local` are its own (system schemas). */
export const databases = new Map<string, Database>();

export function resetDatabases(): void {
  databases.clear();
  databases.set("admin", new Map());
  databases.set("analytics", analytics());
  databases.set("config", new Map());
  databases.set("local", new Map());
  databases.set("shop", shop());
}

resetDatabases();
