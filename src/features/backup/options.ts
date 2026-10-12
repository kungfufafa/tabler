import type {
  DumpBinary,
  DumpTool,
  ParsedVersion,
  RestoreFileInfo,
  RestoreFileKind,
  VersionFlavor,
} from "@/lib/ipc";
import { DEFAULT_PATTERN } from "./pattern";

/** Drivers that have dump binaries (SQLite is backed up by copying its file). */
export type DumpDriver = "postgres" | "mysql";
export type RestoreTool = Extract<DumpTool, "psql" | "pg_restore" | "mysql">;

export const isDumpDriver = (d: string): d is DumpDriver => d === "postgres" || d === "mysql";

/** One entry of the "Add option…" list. */
export interface FlagSpec {
  flag: string;
  description: string;
}

export const PG_DUMP_FLAGS: readonly FlagSpec[] = [
  { flag: "--schema-only", description: "Object definitions only, no data" },
  { flag: "--data-only", description: "Data only, no definitions" },
  { flag: "--no-owner", description: "No ALTER … OWNER TO statements" },
  { flag: "--no-privileges", description: "No GRANT / REVOKE statements" },
  { flag: "--clean", description: "DROP objects before creating them" },
  { flag: "--if-exists", description: "DROP … IF EXISTS (with --clean)" },
  { flag: "--create", description: "CREATE DATABASE, then connect to it" },
  { flag: "--format=custom", description: "Compressed archive for pg_restore (.dump)" },
  { flag: "--format=plain", description: "Plain SQL script (.sql)" },
  { flag: "--inserts", description: "INSERT statements instead of COPY" },
  { flag: "--column-inserts", description: "INSERT statements with column names" },
  { flag: "--verbose", description: "Log each object as it is dumped" },
];

export const MYSQLDUMP_FLAGS: readonly FlagSpec[] = [
  { flag: "--routines", description: "Stored procedures and functions" },
  { flag: "--events", description: "Event scheduler events" },
  { flag: "--triggers", description: "Triggers (on by default)" },
  { flag: "--single-transaction", description: "Consistent InnoDB snapshot without locking" },
  { flag: "--skip-lock-tables", description: "Do not lock tables while dumping" },
  { flag: "--lock-tables=false", description: "Do not lock tables while dumping" },
  { flag: "--no-data", description: "Table definitions only, no rows" },
  { flag: "--no-create-info", description: "Rows only, no CREATE TABLE" },
  { flag: "--add-drop-table", description: "DROP TABLE before each CREATE TABLE" },
  { flag: "--hex-blob", description: "Binary columns as hexadecimal" },
  { flag: "--set-gtid-purged=OFF", description: "No GTID_PURGED statement (MySQL)" },
  {
    flag: "--column-statistics=0",
    description: "Skip column statistics (mysqldump 8 vs older servers)",
  },
  { flag: "--compress", description: "Compress the client/server protocol" },
  { flag: "--all-databases", description: "Every database on the server" },
  { flag: "--verbose", description: "Log progress" },
];

export const PG_RESTORE_FLAGS: readonly FlagSpec[] = [
  { flag: "--clean", description: "DROP objects before recreating them" },
  { flag: "--if-exists", description: "DROP … IF EXISTS (with --clean)" },
  { flag: "--no-owner", description: "Do not restore object ownership" },
  { flag: "--no-privileges", description: "Do not restore GRANT / REVOKE" },
  { flag: "--single-transaction", description: "All or nothing, in one transaction" },
  { flag: "--exit-on-error", description: "Stop at the first error" },
  { flag: "--create", description: "Create the database named in the archive" },
  { flag: "--verbose", description: "Log each object as it is restored" },
];

export const PSQL_FLAGS: readonly FlagSpec[] = [
  { flag: "-v ON_ERROR_STOP=1", description: "Stop at the first error" },
  { flag: "--single-transaction", description: "All or nothing, in one transaction" },
  { flag: "--echo-errors", description: "Print the statement that failed" },
  { flag: "--quiet", description: "Less output" },
];

export const MYSQL_FLAGS: readonly FlagSpec[] = [
  { flag: "--force", description: "Continue after an SQL error" },
  { flag: "--verbose", description: "Log each statement" },
];

