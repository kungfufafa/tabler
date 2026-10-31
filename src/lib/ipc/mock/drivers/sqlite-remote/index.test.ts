import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import cases from "@/features/safety/classify.cases.json";
import type { ApplyRequest } from "../../../bindings/ApplyRequest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { ConnectionInfo } from "../../../bindings/ConnectionInfo";
import type { DatabaseInfo } from "../../../bindings/DatabaseInfo";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { SessionDetails } from "../../../bindings/SessionDetails";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { TableSummary } from "../../../bindings/TableSummary";
import { mockProfiles } from "../../handlers";
import { mockInvoke } from "../../transport";
import { enableMockDrivers, resetMockBuild } from "..";
import { profile } from "../../fixtures";
import lane, { D1_ACCOUNT, libsqlTls } from ".";
import { resetDatabases } from "./data";
import { D1_NO_TRANSACTIONS } from "./sql";
import { hidesCodeInAVariable, READ_ONLY_MESSAGE, refuseWrite, remoteCaps } from "./session";

// The mock's LibSQL and D1 against the backend's behaviour (sqlite_http/*/session.rs and their
// session tests): caps, catalog, offset pages, counts, a staged commit with a failing change, the
// editor, and the refusals of a read-only session.

beforeAll(async () => {
  await import("../..");
});

let added: string[] = [];

beforeEach(async () => {
  resetDatabases();
  added = (await enableMockDrivers(["sqlite-remote"])).map((p) => p.id);
});

afterEach(() => {
  resetMockBuild();
  for (const id of added) mockProfiles.delete(id);
});

const connect = (profileId: string) => mockInvoke<ConnectionInfo>("connect", { profileId });

async function run(connectionId: string, sql: string) {
  const events: QueryEvent[] = [];
  const results = await mockInvoke<StatementResult[]>("run_sql", {
    connectionId,
    queryId: "q",
    sql,
    options: {},
    onEvent: { send: (e: QueryEvent) => events.push(e) },
  });
  const rows = events.flatMap((e) => (e.event === "rows" ? e.data.rows : []));
  return { results, rows };
}

const customers = { schema: "main", name: "customers" };
const page = (connectionId: string, req: Record<string, unknown> = {}) =>
  mockInvoke<BrowsePage>("browse_table", {
    connectionId,
    req: { table: customers, columns: null, filter: null, sort: [], limit: 100, offset: 0, ...req },
  });

// The backend's `HIDDEN_WRITES` (d1/session_tests.rs).
const HIDDEN_WRITES = [
  "SELECT $a(--) ; DELETE FROM t",
  "SELECT $a(') ; DELETE FROM t ; SELECT $b(')",
  "SELECT $a(/*) ; DELETE FROM t ; SELECT 1 /* */",
  "SELECT :a(--) ; DELETE FROM t",
  "SELECT @a(--) ; DELETE FROM t",
  "SELECT $a::b(--) ; DELETE FROM t",
  "WITH x AS (SELECT $a(--)) DELETE FROM t",
  "WITH x AS (SELECT $a(')) DELETE FROM t WHERE $b(') IS NULL",
  "WITH x AS (SELECT $a(/*)) DELETE FROM t /* */",
  'SELECT $a(") ; DELETE FROM t ; SELECT $b(")',
  "SELECT $a([) ; DELETE FROM t ; SELECT ]",
];

