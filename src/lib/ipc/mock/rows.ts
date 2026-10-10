import type { ColumnMeta } from "../bindings/ColumnMeta";
import type { FilterGroup } from "../bindings/FilterGroup";
import type { FilterNode } from "../bindings/FilterNode";
import type { SortSpec } from "../bindings/SortSpec";
import type { Value } from "../bindings/Value";

/** Deterministic pseudo-random generator so mock data is stable across reloads. */
function rng(seed: number): () => number {
  let s = seed >>> 0 || 1;
  return () => {
    s ^= s << 13;
    s ^= s >>> 17;
    s ^= s << 5;
    return ((s >>> 0) % 100000) / 100000;
  };
}

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
];
const NAMES = [
  "Ayu",
  "Budi",
  "Citra",
  "Dewi",
  "Eko",
  "Fajar",
  "Gita",
  "Hadi",
  "Indah",
  "Joko",
  "Kartika",
  "Lina",
  "Made",
  "Nia",
  "Omar",
  "Putri",
  "Rizky",
  "Sari",
  "Tono",
  "Umar",
  "Vina",
  "Wahyu",
  "Yani",
  "Zaki",
];

function pad(n: number, w = 2) {
  return String(n).padStart(w, "0");
}

function dateAt(i: number, withTz: boolean, withTime = true): string {
  const d = new Date(Date.UTC(2024, 0, 1) + i * 3_600_000 * 7);
  const date = `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())}`;
  if (!withTime) return date;
  const time = `${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}:${pad(d.getUTCSeconds())}`;
  return withTz ? `${date} ${time}+07` : `${date} ${time}`;
}

export function cellFor(col: ColumnMeta, i: number, rand: () => number): Value {
  if (col.nullable && !col.isPk && rand() < 0.08) return { t: "null" };
  switch (col.kind) {
    case "int":
      return col.isPk ? { t: "int", v: i + 1 } : { t: "int", v: Math.floor(rand() * 1000) };
    case "bigInt":
      if (col.name === "big_id") {
        return i % 3 === 0
          ? { t: "bigInt", v: "9007199254740993" }
          : i % 3 === 1
            ? { t: "bigInt", v: "-9223372036854775808" }
            : { t: "int", v: i * 1000 };
      }
      return col.isPk ? { t: "int", v: i + 1 } : { t: "int", v: Math.floor(rand() * 100000) };
    case "decimal":
      return {
        t: "decimal",
        v: `${Math.floor(rand() * 100000)}.${pad(Math.floor(rand() * 1000000), 6)}`,
      };
    case "float":
      return { t: "float", v: Math.round(rand() * 1e6) / 1000 };
    case "bool":
      return { t: "bool", v: rand() < 0.7 };
    case "enum":
      return { t: "enum", v: col.enumValues?.[i % (col.enumValues.length || 1)] ?? "a" };
    case "uuid":
      return { t: "uuid", v: `00000000-0000-4000-8000-${pad(i + 1, 12)}` };
    case "json":
      return {
        t: "json",
        v: JSON.stringify({
          theme: i % 2 ? "dark" : "light",
          notifications: { email: rand() < 0.5, push: i % 3 === 0 },
          tags: [WORDS[i % WORDS.length]],
        }),
      };
    case "array":
      return {
        t: "array",
        v: [
          { t: "text", v: WORDS[i % WORDS.length]! },
          { t: "text", v: WORDS[(i * 7) % WORDS.length]! },
        ],
      };
    case "bytes":
      return {
        t: "bytes",
        v: {
          base64:
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
          len: 70,
          truncated: false,
        },
      };
    case "date":
      return { t: "date", v: dateAt(i, false, false) };
    case "time":
      return { t: "time", v: `${pad(i % 24)}:${pad((i * 7) % 60)}:00` };
    case "timestamp":
      return { t: "timestamp", v: dateAt(i, false) };
    case "timestampTz":
      return { t: "timestampTz", v: dateAt(i, true) };
    case "interval":
      return { t: "interval", v: `${i % 5} days 0${i % 10}:00:00` };
    case "other":
      return {
        t: "other",
        v: {
          ty: col.dataType,
          text:
            col.dataType === "inet"
              ? `10.0.${i % 255}.${(i * 3) % 255}`
              : col.dataType === "money"
                ? `$${(rand() * 1000).toFixed(2)}`
                : `(${i},${i * 2})`,
        },
      };
    default: {
      if (col.name === "email")
        return { t: "text", v: `${NAMES[i % NAMES.length]!.toLowerCase()}${i}@example.com` };
      if (col.name === "name") {
        if (i === 5) return { t: "text", v: "Ayu 🌸 Lestari" };
        if (i === 6) return { t: "text", v: "李小龙" };
        if (i === 7) return { t: "text", v: "" };
        return { t: "text", v: `${NAMES[i % NAMES.length]} ${NAMES[(i * 5) % NAMES.length]}` };
      }
      if (col.name === "kind") return { t: "text", v: ["view", "click", "buy", "refund"][i % 4]! };
      if (col.name === "bio" && i === 3) return { t: "text", v: WORDS.join(" ").repeat(100) };
      const n = 2 + Math.floor(rand() * 6);
      return {
        t: "text",
        v: Array.from({ length: n }, (_, k) => WORDS[(i + k * 3) % WORDS.length]).join(" "),
      };
    }
  }
}

