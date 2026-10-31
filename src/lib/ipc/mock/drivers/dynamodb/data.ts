import type { Value } from "../../../bindings/Value";

/**
 * The mock's DynamoDB tables (one region's): their key schemas, secondary indexes, settings and
 * items. An item is its attributes as the grid's values, each keeping its DynamoDB type the way
 * the backend's `attr.rs` maps it: `S` text, `N` an int, a big int or a decimal by its digits,
 * `B` bytes, `BOOL`, `NULL`, `M` / `L` JSON of their content, `SS` / `NS` / `BS` arrays.
 */

export type AttrType = "S" | "N" | "B" | "BOOL" | "NULL" | "M" | "L" | "SS" | "NS" | "BS";
/** `AttrType`'s order (the backend's `BTreeSet` of types sorts by it). */
export const ATTR_TYPES: readonly AttrType[] = [
  "S",
  "N",
  "B",
  "BOOL",
  "NULL",
  "M",
  "L",
  "SS",
  "NS",
  "BS",
];

export type Item = Record<string, Value>;

export interface KeyAttr {
  name: string;
  type: "S" | "N" | "B";
}

export interface KeySchema {
  hash: KeyAttr;
  range: KeyAttr | null;
}

export interface SecondaryIndex {
  name: string;
  keys: KeySchema;
  projection: "ALL" | "KEYS_ONLY" | "INCLUDE";
  nonKey: string[];
}

export interface Table {
  name: string;
  keys: KeySchema;
  global: SecondaryIndex[];
  local: SecondaryIndex[];
  billing: "PAY_PER_REQUEST" | "PROVISIONED";
  stream: string | null;
  ttl: string | null;
  pitr: boolean;
  deletionProtection: boolean;
  /** The item count DynamoDB last reported (refreshed about every six hours, so it lags). */
  itemCount: number;
  items: Item[];
}

export const S = (v: string): Value => ({ t: "text", v });
/** A number by its exact text (never a float). */
export function N(text: string | number): Value {
  const t = String(text);
  if (/^-?\d+$/.test(t))
    return Number.isSafeInteger(Number(t)) ? { t: "int", v: Number(t) } : { t: "bigInt", v: t };
  return { t: "decimal", v: t };
}
export const BOOL = (v: boolean): Value => ({ t: "bool", v });
export const NULL: Value = { t: "null" };
export const M = (v: unknown): Value => ({ t: "json", v: JSON.stringify(v) });
export const SS = (v: string[]): Value => ({ t: "array", v: v.map(S) });
export const NS = (v: (string | number)[]): Value => ({ t: "array", v: v.map(N) });
export const MISSING: Value = { t: "other", v: { ty: "missing", text: "" } };

/** The DynamoDB type of a value as the mock keeps it. */
export function typeOf(v: Value): AttrType | null {
  switch (v.t) {
    case "text":
      return "S";
    case "int":
    case "bigInt":
    case "decimal":
    case "float":
      return "N";
    case "bytes":
      return "B";
    case "bool":
      return "BOOL";
    case "null":
      return "NULL";
    case "json":
      return v.v.trimStart().startsWith("[") ? "L" : "M";
    case "array": {
      const first = v.v[0];
      return first?.t === "bytes" ? "BS" : first && typeOf(first) === "N" ? "NS" : "SS";
    }
    default:
      return null;
  }
}

const ARTISTS = ["Acme Band", "No One You Know", "The Placeholders", "Zero Day"];
const GENRES = ["Rock", "Country", "Jazz", "Pop"];

