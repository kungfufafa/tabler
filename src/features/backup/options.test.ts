import { describe, expect, it } from "vitest";
import type { DumpBinary, ParsedVersion } from "@/lib/ipc";
import { isErrorLine } from "./logLines";
import {
  addFlag,
  BACKUP_OPTIONS_KEY,
  backupExtension,
  backupProblem,
  formatBytes,
  formatElapsed,
  gzipApplies,
  isOlderThanServer,
  loadBackupOptions,
  loadLastDir,
  loadRestoreOptions,
  pgFormat,
  pickBinary,
  removeFlag,
  restoreFileProblem,
  restoreTool,
  saveBackupOptions,
  saveLastDir,
  saveRestoreOptions,
  versionWarning,
} from "./options";
import { DEFAULT_PATTERN } from "./pattern";

function memoryStorage() {
  const map = new Map<string, string>();
  return {
    getItem: (k: string) => map.get(k) ?? null,
    setItem: (k: string, v: string) => void map.set(k, v),
    map,
  };
}

const pg = (text: string): ParsedVersion => {
  const [major = 0, minor = 0] = text.split(".").map(Number);
  return { flavor: "postgres", text, major, minor };
};

function bin(tool: DumpBinary["tool"], path: string, version: ParsedVersion | null): DumpBinary {
  return { tool, path, version, versionOutput: version ? `${tool} ${version.text}` : "?" };
}

describe("option chips", () => {
  it("reads pg_dump's output format", () => {
    expect(pgFormat([])).toBe("plain");
    expect(pgFormat(["--no-owner", "--format=custom"])).toBe("custom");
    expect(pgFormat(["-Fc"])).toBe("custom");
    expect(pgFormat(["-F t"])).toBe("tar");
    expect(pgFormat(["--format d"])).toBe("directory");
    expect(pgFormat(["--format=custom", "--format=plain"])).toBe("plain");
  });

  it("replaces conflicting chips", () => {
    expect(addFlag(["--no-owner", "--format=plain"], "--format=custom")).toEqual([
      "--no-owner",
      "--format=custom",
    ]);
    expect(addFlag(["-Fc"], "--format=plain")).toEqual(["--format=plain"]);
    expect(addFlag(["--schema-only"], "--data-only")).toEqual(["--data-only"]);
    expect(addFlag(["--clean"], " --clean ")).toEqual(["--clean"]);
    expect(addFlag(["--clean"], "-v   ON_ERROR_STOP=1")).toEqual(["--clean", "-v ON_ERROR_STOP=1"]);
    expect(addFlag(["--clean"], "  ")).toEqual(["--clean"]);
    expect(removeFlag(["--clean", "--if-exists"], "--clean")).toEqual(["--if-exists"]);
  });

  it("names the file and decides gzip from the format", () => {
    expect(backupExtension("postgres", [], false)).toBe(".sql");
    expect(backupExtension("postgres", [], true)).toBe(".sql.gz");
    expect(backupExtension("postgres", ["--format=custom"], true)).toBe(".dump");
    expect(backupExtension("postgres", ["-Ft"], false)).toBe(".tar");
    expect(backupExtension("mysql", ["--routines"], true)).toBe(".sql.gz");
    expect(gzipApplies("postgres", ["--format=custom"])).toBe(false);
    expect(gzipApplies("postgres", ["--format=plain"])).toBe(true);
    expect(gzipApplies("mysql", [])).toBe(true);
    expect(backupProblem("postgres", ["-Fd"])).toMatch(/directory/);
    expect(backupProblem("mysql", ["-Fd"])).toBeNull();
  });

  it("picks the restore binary by file kind", () => {
    expect(restoreTool("postgres", "sql")).toBe("psql");
    expect(restoreTool("postgres", null)).toBe("psql");
    expect(restoreTool("postgres", "pgCustom")).toBe("pg_restore");
    expect(restoreTool("postgres", "pgTar")).toBe("pg_restore");
    expect(restoreTool("mysql", "sql")).toBe("mysql");
    const archive = {
      path: "/a.dump",
      name: "a.dump",
      size: 1,
      kind: "pgCustom",
      gzip: false,
    } as const;
    expect(restoreFileProblem("mysql", archive)).toMatch(/PostgreSQL/);
    expect(restoreFileProblem("postgres", archive)).toBeNull();
  });
});

