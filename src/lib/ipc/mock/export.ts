// Mock handlers for export_data, cancel_export and accept_host_key, plus the SSH host-key check the
// mock `connect` / `test_connection` run for profiles with an SSH tunnel.
import type { ConnectionInfo } from "../bindings/ConnectionInfo";
import type { Driver } from "../bindings/Driver";
import type { ExportFormat } from "../bindings/ExportFormat";
import type { ExportProgress } from "../bindings/ExportProgress";
import type { ExportRequest } from "../bindings/ExportRequest";
import type { ExportResult } from "../bindings/ExportResult";
import type { SshConfig } from "../bindings/SshConfig";
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

function validateFormat(format: ExportFormat, isTable: boolean): void {
  if (format.format === "csv") {
    const d = format.delimiter;
    if (d.length !== 1 || d.charCodeAt(0) > 127 || d === '"' || d === "\r" || d === "\n")
      mockError(
        "INVALID_INPUT",
        "the CSV delimiter must be a single ASCII character other than a quote or a line break",
      );
  }
  if (format.format === "sqlInsert") {
    if (format.batchSize < 1)
      mockError("INVALID_INPUT", "the INSERT batch size must be at least 1");
    if (!isTable && format.tableName.trim() === "")
      mockError("INVALID_INPUT", "a target table name is required for an INSERT export");
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
  if (!filter) return total;
  return rowsFor(`${driver}:${schema}.${table.name}`, meta.columns, total).filter((r) =>
    matchesGroup(filter, r, meta.columns),
  ).length;
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
  if (format.format === "csv") return format.quoteAll ? 96 : 84;
  if (format.format === "json") return format.pretty && !format.ndjson ? 210 : 140;
  return 120;
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
  validateFormat(r.format, r.source.kind === "table");
  let total =
    r.source.kind === "table" ? tableRows(conn.driver, r) : sqlRows(conn.driver, r.source.sql);
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