function music(): Item[] {
  const out: Item[] = [];
  for (let a = 0; a < ARTISTS.length; a++)
    for (let s = 1; s <= 12; s++) {
      const item: Item = {
        Artist: S(ARTISTS[a]!),
        SongTitle: S(`Song ${String(s).padStart(2, "0")}`),
        AlbumTitle: S(`Album ${Math.ceil(s / 4)}`),
        Year: N(1990 + ((a * 7 + s) % 30)),
        Genre: S(GENRES[(a + s) % GENRES.length]!),
        Price: N(`${(s % 3) + 1}.${s % 2 === 0 ? "49" : "99"}`),
      };
      if (s % 3 === 0) item.Tags = SS(["live", s % 2 === 0 ? "remaster" : "single"]);
      if (s % 4 === 0) item.Awards = M({ Grammy: { S: String(1990 + s) }, Count: { N: "1" } });
      if (a === 0 && s === 1) item.Plays = N("9007199254740993");
      out.push(item);
    }
  return out;
}

function users(): Item[] {
  const names = ["Ada", "Grace", "Linus", "Margaret", "Ken", "Barbara", "Dennis", "Frances"];
  return names.map((name, i) => {
    const item: Item = {
      userId: S(`u${String(i + 1).padStart(3, "0")}`),
      name: S(name),
      email: S(`${name.toLowerCase()}@example.com`),
      active: BOOL(i % 3 !== 0),
    };
    // A zip that must stay a string, and attributes only some items have.
    if (i % 2 === 0) item.zip = S(`0${2134 + i}`);
    if (i % 3 === 1) item.nickname = NULL;
    if (i === 2) item.scores = NS([10, 12.5, 7]);
    if (i === 3)
      item.profile = M({ city: { S: "Boston" }, langs: { L: [{ S: "en" }, { S: "fr" }] } });
    return item;
  });
}

function events(): Item[] {
  const out: Item[] = [];
  for (let d = 1; d <= 6; d++)
    for (let n = 0; n < 110; n++)
      out.push({
        deviceId: S(`device#${d}`),
        at: N(1_767_225_600 + n * 60),
        kind: S(n % 10 === 0 ? "alert" : "reading"),
        value: N(`${20 + (n % 7)}.${n % 10}`),
      });
  return out;
}

function seed(): Map<string, Table> {
  const tables: Table[] = [
    {
      name: "Music",
      keys: { hash: { name: "Artist", type: "S" }, range: { name: "SongTitle", type: "S" } },
      global: [
        {
          name: "ByGenre",
          keys: { hash: { name: "Genre", type: "S" }, range: { name: "Year", type: "N" } },
          projection: "INCLUDE",
          nonKey: ["AlbumTitle"],
        },
      ],
      local: [
        {
          name: "ByYear",
          keys: { hash: { name: "Artist", type: "S" }, range: { name: "Year", type: "N" } },
          projection: "KEYS_ONLY",
          nonKey: [],
        },
      ],
      billing: "PROVISIONED",
      stream: "NEW_AND_OLD_IMAGES",
      ttl: null,
      pitr: true,
      deletionProtection: true,
      itemCount: 0,
      items: music(),
    },
    {
      name: "Users",
      keys: { hash: { name: "userId", type: "S" }, range: null },
      global: [],
      local: [],
      billing: "PAY_PER_REQUEST",
      stream: null,
      ttl: null,
      pitr: false,
      deletionProtection: false,
      itemCount: 0,
      items: users(),
    },
    {
      name: "Events",
      keys: { hash: { name: "deviceId", type: "S" }, range: { name: "at", type: "N" } },
      global: [
        {
          name: "ByKind",
          keys: { hash: { name: "kind", type: "S" }, range: { name: "at", type: "N" } },
          projection: "ALL",
          nonKey: [],
        },
      ],
      local: [],
      billing: "PAY_PER_REQUEST",
      stream: null,
      ttl: "expiresAt",
      pitr: false,
      deletionProtection: false,
      itemCount: 0,
      items: events(),
    },
  ];
  for (const t of tables) t.itemCount = t.items.length;
  return new Map(tables.map((t) => [t.name, t]));
}

let current = seed();

/** The region's tables, shared by every mock session (as one AWS account's are). */
export const tables = (): Map<string, Table> => current;

/** Puts the seeded tables back (tests). */
export function resetTables(): void {
  current = seed();
}