describe("the mock drivers of the lane", () => {
  it("are LibSQL and D1, with seed profiles for ?mockDrivers=sqlite-remote", () => {
    expect(lane.drivers.map((d) => d.driver)).toEqual(["libsql", "d1"]);
    expect(added).toEqual(["p-mock-libsql", "p-mock-sqld", "p-mock-d1", "p-mock-d1-ro"]);
  });

  it("connect with the backend's version and caps", async () => {
    const ls = await connect("p-mock-libsql");
    expect(ls.serverVersion).toBe("LibSQL (SQLite 3.45.1)");
    expect(ls.capabilities).toEqual(remoteCaps("libsql"));
    const caps = ls.capabilities!;
    expect(caps.features).toContain("editWithoutKey");
    expect(caps.features).toContain("returning");
    expect(caps.features).not.toContain("switchDatabase");
    expect(caps.features).not.toContain("structureEdit");
    expect(caps.data).toMatchObject({ count: "costly", transactions: "atomic", maxParams: 32000 });
    expect(caps.safety).toEqual({
      readOnly: "classifierOnly",
      hardened: false,
      confirm: "passcode",
    });
    const d1 = await connect("p-mock-d1");
    expect(d1.serverVersion).toBe("Cloudflare D1 (SQLite 3.45.0)");
    const d1caps = d1.capabilities!;
    expect(d1caps.features).toContain("switchDatabase");
    expect(d1caps.features).not.toContain("editWithoutKey");
    expect(d1caps.features).not.toContain("returning");
    expect(d1caps.namespaces.databases).toBe(true);
    expect(d1caps.data.maxParams).toBe(100);
  });

  it("refuse a profile the backend's validate refuses, in its words", async () => {
    const d1 = mockProfiles.get("p-mock-d1")!;
    const bad = (id: string, patch: object) => mockProfiles.set(id, { ...d1, id, ...patch });
    bad("p-x1", { options: { ...d1.options, driverParams: { accountId: "abc" } } });
    await expect(connect("p-x1")).rejects.toMatchObject({
      code: "INVALID_INPUT",
      message:
        "the Cloudflare account ID is 32 hexadecimal characters (Workers & Pages ▸ Overview)",
    });
    // No credential in the API base, and the error does not echo it.
    for (const apiBase of [
      "https://u:SECRET@gw.example/client/v4",
      "https://gw.example/?key=SECRET",
      "https://gw.example/v4#SECRET",
    ]) {
      bad("p-x1", {
        options: { ...d1.options, driverParams: { accountId: D1_ACCOUNT, apiBase } },
      });
      await expect(connect("p-x1"), apiBase).rejects.toMatchObject({
        code: "INVALID_INPUT",
        message: expect.stringMatching(/^Cloudflare D1: the API base takes no user(?!.*SECRET)/),
      });
    }
    bad("p-x2", { database: "nope" });
    await expect(connect("p-x2")).rejects.toMatchObject({
      code: "NOT_FOUND",
      message: "not found: Cloudflare D1 database “nope”",
    });
    const ls = mockProfiles.get("p-mock-libsql")!;
    mockProfiles.set("p-x3", {
      ...ls,
      id: "p-x3",
      options: { ...ls.options, driverParams: { url: "libsql://h?authToken=t" } },
    });
    await expect(connect("p-x3")).rejects.toMatchObject({ code: "INVALID_INPUT" });
    // Any user or password, a token key in any casing, empty or repeated (`has_secret`).
    for (const url of [
      "https://:pw@h/db",
      "https://h/db?authToken=SECRET&authToken=",
      "https://h/db?auth_token=",
      "https://h/db?AuthToken=S",
      "u:p@h",
    ]) {
      mockProfiles.set("p-x4", {
        ...ls,
        id: "p-x4",
        options: { ...ls.options, driverParams: { url } },
      });
      await expect(connect("p-x4"), url).rejects.toMatchObject({
        code: "INVALID_INPUT",
        message: expect.stringContaining("Auth Token"),
      });
    }
    mockProfiles.set("p-x4", {
      ...ls,
      id: "p-x4",
      options: { ...ls.options, driverParams: { url: "https://h/db?x=1" } },
    });
    await expect(connect("p-x4")).resolves.toMatchObject({ readOnly: false });
    for (const id of ["p-x1", "p-x2", "p-x3", "p-x4"]) mockProfiles.delete(id);
  });
});