const cache = new Map<string, Value[][]>();

/** All rows of a mock table (generated once, cached). */
export function rowsFor(key: string, columns: ColumnMeta[], count: number): Value[][] {
  const hit = cache.get(key);
  if (hit) return hit;
  const rand = rng(key.length * 7919 + count);
  const rows: Value[][] = new Array(count);
  for (let i = 0; i < count; i++) rows[i] = columns.map((c) => cellFor(c, i, rand));
  cache.set(key, rows);
  return rows;
}

function text(v: Value): string | null {
  switch (v.t) {
    case "null":
    case "default":
      return null;
    case "bytes":
      return v.v.base64;
    case "other":
      return v.v.text;
    case "array":
      return `{${v.v.map((x) => text(x) ?? "NULL").join(",")}}`;
    default:
      return String(v.v);
  }
}

function num(v: Value): number | null {
  if (v.t === "int" || v.t === "float") return v.v;
  if (v.t === "bigInt" || v.t === "decimal") return Number(v.v);
  if (v.t === "bool") return v.v ? 1 : 0;
  return null;
}

function compare(a: Value, b: Value): number {
  if (a.t === "null" && b.t === "null") return 0;
  if (a.t === "null") return -1;
  if (b.t === "null") return 1;
  const na = num(a);
  const nb = num(b);
  if (na !== null && nb !== null) return na - nb;
  return (text(a) ?? "").localeCompare(text(b) ?? "");
}

function likeToRegex(pattern: string): RegExp {
  const esc = pattern
    .replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
    .replace(/%/g, ".*")
    .replace(/_/g, ".");
  return new RegExp(`^${esc}$`, "i");
}

function matches(node: FilterNode, row: Value[], columns: ColumnMeta[]): boolean {
  if (node.kind === "raw") return true;
  if (node.kind === "group") return matchesGroup(node, row, columns);
  const idx = columns.findIndex((c) => c.name === node.column);
  if (idx < 0) return true;
  const cell = row[idx] ?? { t: "null" };
  const [v0, v1] = node.values;
  switch (node.op) {
    case "isNull":
      return cell.t === "null";
    case "isNotNull":
      return cell.t !== "null";
    case "eq":
      return v0 !== undefined && compare(cell, v0) === 0;
    case "ne":
      return v0 !== undefined && compare(cell, v0) !== 0;
    case "gt":
      return v0 !== undefined && compare(cell, v0) > 0;
    case "ge":
      return v0 !== undefined && compare(cell, v0) >= 0;
    case "lt":
      return v0 !== undefined && compare(cell, v0) < 0;
    case "le":
      return v0 !== undefined && compare(cell, v0) <= 0;
    case "like":
      return v0 !== undefined && likeToRegex(text(v0) ?? "").test(text(cell) ?? "");
    case "notLike":
      return v0 !== undefined && !likeToRegex(text(v0) ?? "").test(text(cell) ?? "");
    case "in":
      return node.values.some((v) => compare(cell, v) === 0);
    case "notIn":
      return !node.values.some((v) => compare(cell, v) === 0);
    case "between":
      return (
        v0 !== undefined && v1 !== undefined && compare(cell, v0) >= 0 && compare(cell, v1) <= 0
      );
  }
}

export function matchesGroup(group: FilterGroup, row: Value[], columns: ColumnMeta[]): boolean {
  if (group.items.length === 0) return true;
  return group.conjunction === "and"
    ? group.items.every((n) => matches(n, row, columns))
    : group.items.some((n) => matches(n, row, columns));
}

export function sortRows(rows: Value[][], sort: SortSpec[], columns: ColumnMeta[]): Value[][] {
  if (sort.length === 0) return rows;
  const specs = sort
    .map((s) => ({
      idx: columns.findIndex((c) => c.name === s.column),
      dir: s.dir === "desc" ? -1 : 1,
    }))
    .filter((s) => s.idx >= 0);
  return [...rows].sort((a, b) => {
    for (const s of specs) {
      const c = compare(a[s.idx] ?? { t: "null" }, b[s.idx] ?? { t: "null" }) * s.dir;
      if (c !== 0) return c;
    }
    return 0;
  });
}
