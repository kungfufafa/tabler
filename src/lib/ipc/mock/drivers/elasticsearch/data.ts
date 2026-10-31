/**
 * The mock's clusters (one per session kind): indices with their flattened mappings and
 * documents (`_source` as plain JSON, with `_seq_no` / `_primary_term` guards), aliases, a data
 * stream with its hidden backing index, and a system index, so the mock shows what the backend's
 * sidebar, grid and console show (`drivers/elasticsearch/catalog.rs`).
 */

/** A mapped field by its flattened path (`user.name`), as `catalog::flatten_mapping` gives. */
export interface Field {
  path: string;
  type: string;
  /** The keyword multi-field a text field sorts and matches by (`title.keyword`). */
  keyword?: string;
}

export interface Doc {
  id: string;
  seqNo: number;
  primaryTerm: number;
  source: Record<string, unknown>;
}

export interface Index {
  name: string;
  fields: Field[];
  docs: Doc[];
  /** Hidden, or backing a data stream: listed only with `showSystemIndices`. */
  hidden: boolean;
  status: "open" | "close";
  /** `index.max_result_window`. */
  window: number;
  nextSeqNo: number;
}

export interface Cluster {
  name: string;
  indices: Map<string, Index>;
  aliases: Map<string, string[]>;
  /** Data streams and their backing indices. */
  streams: Map<string, string[]>;
  /** Running tasks of `_tasks` (beyond the listing's own). */
  tasks: Task[];
}

export interface Task {
  id: string;
  action: string;
  description: string;
  startedAt: number;
  cancellable: boolean;
  opaqueId: string | null;
}

const LEVELS = ["info", "warn", "error", "debug"];
const USERS = ["ada", "linus", "grace", "ken", "barbara"];

function logs(name: string, count: number, from: Date): Index {
  const docs: Doc[] = Array.from({ length: count }, (_, i) => {
    const at = new Date(from.getTime() + i * 61_000);
    const source: Record<string, unknown> = {
      "@timestamp": at.toISOString(),
      level: LEVELS[i % LEVELS.length],
      message: `request ${i + 1} served`,
      status: [200, 200, 201, 404, 500][i % 5],
      user: { name: USERS[i % USERS.length], id: 1000 + (i % USERS.length) },
      latency_ms: Math.round((i * 37) % 900) / 10,
    };
    // Every seventh document has no tags: the grid shows the field missing, not NULL.
    if (i % 7 !== 6) source.tags = i % 2 === 0 ? ["web"] : ["web", "api"];
    return { id: `log-${String(i + 1).padStart(4, "0")}`, seqNo: i, primaryTerm: 1, source };
  });
  return {
    name,
    fields: [
      { path: "@timestamp", type: "date" },
      { path: "latency_ms", type: "float" },
      { path: "level", type: "keyword" },
      { path: "message", type: "text" },
      { path: "status", type: "long" },
      { path: "tags", type: "keyword" },
      { path: "user.id", type: "long" },
      { path: "user.name", type: "keyword" },
    ],
    docs,
    hidden: false,
    status: "open",
    window: 10_000,
    nextSeqNo: count,
  };
}

function products(): Index {
  const items: [string, string, number, boolean][] = [
    ["Mechanical keyboard", "KB-001", 89.9, true],
    ["Trackball mouse", "MS-014", 49.5, true],
    ["27-inch monitor", "MN-270", 279, false],
    ["USB-C hub", "HB-007", 34.99, true],
    ["Laptop stand", "ST-003", 51.25, true],
    ["Noise-cancelling headphones", "HP-220", 199, false],
    ["Webcam 4K", "WC-400", 129, true],
    ["Desk lamp", "LP-010", 24.5, true],
  ];
  return {
    name: "products",
    fields: [
      { path: "in_stock", type: "boolean" },
      { path: "name", type: "text", keyword: "name.keyword" },
      { path: "price", type: "double" },
      { path: "sku", type: "keyword" },
      // A big integer: past 2^53 it travels as a BigInt.
      { path: "barcode", type: "unsigned_long" },
    ],
    docs: items.map(([name, sku, price, stock], i) => ({
      id: String(i + 1),
      seqNo: i,
      primaryTerm: 1,
      source: {
        name,
        sku,
        price,
        in_stock: stock,
        barcode: BigInt("9007199254740993") + BigInt(i),
      },
    })),
    hidden: false,
    status: "open",
    window: 10_000,
    nextSeqNo: items.length,
  };
}