describe("a session", () => {
  it("lists SQLite's catalog under main, D1's databases of the account", async () => {
    const { connectionId } = await connect("p-mock-d1");
    const dbs = await mockInvoke<DatabaseInfo[]>("list_databases", { connectionId });
    expect(dbs.map((d) => [d.name, d.isCurrent])).toEqual([
      ["shop", true],
      ["analytics", false],
    ]);
    expect(await mockInvoke("list_schemas", { connectionId })).toEqual([
      { name: "main", isDefault: true },
    ]);
    const tables = await mockInvoke<TableSummary[]>("list_tables", {
      connectionId,
      schema: "main",
    });
    expect(tables.map((t) => [t.name, t.kind])).toEqual([
      ["customers", "table"],
      ["orders", "table"],
      ["events", "table"],
      ["big_spenders", "view"],
      ["d1_migrations", "table"],
    ]);
    expect(await mockInvoke("list_tables", { connectionId, schema: "temp" })).toEqual([]);
    const meta = await mockInvoke<TableMeta>("get_table", {
      connectionId,
      table: { schema: "main", name: "orders" },
    });
    expect(meta.primaryKey).toEqual(["id"]);
    expect(meta.foreignKeys[0]?.refTable).toEqual({ schema: "main", name: "customers" });
    const ddl = await mockInvoke<string>("get_table_ddl", {
      connectionId,
      table: { schema: "main", name: "orders" },
    });
    expect(ddl).toMatch(/^CREATE TABLE "orders"/);
    expect(ddl).toContain('CREATE INDEX "orders_customer"');
    expect(await mockInvoke<SessionDetails>("session_details", { connectionId })).toEqual({
      tls: true,
      tlsVersion: null,
      tlsCipher: null,
      encoding: "UTF-8",
    });
    const ls = await connect("p-mock-sqld");
    expect(await mockInvoke("list_databases", { connectionId: ls.connectionId })).toEqual([
      { name: "127.0.0.1", isCurrent: true, owner: null, encoding: null },
    ]);
    expect(
      (await mockInvoke<SessionDetails>("session_details", { connectionId: ls.connectionId })).tls,
    ).toBe(false);
  });

  it("browses by offset, sorts, filters and counts; big integers stay exact", async () => {
    const { connectionId } = await connect("p-mock-libsql");
    const first = await page(connectionId);
    expect(first.rows).toHaveLength(100);
    expect(first.identity).toBe("pk");
    expect(first.sql).toBe(
      'SELECT "id", "name", "email", "plan", "created_at" FROM "main"."customers" LIMIT 100 OFFSET 0',
    );
    const second = await page(connectionId, { offset: 100 });
    expect(second.rows).toHaveLength(20);
    expect(second.rows[0]?.[0]).toEqual({ t: "int", v: 101 });
    const sorted = await page(connectionId, {
      sort: [{ column: "id", dir: "desc", nulls: null }],
      limit: 1,
    });
    expect(sorted.rows[0]?.[0]).toEqual({ t: "int", v: 120 });
    const pro = {
      conjunction: "and",
      items: [{ kind: "condition", column: "plan", op: "eq", values: [{ t: "text", v: "pro" }] }],
    };
    expect(
      await mockInvoke("count_table_rows", { connectionId, table: customers, filter: pro }),
    ).toEqual({ count: 40, exact: true });
    const orders = await page(connectionId, {
      table: { schema: "main", name: "orders" },
      limit: 1,
    });
    expect(orders.rows[0]?.[4]).toEqual({ t: "bigInt", v: "9007199254740993" });
    expect(
      await mockInvoke("fetch_cell", {
        connectionId,
        table: customers,
        key: { kind: "pk", value: { id: { t: "int", v: 2 } } },
        column: "email",
      }),
    ).toEqual({ t: "text", v: "grace2@example.com" });
  });

  it("edits a table without a key by rowid on LibSQL, not at all on D1", async () => {
    const events = { table: { schema: "main", name: "events" }, limit: 5 };
    const ls = await connect("p-mock-libsql");
    const lsPage = await page(ls.connectionId, events);
    expect(lsPage.identity).toBe("rowid");
    expect(lsPage.rowIds).toEqual([1, 2, 3, 4, 5]);
    const req: ApplyRequest = {
      table: events.table,
      returning: false,
      changes: [{ op: "delete", rowId: "r", key: { kind: "rowid", value: 2 } }],
    };
    expect(
      (await mockInvoke<ApplyResult>("apply_changes", { connectionId: ls.connectionId, req }))
        .success,
    ).toBe(true);
    expect((await page(ls.connectionId, events)).rowIds).toEqual([1, 3, 4, 5, 6]);
    const d1 = await connect("p-mock-d1");
    expect((await page(d1.connectionId, events)).identity).toBe("none");
    await expect(
      mockInvoke("apply_changes", { connectionId: d1.connectionId, req }),
    ).rejects.toMatchObject({ code: "UNSUPPORTED" });
  });

  it("commits staged changes as one batch: a failing change rolls back the others", async () => {
    const { connectionId } = await connect("p-mock-libsql");
    const key = (id: number) => ({
      kind: "pk" as const,
      value: { id: { t: "int" as const, v: id } },
    });
    const req: ApplyRequest = {
      table: customers,
      returning: true,
      changes: [
        { op: "update", rowId: "a", key: key(1), changes: { plan: { t: "text", v: "team" } } },
        {
          op: "insert",
          rowId: "b",
          values: { name: { t: "text", v: "Dup" }, email: { t: "text", v: "ada1@example.com" } },
          exprs: { created_at: "now" },
        },
      ],
    };
    const preview = await mockInvoke<string[]>("preview_changes", { connectionId, req });
    expect(preview).toEqual([
      `UPDATE "main"."customers" SET "plan" = 'team' WHERE "id" = 1 RETURNING *`,
      `INSERT INTO "main"."customers" ("name", "email", "created_at") VALUES ('Dup', 'ada1@example.com', CURRENT_TIMESTAMP) RETURNING *`,
    ]);
    const failed = await mockInvoke<ApplyResult>("apply_changes", { connectionId, req });
    expect(failed.success).toBe(false);
    expect(failed.error).toMatchObject({
      message: "UNIQUE constraint failed: customers.email",
      vendorCode: 2067,
      statementIndex: 1,
    });
    expect(failed.results.map((r) => r.error === null)).toEqual([true, false]);
    // Rolled back with it.
    expect(
      await mockInvoke("fetch_cell", {
        connectionId,
        table: customers,
        key: key(1),
        column: "plan",
      }),
    ).toEqual({ t: "text", v: "free" });
    // A row someone else deleted fails the affected-rows guard.
    const gone = await mockInvoke<ApplyResult>("apply_changes", {
      connectionId,
      req: { ...req, changes: [{ op: "delete", rowId: "c", key: key(999) }] },
    });
    expect(gone.error?.message).toMatch(/^expected to affect 1 row\(s\) but matched 0/);
    const ok = await mockInvoke<ApplyResult>("apply_changes", {
      connectionId,
      req: { ...req, changes: [req.changes[0]!] },
    });
    expect(ok.results[0]?.returned?.plan).toEqual({ t: "text", v: "team" });
    // D1 sends no rows back (no `Returning`) and names the primary code only.
    const d1 = await connect("p-mock-d1");
    const d1Preview = await mockInvoke<string[]>("preview_changes", {
      connectionId: d1.connectionId,
      req,
    });
    expect(d1Preview[0]).toBe(`UPDATE "main"."customers" SET "plan" = 'team' WHERE "id" = 1`);
    const d1Failed = await mockInvoke<ApplyResult>("apply_changes", {
      connectionId: d1.connectionId,
      req,
    });
    expect(d1Failed.error?.vendorCode).toBe(19);
  });

  it("runs the editor's statements, one request each", async () => {
    const { connectionId } = await connect("p-mock-libsql");
    const select = await run(
      connectionId,
      "SELECT id, email FROM customers WHERE plan = 'team' AND id <= 10 ORDER BY id DESC LIMIT 2",
    );
    expect(select.rows).toEqual([
      [
        { t: "int", v: 9 },
        { t: "text", v: "ada9@example.com" },
      ],
      [
        { t: "int", v: 6 },
        { t: "text", v: "margaret6@example.com" },
      ],
    ]);
    expect(select.results[0]?.columns.every((c) => c.table === null)).toBe(true);
    const write = await run(
      connectionId,
      "INSERT INTO customers (name, email) VALUES ('Joan', 'joan@example.com');\nUPDATE customers SET plan = 'pro' WHERE email = 'joan@example.com'\nSELECT count(*) FROM customers\nBEGIN",
    );
    expect(write.results.map((r) => [r.kind, r.affected, r.error])).toEqual([
      ["affected", 1, null],
      ["affected", 1, null],
      ["rows", null, null],
      ["empty", null, null],
    ]);
    expect(write.results[0]?.lastInsertId).toBe("121");
    expect(write.rows).toEqual([[{ t: "int", v: 121 }]]);
    const plan = await run(connectionId, "EXPLAIN QUERY PLAN SELECT * FROM orders");
    expect(plan.rows[0]?.[3]).toEqual({ t: "text", v: "SCAN orders" });
    const broken = await run(connectionId, "SELECT nope FROM customers");
    expect(broken.results[0]?.error?.message).toBe("no such column: nope");
    const d1 = await connect("p-mock-d1");
    const tx = await run(d1.connectionId, "BEGIN TRANSACTION");
    expect(tx.results[0]?.error).toMatchObject({ message: D1_NO_TRANSACTIONS, vendorCode: 23 });
    await expect(run(d1.connectionId, "SELECT 1; SELECT 2")).rejects.toMatchObject({
      message: "Cloudflare D1 would run every statement of this text: send one statement at a time",
    });
  });

  it("on D1, refuses a raw filter that adds a statement, on a writable session too", async () => {
    const { connectionId } = await connect("p-mock-d1");
    const raw = (sql: string) => ({ conjunction: "and", items: [{ kind: "raw", sql }] });
    await expect(
      page(connectionId, {
        filter: raw("1) ; DELETE FROM customers; SELECT * FROM customers WHERE (1"),
      }),
    ).rejects.toMatchObject({ code: "INVALID_INPUT" });
    await expect(
      mockInvoke("count_table_rows", {
        connectionId,
        table: customers,
        filter: raw("1) ; DELETE FROM customers; SELECT 1 WHERE (1"),
      }),
    ).rejects.toMatchObject({ code: "INVALID_INPUT" });
    expect((await page(connectionId, { filter: raw("id = 1") })).rows.length).toBeGreaterThan(0);
    // In a nested group too, and behind a variable whose suffix the shared lexer reads as a
    // comment (`api::one_statement`).
    const nested = {
      conjunction: "or",
      items: [{ kind: "group", ...raw("1) ; DELETE FROM customers; SELECT 1 WHERE (1") }],
    };
    await expect(page(connectionId, { filter: nested })).rejects.toMatchObject({
      code: "INVALID_INPUT",
    });
    await expect(
      page(connectionId, { filter: raw("$a(--) ; DELETE FROM customers") }),
    ).rejects.toMatchObject({ code: "INVALID_INPUT" });
    await expect(run(connectionId, "SELECT $a(--) ; DELETE FROM customers")).rejects.toMatchObject({
      message: "Cloudflare D1 would run every statement of this text: send one statement at a time",
    });
  });
});

