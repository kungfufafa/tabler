// Mock handlers for backup / restore (backup_*): a few installed binaries per driver, the
// servers' databases, restore files detected by name, and jobs that log and count bytes over ~2 s.
import type { BackupEvent } from "../bindings/BackupEvent";
import type { BackupJobRequest } from "../bindings/BackupJobRequest";
import type { BackupJobResult } from "../bindings/BackupJobResult";
import type { BackupLogLine } from "../bindings/BackupLogLine";
import type { BackupTarget } from "../bindings/BackupTarget";
import type { ConnectionInfo } from "../bindings/ConnectionInfo";
import type { ConnectionProfile } from "../bindings/ConnectionProfile";
import type { Driver } from "../bindings/Driver";
import type { DumpBinary } from "../bindings/DumpBinary";
import type { DumpTool } from "../bindings/DumpTool";
import type { ParsedVersion } from "../bindings/ParsedVersion";
import type { RestoreFileInfo } from "../bindings/RestoreFileInfo";
import { mockCheckHostKey } from "./export";
import { mockError, mockInvoke, mockLatency, registerMock } from "./transport";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function bin(tool: DumpTool, path: string, version: ParsedVersion, output: string): DumpBinary {
  return { tool, path, version, versionOutput: output };
}

const pg = (text: string): ParsedVersion => {
  const [major = 0, minor = 0] = text.split(".").map(Number);
  return { flavor: "postgres", text, major, minor };
};
const my = (flavor: "mysql" | "mariadb", text: string): ParsedVersion => {
  const [major = 0, minor = 0] = text.split(".").map(Number);
  return { flavor, text, major, minor };
};

/** What `backup_list_binaries` "finds", newest first per tool (like the backend sorts). */
export const MOCK_BINARIES: Record<"postgres" | "mysql", DumpBinary[]> = {
  postgres: [
    bin("pg_dump", "/opt/homebrew/opt/libpq/bin/pg_dump", pg("18.2"), "pg_dump (PostgreSQL) 18.2"),
    bin(
      "pg_dump",
      "/opt/homebrew/opt/postgresql@17/bin/pg_dump",
      pg("17.6"),
      "pg_dump (PostgreSQL) 17.6 (Homebrew)",
    ),
    bin(
      "pg_dump",
      "/Applications/Postgres.app/Contents/Versions/15/bin/pg_dump",
      pg("15.8"),
      "pg_dump (PostgreSQL) 15.8",
    ),
    bin(
      "pg_restore",
      "/opt/homebrew/opt/libpq/bin/pg_restore",
      pg("18.2"),
      "pg_restore (PostgreSQL) 18.2",
    ),
    bin(
      "pg_restore",
      "/opt/homebrew/opt/postgresql@17/bin/pg_restore",
      pg("17.6"),
      "pg_restore (PostgreSQL) 17.6 (Homebrew)",
    ),
    bin("psql", "/opt/homebrew/opt/libpq/bin/psql", pg("18.2"), "psql (PostgreSQL) 18.2"),
    bin(
      "psql",
      "/opt/homebrew/opt/postgresql@17/bin/psql",
      pg("17.6"),
      "psql (PostgreSQL) 17.6 (Homebrew)",
    ),
  ],
  mysql: [
    bin(
      "mysqldump",
      "/opt/homebrew/opt/mariadb@11.8/bin/mariadb-dump",
      my("mariadb", "11.8.9"),
      "mariadb-dump from 11.8.9-MariaDB, client 10.20 for osx10.21 (arm64)",
    ),
    bin(
      "mysqldump",
      "/opt/homebrew/bin/mysqldump",
      my("mysql", "9.6.0"),
      "mysqldump  Ver 9.6.0 for macos26.4 on arm64 (Homebrew)",
    ),
    bin(
      "mysqldump",
      "/opt/homebrew/opt/mysql-client@8.0/bin/mysqldump",
      my("mysql", "8.0.36"),
      "mysqldump  Ver 8.0.36 for macos14.2 on arm64 (Homebrew)",
    ),
    bin(
      "mysql",
      "/opt/homebrew/bin/mysql",
      my("mysql", "9.6.0"),
      "mysql  Ver 9.6.0 for macos26.4 on arm64 (Homebrew)",
    ),
  ],
};

