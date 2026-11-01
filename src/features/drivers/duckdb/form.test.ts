/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import {
  connectionFormSchema,
  DEFAULT_FORM,
  formToProfileInput,
  profileToForm,
  type ConnectionFormValues,
} from "@/features/connections/schema";
import { DESCRIPTORS } from "@/features/drivers/catalog";
import lane from ".";
import { LOGGER_REFUSED } from "./logger";
import { createFileInput } from "./newFile";
import {
  duckdbParamsOf,
  isMemory,
  MEMORY_READ_ONLY,
  NO_ENCRYPTED_FILES,
  toDuckdbParams,
} from "./params";

// The DuckDB form: what it saves (only the params that differ from the backend's defaults), what
// it reads back, its checks next to the schema's in the words of `entry::validate`, and what
// New… sends.

const ui = lane.kinds!.duckdb!;
const ROOT = path.resolve(import.meta.dirname, "../../../..");
/** A Rust source with its string continuations (`\` and the next line's indent) joined. */
const rust = (file: string) =>
  readFileSync(path.join(ROOT, "src-tauri/src/drivers/duckdb", file), "utf8").replace(
    /\\\r?\n\s*/g,
    "",
  );

const form = (values: Partial<ConnectionFormValues> = {}): ConnectionFormValues => ({
  ...DEFAULT_FORM,
  ...ui.defaults!(),
  name: "n",
  driver: "duckdb",
  sqlitePath: "/Users/me/data/sample.duckdb",
  ...values,
});

/** The schema's issues and the lane's, as `path: message`. */
function issues(values: ConnectionFormValues): string[] {
  const r = connectionFormSchema.safeParse(values);
  const shared = r.success ? [] : r.error.issues.map((i) => `${i.path.join(".")}: ${i.message}`);
  return [...shared, ...(ui.validate?.(values) ?? []).map((i) => `${i.path}: ${i.message}`)];
}

const saved = (values: ConnectionFormValues) =>
  formToProfileInput(values, null, "id-1", ui).profile;

describe("the DuckDB form", () => {
  it("shows the file and the key, with autoload off", () => {
    expect(ui.rows).toEqual(["file", "secret"]);
    expect(ui.defaults!()).toEqual({ driverParams: { autoloadExtensions: false } });
    const desc = DESCRIPTORS.duckdb;
    expect(desc.location).toBe("file");
    expect(desc.secret).toBe("encryptionKey");
    expect(desc.tls).toBe("none");
    expect(desc.ssh).toBe("none");
    expect(ui.sslModes).toBeUndefined();
    expect(ui.url!.schemes).toEqual(desc.urlSchemes);
  });

  it("saves only the params that differ from the defaults, and reads them back", () => {
    const plain = saved(form());
    expect(plain.options.driverParams).toBeNull();
    expect(plain.sqlitePath).toBe("/Users/me/data/sample.duckdb");
    // A file has no host, port, user or database.
    expect([plain.host, plain.port, plain.user, plain.database]).toEqual([null, null, null, null]);
    const p = saved(
      form({
        readOnly: true,
        startupSql: "SET threads = 4",
        driverParams: { autoloadExtensions: true },
      }),
    );
    expect(p.options.driverParams).toEqual({ autoloadExtensions: true });
    expect(p.options.readOnly).toBe(true);
    const back = { ...profileToForm(p), ...ui.fromProfile!(p) };
    expect(back.driverParams).toEqual({ autoloadExtensions: true });
    expect(back.sqlitePath).toBe("/Users/me/data/sample.duckdb");
    expect(saved({ ...form(), ...back, password: "" }).options.driverParams).toEqual(
      p.options.driverParams,
    );
  });

  it("reads the params leniently: the wrong type counts as unset", () => {
    expect(duckdbParamsOf({ autoloadExtensions: "yes" })).toEqual({ autoloadExtensions: false });
    expect(duckdbParamsOf(null)).toEqual({ autoloadExtensions: false });
    expect(toDuckdbParams(duckdbParamsOf(null))).toBeNull();
  });

  it("takes nothing or :memory: as an in-memory database, as `Location::parse` does", () => {
    for (const p of ["", "  ", ":memory:", ":MEMORY:", " :memory: ", null])
      expect(isMemory(p), String(p)).toBe(true);
    for (const p of ["/x.duckdb", "memory", "C:\\data\\x.duckdb"])
      expect(isMemory(p), p).toBe(false);
  });

  it("checks what `entry::validate` refuses, in its words", () => {
    expect(issues(form())).toEqual([]);
    expect(issues(form({ sqlitePath: ":memory:" }))).toEqual([]);
    expect(issues(form({ sqlitePath: "" }))).toEqual(["sqlitePath: Choose a database file"]);
    expect(issues(form({ sqlitePath: ":memory:", readOnly: true }))).toEqual([
      `readOnly: ${MEMORY_READ_ONLY}`,
    ]);
    expect(issues(form({ readOnly: true }))).toEqual([]);
    expect(issues(form({ startupSql: "ATTACH 'x.duckdb';\nCALL enable_logging()" }))).toEqual([
      `startupSql: Startup SQL: ${LOGGER_REFUSED}`,
    ]);
    expect(issues(form({ startupSql: "SET threads = 4; SELECT 'logging'" }))).toEqual([]);
    // The backend's words.
    const entry = rust("entry.rs");
    expect(entry).toContain(`"${MEMORY_READ_ONLY}"`);
    expect(entry).toContain(`"${NO_ENCRYPTED_FILES}"`);
    expect(entry).toContain('pub const CHECK_FILE: &str = "checkFile";');
    expect(entry).toContain('pub const CREATE_FILE: &str = "createFile";');
    expect(rust("session.rs")).toContain(`pub const LOGGER_REFUSED: &str = "${LOGGER_REFUSED}";`);
  });

  it("sums a profile up as its file", () => {
    expect(ui.summary!(saved(form()))).toBe("/Users/me/data/sample.duckdb");
    expect(ui.summary!(saved(form({ sqlitePath: ":memory:" })))).toBe("In memory");
    expect(ui.summary!(saved(form({ driverParams: { autoloadExtensions: true } })))).toBe(
      "/Users/me/data/sample.duckdb (autoload extensions)",
    );
  });
});

describe("New…", () => {
  it("sends the form's profile with a typed key only, never a stored one", () => {
    const plain = createFileInput(form());
    expect(plain.profile.id).toBe("00000000-0000-0000-0000-000000000000");
    expect(plain.profile.options.passwordMode).toBe("none");
    expect(plain.password).toBeNull();
    const keyed = createFileInput(
      form({ password: "k", driverParams: { autoloadExtensions: true } }),
    );
    // The backend refuses it (`NO_ENCRYPTED_FILES`).
    expect(keyed.password).toBe("k");
    expect(keyed.profile.options.passwordMode).toBe("keychain");
    expect(keyed.profile.options.driverParams).toEqual({ autoloadExtensions: true });
    expect(createFileInput(form({ password: "k", passwordMode: "none" })).password).toBeNull();
    // A read-only profile goes as it is: the backend refuses the write.
    expect(createFileInput(form({ readOnly: true })).profile.options.readOnly).toBe(true);
  });
});
