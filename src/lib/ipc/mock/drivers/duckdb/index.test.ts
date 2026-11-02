/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { LOGGER_REFUSED } from "@/features/drivers/duckdb/logger";
import { PLUGIN_FILES_REFUSED } from "@/features/drivers/duckdb/files";
import {
  EXTERNAL_ACCESS_HINT,
  MEMORY_READ_ONLY,
  NO_ENCRYPTED_FILES,
} from "@/features/drivers/duckdb/params";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { ConnectionInfo } from "../../../bindings/ConnectionInfo";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { DatabaseInfo } from "../../../bindings/DatabaseInfo";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { RoutineSummary } from "../../../bindings/RoutineSummary";
import type { RowCount } from "../../../bindings/RowCount";
import type { SchemaInfo } from "../../../bindings/SchemaInfo";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import type { SessionDetails } from "../../../bindings/SessionDetails";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { TableSummary } from "../../../bindings/TableSummary";
import type { Value } from "../../../bindings/Value";
import { buildMockDrivers } from "..";
import { mockProfiles } from "../../handlers";
import { mockInvoke } from "../../transport";
import { enableMockDrivers, resetMockBuild } from "..";
import lane, { resetDuckdb } from ".";
import {
  duckdbCaps,
  EDIT_FEATURES,
  GUARD_MESSAGE,
  READ_ONLY_EDIT,
  READ_ONLY_MESSAGE,
} from "./session";

// The mock's DuckDB against the backend's behaviour (drivers/duckdb/session.rs, entry.rs and
// their tests): the recorded caps, the catalog, offset pages, counts, a whole cell, a staged
// commit with a failing change, the editor, the logger refusal, a read-only session's refusals,
// a hardened session's file refusal, and the form's calls.

const ROOT = path.resolve(import.meta.dirname, "../../../../../..");
const RECORDED = JSON.parse(
  readFileSync(path.join(ROOT, "src-tauri/tests/fixtures/duckdb/caps.json"), "utf8"),
) as Record<"readWrite" | "command" | "readOnly" | "memory", SessionCaps>;

beforeAll(async () => {
  await import("../..");
});

let added: string[] = [];

beforeEach(async () => {
  resetDuckdb();
  added = (await enableMockDrivers(["duckdb"])).map((p) => p.id);
});

afterEach(() => {
  resetMockBuild();
  for (const id of added) mockProfiles.delete(id);
});

const connect = (profileId: string) => mockInvoke<ConnectionInfo>("connect", { profileId });
const sample = () => connect("p-mock-duckdb");
const archive = () => connect("p-mock-duckdb-ro");
const memory = () => connect("p-mock-duckdb-memory");

async function run(connectionId: string, sql: string, options: object = {}) {
  const events: QueryEvent[] = [];
  const results = await mockInvoke<StatementResult[]>("run_sql", {
    connectionId,
    queryId: "q",
    sql,
    options,
    onEvent: { send: (e: QueryEvent) => events.push(e) },
  });
  const rows = events.flatMap((e) => (e.event === "rows" ? e.data.rows : []));
  return { results, rows, events };
}

const orders = { schema: "main", name: "orders" };
const page = (connectionId: string, req: Record<string, unknown> = {}) =>
  mockInvoke<BrowsePage>("browse_table", {
    connectionId,
    req: { table: orders, columns: null, filter: null, sort: [], limit: 10, offset: 0, ...req },
  });
const raw = (sql: string) => ({ conjunction: "and", items: [{ kind: "raw", sql }] });
const id = (n: number) => ({ kind: "pk", value: { id: { t: "int", v: n } } });
const int = (v: number): Value => ({ t: "int", v });

/** A profile like `id`'s with `patch` over it, under a new id. */
function variant(from: string, newId: string, patch: (p: ConnectionProfile) => object) {
  const p = mockProfiles.get(from)!;
  mockProfiles.set(newId, { ...p, id: newId, ...patch(p) });
  added.push(newId);
}

