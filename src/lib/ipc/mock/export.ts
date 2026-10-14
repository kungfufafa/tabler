// Mock handlers for export_data, export_tables, export_results, export_database_tables, cancel_export and
// accept_host_key, plus the SSH host-key check the mock `connect` / `test_connection` run for
// profiles with an SSH tunnel.
import type { ConnectionInfo } from "../bindings/ConnectionInfo";
import type { Driver } from "../bindings/Driver";
import type { ExportFormat } from "../bindings/ExportFormat";
import type { ExportProgress } from "../bindings/ExportProgress";
import type { ExportRequest } from "../bindings/ExportRequest";
import type { ExportResult } from "../bindings/ExportResult";
import type { ExportResultsRequest } from "../bindings/ExportResultsRequest";
import type { ExportResultsResult } from "../bindings/ExportResultsResult";
import type { ExportTablesProgress } from "../bindings/ExportTablesProgress";
import type { ExportTablesRequest } from "../bindings/ExportTablesRequest";
import type { ExportTablesResult } from "../bindings/ExportTablesResult";
import type { SchemaInfo } from "../bindings/SchemaInfo";
import type { SshConfig } from "../bindings/SshConfig";
import type { TableSummary } from "../bindings/TableSummary";
import type { Value } from "../bindings/Value";
import { tableMetaFor } from "./fixtures";
import { matchesGroup, rowsFor } from "./rows";
import { mockError, mockInvoke, mockLatency, registerMock } from "./transport";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// --- SSH host keys ---------------------------------------------------------------------------

/** Stable fingerprints for the seeded bastions; other hosts get one derived from their name. */
const FINGERPRINTS: Record<string, string> = {
  "bastion.example.com": "SHA256:2h0YqLx8vT3mKc9RfW1nZp5sJdA7uE4gHbN6yQoXkIs",
  "rotated.example.com": "SHA256:Qm8VtR2xN5cLw0pZ7aJ3hK9sFgD1yE6uB4oT8iXnMvc",
};

const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

export function mockFingerprint(host: string): string {
  const known = FINGERPRINTS[host.trim().toLowerCase()];
  if (known) return known;
  let h = 2166136261;
  for (const ch of host) h = Math.imul(h ^ ch.charCodeAt(0), 16777619) >>> 0;
  let out = "";
  for (let i = 0; i < 43; i++) {
    h = Math.imul(h ^ (h >>> 13), 0x5bd1e995) >>> 0;
    out += B64[h % 64];
  }
  return `SHA256:${out}`;
}

interface HostKey {
  host: string;
  port: number;
  keyType: string;
  fingerprint: string;
}

/** Keys trusted in the app's own store (`host:port` → fingerprint), like `trust_host_key`. */
const trusted = new Map<string, string>();
/** Entries "written" to ~/.ssh/known_hosts. */
const knownHosts = new Map<string, string>();
/** Keys waiting for confirmation, by profile id (the backend's `pending_host_keys`). */
const pending = new Map<string, HostKey>();

const hostPort = (k: { host: string; port: number }) => `${k.host.trim().toLowerCase()}:${k.port}`;

function hostKeyError(key: HostKey, changed: boolean): never {
  throw {
    code: changed ? "SSH_HOST_KEY_CHANGED" : "SSH_HOST_KEY_UNKNOWN",
    message: changed
      ? `the host key for ${key.host}:${key.port} has changed`
      : `unknown host key for ${key.host}:${key.port}`,
    detail: null,
    hint: null,
    sqlstate: null,
    vendorCode: null,
    position: null,
    statementIndex: null,
    data: { ...key, changed },
  };
}

/**
 * What the backend's tunnel does before it authenticates: verifies the server key. Hosts whose name
 * contains "rotated" always present a key that differs from the one on record.
 */
export function mockCheckHostKey(profileId: string, ssh: SshConfig | null): void {
  if (!ssh) return;
  const key: HostKey = {
    host: ssh.host,
    port: ssh.port,
    keyType: "ssh-ed25519",
    fingerprint: mockFingerprint(ssh.host),
  };
  if (ssh.host.includes("rotated")) hostKeyError(key, true);
  const at = hostPort(key);
  if (trusted.get(at) === key.fingerprint || knownHosts.get(at) === key.fingerprint) return;
  pending.set(profileId, key);
  hostKeyError(key, false);
}

