/**
 * The mock's Redis server: 16 databases of keys of every type the grid shows (strings, hashes,
 * lists, sets, sorted sets, streams, RedisJSON documents and a module's type it does not
 * preview), some with a TTL, plus the clients and the slow log. Every mock connection shares it,
 * so a read-only connection sees what another one wrote. TTLs do not run down (the mock stays
 * deterministic).
 */

export type StreamEntry = { id: string; fields: [string, string][] };

export type Data =
  | { type: "string"; value: string }
  | { type: "hash"; value: Map<string, string> }
  | { type: "list"; value: string[] }
  | { type: "set"; value: Set<string> }
  | { type: "zset"; value: Map<string, number> }
  | { type: "stream"; value: StreamEntry[] }
  | { type: "ReJSON-RL"; value: string }
  /** A module's type (RedisTimeSeries): shown, never previewed. */
  | { type: "TSDB-TYPE"; value: null };

/** A key: its value and the milliseconds until it expires (null: no expiry). */
export type Entry = Data & { pttl: number | null };

export interface Client {
  id: number;
  addr: string;
  name: string;
  user: string;
  db: number;
  cmd: string;
  idle: number;
  flags: string;
}

export interface SlowEntry {
  id: number;
  at: number;
  us: number;
  argv: string[];
  client: string;
  name: string;
}

export interface Server {
  product: string;
  version: string;
  mode: string;
  databases: number;
  dbs: Map<number, Map<string, Entry>>;
  clients: Client[];
  slowlog: SlowEntry[];
  nextClient: number;
}

const DATABASES = 16;

function seedDb0(): Map<string, Entry> {
  const db = new Map<string, Entry>();
  const put = (key: string, data: Data, pttl: number | null = null) =>
    db.set(key, { ...data, pttl } as Entry);
  const names = ["Ada", "Grace", "Linus", "Barbara", "Ken", "Margaret", "Dennis", "Frances"];
  for (let i = 1; i <= 40; i++) {
    const name = names[i % names.length]!;
    put(`user:${i}`, {
      type: "hash",
      value: new Map([
        ["name", `${name} ${i}`],
        ["email", `${name.toLowerCase()}${i}@example.com`],
        ["plan", i % 3 === 0 ? "pro" : "free"],
      ]),
    });
    if (i % 4 === 0)
      put(
        `session:${(i * 7919).toString(16)}`,
        { type: "string", value: `user:${i}` },
        1800_000 + i * 1000,
      );
  }
  put("greeting", { type: "string", value: "hello, world" });
  put("counter:visits", { type: "string", value: "4821" });
  put("config:feature-flags", { type: "string", value: '{"newCheckout":true,"beta":false}' });
  put("big:blob", { type: "string", value: "x".repeat(40_000) });
  put("queue:emails", {
    type: "list",
    value: Array.from({ length: 120 }, (_, i) => `email-job-${i + 1}`),
  });
  put("tags:popular", {
    type: "set",
    value: new Set(["redis", "valkey", "cache", "queue", "pubsub"]),
  });
  put("leaderboard", {
    type: "zset",
    value: new Map(names.map((n, i) => [n, 1000 - i * 75.5])),
  });
  put("events:orders", {
    type: "stream",
    value: Array.from({ length: 30 }, (_, i) => ({
      id: `1730000000${String(i).padStart(3, "0")}-0`,
      fields: [
        ["order", `o${i + 1}`],
        ["status", i % 3 === 0 ? "paid" : "new"],
      ] as [string, string][],
    })),
  });
  put("doc:settings", {
    type: "ReJSON-RL",
    value: '{"theme":"dark","fontSize":13,"recent":["a","b"]}',
  });
  put("ts:cpu", { type: "TSDB-TYPE", value: null });
  put("my key with spaces", { type: "string", value: 'quotes " and ; semicolons' });
  return db;
}

function seedDb1(): Map<string, Entry> {
  const db = new Map<string, Entry>();
  for (let i = 1; i <= 5; i++)
    db.set(`cache:page:${i}`, {
      type: "string",
      value: `<html>page ${i}</html>`,
      pttl: 60_000 * i,
    });
  return db;
}

