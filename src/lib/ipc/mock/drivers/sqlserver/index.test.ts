/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { isWindows } from "@/lib/platform";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { ConnectionInfo } from "../../../bindings/ConnectionInfo";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { DatabaseInfo } from "../../../bindings/DatabaseInfo";
import type { KillOutcome } from "../../../bindings/KillOutcome";
import type { ProcessRow } from "../../../bindings/ProcessRow";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { RoutineSummary } from "../../../bindings/RoutineSummary";
import type { SessionDetails } from "../../../bindings/SessionDetails";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { TableSummary } from "../../../bindings/TableSummary";
import { mockProfiles } from "../../handlers";
import { mockInvoke } from "../../transport";
import { enableMockDrivers, resetMockBuild } from "..";
import lane from ".";
import { resetSqlServer } from "./data";
import {
  ADDED_STATEMENT_MESSAGE,
  FEATURES,
  hidesStatement,
  READ_ONLY_MESSAGE,
  refusedReadOnly,
  resetProcesses,
  sqlserverCaps,
} from "./session";

// The mock's SQL Server against the backend's behaviour (drivers/sqlserver/session.rs and its
// tests): caps, catalog, offset pages, counts, a whole binary cell, a staged commit with a failing
// change, the editor's GO batches, processes and kills, and a read-only session's refusals.

beforeAll(async () => {
  await import("../..");
});

let added: string[] = [];

beforeEach(async () => {
  resetSqlServer();
  resetProcesses();
  added = (await enableMockDrivers(["sqlserver"])).map((p) => p.id);
});

afterEach(() => {
  resetMockBuild();
  for (const id of added) mockProfiles.delete(id);
});

const connect = (profileId: string) => mockInvoke<ConnectionInfo>("connect", { profileId });
const shop = () => connect("p-mock-sqlserver");
const azure = () => connect("p-mock-azure-sql");

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
  return { results, rows, events };
}

const customers = { schema: "dbo", name: "customers" };
const orders = { schema: "dbo", name: "orders" };
const invoices = { schema: "sales", name: "invoices" };
const page = (connectionId: string, req: Record<string, unknown> = {}) =>
  mockInvoke<BrowsePage>("browse_table", {
    connectionId,
    req: { table: customers, columns: null, filter: null, sort: [], limit: 10, offset: 0, ...req },
  });
const raw = (sql: string) => ({ conjunction: "and", items: [{ kind: "raw", sql }] });

/** A profile like `id`'s with `patch` over it, under a new id. */
function variant(id: string, newId: string, patch: (p: ConnectionProfile) => object) {
  const p = mockProfiles.get(id)!;
  mockProfiles.set(newId, { ...p, id: newId, ...patch(p) });
  added.push(newId);
}