export function flagCatalog(tool: DumpTool): readonly FlagSpec[] {
  switch (tool) {
    case "pg_dump":
      return PG_DUMP_FLAGS;
    case "pg_restore":
      return PG_RESTORE_FLAGS;
    case "psql":
      return PSQL_FLAGS;
    case "mysqldump":
      return MYSQLDUMP_FLAGS;
    case "mysql":
      return MYSQL_FLAGS;
  }
}

// ---- flags -----------------------------------------------------------------------------------

export type PgFormat = "plain" | "custom" | "directory" | "tar";

const PG_FORMATS: Record<string, PgFormat> = {
  p: "plain",
  plain: "plain",
  c: "custom",
  custom: "custom",
  d: "directory",
  directory: "directory",
  t: "tar",
  tar: "tar",
};

/** pg_dump's output format from the option chips (`--format=x`, `-Fx`, `-F x`; the last wins). */
export function pgFormat(flags: readonly string[]): PgFormat {
  const words = flags.flatMap((f) => f.trim().split(/\s+/)).filter(Boolean);
  let format: PgFormat = "plain";
  for (let i = 0; i < words.length; i++) {
    const w = words[i]!;
    let value: string | undefined;
    if (w === "-F" || w === "--format") value = words[++i];
    else if (w.startsWith("--format=")) value = w.slice("--format=".length);
    else if (w.startsWith("-F")) value = w.slice(2);
    const f = value ? PG_FORMATS[value.toLowerCase()] : undefined;
    if (f) format = f;
  }
  return format;
}

const isFormatChip = (chip: string) => /^(--format(=|\s|$)|-F)/.test(chip.trim());

/** Chips that cannot be combined with `flag` (one output format; schema vs data only). */
function conflictsWith(flag: string, chip: string): boolean {
  if (isFormatChip(flag)) return isFormatChip(chip);
  const pairs: Array<[string, string]> = [
    ["--schema-only", "--data-only"],
    ["--no-data", "--no-create-info"],
  ];
  return pairs.some(([a, b]) => (flag === a && chip === b) || (flag === b && chip === a));
}

/** Adds an option chip (trimmed; duplicates and conflicting chips are replaced). */
export function addFlag(flags: readonly string[], flag: string): string[] {
  const f = flag.trim().replace(/\s+/g, " ");
  if (!f) return [...flags];
  return [...flags.filter((x) => x !== f && !conflictsWith(f, x)), f];
}

export function removeFlag(flags: readonly string[], flag: string): string[] {
  return flags.filter((x) => x !== flag);
}

/** Why a free-typed option can't be added, or null. */
export function flagProblem(flag: string): string | null {
  const f = flag.trim();
  if (!f) return null;
  if (!f.startsWith("-")) return "Options start with - or --";
  return null;
}

/** Whether gzip applies: plain SQL output (custom archives are compressed already). */
export function gzipApplies(driver: DumpDriver, flags: readonly string[]): boolean {
  return driver === "mysql" || pgFormat(flags) === "plain";
}

/** The backup file's extension for these options. */
export function backupExtension(
  driver: DumpDriver,
  flags: readonly string[],
  gzip: boolean,
): string {
  if (driver === "postgres") {
    const format = pgFormat(flags);
    if (format === "custom") return ".dump";
    if (format === "tar") return ".tar";
  }
  return gzip ? ".sql.gz" : ".sql";
}

/** Problems that keep a backup from starting. */
export function backupProblem(driver: DumpDriver, flags: readonly string[]): string | null {
  if (driver === "postgres" && pgFormat(flags) === "directory")
    return "The directory format writes a folder: use --format=custom or --format=plain.";
  return null;
}

// ---- binaries and versions --------------------------------------------------------------------

export function backupTool(driver: DumpDriver): DumpTool {
  return driver === "postgres" ? "pg_dump" : "mysqldump";
}

/** The binary that restores `kind`: psql (plain SQL) or pg_restore (archives); mysql. */
export function restoreTool(driver: DumpDriver, kind: RestoreFileKind | null): RestoreTool {
  if (driver === "mysql") return "mysql";
  return kind === "pgCustom" || kind === "pgTar" ? "pg_restore" : "psql";
}

const FLAVOR_LABEL: Record<VersionFlavor, string> = {
  postgres: "PostgreSQL",
  mysql: "MySQL",
  mariadb: "MariaDB",
};