describe("a read-only session", () => {
  it("refuses writes before running them, on every path, and still reads", async () => {
    const { connectionId, readOnly } = await connect("p-mock-d1-ro");
    expect(readOnly).toBe(true);
    const refused = { code: "INVALID_INPUT", message: READ_ONLY_MESSAGE };
    await expect(run(connectionId, "DELETE FROM page_views")).rejects.toMatchObject(refused);
    await expect(run(connectionId, HIDDEN_WRITES[0]!)).rejects.toMatchObject(refused);
    const req: ApplyRequest = {
      table: { schema: "main", name: "page_views" },
      returning: false,
      changes: [
        { op: "delete", rowId: "r", key: { kind: "pk", value: { id: { t: "int", v: 1 } } } },
      ],
    };
    await expect(mockInvoke("apply_changes", { connectionId, req })).rejects.toMatchObject(refused);
    const read = await run(connectionId, "SELECT count(*) FROM page_views");
    expect(read.rows).toEqual([[{ t: "int", v: 4 }]]);
    // The SELECT of a browse or a count holds the raw filters: checked as a whole (`worker_query`).
    const views = { schema: "main", name: "page_views" };
    const raw = (sql: string) => ({
      conjunction: "and",
      items: [{ kind: "group", conjunction: "and", items: [{ kind: "raw", sql }] }],
    });
    await expect(
      page(connectionId, {
        table: views,
        filter: raw("1=1) ; DELETE FROM page_views ; SELECT (1"),
      }),
    ).rejects.toMatchObject(refused);
    await expect(
      mockInvoke("count_table_rows", {
        connectionId,
        table: views,
        filter: raw("1=1) ; DELETE FROM page_views ; SELECT (1"),
      }),
    ).rejects.toMatchObject(refused);
    expect((await page(connectionId, { table: views, filter: raw("id > 0") })).rows).toHaveLength(
      4,
    );
    // The session's own calls: none, refused as unknown.
    await expect(
      mockInvoke("session_call", { connectionId, method: "vacuum" }),
    ).rejects.toMatchObject({
      code: "UNSUPPORTED",
      message: "unsupported: Cloudflare D1: unknown call “vacuum”",
    });
  });

  it("on LibSQL too, refuses a raw filter that writes", async () => {
    const ls = mockProfiles.get("p-mock-libsql")!;
    mockProfiles.set("p-ro", { ...ls, id: "p-ro", options: { ...ls.options, readOnly: true } });
    const { connectionId } = await connect("p-ro");
    const filter = {
      conjunction: "and",
      items: [{ kind: "raw", sql: "1=1) ; DELETE FROM customers ; SELECT (1" }],
    };
    await expect(page(connectionId, { filter })).rejects.toMatchObject({
      code: "INVALID_INPUT",
      message: READ_ONLY_MESSAGE,
    });
    mockProfiles.delete("p-ro");
  });

  it("on LibSQL, fails the connect when a startup command writes", async () => {
    const ls = mockProfiles.get("p-mock-libsql")!;
    const withStartup = (startupSql: string, readOnly = true) =>
      mockProfiles.set("p-ro", {
        ...ls,
        id: "p-ro",
        options: { ...ls.options, readOnly, startupSql },
      });
    // A setting given a value counts too, as in the backend's `read_only_startup_commands_cannot_write`.
    for (const startupSql of [
      "SELECT 1; DELETE FROM customers",
      "PRAGMA cache_size = 100",
      "SELECT $a(--) ; DELETE FROM customers",
    ]) {
      withStartup(startupSql);
      await expect(connect("p-ro"), startupSql).rejects.toMatchObject({
        code: "INVALID_INPUT",
        message: `${READ_ONLY_MESSAGE}: a startup command writes`,
      });
    }
    withStartup("SELECT 1; PRAGMA foreign_keys");
    // `sqld` and Turso refuse `PRAGMA query_only = 1`: the classifier alone keeps it read-only.
    await expect(connect("p-ro")).resolves.toMatchObject({
      readOnly: true,
      capabilities: { safety: { readOnly: "classifierOnly" } },
    });
    withStartup("PRAGMA cache_size = 100", false);
    await expect(connect("p-ro")).resolves.toMatchObject({ readOnly: false });
    mockProfiles.delete("p-ro");
    // `libsql::session::capabilities(query_only)`; D1 is the classifier's either way.
    expect(remoteCaps("libsql", true).safety.readOnly).toBe("perRequest");
    expect(remoteCaps("d1", true).safety.readOnly).toBe("classifierOnly");
  });

  it("refuses what the SQLite classifier calls a write (the shared cases) and hidden variables", () => {
    const sqlite = (cases as { sql: string; dialect?: string; class: string }[]).filter(
      (c) => !c.dialect || c.dialect === "sqlite",
    );
    for (const c of sqlite) {
      const refused = (() => {
        try {
          refuseWrite(true, c.sql);
          return false;
        } catch {
          return true;
        }
      })();
      expect([c.sql, refused]).toEqual([c.sql, c.class === "write" || hidesCodeInAVariable(c.sql)]);
      expect(() => refuseWrite(false, c.sql)).not.toThrow();
    }
    for (const sql of HIDDEN_WRITES) {
      expect(hidesCodeInAVariable(sql)).toBe(true);
      expect(() => refuseWrite(true, sql)).toThrow();
    }
    for (const sql of [
      "SELECT * FROM t WHERE id = ?1 AND v = :v AND n = @n AND at = $at",
      "SELECT json_extract(doc, '$.k[0]'), '12:30:00', x$a(1) FROM t",
      "SELECT $a(1), :b::c(x) FROM t",
      "SELECT 'a:b(c)', '$', ':', ''",
    ])
      expect(hidesCodeInAVariable(sql)).toBe(false);
    expect(hidesCodeInAVariable("SELECT ':a(--)'")).toBe(true);
    // Only `)` and ASCII whitespace end a variable's suffix, as in SQLite and the backend: a
    // Unicode space does not, so the `--` after it still hides the DELETE.
    for (const space of [" ", " ", "﻿"]) {
      const sql = `SELECT $a(${space}--) ; DELETE FROM t`;
      expect(hidesCodeInAVariable(sql)).toBe(true);
      expect(() => refuseWrite(true, sql)).toThrow();
    }
    expect(hidesCodeInAVariable("SELECT $a(\v--) FROM t")).toBe(false);
  });

  it("read `?tls=` as the backend does: any key casing, libsql:// only", () => {
    const tls = (url: string) => {
      const p = profile({ id: "p-tls", name: "TLS", driver: "libsql" });
      return libsqlTls({ ...p, options: { ...p.options, driverParams: { url } } });
    };
    expect(tls("libsql://h/db?TLS=0")).toBe(false);
    expect(tls("libsql://h/db?a=1&Tls=false")).toBe(false);
    expect(tls("h/db?tls=0")).toBe(false);
    expect(tls("libsql://h/db?TLS=FALSE")).toBe(true);
    expect(tls("libsql://h/db?stls=0")).toBe(true);
    expect(tls("https://h/db?TLS=0")).toBe(true);
    expect(tls("WS://h/db")).toBe(false);
  });
});

describe("the form's calls", () => {
  it("list D1's databases (a read), and nothing for LibSQL", async () => {
    const input = (id: string) => ({ profile: mockProfiles.get(id)!, password: "tok" });
    expect(
      await mockInvoke("driver_util", { input: input("p-mock-d1"), method: "listDatabases" }),
    ).toEqual([
      {
        uuid: "5a0c1f9e-2b7d-4c51-9a3e-0d6f2e8b7c41",
        name: "shop",
        createdAt: "2026-03-02T10:12:00.000Z",
        fileSize: 1232896,
      },
      {
        uuid: "c3e2d4a1-8f60-4b1e-b7d9-52a0f6e1c9d8",
        name: "analytics",
        createdAt: "2026-05-18T07:40:00.000Z",
        fileSize: 98304,
      },
    ]);
    await expect(
      mockInvoke("driver_util", { input: input("p-mock-libsql"), method: "listDatabases" }),
    ).rejects.toMatchObject({ message: "unsupported: LibSQL: unknown call “listDatabases”" });
    expect(D1_ACCOUNT).toMatch(/^[0-9a-f]{32}$/);
  });
});