registerMock("backup_list_binaries", ({ driver }) => {
  const d = driver as Driver;
  return d === "sqlite" ? [] : MOCK_BINARIES[d];
});

async function profileOf(profileId: unknown): Promise<ConnectionProfile> {
  return mockInvoke<ConnectionProfile>("get_profile", { id: profileId });
}

function serverOf(p: ConnectionProfile): { version: string; parsed: ParsedVersion } {
  return p.driver === "postgres"
    ? {
        version: "PostgreSQL 17.6 (Homebrew) on aarch64-apple-darwin",
        parsed: pg("17.6"),
      }
    : { version: "MySQL 9.6.0", parsed: my("mysql", "9.6.0") };
}

function databasesOf(p: ConnectionProfile): string[] {
  const names =
    p.driver === "postgres"
      ? ["tabler_dev", "postgres", "ecommerce_sales", "analytics", "app_staging"]
      : ["tabler_dev", "tabler_inv", "shop", "information_schema", "mysql", "sys"];
  const db = p.database ?? (p.driver === "postgres" ? "postgres" : null);
  if (db && !names.includes(db)) names.unshift(db);
  return names;
}

registerMock("backup_inspect_target", async ({ profileId }) => {
  const p = await profileOf(profileId);
  if (p.driver === "sqlite")
    mockError("UNSUPPORTED", "SQLite databases are backed up by copying the database file");
  const live = (await mockInvoke<ConnectionInfo[]>("list_connections", {})).find(
    (c) => c.profileId === p.id,
  );
  if (!live) {
    await sleep(mockLatency === 0 ? 0 : 300);
    mockCheckHostKey(p.id, p.ssh);
    if ((p.host ?? "").includes("bad"))
      mockError("CONNECT_FAILED", `could not connect to ${p.host}:${p.port}: connection refused`);
  }
  const server = serverOf(p);
  return {
    profileId: p.id,
    driver: p.driver,
    serverVersion: server.version,
    server: server.parsed,
    databases: databasesOf(p),
    currentDatabase:
      live?.currentDatabase ?? p.database ?? (p.driver === "postgres" ? "postgres" : null),
    live: live !== undefined,
  } satisfies BackupTarget;
});

/** Pretend detection: by name (the backend reads the file's first bytes). */
export function mockFileInfo(path: string): RestoreFileInfo {
  const name = path.split("/").pop() ?? path;
  const lower = name.toLowerCase();
  const gzip = lower.endsWith(".gz");
  const base = gzip ? lower.slice(0, -3) : lower;
  const kind =
    base.endsWith(".dump") || base.endsWith(".backup")
      ? "pgCustom"
      : base.endsWith(".tar")
        ? "pgTar"
        : "sql";
  let h = 0;
  for (const ch of name) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return { path, name, size: 200_000 + (h % 40_000_000), kind, gzip };
}

registerMock("backup_inspect_file", ({ path }) => {
  const p = String(path ?? "");
  if (!p.startsWith("/")) mockError("INVALID_INPUT", "the restore file path must be absolute");
  if (p.includes("missing")) mockError("IO", `cannot read ${p}: No such file or directory`);
  return mockFileInfo(p);
});

const running = new Set<string>();
const cancelled = new Set<string>();

const info = (text: string): BackupLogLine => ({ stream: "info", text });
const err = (text: string): BackupLogLine => ({ stream: "stderr", text });

function commandLine(req: BackupJobRequest, p: ConnectionProfile): string {
  const host = p.ssh ? "127.0.0.1" : (p.host ?? "127.0.0.1");
  const port = p.ssh ? 50123 : (p.port ?? (p.driver === "postgres" ? 5432 : 3306));
  const user = p.user
    ? p.driver === "postgres"
      ? ` --username=${p.user}`
      : ` --user=${p.user}`
    : "";
  const flags = req.flags.length ? ` ${req.flags.join(" ")}` : "";
  if (p.driver === "postgres")
    return `${req.binaryPath} --host=${host} --port=${port}${user} --no-password${flags} --dbname=${req.database}`;
  return `${req.binaryPath} --host=${host} --port=${port} --protocol=TCP${user} --ssl-mode=PREFERRED${flags} ${req.database}`;
}

