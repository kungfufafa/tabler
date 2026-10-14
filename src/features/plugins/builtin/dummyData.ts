import type { ColumnMeta, Dialect, TableMeta, Value } from "@/lib/ipc";
import { literal, qualify, quoteIdent } from "@/lib/sql/quote";
import type { BuiltinPlugin } from "../types";
import { uuidV4 } from "./uuid";

/** A random number in [0, 1). */
export type Rng = () => number;

/** A small seeded generator (mulberry32), so tests and previews are reproducible. */
export function seededRng(seed: number): Rng {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const FIRST = [
  "Ada",
  "Alan",
  "Grace",
  "Linus",
  "Margaret",
  "Dennis",
  "Barbara",
  "Ken",
  "Edsger",
  "Frances",
  "Tim",
  "Radia",
];
const LAST = [
  "Lovelace",
  "Turing",
  "Hopper",
  "Torvalds",
  "Hamilton",
  "Ritchie",
  "Liskov",
  "Thompson",
  "Dijkstra",
  "Allen",
  "Berners-Lee",
  "Perlman",
];
const CITIES = [
  "Jakarta",
  "Bandung",
  "Surabaya",
  "Lisbon",
  "Oslo",
  "Kyoto",
  "Austin",
  "Nairobi",
  "Lima",
  "Tallinn",
];
const COUNTRIES = [
  "Indonesia",
  "Portugal",
  "Norway",
  "Japan",
  "United States",
  "Kenya",
  "Peru",
  "Estonia",
];
const WORDS = [
  "lorem",
  "ipsum",
  "dolor",
  "sit",
  "amet",
  "consectetur",
  "adipiscing",
  "elit",
  "sed",
  "do",
  "eiusmod",
  "tempor",
  "incididunt",
  "labore",
  "magna",
  "aliqua",
];
const STATUSES = ["active", "pending", "archived", "draft"];

const pick = <T>(rng: Rng, list: readonly T[]): T => list[Math.floor(rng() * list.length)]!;
const int = (rng: Rng, min: number, max: number) => min + Math.floor(rng() * (max - min + 1));
const pad = (n: number, w = 2) => String(n).padStart(w, "0");

function words(rng: Rng, n: number): string {
  return Array.from({ length: n }, () => pick(rng, WORDS)).join(" ");
}

function dateParts(rng: Rng) {
  // Within the last three years.
  const t = Date.now() - Math.floor(rng() * 3 * 365 * 24 * 3600 * 1000);
  const d = new Date(t);
  return {
    date: `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())}`,
    time: `${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}:${pad(d.getUTCSeconds())}`,
  };
}

/** Text that fits the column, guessed from its name (email, name, city, phone, url…). */
export function dummyText(name: string, row: number, rng: Rng, maxLen: number | null): string {
  const n = name.toLowerCase();
  const first = pick(rng, FIRST);
  const last = pick(rng, LAST);
  let text: string;
  if (n.includes("email")) text = `${first}.${last}${row + 1}@example.com`.toLowerCase();
  else if (n.includes("first")) text = first;
  else if (n.includes("last") || n.includes("surname")) text = last;
  else if (n.includes("user") || n === "login" || n.includes("handle"))
    text = `${first}${int(rng, 1, 999)}`.toLowerCase();
  else if (n.includes("name")) text = `${first} ${last}`;
  else if (n.includes("city")) text = pick(rng, CITIES);
  else if (n.includes("country")) text = pick(rng, COUNTRIES);
  else if (n.includes("phone") || n.includes("mobile"))
    text = `+1-555-${pad(int(rng, 0, 9999), 4)}`;
  else if (n.includes("url") || n.includes("website") || n.includes("link"))
    text = `https://example.com/${pick(rng, WORDS)}/${row + 1}`;
  else if (n.includes("status") || n.includes("state")) text = pick(rng, STATUSES);
  else if (n.includes("slug")) text = `${pick(rng, WORDS)}-${pick(rng, WORDS)}-${row + 1}`;
  else if (n.includes("title") || n.includes("subject")) text = words(rng, int(rng, 2, 5));
  else if (n.includes("desc") || n.includes("body") || n.includes("content") || n.includes("note"))
    text = `${words(rng, int(rng, 6, 14))}.`;
  else text = words(rng, int(rng, 1, 3));
  if (maxLen !== null && maxLen > 0 && text.length > maxLen) text = text.slice(0, maxLen);
  return text;
}

function decimalValue(col: ColumnMeta, rng: Rng): string {
  const scale = Math.max(0, Math.min(col.numericScale ?? 2, 6));
  const digits = col.numericPrecision !== null ? Math.max(1, col.numericPrecision - scale) : 5;
  const max = Math.min(10 ** Math.min(digits, 6) - 1, 99_999);
  const whole = int(rng, 0, max);
  if (scale === 0) return String(whole);
  return `${whole}.${pad(int(rng, 0, 10 ** scale - 1), scale)}`;
}

function intRange(col: ColumnMeta): [number, number] {
  const t = col.dataType.toLowerCase();
  if (t.startsWith("tinyint")) return [col.isUnsigned ? 0 : 1, 127];
  if (t.startsWith("smallint") || t.startsWith("int2")) return [1, 1_000];
  return [1, 10_000];
}

/** Columns the server fills itself: generated, auto-increment, or keys with a default. */
export function skipsColumn(col: ColumnMeta): boolean {
  return col.isGenerated || col.isAutoIncrement || (col.isPk && col.defaultExpr !== null);
}

/**
 * A fake value for `col` in row `row`, by its kind (text / int / decimal / bool / date / uuid…),
 * or null when Tabler has nothing sensible for it (bytes, geometry…): the column is then left to
 * its default, or NULL. `fk`: a foreign-key column (NULL when nullable, else a small id).
 */
export function dummyValue(col: ColumnMeta, row: number, rng: Rng, fk = false): Value | null {
  if (fk) return col.nullable ? { t: "null" } : { t: "int", v: int(rng, 1, 10) };
  switch (col.kind) {
    case "int": {
      const [min, max] = intRange(col);
      return { t: "int", v: col.isPk ? row + 1 + int(rng, 0, 1_000_000) : int(rng, min, max) };
    }
    case "bigInt":
      return { t: "bigInt", v: String(int(rng, 1, 1_000_000)) };
    case "decimal":
      return { t: "decimal", v: decimalValue(col, rng) };
    case "float":
      return { t: "float", v: Math.round(rng() * 100_000) / 100 };
    case "bool":
      return { t: "bool", v: rng() < 0.5 };
    case "text":
      return { t: "text", v: dummyText(col.name, row, rng, col.charMaxLen) };
    case "enum":
    case "set":
      return col.enumValues && col.enumValues.length > 0
        ? { t: "enum", v: pick(rng, col.enumValues) }
        : null;
    case "uuid": {
      const bytes = Uint8Array.from({ length: 16 }, () => Math.floor(rng() * 256));
      return { t: "uuid", v: uuidV4(bytes) };
    }
    case "date":
      return { t: "date", v: dateParts(rng).date };
    case "time":
      return { t: "time", v: dateParts(rng).time };
    case "timestamp":
    case "timestampTz": {
      const p = dateParts(rng);
      return { t: col.kind, v: `${p.date} ${p.time}` };
    }
    case "json":
      return { t: "json", v: JSON.stringify({ id: row + 1, tag: pick(rng, WORDS) }) };
    default:
      return null;
  }
}

/** Rows per INSERT statement. */
export const ROWS_PER_INSERT = 100;
export const MAX_DUMMY_ROWS = 1_000;

/** The INSERTs that add `count` fake rows to `meta`'s table (one statement per 100 rows). */
export function dummyInserts(
  dialect: Dialect,
  meta: Pick<TableMeta, "table" | "columns" | "foreignKeys">,
  count: number,
  rng: Rng,
): string[] {
  const fkCols = new Set(meta.foreignKeys.flatMap((f) => f.columns));
  const probe = meta.columns.filter((c) => !skipsColumn(c));
  // Columns Tabler can fill (decided once, from a sample value); the others keep their default.
  const cols = probe.filter((c) => dummyValue(c, 0, seededRng(1), fkCols.has(c.name)) !== null);
  const target = qualify(dialect, meta.table.schema, meta.table.name);
  const n = Math.max(1, Math.min(Math.floor(count), MAX_DUMMY_ROWS));
  if (cols.length === 0) {
    const empty =
      dialect === "mysql"
        ? `INSERT INTO ${target} () VALUES ()`
        : `INSERT INTO ${target} DEFAULT VALUES`;
    return Array.from({ length: n }, () => `${empty};`);
  }
  const head = `INSERT INTO ${target} (${cols.map((c) => quoteIdent(dialect, c.name)).join(", ")}) VALUES`;
  const out: string[] = [];
  for (let start = 0; start < n; start += ROWS_PER_INSERT) {
    const rows: string[] = [];
    for (let r = start; r < Math.min(n, start + ROWS_PER_INSERT); r++) {
      const values = cols.map((c) => {
        const v = dummyValue(c, r, rng, fkCols.has(c.name)) ?? { t: "null" };
        return literal(dialect, v);
      });
      rows.push(`  (${values.join(", ")})`);
    }
    out.push(`${head}\n${rows.join(",\n")};`);
  }
  return out;
}

export const DUMMY_DATA: BuiltinPlugin = {
  identifier: "tabler.dummy-data",
  name: "Dummy Data",
  version: "1.0",
  author: "Tabler",
  description:
    "Inserts fake rows into a table: names, emails, numbers, dates, UUIDs… by column type. Previews the INSERTs first.",
  commands: [
    {
      name: "Generate Dummy Data…",
      contexts: ["sidebar", "global"],
      run: async (inv, host) => {
        if (!inv.connectionId || !inv.item) {
          host.notify("Dummy Data", "Select a table in the sidebar, or open one, first.");
          return;
        }
        if (inv.item.type !== "table" && inv.item.type !== "partitioned") {
          host.notify("Dummy Data", `${inv.item.name} is not a table.`);
          return;
        }
        const { openDummyData } = await import("../store");
        openDummyData({ connectionId: inv.connectionId, table: inv.item });
      },
    },
  ],
};