describe("the mock driver", () => {
  it("is DuckDB's, with seed profiles for ?mockDrivers=duckdb", () => {
    expect(lane.drivers.map((d) => d.driver)).toEqual(["duckdb"]);
    expect(added).toEqual(["p-mock-duckdb", "p-mock-duckdb-ro", "p-mock-duckdb-memory"]);
  });

  it("grants the caps the backend records (caps.json) for each kind of session", async () => {
    expect((await sample()).capabilities).toEqual(RECORDED.readWrite);
    expect((await archive()).capabilities).toEqual(RECORDED.readOnly);
    expect((await memory()).capabilities).toEqual(RECORDED.memory);
    variant("p-mock-duckdb", "p-cmd", (p) => ({
      options: { ...p.options, passwordMode: "command", passwordCommand: "printf ''" },
    }));
    expect((await connect("p-cmd")).capabilities).toEqual(RECORDED.command);
    // An encrypted file (read-only, the profile not): no edits, ClassifierOnly, not hardened.
    const encrypted = duckdbCaps({
      serverReadOnly: false,
      hardened: false,
      fileReadOnly: true,
      shared: true,
      passcode: false,
    });
    expect(encrypted.features).not.toEqual(expect.arrayContaining([...EDIT_FEATURES]));
    for (const f of EDIT_FEATURES) expect(encrypted.features).not.toContain(f);
    expect(encrypted.safety).toEqual({
      readOnly: "classifierOnly",
      hardened: false,
      confirm: "password",
    });
  });

  it("keeps the backend's lists (session.rs)", () => {
    const rs = readFileSync(path.join(ROOT, "src-tauri/src/drivers/duckdb/session.rs"), "utf8");
    const listed = (name: string, kind: string) =>
      [
        ...(
          new RegExp(`const ${name}: \\[${kind}; \\d+\\] = \\[([^\\]]*)\\];`).exec(rs)?.[1] ?? ""
        ).matchAll(new RegExp(`${kind}::(\\w+)`, "g")),
      ].map(([, v]) => v!.charAt(0).toLowerCase() + v!.slice(1));
    expect(EDIT_FEATURES).toEqual(listed("EDIT_FEATURES", "Feature"));
    const all = duckdbCaps({
      serverReadOnly: false,
      hardened: false,
      fileReadOnly: false,
      shared: true,
      passcode: false,
    });
    expect([...all.features].sort()).toEqual(
      [
        "editor",
        "structureView",
        "switchDatabase",
        ...listed("FEATURES", "Feature"),
        ...EDIT_FEATURES,
      ].sort(),
    );
    expect(all.data.filterOps).toEqual(listed("FILTER_OPS", "FilterOp"));
  });

  it("refuses what the backend's validate and open refuse, in its words", async () => {
    const refused = async (patch: (p: ConnectionProfile) => object, message: string) => {
      const newId = `p-x${added.length}`;
      variant("p-mock-duckdb", newId, patch);
      await expect(connect(newId)).rejects.toMatchObject({ message });
    };
    await refused(
      (p) => ({ sqlitePath: ":memory:", options: { ...p.options, readOnly: true } }),
      MEMORY_READ_ONLY,
    );
    await refused(
      (p) => ({ options: { ...p.options, startupSql: "CALL enable_logging()" } }),
      `Startup SQL: ${LOGGER_REFUSED}`,
    );
    await refused(
      (p) => ({ options: { ...p.options, driverParams: { autoloadExtensions: "yes" } } }),
      'DuckDB settings: invalid type: string "yes", expected a boolean',
    );
    await refused(
      () => ({ sqlitePath: "/Users/me/data/moved.duckdb" }),
      "The database file \u201c/Users/me/data/moved.duckdb\u201d cannot be opened: No such file or directory (os error 2)",
    );
    await refused(
      (p) => ({ options: { ...p.options, startupSql: "SELECT * FROM nowhere" } }),
      "startup SQL failed (SELECT * FROM nowhere): Catalog Error: Table with name nowhere does not exist!",
    );
  });

  it("connects with DuckDB's version and lists the catalog", async () => {
    const info = await sample();
    expect(info.serverVersion).toBe("DuckDB v1.5.6 (069cc9f9b5)");
    const c = info.connectionId;
    expect(await mockInvoke<DatabaseInfo[]>("list_databases", { connectionId: c })).toEqual([
      { name: "sample", owner: null, encoding: null, isCurrent: true },
    ]);
    expect(await mockInvoke<SchemaInfo[]>("list_schemas", { connectionId: c })).toEqual([
      { name: "crm", isDefault: false },
      { name: "main", isDefault: true },
    ]);
    const tables = await mockInvoke<TableSummary[]>("list_tables", {
      connectionId: c,
      schema: "main",
    });
    expect(tables.map((t) => `${t.name}:${t.kind}:${t.rowEstimate}`)).toEqual([
      "all_types:table:2",
      "customers:table:3",
      "events:table:2",
      "order_items:table:3",
      "orders:table:3",
      "page_views:table:1240",
      "recent_orders:view:null",
    ]);
    const meta = await mockInvoke<TableMeta>("get_table", { connectionId: c, table: orders });
    expect(meta.primaryKey).toEqual(["id"]);
    expect(meta.columns.find((x) => x.name === "id")).toMatchObject({
      dataType: "BIGINT",
      kind: "bigInt",
      isAutoIncrement: true,
      defaultExpr: "nextval('order_seq')",
    });
    expect(meta.indexes.map((i) => i.name)).toEqual([
      "orders_id_pkey",
      "orders_placed",
      "orders_customer_status",
    ]);
    expect(meta.foreignKeys[0]).toMatchObject({ refTable: { schema: "main", name: "customers" } });
    expect(meta.checks).toEqual([
      { name: "orders_total_check", expression: "(total >= 0)", columns: ["total"] },
    ]);
    const routines = await mockInvoke<RoutineSummary[]>("list_routines", {
      connectionId: c,
      schema: "main",
    });
    expect(routines.map((r) => `${r.name}(${r.arguments}) ${r.returnType}`)).toEqual([
      "add_tax(x, rate) MACRO",
      "top_orders(n) TABLE",
    ]);
    expect(
      await mockInvoke<string>("get_routine_definition", {
        connectionId: c,
        routine: { schema: "main", name: "add_tax", kind: "function", identityArguments: "" },
      }),
    ).toBe('CREATE MACRO "main"."add_tax"("x", "rate") AS (x * (1 + rate));');
    const ddl = await mockInvoke<string>("get_table_ddl", { connectionId: c, table: orders });
    expect(ddl).toMatch(
      /^CREATE TABLE orders\(id BIGINT NOT NULL DEFAULT\(nextval\('order_seq'\)\)/,
    );
    expect(ddl).toContain("CREATE INDEX orders_placed ON orders(placed_at);");
    expect(await mockInvoke<SessionDetails>("session_details", { connectionId: c })).toEqual({
      tls: false,
      tlsVersion: null,
      tlsCipher: null,
      encoding: "UTF-8",
    });
  });

  it("pages by offset, counts and fetches a whole cell", async () => {
    const c = (await sample()).connectionId;
    const views = { schema: "main", name: "page_views" };
    const first = await page(c, { table: views, limit: 100 });
    expect(first.rows).toHaveLength(100);
    expect(first.identity).toBe("pk");
    expect(first.sql).toBe(
      'SELECT "id", "path", "viewed_at", "agent", "load_ms" FROM "main"."page_views" ORDER BY "id" ASC LIMIT 100 OFFSET 0',
    );
    const last = await page(c, { table: views, limit: 100, offset: 1200 });
    expect(last.rows).toHaveLength(40);
    expect(last.rows[0]![0]).toEqual(int(1201));
    const count = await mockInvoke<RowCount>("count_table_rows", {
      connectionId: c,
      table: views,
      filter: null,
      exact: true,
    });
    expect(count).toEqual({ count: 1240, exact: true });
    // No primary key: the rows cannot be edited.
    expect((await page(c, { table: { schema: "main", name: "events" } })).identity).toBe("none");
    const types = await page(c, { table: { schema: "main", name: "all_types" } });
    const cell = (name: string) => types.rows[0]![types.columns.findIndex((x) => x.name === name)];
    expect(cell("hi")).toEqual({ t: "bigInt", v: "-170141183460469231731687303715884105728" });
    expect(cell("bl")).toEqual({ t: "bytes", v: { base64: "qgBhYg==", len: 4, truncated: false } });
    expect(cell("bt")).toEqual({ t: "other", v: { ty: "bit", text: "10101" } });
    expect(
      await mockInvoke<Value>("fetch_cell", {
        connectionId: c,
        table: orders,
        key: id(1000),
        column: "meta",
      }),
    ).toEqual({ t: "json", v: '{"coupon": "SPRING"}' });
    // The view shows what its query would.
    const recent = await page(c, { table: { schema: "main", name: "recent_orders" } });
    expect(recent.rows.map((r) => r[0])).toEqual([int(1000), int(1002)]);
  });

  it("commits staged changes in one transaction that a failing change rolls back", async () => {
    const c = (await sample()).connectionId;
    const req = {
      table: orders,
      returning: true,
      changes: [
        { op: "update", rowId: "r1", key: id(1001), changes: { status: { t: "text", v: "paid" } } },
        {
          op: "insert",
          rowId: "r2",
          values: { customer_id: int(3), total: { t: "decimal", v: "7.50" } },
        },
      ],
    };
    expect(await mockInvoke<string[]>("preview_changes", { connectionId: c, req })).toEqual([
      `UPDATE "main"."orders" SET "status" = 'paid' WHERE "id" = 1001 RETURNING "id", "customer_id", "total", "status", "placed_at", "meta"`,
      `INSERT INTO "main"."orders" ("customer_id", "total") VALUES (3, 7.50) RETURNING "id", "customer_id", "total", "status", "placed_at", "meta"`,
    ]);
    const ok = await mockInvoke<ApplyResult>("apply_changes", { connectionId: c, req });
    expect(ok.success).toBe(true);
    // RETURNING: the sequence's next id and the defaults.
    expect(ok.results[1]!.returned).toMatchObject({
      id: int(1003),
      status: { t: "text", v: "new" },
    });
    // A CHECK that fails: nothing of the commit stays, the error names its statement.
    const failing = await mockInvoke<ApplyResult>("apply_changes", {
      connectionId: c,
      req: {
        table: orders,
        returning: false,
        changes: [
          { op: "delete", rowId: "d", key: id(1003) },
          {
            op: "update",
            rowId: "u",
            key: id(1000),
            changes: { total: { t: "decimal", v: "-1.00" } },
          },
        ],
      },
    });
    expect(failing.success).toBe(false);
    expect(failing.error).toMatchObject({
      message:
        "Constraint Error: CHECK constraint failed on table orders with expression CHECK((total >= 0))",
      statementIndex: 1,
      sqlstate: null,
    });
    expect(failing.results.map((r) => r.error !== null)).toEqual([false, true]);
    expect((await page(c)).rows.map((r) => r[0])).toEqual([
      int(1000),
      int(1001),
      int(1002),
      int(1003),
    ]);
    // A row someone else deleted fails the affected-rows guard.
    const gone = await mockInvoke<ApplyResult>("apply_changes", {
      connectionId: c,
      req: { table: orders, returning: false, changes: [{ op: "delete", rowId: "x", key: id(9) }] },
    });
    expect(gone.error?.message).toBe(GUARD_MESSAGE);
    // What DuckDB SQL cannot write.
    await expect(
      mockInvoke("preview_changes", {
        connectionId: c,
        req: {
          table: orders,
          returning: false,
          changes: [
            { op: "update", rowId: "u", key: id(1000), changes: {}, exprs: { status: "default" } },
          ],
        },
      }),
    ).rejects.toMatchObject({
      message: "unsupported: DuckDB SQL cannot SET a column to DEFAULT; enter the value explicitly",
    });
    await expect(
      mockInvoke("preview_changes", {
        connectionId: c,
        req: {
          table: { schema: "main", name: "events" },
          returning: false,
          changes: [{ op: "delete", rowId: "e", key: { kind: "allColumns", value: {} } }],
        },
      }),
    ).rejects.toMatchObject({
      message: "unsupported: DuckDB: editing rows without a primary key is not available",
    });
  });

  it("runs the editor's statements and refuses a logger switch in every session", async () => {
    const c = (await sample()).connectionId;
    const { results, rows } = await run(
      c,
      "SELECT version();\nFROM orders WHERE status = 'paid';\nUPDATE orders SET status = 'x' WHERE id = 1001;\nEXPLAIN SELECT * FROM orders",
    );
    expect(results.map((r) => r.kind)).toEqual(["rows", "rows", "affected", "rows"]);
    expect(rows[0]).toEqual([{ t: "text", v: "v1.5.6" }]);
    expect(rows[1]![0]).toEqual(int(1000));
    expect(results[2]!.affected).toBe(1);
    expect(rows[2]![0]).toEqual({ t: "text", v: "physical_plan" });
    const failed = await run(c, "SELECT * FROM nowhere");
    expect(failed.results[0]!.error).toMatchObject({
      message: "Catalog Error: Table with name nowhere does not exist!",
      sqlstate: null,
    });
    // No files in the mock: a replacement scan finds none.
    expect((await run(c, "SELECT * FROM 'x.csv'")).results[0]!.error?.message).toBe(
      'IO Error: No files found that match the pattern "x.csv"',
    );
    await expect(run(c, "CALL enable_logging(storage := 'file')")).rejects.toMatchObject({
      message: LOGGER_REFUSED,
    });
    await expect(
      run(c, "SELECT * FROM query('SELECT * FROM enable_' || 'logging()')"),
    ).rejects.toMatchObject({
      message: LOGGER_REFUSED,
    });
  });

  it("refuses a read-only session's writes before anything runs, and its files", async () => {
    const c = (await archive()).connectionId;
    expect((await run(c, "SELECT count(*) FROM orders")).rows).toEqual([[int(3)]]);
    for (const sql of [
      "DELETE FROM orders",
      "SELECT * FROM read_csv('x.csv')",
      "COPY orders TO 'x.csv'",
      "SET enable_external_access = true",
      "frobnicate",
    ])
      await expect(run(c, sql), sql).rejects.toMatchObject({ message: READ_ONLY_MESSAGE });
    // The classifier reads a replacement scan; the hardened engine refuses the file.
    expect((await run(c, "SELECT * FROM 'C:/data/x.csv'")).results[0]!.error).toMatchObject({
      message:
        'Permission Error: Cannot access file "C:/data/x.csv" - file system operations are disabled by configuration',
      hint: EXTERNAL_ACCESS_HINT,
    });
    // A grid filter that writes.
    await expect(page(c, { filter: raw("nextval('order_seq') > 0") })).rejects.toMatchObject({
      message: READ_ONLY_MESSAGE,
    });
    await expect(
      mockInvoke("apply_changes", {
        connectionId: c,
        req: {
          table: orders,
          returning: false,
          changes: [{ op: "delete", rowId: "d", key: id(1000) }],
        },
      }),
    ).rejects.toMatchObject({ message: READ_ONLY_EDIT });
  });

  it("refuses a plugin's run that reaches files unless the engine is hardened", async () => {
    const plugin = { origin: "plugin" };
    const c = (await sample()).connectionId;
    for (const sql of [
      "SELECT * FROM 'x.csv'",
      "DESCRIBE 'x.csv'",
      "SELECT * FROM read_csv('x.csv')",
      "SELECT 1; COPY orders TO 'x.csv'",
      "SET threads = 1",
    ])
      await expect(run(c, sql, plugin), sql).rejects.toMatchObject({
        message: PLUGIN_FILES_REFUSED,
      });
    const ran = await run(
      c,
      "SELECT count(*) FROM orders; DELETE FROM orders WHERE id = 1001",
      plugin,
    );
    expect(ran.results.map((r) => r.error)).toEqual([null, null]);
    // The user's own run is not a plugin's: the engine looks for the file.
    expect((await run(c, "SELECT * FROM 'x.csv'")).results[0]!.error?.message).toMatch(/^IO Error/);
    // Hardened: admitted, and the engine refuses the file.
    const ro = (await archive()).connectionId;
    expect(
      (await run(ro, "SELECT * FROM 'C:/data/x.csv'", plugin)).results[0]!.error?.message,
    ).toMatch(/^Permission Error/);
  });

  it("reports the file's catalog as the current database", async () => {
    expect((await sample()).currentDatabase).toBe("sample");
    expect((await memory()).currentDatabase).toBe("memory");
  });

  it("keeps an in-memory database to its session", async () => {
    const a = (await memory()).connectionId;
    expect(await mockInvoke<DatabaseInfo[]>("list_databases", { connectionId: a })).toEqual([
      { name: "memory", owner: null, encoding: null, isCurrent: true },
    ]);
    expect(
      await mockInvoke<TableSummary[]>("list_tables", { connectionId: a, schema: "main" }),
    ).toEqual([]);
  });
});

describe("the form's calls (entry::util)", () => {
  const util = (
    method: string,
    args: unknown,
    from = "p-mock-duckdb",
    password: string | null = null,
  ) =>
    mockInvoke("driver_util", {
      input: {
        profile: mockProfiles.get(from),
        password,
        sshPassword: null,
        sshPassphrase: null,
        clearPassword: false,
      },
      method,
      args,
    });

  it("checks a file, creates a new one once, and knows no other call", async () => {
    buildMockDrivers(["duckdb"]);
    expect(await util("checkFile", { path: "/Users/me/data/sample.duckdb" })).toEqual({
      path: "/Users/me/data/sample.duckdb",
    });
    await expect(util("checkFile", { path: "/nope.duckdb" })).rejects.toMatchObject({
      message:
        "The database file \u201c/nope.duckdb\u201d cannot be opened: No such file or directory (os error 2)",
    });
    expect(await util("createFile", { path: " /Users/me/new.duckdb " })).toEqual({
      path: "/Users/me/new.duckdb",
    });
    await expect(util("createFile", { path: "/Users/me/new.duckdb" })).rejects.toMatchObject({
      message: "\u201c/Users/me/new.duckdb\u201d already exists.",
    });
    expect(await util("checkFile", { path: "/Users/me/new.duckdb" })).toEqual({
      path: "/Users/me/new.duckdb",
    });
    // Without a path, the form's file.
    expect(await util("checkFile", {})).toEqual({ path: "/Users/me/data/sample.duckdb" });
    await expect(util("createFile", {})).rejects.toMatchObject({
      message: "“/Users/me/data/sample.duckdb” already exists.",
    });
    variant("p-mock-duckdb", "p-no-file", () => ({ sqlitePath: " " }));
    await expect(util("createFile", {}, "p-no-file")).rejects.toMatchObject({
      message: "DuckDB: choose a database file",
    });
    await expect(util("vacuum", null)).rejects.toMatchObject({ code: "UNSUPPORTED" });
    // The new file opens empty, its catalog named after it.
    variant("p-mock-duckdb", "p-new", () => ({ sqlitePath: "/Users/me/new.duckdb" }));
    const c = (await connect("p-new")).connectionId;
    expect(await mockInvoke<DatabaseInfo[]>("list_databases", { connectionId: c })).toEqual([
      { name: "new", owner: null, encoding: null, isCurrent: true },
    ]);
  });

  it("refuses a key and, on a read-only profile, anything but a check", async () => {
    buildMockDrivers(["duckdb"]);
    expect(lane.drivers[0]!.utilKind?.("checkFile")).toBe("read");
    expect(lane.drivers[0]!.utilKind?.("createFile")).toBe("write");
    expect(lane.drivers[0]!.utilKind?.("vacuum")).toBe("write");
    await expect(
      util("createFile", { path: "/Users/me/k.duckdb" }, "p-mock-duckdb", "s3cret"),
    ).rejects.toMatchObject({ message: NO_ENCRYPTED_FILES });
    await expect(
      util("createFile", { path: "/Users/me/ro.duckdb" }, "p-mock-duckdb-ro"),
    ).rejects.toMatchObject({ message: "This connection is read-only" });
    expect(await util("checkFile", {}, "p-mock-duckdb-ro")).toEqual({
      path: "/Users/me/data/archive-2023.duckdb",
    });
  });
});