export function newServer(): Server {
  const dbs = new Map<number, Map<string, Entry>>();
  for (let i = 0; i < DATABASES; i++) dbs.set(i, new Map());
  dbs.set(0, seedDb0());
  dbs.set(1, seedDb1());
  return {
    product: "Redis",
    version: "7.4.1",
    mode: "standalone",
    databases: DATABASES,
    dbs,
    clients: [
      {
        id: 3,
        addr: "10.0.0.12:51234",
        name: "worker",
        user: "default",
        db: 0,
        cmd: "brpop",
        idle: 2,
        flags: "b",
      },
      {
        id: 4,
        addr: "10.0.0.13:40110",
        name: "",
        user: "app",
        db: 1,
        cmd: "get",
        idle: 41,
        flags: "N",
      },
    ],
    slowlog: [
      {
        id: 2,
        at: 1730000500,
        us: 18_250,
        argv: ["KEYS", "*"],
        client: "10.0.0.13:40110",
        name: "",
      },
      {
        id: 1,
        at: 1730000100,
        us: 11_900,
        argv: ["HGETALL", "user:7"],
        client: "10.0.0.12:51234",
        name: "worker",
      },
    ],
    nextClient: 10,
  };
}

let shared: Server | null = null;

/** The server every mock connection shares. */
export function server(): Server {
  shared ??= newServer();
  return shared;
}

/** A fresh server (tests). */
export function resetServer(): void {
  shared = null;
}

/** The byte length of a string as Redis counts it (UTF-8). */
export const byteLength = (s: string): number => new TextEncoder().encode(s).length;

/** The number of elements of a container, the bytes of a string; null for JSON and modules. */
export function sizeOf(e: Data): number | null {
  switch (e.type) {
    case "string":
      return byteLength(e.value);
    case "hash":
    case "zset":
      return e.value.size;
    case "set":
      return e.value.size;
    case "list":
    case "stream":
      return e.value.length;
    default:
      return null;
  }
}

/** A rough `MEMORY USAGE`. */
export function memoryOf(key: string, e: Data): number {
  const body =
    e.type === "string" || e.type === "ReJSON-RL"
      ? byteLength(e.value)
      : e.type === "TSDB-TYPE"
        ? 4096
        : (sizeOf(e) ?? 0) * 24;
  return 56 + byteLength(key) + body;
}

/** A sorted set's members by score, then member (ZRANGE's order). */
export function zsetOrder(z: Map<string, number>): [string, number][] {
  return [...z].sort((a, b) => a[1] - b[1] || (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
}

/**
 * Redis's glob (`stringmatchlen`): `*`, `?`, `[abc]`, `[^a]`, `[a-z]`, `\x` literal. The filter
 * bar's case-insensitive forms arrive as `[aA]` classes.
 */
export function globMatch(pattern: string, text: string): boolean {
  const p = Array.from(pattern);
  const s = Array.from(text);
  const go = (pi: number, si: number): boolean => {
    while (pi < p.length) {
      const c = p[pi]!;
      if (c === "*") {
        while (p[pi + 1] === "*") pi++;
        if (pi + 1 === p.length) return true;
        for (let k = si; k <= s.length; k++) if (go(pi + 1, k)) return true;
        return false;
      }
      if (si >= s.length) return false;
      if (c === "?") {
        pi++;
        si++;
        continue;
      }
      if (c === "[") {
        let j = pi + 1;
        const not = p[j] === "^";
        if (not) j++;
        let hit = false;
        while (j < p.length && p[j] !== "]") {
          if (p[j] === "\\" && j + 1 < p.length) {
            j++;
            if (p[j] === s[si]) hit = true;
          } else if (p[j + 1] === "-" && j + 2 < p.length && p[j + 2] !== "]") {
            let lo = p[j]!;
            let hi = p[j + 2]!;
            if (lo > hi) [lo, hi] = [hi, lo];
            if (s[si]! >= lo && s[si]! <= hi) hit = true;
            j += 2;
          } else if (p[j] === s[si]) hit = true;
          j++;
        }
        if (hit === not) return false;
        pi = j + 1;
        si++;
        continue;
      }
      const lit = c === "\\" && pi + 1 < p.length ? p[++pi]! : c;
      if (lit !== s[si]) return false;
      pi++;
      si++;
    }
    return si === s.length;
  };
  return go(0, 0);
}