registerMock("backup_run", async ({ req, onEvent }) => {
  const r = req as BackupJobRequest;
  const send = (onEvent as { send?: (e: BackupEvent) => void } | undefined)?.send ?? (() => {});
  if (!r.jobId?.trim()) mockError("INVALID_INPUT", "jobId is required");
  if (running.has(r.jobId)) mockError("INVALID_INPUT", `job "${r.jobId}" is already running`);
  const p = await profileOf(r.profileId);
  if (p.driver === "sqlite")
    mockError("UNSUPPORTED", "SQLite databases are backed up by copying the database file");
  if (!r.path.startsWith("/")) mockError("INVALID_INPUT", "the path must be absolute");
  for (const f of r.flags)
    if (/^(--host|--port|--user|--username|--password|--file|--dbname)(=|$)/.test(f.split(" ")[0]!))
      mockError(
        "INVALID_INPUT",
        `${f.split("=")[0]} is set by Tabler from the connection; remove “${f}”`,
      );
  const tool = r.binaryPath.split("/").pop() ?? "";
  const file = r.kind === "restore" ? mockFileInfo(r.path) : null;
  if (file && p.driver === "mysql" && file.kind !== "sql")
    mockError("INVALID_INPUT", "this file is a PostgreSQL pg_dump archive, not a MySQL dump");
  mockCheckHostKey(p.id, p.ssh);

  running.add(r.jobId);
  const started = performance.now();
  const steps = mockLatency === 0 ? 4 : 12;
  const delay = mockLatency === 0 ? 5 : 160;
  const total = file ? file.size : null;
  const finalBytes = file ? file.size : r.gzip ? 184_320 : 1_258_291;
  const path = r.path;
  try {
    send({ event: "started", data: { command: commandLine(r, p), path } });
    for (let i = 1; i <= steps; i++) {
      await sleep(delay);
      if (cancelled.has(r.jobId)) mockError("CANCELLED", "cancelled");
      const lines: BackupLogLine[] = [];
      if (i === 1 && p.ssh)
        lines.push(info("Connecting through the SSH tunnel on 127.0.0.1:50123"));
      if (r.flags.some((f) => f === "--verbose" || f === "-v")) {
        lines.push(
          err(
            `${tool}: ${r.kind === "backup" ? "dumping" : "restoring"} contents of table "public.table_${i}"`,
          ),
        );
      }
      if (i === Math.ceil(steps / 2) && file?.name.includes("broken")) {
        lines.push(err(`psql:<stdin>:${i * 40}: ERROR:  relation "users" already exists`));
        if (r.flags.includes("-v ON_ERROR_STOP=1")) {
          send({ event: "log", data: { lines } });
          mockError(
            "DB_ERROR",
            `${tool} failed: psql:<stdin>:${i * 40}: ERROR:  relation "users" already exists`,
          );
        }
      }
      if (lines.length) send({ event: "log", data: { lines } });
      send({ event: "progress", data: { bytes: Math.round((finalBytes * i) / steps), total } });
    }
    if (p.driver === "mysql" && r.database === "sys" && r.kind === "backup") {
      send({
        event: "log",
        data: {
          lines: [
            err(
              "mysqldump: Got error: 1044: Access denied for user 'root'@'%' to database 'sys' when using LOCK TABLES",
            ),
          ],
        },
      });
      mockError(
        "DB_ERROR",
        "mysqldump failed: Got error: 1044: Access denied for user 'root'@'%' to database 'sys' when using LOCK TABLES",
      );
    }
    const errors = file?.name.includes("broken") ? 1 : 0;
    return {
      path,
      bytes: finalBytes,
      durationMs: Math.max(1, Math.round(performance.now() - started)),
      errors,
    } satisfies BackupJobResult;
  } finally {
    running.delete(r.jobId);
    cancelled.delete(r.jobId);
  }
});

registerMock("backup_cancel", ({ jobId }) => {
  if (running.has(jobId as string)) cancelled.add(jobId as string);
  return null;
});

registerMock("backup_copy_sqlite", async ({ profileId, path }) => {
  const p = await profileOf(profileId);
  if (p.driver !== "sqlite")
    mockError("INVALID_INPUT", "only SQLite databases are copied as a file");
  if (!String(path ?? "").startsWith("/"))
    mockError("INVALID_INPUT", "the destination path must be absolute");
  return 1_064_960;
});