function metrics(): Index {
  const base = new Date("2026-10-31T00:00:00Z").getTime();
  return {
    name: ".ds-metrics-app-2026.10.31-000001",
    fields: [
      { path: "@timestamp", type: "date" },
      { path: "cpu", type: "float" },
      { path: "host.name", type: "keyword" },
    ],
    docs: Array.from({ length: 12 }, (_, i) => ({
      id: `m${i + 1}`,
      seqNo: i,
      primaryTerm: 1,
      source: {
        "@timestamp": new Date(base + i * 300_000).toISOString(),
        cpu: Math.round(((i * 13) % 100) * 10) / 1000,
        host: { name: `web-${(i % 3) + 1}` },
      },
    })),
    hidden: true,
    status: "open",
    window: 10_000,
    nextSeqNo: 12,
  };
}

function kibana(): Index {
  return {
    name: ".kibana_1",
    fields: [{ path: "type", type: "keyword" }],
    docs: [{ id: "config:9.1.0", seqNo: 0, primaryTerm: 1, source: { type: "config" } }],
    hidden: true,
    status: "open",
    window: 10_000,
    nextSeqNo: 1,
  };
}

/** A new cluster: what each session starts from (a session's edits stay in it). */
export function seedCluster(name: string): Cluster {
  const indices = [
    logs("logs-2026.10", 120, new Date("2026-10-01T08:00:00Z")),
    logs("logs-2026.09", 30, new Date("2026-09-01T08:00:00Z")),
    products(),
    metrics(),
    kibana(),
  ];
  return {
    name,
    indices: new Map(indices.map((i) => [i.name, i])),
    aliases: new Map([["logs", ["logs-2026.10", "logs-2026.09"]]]),
    streams: new Map([["metrics-app", [".ds-metrics-app-2026.10.31-000001"]]]),
    tasks: [
      {
        id: "oTUltX4IQMOUUVeiohTt8A:12345",
        action: "indices:data/write/reindex",
        description: "reindex from [logs-2026.09] to [logs-archive]",
        startedAt: Date.now() - 95_000,
        cancellable: true,
        opaqueId: null,
      },
      {
        id: "oTUltX4IQMOUUVeiohTt8A:12360",
        action: "indices:data/read/search",
        description: "indices[logs-*], search_type[QUERY_THEN_FETCH]",
        startedAt: Date.now() - 2_400,
        cancellable: true,
        opaqueId: "kibana",
      },
    ],
  };
}

/** The indices a target names: an index, an alias, a data stream or a pattern (`*`, lists). */
export function indicesOf(cluster: Cluster, target: string): Index[] {
  const out = new Map<string, Index>();
  for (const part of target.split(",")) {
    const names = cluster.aliases.get(part) ?? cluster.streams.get(part);
    if (names) for (const n of names) addIndex(out, cluster, n);
    else if (part.includes("*")) {
      const re = new RegExp(`^${part.split("*").map(escape).join(".*")}$`);
      for (const i of cluster.indices.values())
        if (re.test(i.name) && !i.name.startsWith(".")) out.set(i.name, i);
    } else addIndex(out, cluster, part);
  }
  return [...out.values()];
}

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

function addIndex(out: Map<string, Index>, cluster: Cluster, name: string) {
  const i = cluster.indices.get(name);
  if (i) out.set(name, i);
}

/** The fields of several indices merged (`catalog::merge`): a conflict becomes a JSON column. */
export function mergedFields(indices: readonly Index[]): Field[] {
  const by = new Map<string, Field>();
  for (const i of indices)
    for (const f of i.fields) {
      const seen = by.get(f.path);
      if (!seen) by.set(f.path, { ...f });
      else if (seen.type !== f.type && !seen.type.split("|").includes(f.type))
        by.set(f.path, { path: f.path, type: `${seen.type}|${f.type}` });
    }
  return [...by.values()].sort((a, b) => a.path.localeCompare(b.path));
}