describe("the mock driver", () => {
  it("grants what the backend's session.rs lists, in its order", () => {
    // The constant arrays of `drivers/sqlserver/session.rs`, read from the source, so that a
    // change there fails here.
    const rs = readFileSync(
      path.resolve(
        import.meta.dirname,
        "../../../../../../src-tauri/src/drivers/sqlserver/session.rs",
      ),
      "utf8",
    );
    const listed = (name: string, kind: string) => {
      const body = new RegExp(`const ${name}: \\[${kind}; \\d+\\] = \\[([^\\]]*)\\];`).exec(
        rs,
      )?.[1];
      return [...(body ?? "").matchAll(new RegExp(`${kind}::(\\w+)`, "g"))].map(
        ([, v]) => v!.charAt(0).toLowerCase() + v!.slice(1),
      );
    };
    const caps = sqlserverCaps("dbo", false);
    expect(caps.features).toHaveLength(18);
    expect(caps.features).toEqual(listed("FEATURES", "Feature"));
    expect(caps.data.filterOps).toEqual(listed("FILTER_OPS", "FilterOp"));
    expect(caps.data.serverExprs).toEqual(listed("SERVER_EXPRS", "ServerExpr"));
    expect(rs).toContain(`const KEEPALIVE_SECS: u32 = ${caps.session.keepaliveSecs};`);
    expect(rs).toContain("paging: Paging::Offset,");
    expect(rs).toContain("count: CountMode::Estimate,");
    expect(rs).toContain("read_only: ReadOnlyMode::ClassifierOnly,");
  });

  it("is SQL Server's, with seed profiles for ?mockDrivers=sqlserver", () => {
    expect(lane.drivers.map((d) => d.driver)).toEqual(["sqlserver"]);
    expect(added).toEqual(["p-mock-sqlserver", "p-mock-azure-sql"]);
    expect(lane.drivers[0]!.util).toBeUndefined();
  });

  it("connects with the backend's version and caps", async () => {
    const info = await shop();
    expect(info.serverVersion).toBe("SQL Server 2022 (16.0.4135.4)");
    expect(info.capabilities).toEqual(sqlserverCaps("dbo", false));
    const caps = info.capabilities!;
    expect(caps.dialect).toBe("tsql");
    expect(caps.features).toEqual(FEATURES);
    for (const f of ["format", "returning", "editWithoutKey", "mcp", "users", "exportTables"])
      expect(caps.features, f).not.toContain(f);
    expect(caps.namespaces).toMatchObject({
      databases: true,
      schemas: "schemas",
      defaultSchema: "dbo",
    });
    expect(caps.data).toMatchObject({
      paging: "offset",
      count: "estimate",
      rawFilter: "sqlWhere",
      transactions: "atomic",
      affected: "exact",
      upsert: false,
      maxParams: 2098,
    });
    expect(caps.data.filterOps).not.toContain("jsonContains");
    expect(caps.safety).toEqual({
      readOnly: "classifierOnly",
      hardened: false,
      confirm: "password",
    });
    expect(caps.session).toEqual({ keepaliveSecs: 30, pingBilled: false });
    const ro = await azure();
    expect(ro.serverVersion).toBe("Azure SQL Database (12.0.2000.8)");
    expect(ro.readOnly).toBe(true);
    // A token from a command is confirmed with the app passcode.
    expect(ro.capabilities!.safety.confirm).toBe("passcode");
  });

  it("refuses what the backend's validate refuses, in its words", async () => {
    variant("p-mock-sqlserver", "p-x1", () => ({ user: " " }));
    await expect(connect("p-x1")).rejects.toMatchObject({
      code: "INVALID_INPUT",
      message: "User is required for this authentication",
    });
    variant("p-mock-sqlserver", "p-x2", (p) => ({
      options: { ...p.options, driverParams: { packetSize: 100 } },
    }));
    await expect(connect("p-x2")).rejects.toMatchObject({
      message: "SQL Server packet size: 100 is not between 512 and 32767",
    });
    variant("p-mock-sqlserver", "p-x3", (p) => ({
      options: { ...p.options, driverParams: { instance: "a;b" } },
    }));
    await expect(connect("p-x3")).rejects.toMatchObject({ code: "INVALID_INPUT" });
    variant("p-mock-sqlserver", "p-x4", (p) => ({
      options: { ...p.options, sslCertPath: "/c.pem" },
    }));
    await expect(connect("p-x4")).rejects.toMatchObject({
      message: "SQL Server: a client certificate needs both its certificate and its key file",
    });
    variant("p-mock-sqlserver", "p-x5", (p) => ({
      user: null,
      options: { ...p.options, driverParams: { auth: "integrated" } },
    }));
    if (isWindows) await expect(connect("p-x5")).resolves.toMatchObject({ readOnly: false });
    else await expect(connect("p-x5")).rejects.toMatchObject({ code: "UNSUPPORTED" });
    variant("p-mock-sqlserver", "p-x6", () => ({ database: "nope" }));
    await expect(connect("p-x6")).rejects.toMatchObject({
      code: "DB_ERROR",
      message: 'Cannot open database "nope" requested by the login. The login failed.',
    });
    // A read-only profile whose startup commands write is refused before it connects.
    variant("p-mock-azure-sql", "p-x7", (p) => ({
      options: { ...p.options, startupSql: "SET NOCOUNT ON" },
    }));
    await expect(connect("p-x7")).rejects.toMatchObject({ code: "INVALID_INPUT" });
  });
});