/** "PostgreSQL 17.6", "MariaDB 11.8.9", or the raw `--version` line. */
export function binaryLabel(b: DumpBinary): string {
  return b.version
    ? `${FLAVOR_LABEL[b.version.flavor]} ${b.version.text}`
    : b.versionOutput || b.path;
}

export function serverLabel(v: ParsedVersion): string {
  return `${FLAVOR_LABEL[v.flavor]} ${v.text}`;
}

/** PostgreSQL majors before 10 are two numbers (9.6). */
function pgKey(v: ParsedVersion): [number, number] {
  return v.major >= 10 ? [v.major, 0] : [v.major, v.minor];
}

function older(a: [number, number], b: [number, number]): boolean {
  return a[0] < b[0] || (a[0] === b[0] && a[1] < b[1]);
}

/** Whether the binary's major version is older than the server's (same flavor only). */
export function isOlderThanServer(b: DumpBinary, server: ParsedVersion | null): boolean {
  const v = b.version;
  if (!v || !server || v.flavor !== server.flavor) return false;
  if (v.flavor === "postgres") return older(pgKey(v), pgKey(server));
  return v.major < server.major;
}

/** The warning shown under "Choose version", or null. */
export function versionWarning(b: DumpBinary | null, server: ParsedVersion | null): string | null {
  if (!b?.version || !server) return null;
  const v = b.version;
  if (isOlderThanServer(b, server)) {
    const extra =
      b.tool === "pg_dump" ? " pg_dump refuses to dump a server newer than itself." : "";
    return `${b.tool} ${v.text} is older than the server (${serverLabel(server)}).${extra}`;
  }
  const mysqlFamily = (f: VersionFlavor) => f === "mysql" || f === "mariadb";
  if (v.flavor !== server.flavor && mysqlFamily(v.flavor) && mysqlFamily(server.flavor))
    return `A ${FLAVOR_LABEL[v.flavor]} client with a ${FLAVOR_LABEL[server.flavor]} server: some options differ.`;
  return null;
}

/**
 * The binary to preselect: the remembered one while it is still installed, else one of the
 * server's major version, else the newest that is not older than the server, else the newest.
 * `list` is sorted newest first.
 */
export function pickBinary(
  list: readonly DumpBinary[],
  tool: DumpTool,
  server: ParsedVersion | null,
  remembered: string | null,
): DumpBinary | null {
  const candidates = list.filter((b) => b.tool === tool);
  const kept = remembered ? candidates.find((b) => b.path === remembered) : undefined;
  if (kept) return kept;
  if (server) {
    const sameFlavor = candidates.filter((b) => b.version?.flavor === server.flavor);
    const sameMajor = sameFlavor.find(
      (b) =>
        b.version !== null &&
        (server.flavor === "postgres"
          ? pgKey(b.version).join(".") === pgKey(server).join(".")
          : b.version.major === server.major),
    );
    if (sameMajor) return sameMajor;
    const notOlder = sameFlavor.find((b) => !isOlderThanServer(b, server));
    if (notOlder) return notOlder;
  }
  return candidates[0] ?? null;
}

const KIND_LABEL: Record<RestoreFileKind, string> = {
  sql: "Plain SQL",
  pgCustom: "pg_dump custom archive",
  pgTar: "pg_dump tar archive",
};

export function fileKindLabel(file: RestoreFileInfo): string {
  return `${KIND_LABEL[file.kind]}${file.gzip ? " (gzip)" : ""}`;
}

/** Why `file` cannot be restored into a `driver` database, or null. */
export function restoreFileProblem(
  driver: DumpDriver,
  file: RestoreFileInfo | null,
): string | null {
  if (!file) return null;
  if (driver === "mysql" && file.kind !== "sql")
    return "This is a PostgreSQL pg_dump archive, not a MySQL dump.";
  return null;
}

// ---- remembered options ----------------------------------------------------------------------

export interface BackupOptions {
  flags: string[];
  gzip: boolean;
  /** The binary last used; null = pick one for the server. */
  binaryPath: string | null;
  /** File name pattern (see pattern.ts). */
  pattern: string;
}

export interface RestoreOptions {
  flags: string[];
  binaryPath: string | null;
}

export const DEFAULT_BACKUP_OPTIONS: Readonly<Record<DumpDriver, Readonly<BackupOptions>>> = {
  postgres: { flags: [], gzip: false, binaryPath: null, pattern: DEFAULT_PATTERN },
  mysql: {
    flags: ["--single-transaction"],
    gzip: false,
    binaryPath: null,
    pattern: DEFAULT_PATTERN,
  },
};