registerMock("accept_host_key", ({ profileId, fingerprint, persistToKnownHosts }) => {
  const key = pending.get(profileId as string);
  if (!key) mockError("NOT_FOUND", "no SSH host key is waiting for confirmation; connect again");
  const fp = String(fingerprint ?? "").trim();
  if (key.fingerprint !== fp)
    mockError(
      "INVALID_INPUT",
      `fingerprint ${fp} does not match the server's key ${key.fingerprint}; connect again`,
    );
  if (persistToKnownHosts === true) knownHosts.set(hostPort(key), key.fingerprint);
  trusted.set(hostPort(key), key.fingerprint);
  pending.delete(profileId as string);
  return null;
});

/** Test helper: forget every trusted / pending key. */
export function resetMockHostKeys(): void {
  trusted.clear();
  knownHosts.clear();
  pending.clear();
}

/** Test helper: the `known_hosts` entries written so far. */
export function mockKnownHosts(): ReadonlyMap<string, string> {
  return knownHosts;
}

// --- export ----------------------------------------------------------------------------------

const running = new Set<string>();
const cancelled = new Set<string>();

/** A binary value cut to its preview (also inside arrays). */
const capped = (v: Value): boolean =>
  (v.t === "bytes" && v.v.truncated) || (v.t === "array" && v.v.some(capped));

/** Rows held by the renderer: written as they are (what the backend checks first). */
function heldRows(
  driver: Driver,
  source: Extract<ExportRequest["source"], { kind: "rows" }>,
): number {
  if (source.columns.length === 0)
    mockError("INVALID_INPUT", "the result to export has no columns");
  source.rows.forEach((row, i) => {
    if (row.length !== source.columns.length)
      mockError(
        "INVALID_INPUT",
        `row ${i + 1} of the result has ${row.length} values for ${source.columns.length} columns`,
      );
    // A PostgreSQL query export reads bytea in full; MySQL / SQLite only through the table.
    if (row.some(capped))
      mockError(
        "INVALID_INPUT",
        `a binary value in this result was truncated to 64 KiB; export ${driver === "postgres" ? "the query" : "its table"} instead`,
      );
  });
  return source.rows.length;
}