describe("versions", () => {
  const list = [
    bin("pg_dump", "/libpq/pg_dump", pg("18.2")),
    bin("pg_dump", "/pg17/pg_dump", pg("17.6")),
    bin("pg_dump", "/pg15/pg_dump", pg("15.8")),
    bin("psql", "/pg17/psql", pg("17.6")),
    bin("pg_dump", "/odd/pg_dump", null),
  ];

  it("warns when the binary is older than the server", () => {
    const server = pg("17.6");
    expect(isOlderThanServer(list[2]!, server)).toBe(true);
    expect(isOlderThanServer(list[1]!, server)).toBe(false);
    expect(isOlderThanServer(list[0]!, server)).toBe(false);
    expect(versionWarning(list[2]!, server)).toMatch(/older than the server \(PostgreSQL 17\.6\)/);
    expect(versionWarning(list[0]!, server)).toBeNull();
    expect(versionWarning(list[4]!, server)).toBeNull();
    expect(versionWarning(list[2]!, null)).toBeNull();
    // 9.6 is older than 9.6's successor 10 and than 9.7-style minors.
    expect(isOlderThanServer(bin("pg_dump", "/x", pg("9.5")), pg("9.6"))).toBe(true);
    expect(isOlderThanServer(bin("pg_dump", "/x", pg("16.1")), pg("16.9"))).toBe(false);
  });

  it("compares MySQL majors and flags MariaDB / MySQL mixes", () => {
    const mysql8: ParsedVersion = { flavor: "mysql", text: "8.0.36", major: 8, minor: 0 };
    const mysql9: ParsedVersion = { flavor: "mysql", text: "9.6.0", major: 9, minor: 6 };
    const maria: ParsedVersion = { flavor: "mariadb", text: "11.8.9", major: 11, minor: 8 };
    expect(versionWarning(bin("mysqldump", "/a", mysql8), mysql9)).toMatch(/older/);
    expect(versionWarning(bin("mysqldump", "/a", mysql9), mysql8)).toBeNull();
    expect(versionWarning(bin("mysqldump", "/a", maria), mysql9)).toMatch(/MariaDB client/);
  });

  it("preselects the remembered binary, else the server's major, else the newest", () => {
    expect(pickBinary(list, "pg_dump", pg("17.2"), "/pg15/pg_dump")?.path).toBe("/pg15/pg_dump");
    expect(pickBinary(list, "pg_dump", pg("17.2"), "/gone/pg_dump")?.path).toBe("/pg17/pg_dump");
    expect(pickBinary(list, "pg_dump", pg("16.4"), null)?.path).toBe("/libpq/pg_dump");
    expect(pickBinary(list, "pg_dump", pg("19.0"), null)?.path).toBe("/libpq/pg_dump");
    expect(pickBinary(list, "pg_dump", null, null)?.path).toBe("/libpq/pg_dump");
    expect(pickBinary(list, "psql", pg("17.2"), null)?.path).toBe("/pg17/psql");
    expect(pickBinary(list, "pg_restore", pg("17.2"), null)).toBeNull();
  });
});

describe("remembered options", () => {
  it("starts from the defaults", () => {
    const storage = memoryStorage();
    expect(loadBackupOptions("postgres", storage)).toEqual({
      flags: [],
      gzip: false,
      binaryPath: null,
      pattern: DEFAULT_PATTERN,
    });
    expect(loadBackupOptions("mysql", storage).flags).toEqual(["--single-transaction"]);
    expect(loadRestoreOptions("psql", storage)).toEqual({ flags: [], binaryPath: null });
    expect(loadLastDir(storage)).toBeNull();
  });

  it("remembers flags, gzip, binary and pattern per driver", () => {
    const storage = memoryStorage();
    saveBackupOptions(
      "postgres",
      {
        flags: ["--no-owner"],
        gzip: true,
        binaryPath: "/pg17/pg_dump",
        pattern: "{database}-{year}",
      },
      storage,
    );
    saveRestoreOptions(
      "pg_restore",
      { flags: ["--clean"], binaryPath: "/pg17/pg_restore" },
      storage,
    );
    saveLastDir("/Users/me/Backups", storage);
    expect(loadBackupOptions("postgres", storage)).toEqual({
      flags: ["--no-owner"],
      gzip: true,
      binaryPath: "/pg17/pg_dump",
      pattern: "{database}-{year}",
    });
    // Other drivers / tools keep their own.
    expect(loadBackupOptions("mysql", storage).flags).toEqual(["--single-transaction"]);
    expect(loadRestoreOptions("pg_restore", storage).flags).toEqual(["--clean"]);
    expect(loadRestoreOptions("psql", storage).flags).toEqual([]);
    expect(loadLastDir(storage)).toBe("/Users/me/Backups");
    expect(storage.map.has(BACKUP_OPTIONS_KEY)).toBe(true);
  });

  it("ignores broken stored values", () => {
    const storage = memoryStorage();
    storage.setItem(
      BACKUP_OPTIONS_KEY,
      JSON.stringify({
        backup: { postgres: { flags: [1, 2], gzip: "yes", binaryPath: "relative", pattern: " " } },
        lastDir: 42,
      }),
    );
    expect(loadBackupOptions("postgres", storage)).toEqual({
      flags: [],
      gzip: false,
      binaryPath: null,
      pattern: DEFAULT_PATTERN,
    });
    expect(loadLastDir(storage)).toBeNull();
    storage.setItem(BACKUP_OPTIONS_KEY, "{not json");
    expect(loadBackupOptions("mysql", storage).flags).toEqual(["--single-transaction"]);
  });
});

describe("formatting", () => {
  it("formats sizes and elapsed times", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(1_258_291)).toBe("1.2 MB");
    expect(formatBytes(50 * 1024 * 1024)).toBe("50 MB");
    expect(formatElapsed(7_400)).toBe("0:07");
    expect(formatElapsed(754_000)).toBe("12:34");
    expect(formatElapsed(3_723_000)).toBe("1:02:03");
  });

  it("recognises error lines like the backend", () => {
    expect(isErrorLine('psql:<stdin>:12: ERROR:  relation "t" already exists')).toBe(true);
    expect(isErrorLine("pg_dump: error: connection failed")).toBe(true);
    expect(isErrorLine("ERROR 1064 (42000) at line 5")).toBe(true);
    expect(isErrorLine('pg_dump: dumping contents of table "public.error_log"')).toBe(false);
  });
});