export const DEFAULT_RESTORE_OPTIONS: Readonly<RestoreOptions> = { flags: [], binaryPath: null };

export const BACKUP_OPTIONS_KEY = "tabler.backup.options";

interface Stored {
  backup: Partial<Record<DumpDriver, BackupOptions>>;
  restore: Partial<Record<RestoreTool, RestoreOptions>>;
  lastDir: string | null;
}

type StorageLike = Pick<Storage, "getItem" | "setItem">;

function defaultStorage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    // Accessing localStorage throws when site data is blocked.
    return null;
  }
}

const isObj = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const strings = (v: unknown): string[] | null =>
  Array.isArray(v) && v.every((x) => typeof x === "string")
    ? v
        .map((x) => x.trim())
        .filter((x) => x !== "")
        .slice(0, 64)
    : null;
const pathOrNull = (v: unknown): string | null =>
  typeof v === "string" && v.startsWith("/") ? v : null;

function readStored(storage: StorageLike | null): Stored {
  const empty: Stored = { backup: {}, restore: {}, lastDir: null };
  try {
    const raw = storage?.getItem(BACKUP_OPTIONS_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : null;
    if (!isObj(parsed)) return empty;
    return {
      backup: isObj(parsed.backup) ? (parsed.backup as Stored["backup"]) : {},
      restore: isObj(parsed.restore) ? (parsed.restore as Stored["restore"]) : {},
      lastDir: pathOrNull(parsed.lastDir),
    };
  } catch {
    return empty;
  }
}

function writeStored(storage: StorageLike | null, stored: Stored): void {
  try {
    storage?.setItem(BACKUP_OPTIONS_KEY, JSON.stringify(stored));
  } catch {
    // Quota exceeded / storage disabled: the next backup starts from the defaults.
  }
}

/** The options last used for `driver` (validated field by field, defaults otherwise). */
export function loadBackupOptions(
  driver: DumpDriver,
  storage: StorageLike | null = defaultStorage(),
): BackupOptions {
  const d = DEFAULT_BACKUP_OPTIONS[driver];
  const raw: unknown = readStored(storage).backup[driver];
  if (!isObj(raw)) return { ...d, flags: [...d.flags] };
  return {
    flags: strings(raw.flags) ?? [...d.flags],
    gzip: typeof raw.gzip === "boolean" ? raw.gzip : d.gzip,
    binaryPath: pathOrNull(raw.binaryPath),
    pattern:
      typeof raw.pattern === "string" && raw.pattern.trim() !== ""
        ? raw.pattern.slice(0, 200)
        : d.pattern,
  };
}

export function saveBackupOptions(
  driver: DumpDriver,
  options: BackupOptions,
  storage: StorageLike | null = defaultStorage(),
): void {
  const stored = readStored(storage);
  writeStored(storage, { ...stored, backup: { ...stored.backup, [driver]: options } });
}

export function loadRestoreOptions(
  tool: RestoreTool,
  storage: StorageLike | null = defaultStorage(),
): RestoreOptions {
  const raw: unknown = readStored(storage).restore[tool];
  if (!isObj(raw)) return { ...DEFAULT_RESTORE_OPTIONS, flags: [] };
  return { flags: strings(raw.flags) ?? [], binaryPath: pathOrNull(raw.binaryPath) };
}

export function saveRestoreOptions(
  tool: RestoreTool,
  options: RestoreOptions,
  storage: StorageLike | null = defaultStorage(),
): void {
  const stored = readStored(storage);
  writeStored(storage, { ...stored, restore: { ...stored.restore, [tool]: options } });
}

/** The folder of the last backup. */
export function loadLastDir(storage: StorageLike | null = defaultStorage()): string | null {
  return readStored(storage).lastDir;
}

export function saveLastDir(dir: string, storage: StorageLike | null = defaultStorage()): void {
  writeStored(storage, { ...readStored(storage), lastDir: dir });
}

// ---- formatting -------------------------------------------------------------------------------

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

/** `0:07`, `12:34`, `1:02:03`. */
export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

export function basename(path: string): string {
  return path.split("/").filter(Boolean).pop() ?? path;
}

export function dirname(path: string): string | null {
  const i = path.lastIndexOf("/");
  return i > 0 ? path.slice(0, i) : null;
}