function validateFormat(format: ExportFormat): void {
  if (format.format === "csv") {
    const d = format.delimiter;
    if (d.length !== 1 || d.charCodeAt(0) > 127 || ['"', "'", "\r", "\n"].includes(d))
      mockError(
        "INVALID_INPUT",
        "the CSV delimiter must be a single ASCII character other than a quote or a line break",
      );
    if (!["\n", "\r\n", "\r"].includes(format.lineBreak))
      mockError("INVALID_INPUT", "the CSV line break must be \\n, \\r\\n or \\r");
    if (format.quote === "space" && d === " ")
      mockError(
        "INVALID_INPUT",
        "the CSV delimiter cannot be a space when values are quoted with spaces",
      );
    // One printable ASCII character that is no digit or quote; not the delimiter, but `,`.
    const dec = format.decimal;
    if (
      dec.length !== 1 ||
      dec.charCodeAt(0) < 0x20 ||
      dec.charCodeAt(0) > 0x7e ||
      /[0-9"']/.test(dec) ||
      (dec === d && dec !== ",")
    )
      mockError(
        "INVALID_INPUT",
        "the CSV decimal separator must be a single ASCII character other than a digit, the delimiter, a quote or a line break",
      );
  }
  if (format.format === "sqlInsert") {
    if (format.batchSize < 1)
      mockError("INVALID_INPUT", "the INSERT batch size must be at least 1");
    if (!format.content && !format.structure && !format.dropTable)
      mockError("INVALID_INPUT", "nothing to export: include the table structure or its content");
    // No table name and no source table: the INSERTs go into `untitled` (as the backend does).
  }
}

function tableRows(driver: Driver, req: ExportRequest): number {
  if (req.source.kind !== "table") return 0;
  const { table, filter, columns } = req.source;
  const schema = table.schema ?? "public";
  const meta =
    tableMetaFor(driver, schema, table.name) ?? mockError("NOT_FOUND", `table ${table.name}`);
  if (columns?.length === 0) mockError("INVALID_INPUT", "no columns are selected for the export");
  for (const name of columns ?? []) {
    if (!meta.columns.some((c) => c.name === name))
      mockError("INVALID_INPUT", `unknown column "${name}"`);
  }
  const total = meta.rowEstimate ?? 25;
  const matching = filter
    ? rowsFor(`${driver}:${schema}.${table.name}`, meta.columns, total).filter((r) =>
        matchesGroup(filter, r, meta.columns),
      ).length
    : total;
  // A page: the rows after the offset.
  return Math.max(0, matching - (req.source.offset ?? 0));
}

function sqlRows(driver: Driver, sql: string): number {
  const text = sql.trim().replace(/;\s*$/, "");
  if (text === "") mockError("INVALID_INPUT", "there is no SQL statement to export");
  const statements = text.split(/;\s*\n?/).filter((s) => s.trim() !== "");
  if (statements.length > 1)
    mockError(
      "INVALID_INPUT",
      `an export runs a single statement, but the SQL contains ${statements.length}`,
    );
  if (
    !/^\s*(select|with|values|table|show|explain|pragma)\b/i.test(text) &&
    !/returning/i.test(text)
  )
    mockError("INVALID_INPUT", "the statement returned no result set to export");
  const from = /\bfrom\s+(?:"?(\w+)"?\.)?"?([\w ]+?)"?(?:\s|$|;)/i.exec(text);
  const meta = from ? tableMetaFor(driver, from[1] ?? "public", from[2] ?? "") : null;
  const available = meta ? (meta.rowEstimate ?? 25) : 1;
  const limit = /\blimit\s+(\d+)/i.exec(text);
  return limit ? Math.min(available, Number(limit[1])) : available;
}

/** Rough size of one written row, so the progress shows plausible megabytes. */
function bytesPerRow(format: ExportFormat): number {
  if (format.format === "csv") return format.quote === "ifNeeded" ? 84 : 96;
  if (format.format === "json") return format.pretty && !format.ndjson ? 210 : 140;
  if (format.format === "xlsx") return 48;
  if (format.format === "html") return 150;
  return format.gzip ? 30 : 120;
}

/** Characters a single-byte encoding cannot hold in the generated rows (one per ~40 rows). */
function replacedFor(encoding: string | undefined, rows: number): { replaced?: number } {
  return encoding === "windows1252" || encoding === "latin1"
    ? rows >= 40
      ? { replaced: Math.floor(rows / 40) }
      : {}
    : {};
}

registerMock("export_data", async ({ connectionId, req, onProgress }) => {
  const r = req as ExportRequest;
  const channel = onProgress as { send?: (p: ExportProgress) => void } | undefined;
  if (!r.exportId?.trim()) mockError("INVALID_INPUT", "exportId is required");
  if (running.has(r.exportId))
    mockError("INVALID_INPUT", `export "${r.exportId}" is already running`);
  if (!r.path.startsWith("/")) mockError("INVALID_INPUT", "the export path must be absolute");
  const conns = await mockInvoke<ConnectionInfo[]>("list_connections", {});
  const conn =
    conns.find((c) => c.connectionId === connectionId) ??
    mockError("NOT_CONNECTED", "not connected");
  validateFormat(r.format);
  let total =
    r.source.kind === "table"
      ? tableRows(conn.driver, r)
      : r.source.kind === "rows"
        ? heldRows(conn.driver, r.source)
        : sqlRows(conn.driver, r.source.sql);
  if (r.maxRows !== null) total = Math.min(total, r.maxRows);

  const perRow = bytesPerRow(r.format);
  const head = r.format.format === "csv" && r.format.header ? 64 : 2;
  const big = total > 5000;
  // ~20 chunks over ~2 s for big tables; a few quick ones otherwise. Tests run without latency.
  const chunks = big ? 20 : Math.max(1, Math.min(4, Math.ceil(total / 50)));
  const delay = mockLatency === 0 ? 5 : big ? 100 : 40;
  const started = performance.now();
  running.add(r.exportId);
  try {
    for (let i = 1; i <= chunks; i++) {
      await sleep(delay);
      if (cancelled.has(r.exportId)) mockError("CANCELLED", "export cancelled");
      const rows = Math.round((total * i) / chunks);
      channel?.send?.({ rows, bytes: head + rows * perRow });
    }
    return {
      path: r.path,
      rows: total,
      bytes: head + total * perRow,
      durationMs: Math.max(1, Math.round(performance.now() - started)),
      ...replacedFor(r.encoding, total),
    } satisfies ExportResult;
  } finally {
    running.delete(r.exportId);
    cancelled.delete(r.exportId);
  }
});

registerMock("cancel_export", ({ exportId }) => {
  if (running.has(exportId as string)) cancelled.add(exportId as string);
  return null;
});

// --- multi-table export ----------------------------------------------------------------------

function fileExt(format: ExportFormat): string {
  switch (format.format) {
    case "csv":
      return format.delimiter === "\t" ? "tsv" : "csv";
    case "json":
      return format.ndjson ? "ndjson" : "json";
    case "sqlInsert":
      return format.gzip ? "sql.gz" : "sql";
    case "xlsx":
      return "xlsx";
    case "html":
      return "html";
  }
}

registerMock("export_tables", async ({ connectionId, req, onProgress }) => {
  const r = req as ExportTablesRequest;
  const channel = onProgress as { send?: (p: ExportTablesProgress) => void } | undefined;
  if (!r.exportId?.trim()) mockError("INVALID_INPUT", "exportId is required");
  if (running.has(r.exportId))
    mockError("INVALID_INPUT", `export "${r.exportId}" is already running`);
  if (r.tables.length === 0) mockError("INVALID_INPUT", "no tables to export");
  if (!r.target.startsWith("/")) mockError("INVALID_INPUT", "the export target must be absolute");
  if (r.layout === "singleFile" && (r.format.format === "csv" || r.format.format === "json"))
    mockError("INVALID_INPUT", "CSV and JSON exports write one file per table");
  if (r.target.includes("..")) mockError("INVALID_INPUT", "the export folder cannot contain ..");
  const conns = await mockInvoke<ConnectionInfo[]>("list_connections", {});
  const conn =
    conns.find((c) => c.connectionId === connectionId) ??
    mockError("NOT_CONNECTED", "not connected");
  validateFormat(r.format);
  const counts = r.tables.map(
    (t) => tableMetaFor(conn.driver, t.schema ?? "public", t.name)?.rowEstimate ?? 25,
  );
  const perRow = bytesPerRow(r.format);
  const content = r.format.format !== "sqlInsert" || r.format.content;
  const delay = mockLatency === 0 ? 2 : 60;
  const started = performance.now();
  let rows = 0;
  running.add(r.exportId);
  try {
    const tables = [];
    for (const [i, table] of r.tables.entries()) {
      const total = content ? (counts[i] ?? 0) : 0;
      for (let step = 1; step <= 3; step++) {
        await sleep(delay);
        if (cancelled.has(r.exportId)) mockError("CANCELLED", "export cancelled");
        const done = Math.round((total * step) / 3);
        channel?.send?.({
          tableIndex: i,
          tableCount: r.tables.length,
          table: table.name,
          tableRows: done,
          rows: rows + done,
          bytes: (rows + done) * perRow,
        });
      }
      rows += total;
      const path =
        r.layout === "singleFile" ? r.target : `${r.target}/${table.name}.${fileExt(r.format)}`;
      tables.push({ table, path, rows: total });
    }
    return {
      tables,
      paths: r.layout === "singleFile" ? [r.target] : tables.map((t) => t.path),
      rows,
      bytes: rows * perRow,
      durationMs: Math.max(1, Math.round(performance.now() - started)),
      ...replacedFor(r.encoding, rows),
    } satisfies ExportTablesResult;
  } finally {
    running.delete(r.exportId);
    cancelled.delete(r.exportId);
  }
});

// --- all results of a run --------------------------------------------------------------------

registerMock("export_results", async ({ connectionId, req, onProgress }) => {
  const r = req as ExportResultsRequest;
  const channel = onProgress as { send?: (p: ExportTablesProgress) => void } | undefined;
  if (!r.exportId?.trim()) mockError("INVALID_INPUT", "exportId is required");
  if (running.has(r.exportId))
    mockError("INVALID_INPUT", `export "${r.exportId}" is already running`);
  if (r.results.length === 0) mockError("INVALID_INPUT", "no results to export");
  if (!r.target.startsWith("/")) mockError("INVALID_INPUT", "the export target must be absolute");
  if (r.layout === "singleFile" && (r.format.format === "csv" || r.format.format === "json"))
    mockError("INVALID_INPUT", "CSV and JSON exports write one file per result");
  if (r.format.format === "sqlInsert" && !r.format.content)
    mockError("INVALID_INPUT", "a result has no table structure: include its content");
  const conns = await mockInvoke<ConnectionInfo[]>("list_connections", {});
  const conn =
    conns.find((c) => c.connectionId === connectionId) ??
    mockError("NOT_CONNECTED", "not connected");
  validateFormat(r.format);
  for (const item of r.results) {
    try {
      heldRows(conn.driver, {
        kind: "rows",
        columns: item.columns,
        rows: item.rows,
        table: item.table,
      });
    } catch (e) {
      const message = (e as { message?: string }).message ?? String(e);
      mockError("INVALID_INPUT", `${item.label}: ${message}`);
    }
  }
  const perRow = bytesPerRow(r.format);
  const delay = mockLatency === 0 ? 2 : 60;
  const started = performance.now();
  const stem = (r.name?.trim() || "result").replace(/[/\\:*?"<>|]/g, "_");
  let rows = 0;
  running.add(r.exportId);
  try {
    const results = [];
    for (const [i, item] of r.results.entries()) {
      await sleep(delay);
      if (cancelled.has(r.exportId)) mockError("CANCELLED", "export cancelled");
      rows += item.rows.length;
      channel?.send?.({
        tableIndex: i,
        tableCount: r.results.length,
        table: item.label,
        tableRows: item.rows.length,
        rows,
        bytes: rows * perRow,
      });
      const path =
        r.layout === "singleFile" ? r.target : `${r.target}/${stem}-${i + 1}.${fileExt(r.format)}`;
      results.push({ label: item.label, path, rows: item.rows.length });
    }
    return {
      results,
      paths: r.layout === "singleFile" ? [r.target] : results.map((x) => x.path),
      rows,
      bytes: rows * perRow,
      durationMs: Math.max(1, Math.round(performance.now() - started)),
      ...replacedFor(r.encoding, rows),
    } satisfies ExportResultsResult;
  } finally {
    running.delete(r.exportId);
    cancelled.delete(r.exportId);
  }
});

// --- multi-database export ------------------------------------------------------------------

/** Tables of the mock's other PostgreSQL databases (read "through a session of their own"). */
const OTHER_DB_TABLES: Record<string, Array<[string, TableSummary["kind"], number]>> = {
  ecommerce_sales: [
    ["customers", "table", 1_250],
    ["orders", "table", 8_400],
    ["order_items", "table", 23_100],
    ["daily_revenue", "view", 0],
  ],
  postgres: [],
};

registerMock("export_database_tables", async ({ connectionId, database }) => {
  const conns = await mockInvoke<ConnectionInfo[]>("list_connections", {});
  const conn =
    conns.find((c) => c.connectionId === connectionId) ??
    mockError("NOT_CONNECTED", "not connected");
  const db = (database as string | null) ?? null;
  await sleep(mockLatency === 0 ? 0 : 150);
  if (conn.driver === "mysql") {
    const name = db ?? conn.currentDatabase ?? mockError("INVALID_INPUT", "choose a database");
    return mockInvoke<TableSummary[]>("list_tables", { connectionId, schema: name });
  }
  if (conn.driver === "postgres" && db !== null && db !== conn.currentDatabase) {
    const list = OTHER_DB_TABLES[db] ?? [["items", "table", 42] as const];
    return list.map(([name, kind, rows]) => ({
      name,
      schema: "public",
      kind,
      rowEstimate: kind === "view" ? null : rows,
      sizeBytes: kind === "view" ? null : rows * 160,
      comment: null,
    })) satisfies TableSummary[];
  }
  const schemas = await mockInvoke<SchemaInfo[]>("list_schemas", { connectionId });
  const lists = await Promise.all(
    schemas.map((s) => mockInvoke<TableSummary[]>("list_tables", { connectionId, schema: s.name })),
  );
  return lists.flat();
});