describe("a session", () => {
  it("lists databases, schemas, tables, structure, routines, Show DDL and session details", async () => {
    const { connectionId } = await shop();
    const dbs = await mockInvoke<DatabaseInfo[]>("list_databases", { connectionId });
    expect(dbs.map((d) => [d.name, d.isCurrent])).toEqual([
      ["master", false],
      ["reporting", false],
      ["shop", true],
    ]);
    expect(await mockInvoke("list_schemas", { connectionId })).toEqual([
      { name: "dbo", isDefault: true },
      { name: "sales", isDefault: false },
    ]);
    const tables = await mockInvoke<TableSummary[]>("list_tables", { connectionId, schema: "dbo" });
    expect(tables.map((t) => [t.name, t.kind, t.rowEstimate])).toEqual([
      ["customers", "table", 30],
      ["orders", "table", 120],
      ["v_customer_totals", "view", null],
    ]);
    const meta = await mockInvoke<TableMeta>("get_table", { connectionId, table: orders });
    expect(meta.primaryKey).toEqual(["id"]);
    expect(meta.columns.map((c) => [c.name, c.dataType, c.kind])).toContainEqual([
      "placed_at",
      "datetimeoffset(7)",
      "timestampTz",
    ]);
    expect(meta.foreignKeys[0]).toMatchObject({ name: "FK_orders_customers", refTable: customers });
    expect(meta.indexes[1]).toMatchObject({
      include: ["total"],
      predicate: "([status]<>N'cancelled')",
    });
    expect(meta.checks[0]?.name).toBe("CK_orders_status");
    const idCol = (await mockInvoke<TableMeta>("get_table", { connectionId, table: customers }))
      .columns[0];
    expect(idCol).toMatchObject({ isAutoIncrement: true, extra: "IDENTITY(1,1)", isPk: true });
    const ddl = await mockInvoke<string>("get_table_ddl", { connectionId, table: orders });
    expect(ddl).toMatch(/^CREATE TABLE \[dbo\]\.\[orders\] \(/);
    expect(ddl).toContain("\nGO\n\nCREATE NONCLUSTERED INDEX [IX_orders_customer]");
    expect(
      await mockInvoke<string>("get_table_ddl", {
        connectionId,
        table: { schema: "dbo", name: "v_customer_totals" },
      }),
    ).toMatch(/^CREATE VIEW \[dbo\]\.\[v_customer_totals\] AS/);
    const routines = await mockInvoke<RoutineSummary[]>("list_routines", {
      connectionId,
      schema: "dbo",
    });
    expect(routines.map((r) => [r.name, r.kind])).toEqual([
      ["usp_place_order", "procedure"],
      ["fn_order_total", "function"],
      ["usp_legacy_export", "procedure"],
    ]);
    const def = (name: string) =>
      mockInvoke<string>("get_routine_definition", {
        connectionId,
        routine: { schema: "dbo", name, kind: "procedure", identityArguments: "" },
      });
    expect(await def("usp_place_order")).toMatch(/^CREATE PROCEDURE \[dbo\]\.\[usp_place_order\]/);
    expect(await def("usp_legacy_export")).toMatch(/WITH ENCRYPTION or is a CLR one/);
    expect(await mockInvoke<SessionDetails>("session_details", { connectionId })).toEqual({
      tls: true,
      tlsVersion: null,
      tlsCipher: null,
      encoding: "UTF-16",
    });
  });

  it("browses pages by offset in the key's order, sorts, filters and counts", async () => {
    const { connectionId } = await shop();
    const first = await page(connectionId);
    expect(first.rows).toHaveLength(10);
    expect(first.identity).toBe("pk");
    expect(first.sql).toBe(
      "SELECT [id], [name], [email], CONVERT(NVARCHAR(40), [balance], 2) AS [balance], [external_id], [created_at]\nFROM [dbo].[customers] ORDER BY [id] ASC\nOFFSET 0 ROWS FETCH NEXT 10 ROWS ONLY",
    );
    const last = await page(connectionId, { offset: 25 });
    expect(last.rows).toHaveLength(5);
    expect(last.rows[0]?.[0]).toEqual({ t: "int", v: 26 });
    expect(last.rows[0]?.[3]).toMatchObject({ t: "decimal" });
    const sorted = await page(connectionId, {
      sort: [{ column: "id", dir: "desc", nulls: null }],
      limit: 1,
    });
    expect(sorted.rows[0]?.[0]).toEqual({ t: "int", v: 30 });
    const noEmail = {
      conjunction: "and",
      items: [{ kind: "condition", column: "email", op: "isNull", values: [] }],
    };
    expect(
      await mockInvoke("count_table_rows", { connectionId, table: customers, filter: noEmail }),
    ).toEqual({ count: 4, exact: true });
    expect(
      await mockInvoke("count_table_rows", {
        connectionId,
        table: customers,
        filter: raw("[id] > 20"),
      }),
    ).toEqual({ count: 30, exact: true });
    // bigint past 2^53 as bigInt text, datetimeoffset in its own offset.
    const o = await page(connectionId, { table: orders, limit: 1 });
    expect(o.rows[0]?.[0]).toEqual({ t: "int", v: 1000 });
    expect(o.rows[0]?.[5]).toEqual({ t: "bigInt", v: "9007199254740993" });
    expect(o.rows[0]?.[4]).toMatchObject({
      t: "timestampTz",
      v: expect.stringMatching(/\+07:00$/),
    });
  });

  it("previews a big binary cell and fetches it whole; a keyless table has no identity", async () => {
    const { connectionId } = await shop();
    const inv = await page(connectionId, { table: invoices });
    expect(inv.identity).toBe("none");
    expect(inv.pkColumns).toEqual([]);
    expect(inv.sql).toContain("ORDER BY (SELECT NULL)");
    const cell = inv.rows[0]?.[4];
    expect(cell).toMatchObject({ t: "bytes", v: { len: 70_000, truncated: true } });
    const { connectionId: id2 } = await shop();
    const o = await mockInvoke("fetch_cell", {
      connectionId: id2,
      table: customers,
      key: { kind: "pk", value: { id: { t: "int", v: 2 } } },
      column: "email",
    });
    expect(o).toEqual({ t: "text", v: "grace2@example.com" });
  });

  it("refuses a raw filter that adds a statement, on a writable session too", async () => {
    const { connectionId } = await shop();
    for (const filter of [
      "1=1) DELETE FROM dbo.orders WHERE (1=1",
      "1=1 {call p}",
      "[id] = 1 DUMP DATABASE shop TO x",
    ])
      await expect(page(connectionId, { filter: raw(filter) }), filter).rejects.toMatchObject({
        code: "INVALID_INPUT",
        message: ADDED_STATEMENT_MESSAGE,
      });
    expect((await page(connectionId, { table: orders, limit: 200 })).rows).toHaveLength(120);
  });

  it("commits staged changes in one transaction, and a failing change rolls back all", async () => {
    const { connectionId } = await shop();
    const req = (changes: object[]) => ({ table: customers, changes, returning: false });
    const good = [
      {
        op: "update",
        rowId: "r1",
        key: { kind: "pk", value: { id: { t: "int", v: 1 } } },
        changes: { name: { t: "text", v: "Ada K." } },
      },
      {
        op: "insert",
        rowId: "r2",
        values: {
          name: { t: "text", v: "Hedy Lamarr" },
          email: { t: "text", v: "hedy@example.com" },
        },
      },
    ];
    expect(await mockInvoke<string[]>("preview_changes", { connectionId, req: req(good) })).toEqual(
      [
        "UPDATE [dbo].[customers] SET [name] = N'Ada K.' WHERE [id] = 1",
        "INSERT INTO [dbo].[customers] ([email], [name]) VALUES (N'hedy@example.com', N'Hedy Lamarr')",
      ],
    );
    const ok = await mockInvoke<ApplyResult>("apply_changes", { connectionId, req: req(good) });
    expect(ok.success).toBe(true);
    expect(ok.results.map((r) => r.affected)).toEqual([1, 1]);
    // The identity column took the next value.
    expect((await page(connectionId, { offset: 30 })).rows[0]?.slice(0, 2)).toEqual([
      { t: "int", v: 31 },
      { t: "text", v: "Hedy Lamarr" },
    ]);
    const bad = [
      { op: "delete", rowId: "d1", key: { kind: "pk", value: { id: { t: "int", v: 31 } } } },
      {
        op: "update",
        rowId: "u1",
        key: { kind: "pk", value: { id: { t: "int", v: 2 } } },
        changes: { email: { t: "text", v: "ada1@example.com" } },
      },
    ];
    const failed = await mockInvoke<ApplyResult>("apply_changes", { connectionId, req: req(bad) });
    expect(failed.success).toBe(false);
    expect(failed.error).toMatchObject({
      code: "DB_ERROR",
      vendorCode: 2627,
      statementIndex: 1,
      message: expect.stringContaining("Violation of UNIQUE KEY constraint 'UQ_customers_email'"),
    });
    expect(failed.results.map((r) => r.error?.vendorCode ?? null)).toEqual([null, 2627]);
    // The DELETE before it was rolled back with it.
    expect((await page(connectionId, { offset: 30 })).rows).toHaveLength(1);
    // A row that is gone fails the @@ROWCOUNT guard.
    const gone = await mockInvoke<ApplyResult>("apply_changes", {
      connectionId,
      req: req([
        { op: "delete", rowId: "x", key: { kind: "pk", value: { id: { t: "int", v: 999 } } } },
      ]),
    });
    expect(gone.error?.message).toMatch(/^expected to affect 1 row\(s\) but matched 0/);
    // A table without a key cannot be edited.
    await expect(
      mockInvoke("apply_changes", {
        connectionId,
        req: {
          table: invoices,
          returning: false,
          changes: [{ op: "delete", rowId: "k", key: { kind: "allColumns", value: {} } }],
        },
      }),
    ).rejects.toMatchObject({ code: "UNSUPPORTED" });
  });

  it("runs the editor's GO batches with T-SQL's errors and a count for a lone DML", async () => {
    const { connectionId } = await shop();
    const r = await run(
      connectionId,
      "SELECT TOP (2) id, name FROM dbo.customers ORDER BY id DESC\nSELECT @@SPID AS spid\nGO\nUPDATE dbo.orders SET status = N'paid' WHERE customer_id = 1\nGO\nSELECT * FROM nope",
    );
    expect(r.results.map((s) => [s.kind, s.resultSets, s.affected])).toEqual([
      ["rows", 2, null],
      ["affected", 0, 4],
      ["empty", 0, null],
    ]);
    expect(r.rows[0]).toEqual([
      { t: "int", v: 30 },
      { t: "text", v: expect.any(String) },
    ]);
    expect(r.results[2]?.error).toMatchObject({
      vendorCode: 208,
      message: "Invalid object name 'nope'.",
    });
    // A batch of a DML and a read reports no count; a transaction rolls back.
    const tx = await run(
      connectionId,
      "BEGIN TRAN\nDELETE FROM sales.invoices\nSELECT COUNT(*) AS n FROM sales.invoices\nGO\nROLLBACK\nGO\nSELECT COUNT_BIG(*) FROM sales.invoices",
    );
    expect(tx.rows).toEqual([[{ t: "int", v: 0 }], [{ t: "int", v: 12 }]]);
    // A USE switches the editor's own connection only: the catalog and the grid read through
    // workers that log in to the profile's database.
    const use = await run(
      connectionId,
      "USE reporting\nGO\nSELECT DB_NAME() AS db, COUNT(*) FROM daily_sales",
    );
    expect(use.results[1]?.error).toMatchObject({ vendorCode: 8120 });
    const db = await run(connectionId, "SELECT DB_NAME() AS db");
    expect(db.rows).toEqual([[{ t: "text", v: "reporting" }]]);
    const dbs = await mockInvoke<DatabaseInfo[]>("list_databases", { connectionId });
    expect(dbs.filter((d) => d.isCurrent).map((d) => d.name)).toEqual(["shop"]);
    expect((await page(connectionId)).rows).toHaveLength(10);
    const tables = await mockInvoke<TableSummary[]>("list_tables", { connectionId, schema: "dbo" });
    expect(tables.map((t) => t.name)).toContain("customers");
  });

  it("opens a session from the profile with another database, as Switch Database does", async () => {
    // The backend's switch_database connects anew with the profile's `database` replaced
    // (`a_switched_session_logs_in_to_the_other_database`); the lane's part is that connect.
    const profile = mockProfiles.get("p-mock-sqlserver")!;
    const session = lane.drivers[0]!.connect({ ...profile, database: " reporting " });
    expect(session.caps.features).toContain("switchDatabase");
    const dbs = session.listDatabases();
    expect(dbs.filter((d) => d.isCurrent).map((d) => d.name)).toEqual(["reporting"]);
    const tables = session.listTables("dbo");
    expect(tables.map((t) => t.name)).toContain("daily_sales");
    expect(tables.map((t) => t.name)).not.toContain("customers");
  });

  it("lists processes and kills whole sessions only", async () => {
    const { connectionId } = await shop();
    const list = await mockInvoke<ProcessRow[]>("admin_list_processes", { connectionId });
    expect(list.filter((p) => p.current)).toHaveLength(1);
    expect(list.map((p) => p.id)).toContain("61");
    const kill = (ids: string[], scope: string) =>
      mockInvoke<KillOutcome[]>("admin_kill_processes", { connectionId, ids, scope });
    expect(await kill(["61"], "query")).toEqual([
      {
        id: "61",
        ok: false,
        message:
          "unsupported: SQL Server ends a whole session, not one of its queries: use Kill Connection",
      },
    ]);
    expect(await kill(["61", "999"], "connection")).toEqual([
      { id: "61", ok: true, message: null },
      { id: "999", ok: false, message: "Process ID 999 is not an active process ID." },
    ]);
    // `kill_one`'s InvalidInput prints its text alone; `tsql::kill` takes smallint ids from 1.
    expect(await kill(["x1", "0", "40000"], "connection")).toEqual([
      { id: "x1", ok: false, message: "x1 is not a session id" },
      { id: "0", ok: false, message: "0 is not a user session id" },
      { id: "40000", ok: false, message: "40000 is not a user session id" },
    ]);
    expect(
      (await mockInvoke<ProcessRow[]>("admin_list_processes", { connectionId })).map((p) => p.id),
    ).not.toContain("61");
  });
});

describe("a read-only session", () => {
  it("refuses writes, hidden statements and side-effect functions before anything runs", async () => {
    const { connectionId } = await azure();
    expect((await run(connectionId, "SELECT TOP (1) * FROM dbo.daily_sales")).rows).toHaveLength(1);
    for (const sql of [
      "DELETE FROM dbo.daily_sales",
      "SELECT 1 DELETE FROM dbo.daily_sales",
      "SELECT * INTO x FROM dbo.daily_sales",
      "EXEC sp_who",
      "SELECT NEXT VALUE FOR dbo.order_ids",
      "SELECT * FROM OPENQUERY(srv, 'SELECT 1')",
      "{call dbo.p}",
      "SELECT 1 DUMP DATABASE reporting TO x",
      "frobnicate",
    ])
      await expect(run(connectionId, sql), sql).rejects.toMatchObject({
        code: "INVALID_INPUT",
        message: READ_ONLY_MESSAGE,
      });
    // In the grid's reads too, and in the commit of staged changes.
    await expect(
      page(connectionId, {
        table: { schema: "dbo", name: "daily_sales" },
        filter: raw("NEXT VALUE FOR s > 0"),
      }),
    ).rejects.toMatchObject({ message: READ_ONLY_MESSAGE });
    await expect(
      mockInvoke("apply_changes", {
        connectionId,
        req: {
          table: { schema: "dbo", name: "daily_sales" },
          returning: false,
          changes: [
            {
              op: "delete",
              rowId: "d",
              key: { kind: "pk", value: { day: { t: "date", v: "2026-08-01" } } },
            },
          ],
        },
      }),
    ).rejects.toMatchObject({ message: READ_ONLY_MESSAGE });
    expect(
      (await page(connectionId, { table: { schema: "dbo", name: "daily_sales" }, limit: 100 }))
        .rows,
    ).toHaveLength(60);
    // Kills are refused (the mock's command refuses a read-only connection before the session).
    await expect(
      mockInvoke("admin_kill_processes", { connectionId, ids: ["61"], scope: "connection" }),
    ).rejects.toMatchObject({ code: "UNSUPPORTED" });
  });

  it("refuses what the backend's refused_read_only refuses", () => {
    expect(refusedReadOnly("SELECT * FROM t WHERE a = 'DELETE'")).toBe(false);
    expect(refusedReadOnly("SELECT [call] FROM t")).toBe(false);
    expect(refusedReadOnly("SELECT call FROM t")).toBe(true);
    expect(hidesStatement("SELECT 1e0DUMP")).toBe(true);
    expect(hidesStatement("SELECT {? = call p}")).toBe(true);
    expect(hidesStatement("SELECT 'call' -- load")).toBe(false);
    expect(refusedReadOnly("SELECT xp_cmdshell('dir')")).toBe(true);
  });

  it("finds the statements tsql.rs finds (statements_the_rules_do_not_name_are_found)", () => {
    for (const sql of [
      "SELECT 1 {call p}",
      "SELECT 1 {CALL p}",
      "SELECT 1 { ? = call p (1) }",
      "SELECT 1 {/* x */?=call p}",
      "SELECT 1 DUMP TRANSACTION x",
      "SELECT 1 LOAD DATABASE x",
      "SELECT 1 CALL p",
      "SELECT 1 WHERE a = 1e0DUMP TRANSACTION x",
      "SELECT 1.e5LOAD DATABASE x",
      "SELECT 1 /* a /* b */ DUMP DATABASE x */",
      // A tail of a word that starts with a digit (`hidden_in_number`).
      "SELECT 1eCALL p",
    ]) {
      expect(hidesStatement(sql), sql).toBe(true);
      expect(refusedReadOnly(sql), sql).toBe(true);
    }
    for (const sql of [
      "SELECT 1",
      "SELECT {fn NOW()}, {d '2024-01-01'}",
      "SELECT [call], 'DUMP', \"load\" -- CALL p",
      "SELECT payload, download FROM t",
      "SELECT 1e5",
    ])
      expect(hidesStatement(sql), sql).toBe(false);
  });
});
