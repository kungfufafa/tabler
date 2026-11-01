import { split } from "@/features/drivers/portable/scanner";
import { LEX, LOGGER_REFUSED, switchesLogger } from "@/features/drivers/duckdb/logger";
import {
  CHECK_FILE,
  CREATE_FILE,
  isMemory,
  MEMORY_READ_ONLY,
} from "@/features/drivers/duckdb/params";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import { profile } from "../../fixtures";
import { mockError } from "../../transport";
import type { MockDriver, MockLane } from "../types";
import { type Database, emptyDatabase, seedSample, VERSION_TEXT } from "./data";
import { duckdbSession } from "./session";
import { EngineFailure, runSql } from "./sql";

/**
 * The mock driver of the duckdb lane (DuckDB), served in `vite --mode mock` with
 * `?mockDrivers=duckdb`: the lane's sample database in a file, a copy of it opened read-only
 * (hardened, `ReadOnlyMode::Server`), and a scratch database in memory. The files live in a
 * pretend file system, which the form's calls (`checkFile`, `createFile` for New…) read and
 * write as `entry::util` does the disk. Connecting checks what `entry::validate` and
 * `open_session` check, in the same words. There are no session calls.
 */

/** The pretend file system: each file's database, created when first opened. */
const files = new Map<string, () => Database>();
const opened = new Map<string, Database>();

const SAMPLE = "/Users/me/data/sample.duckdb";
const ARCHIVE = "/Users/me/data/archive-2023.duckdb";

/** A file's catalog name: its stem, as DuckDB names an attached file. */
const stem = (path: string) =>
  (path.split(/[\\/]/).at(-1) ?? path).replace(/\.[^.]*$/, "") || "main";

function seedFiles(): void {
  if (files.size > 0) return;
  files.set(SAMPLE, () => seedSample(stem(SAMPLE), SAMPLE));
  files.set(ARCHIVE, () => seedSample(stem(ARCHIVE), ARCHIVE));
}

/** Tests: the files as seeded again. */
export function resetDuckdb(): void {
  files.clear();
  opened.clear();
}

/** The database of a file, opened once (one instance per file). */
function databaseOf(path: string): Database {
  let db = opened.get(path);
  if (!db) {
    db = files.get(path)!();
    opened.set(path, db);
  }
  return db;
}

/** `engine::check_database_file`: a missing file, in the backend's words. */
function checkFile(path: string): void {
  seedFiles();
  if (!files.has(path))
    mockError(
      "INVALID_INPUT",
      `The database file “${path}” cannot be opened: No such file or directory (os error 2)`,
    );
}

/** What `DuckdbParams::from_driver_params` refuses (serde's words). */
function validateParams(p: ConnectionProfile): void {
  const v = p.options.driverParams?.autoloadExtensions;
  if (v !== undefined && typeof v !== "boolean")
    mockError(
      "INVALID_INPUT",
      `DuckDB settings: invalid type: ${typeof v === "string" ? `string ${JSON.stringify(v)}` : typeof v}, expected a boolean`,
    );
}

/** The startup SQL's settings that keep DuckDB's local file system on (`Instance::local_files_off`). */
const ALLOWS_FILES = /\b(temp_directory|allowed_directories|allowed_paths)\b/i;

/** A file profile, as the form saves one: no host, port, user or database. */
const file = (p: Parameters<typeof profile>[0]): ConnectionProfile => ({
  ...profile(p),
  host: null,
  port: null,
  user: null,
  database: null,
});

export const duckdbMock: MockDriver = {
  driver: "duckdb",
  // `DuckdbServerInfo::label`.
  serverVersion: () => VERSION_TEXT,
  seedProfiles: () => [
    file({
      id: "p-mock-duckdb",
      name: "DuckDB sample",
      driver: "duckdb",
      color: "yellow",
      sqlitePath: SAMPLE,
      sortOrder: 140,
    }),
    ((): ConnectionProfile => {
      const base = file({
        id: "p-mock-duckdb-ro",
        name: "DuckDB archive (read-only)",
        driver: "duckdb",
        sqlitePath: ARCHIVE,
        sortOrder: 141,
      });
      return {
        ...base,
        options: {
          ...base.options,
          readOnly: true,
          passwordMode: "none",
          startupSql: "SET threads = 2",
        },
      };
    })(),
    ((): ConnectionProfile => {
      const base = file({
        id: "p-mock-duckdb-memory",
        name: "DuckDB scratch (in memory)",
        driver: "duckdb",
        color: "green",
        sqlitePath: ":memory:",
        sortOrder: 142,
      });
      return {
        ...base,
        options: {
          ...base.options,
          passwordMode: "none",
          driverParams: { autoloadExtensions: true },
        },
      };
    })(),
  ],
  connect: (p) => {
    // `entry::validate`.
    validateParams(p);
    const memory = isMemory(p.sqlitePath);
    const readOnly = p.options.readOnly;
    if (memory && readOnly) mockError("INVALID_INPUT", MEMORY_READ_ONLY);
    const startup = (p.options.startupSql ?? "").trim();
    if (switchesLogger(startup)) mockError("INVALID_INPUT", `Startup SQL: ${LOGGER_REFUSED}`);
    const path = (p.sqlitePath ?? "").trim();
    if (!memory) checkFile(path);
    const db = memory ? emptyDatabase("memory", null) : databaseOf(path);
    // The startup SQL runs once, before the hardening; a failure fails the open.
    for (const st of split(LEX.lex, startup)) {
      try {
        runSql({ db, hardened: false, fileReadOnly: readOnly }, st.sql);
      } catch (err) {
        if (!(err instanceof EngineFailure)) throw err;
        mockError("CONNECT_FAILED", `startup SQL failed (${st.sql.slice(0, 80)}): ${err.message}`);
      }
    }
    return duckdbSession({
      db,
      readOnly,
      facts: {
        serverReadOnly: readOnly && !ALLOWS_FILES.test(startup),
        hardened: readOnly,
        fileReadOnly: readOnly,
        shared: !memory,
        passcode: p.options.passwordMode === "command",
      },
    });
  },
  // `entry::util`: each call answers `{ path }`.
  util: (method, args) => {
    if (method !== CHECK_FILE && method !== CREATE_FILE)
      mockError("UNSUPPORTED", `unsupported: DuckDB: unknown call “${method}”`);
    const raw = (args as { path?: unknown } | null)?.path;
    const path = typeof raw === "string" ? raw.trim() : "";
    if (path === "") mockError("INVALID_INPUT", "DuckDB: choose a database file");
    seedFiles();
    if (method === CHECK_FILE) checkFile(path);
    else {
      if (files.has(path)) mockError("INVALID_INPUT", `“${path}” already exists.`);
      files.set(path, () => emptyDatabase(stem(path), path));
    }
    return { path };
  },
};

const lane: MockLane = { drivers: [duckdbMock] };

export default lane;
