/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { classifyDialect } from "@/features/drivers/portable/classify";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { ConnectionInfo } from "../../../bindings/ConnectionInfo";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { DatabaseInfo } from "../../../bindings/DatabaseInfo";
import type { KillOutcome } from "../../../bindings/KillOutcome";
import type { ProcessRow } from "../../../bindings/ProcessRow";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { RoutineSummary } from "../../../bindings/RoutineSummary";
import type { SchemaInfo } from "../../../bindings/SchemaInfo";
import type { SessionDetails } from "../../../bindings/SessionDetails";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { TableSummary } from "../../../bindings/TableSummary";
import { mockProfiles } from "../../handlers";
import { mockInvoke } from "../../transport";
import { enableMockDrivers, resetMockBuild } from "..";
import lane, { resetVertica } from ".";
import {
  advancesSequence,
  classifyVertica,
  foldAsciiLookalikes,
  LEX,
  refusedReadOnly,
} from "./classify";
import {
  ADDED_WRITE_MESSAGE,
  FEATURES,
  FILTER_OPS,
  GUARD_MESSAGE,
  NO_DEFINITION,
  READ_ONLY_EDIT,
  READ_ONLY_MESSAGE,
  resetSessions,
  SEVERAL_MESSAGE,
  verticaCaps,
} from "./session";

// The mock's Vertica against the backend's behaviour (drivers/vertica/session.rs and its tests):
// caps, catalog, offset pages, counts, a whole cell, a staged commit with a failing change, the
// editor's statements one at a time, sessions and kills, and a read-only session's refusals.

beforeAll(async () => {
  await import("../..");
});

let added: string[] = [];

beforeEach(async () => {
  resetVertica();
  resetSessions();
  added = (await enableMockDrivers(["vertica"])).map((p) => p.id);
});

afterEach(() => {
  resetMockBuild();
  for (const id of added) mockProfiles.delete(id);
});

const connect = (profileId: string) => mockInvoke<ConnectionInfo>("connect", { profileId });
const vmart = () => connect("p-mock-vertica");
const analytics = () => connect("p-mock-vertica-oauth");

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

const customers = { schema: "public", name: "customer_dimension" };
const sales = { schema: "store", name: "store_sales_fact" };
const page = (connectionId: string, req: Record<string, unknown> = {}) =>
  mockInvoke<BrowsePage>("browse_table", {
    connectionId,
    req: { table: customers, columns: null, filter: null, sort: [], limit: 10, offset: 0, ...req },
  });
const raw = (sql: string) => ({ conjunction: "and", items: [{ kind: "raw", sql }] });
const key = (n: number) => ({ kind: "pk", value: { customer_key: { t: "int", v: n } } });

/** A profile like `id`'s with `patch` over it, under a new id. */
function variant(id: string, newId: string, patch: (p: ConnectionProfile) => object) {
  const p = mockProfiles.get(id)!;
  mockProfiles.set(newId, { ...p, id: newId, ...patch(p) });
  added.push(newId);
}

describe("the mock driver", () => {
  it("grants what the backend's session.rs lists", () => {
    // The constant arrays of `drivers/vertica/session.rs`, read from the source, so that a
    // change there fails here.
    const rs = readFileSync(
      path.resolve(
        import.meta.dirname,
        "../../../../../../src-tauri/src/drivers/vertica/session.rs",
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
    const caps = verticaCaps("public", false);
    expect(caps.features).toHaveLength(17);
    // `SessionCaps::with` sorts them in `Feature`'s order.
    expect([...caps.features].sort()).toEqual(listed("FEATURES", "Feature").sort());
    expect(caps.data.filterOps).toEqual(listed("FILTER_OPS", "FilterOp"));
    expect(rs).toContain(`const KEEPALIVE_SECS: u32 = ${caps.session.keepaliveSecs};`);
    expect(rs).toContain("paging: Paging::Offset,");
    expect(rs).toContain("count: CountMode::Estimate,");
    expect(rs).toContain("read_only: ReadOnlyMode::Server,");
    expect(rs).toContain("raw_filter: Some(RawFilter::SqlWhere),");
    expect(rs).toContain("transactions: TxMode::Atomic,");
    expect(rs).toContain("default_keyword: false,");
    expect(rs).toContain(`const DEFAULT_SCHEMA: &str = "${caps.namespaces.defaultSchema}";`);
    expect(rs).toContain(`const NO_DEFINITION: &str = "${NO_DEFINITION.slice(0, 60)}`);
    expect(rs).toContain(`"${SEVERAL_MESSAGE}"`);
    expect(rs).toContain(`"${ADDED_WRITE_MESSAGE}"`);
  });

  it("is Vertica's, with seed profiles for ?mockDrivers=vertica", () => {
    expect(lane.drivers.map((d) => d.driver)).toEqual(["vertica"]);
    expect(added).toEqual(["p-mock-vertica", "p-mock-vertica-oauth"]);
    expect(lane.drivers[0]!.util).toBeUndefined();
  });

  it("connects with the backend's version and caps", async () => {
    const info = await vmart();
    expect(info.serverVersion).toBe("Vertica Analytic Database 24.1.0-0");
    expect(info.capabilities).toEqual(verticaCaps("public", false));
    const caps = info.capabilities!;
    expect(caps.dialect).toBe("vertica");
    expect(caps.features).toEqual(FEATURES);
    for (const f of [
      "format",
      "returning",
      "editWithoutKey",
      "serverExpressions",
      "explainAnalyze",
      "structureEdit",
      "switchDatabase",
      "users",
      "sequences",
      "exportTables",
    ])
      expect(caps.features, f).not.toContain(f);
    expect(caps.namespaces).toMatchObject({
      databases: true,
      schemas: "schemas",
      defaultSchema: "public",
    });
    expect(caps.data).toMatchObject({
      paging: "offset",
      count: "estimate",
      rawFilter: "sqlWhere",
      transactions: "atomic",
      affected: "exact",
      defaultKeyword: false,
      upsert: false,
      maxParams: 1000,
    });
    expect(caps.data.filterOps).toEqual(FILTER_OPS);
    expect(caps.data.filterOps).not.toContain("jsonContains");
    expect(caps.safety).toEqual({ readOnly: "server", hardened: false, confirm: "password" });
    expect(caps.session).toEqual({ keepaliveSecs: 30, pingBilled: false });
    const ro = await analytics();
    expect(ro.readOnly).toBe(true);
    // A token from a command is confirmed with the app passcode.
    expect(ro.capabilities!.safety.confirm).toBe("passcode");
  });

  it("refuses what the backend's validate refuses, in its words", async () => {
    const refused = async (patch: (p: ConnectionProfile) => object, message: string) => {
      const id = `p-x${added.length}`;
      variant("p-mock-vertica", id, patch);
      await expect(connect(id)).rejects.toMatchObject({ message });
    };
    const params = (p: ConnectionProfile, driverParams: object) => ({
      options: { ...p.options, driverParams },
    });
    await refused(
      (p) => params(p, { auth: "kerberos" }),
      "Vertica settings: unknown variant `kerberos`, expected `password` or `oauth`",
    );
    await refused(
      (p) => params(p, { clientLabel: "a\nb" }),
      "Vertica client label “a\nb”: up to 128 characters, no line breaks",
    );
    await refused(
      (p) => ({ options: { ...p.options, sslKeyPath: "/k.pem" } }),
      "Vertica: a client certificate needs both its certificate and its key file",
    );
    await refused(
      (p) => params(p, { auth: "oauth" }),
      "Vertica: an OAuth token is sent only over TLS (set SSL mode to Require or stricter)",
    );
    await refused(
      (p) => ({
        ssl: "require",
        options: { ...p.options, passwordMode: "none", driverParams: { auth: "oauth" } },
      }),
      "Vertica: an OAuth token comes from the password field or its Command",
    );
    await refused(() => ({ user: " " }), "User is required");
    await refused(() => ({ database: "" }), "Database is required");
    await refused(() => ({ database: "Other" }), 'Database "Other" does not exist');
    // A read-only profile refuses a startup command that writes or advances a sequence.
    for (const startupSql of ["SET SEARCH_PATH TO store", "SELECT s.NEXTVAL"]) {
      const id = `p-ro${added.length}`;
      variant("p-mock-vertica-oauth", id, (p) => ({ options: { ...p.options, startupSql } }));
      await expect(connect(id), startupSql).rejects.toMatchObject({
        code: "INVALID_INPUT",
        message: `Startup command “${startupSql}”: it writes (or is not known to only read), and the connection is read-only`,
      });
    }
    // Its credentials are masked in the message, as the backend's `redact` masks them.
    const id = `p-ro${added.length}`;
    const startupSql = "ALTER USER x IDENTIFIED BY 'pw' REPLACE 'old'";
    variant("p-mock-vertica-oauth", id, (p) => ({ options: { ...p.options, startupSql } }));
    await expect(connect(id)).rejects.toMatchObject({
      code: "INVALID_INPUT",
      message:
        "Startup command “ALTER USER x IDENTIFIED BY '********' REPLACE '********'”: it writes (or is not known to only read), and the connection is read-only",
    });
  });
});

describe("a session", () => {
  it("lists the database, schemas, tables, structure, routines, DDL and session details", async () => {
    const { connectionId } = await vmart();
    expect(await mockInvoke<DatabaseInfo[]>("list_databases", { connectionId })).toEqual([
      { name: "VMart", owner: "dbadmin", encoding: "UTF-8", isCurrent: true },
    ]);
    const schemas = await mockInvoke<SchemaInfo[]>("list_schemas", { connectionId });
    expect(schemas.map((s) => s.name)).toEqual([
      "online_sales",
      "public",
      "store",
      "v_catalog",
      "v_func",
      "v_internal",
      "v_monitor",
      "v_txtindex",
    ]);
    expect(schemas.filter((s) => s.isDefault).map((s) => s.name)).toEqual(["public"]);
    const tables = await mockInvoke<TableSummary[]>("list_tables", {
      connectionId,
      schema: "public",
    });
    expect(tables.map((t) => [t.name, t.kind, t.rowEstimate])).toEqual([
      ["customer_dimension", "table", 60],
      ["customer_summary", "view", null],
      ["date_dimension", "table", 31],
      ["product_dimension", "table", 16],
    ]);
    expect(
      (await mockInvoke<TableSummary[]>("list_tables", { connectionId, schema: "online_sales" }))[0]
        ?.kind,
    ).toBe("foreignTable");
    const meta = await mockInvoke<TableMeta>("get_table", { connectionId, table: sales });
    expect(meta.primaryKey).toEqual([]);
    expect(meta.foreignKeys.map((f) => f.refTable.name)).toEqual([
      "date_dimension",
      "product_dimension",
      "store_dimension",
      "customer_dimension",
    ]);
    // Projections are the Structure tab's indexes, as `parse_projections` shows them.
    expect(meta.indexes.map((i) => [i.name, i.method])).toEqual([
      [
        "store_sales_fact_super",
        "super projection, segmented by hash(store_sales_fact.date_key, store_sales_fact.product_key)",
      ],
      [
        "store_sales_fact_by_customer",
        "projection, segmented by hash(store_sales_fact.customer_key)",
      ],
    ]);
    const c = await mockInvoke<TableMeta>("get_table", { connectionId, table: customers });
    expect(c.columns[0]).toMatchObject({ name: "customer_key", isPk: true, pkOrdinal: 1 });
    expect(c.checks).toEqual([
      {
        name: "C_INCOME_CHECK",
        expression: "(customer_dimension.annual_income >= 0)",
        columns: ["annual_income"],
      },
    ]);
    const ddl = await mockInvoke<string>("get_table_ddl", { connectionId, table: sales });
    expect(ddl).toMatch(/^CREATE TABLE store\.store_sales_fact\n\(/);
    expect(ddl).toContain("SEGMENTED BY hash(store_sales_fact.customer_key) ALL NODES;");
    const routines = await mockInvoke<RoutineSummary[]>("list_routines", {
      connectionId,
      schema: "public",
    });
    expect(routines.map((r) => [r.name, r.kind, r.language])).toEqual([
      ["fiscal_quarter", "function", "SQL"],
      ["geo_distance", "function", "C++"],
    ]);
    const def = (schema: string, name: string, kind: string) =>
      mockInvoke<string>("get_routine_definition", {
        connectionId,
        routine: { schema, name, kind, identityArguments: "" },
      });
    expect(await def("public", "fiscal_quarter", "function")).toMatch(/^RETURN /);
    // A UDx keeps no text.
    expect(await def("public", "geo_distance", "function")).toBe(NO_DEFINITION);
    expect(await def("store", "refresh_daily_totals", "procedure")).toMatch(
      /^CREATE PROCEDURE store\.refresh_daily_totals/,
    );
    expect(await mockInvoke<SessionDetails>("session_details", { connectionId })).toEqual({
      tls: true,
      tlsVersion: "TLSv1.3",
      tlsCipher: "TLS13_AES_256_GCM_SHA384",
      encoding: "UTF-8",
    });
  });

  it("pages by offset, sorts, filters, counts and fetches a whole cell", async () => {
    const { connectionId } = await vmart();
    const first = await page(connectionId);
    expect(first.identity).toBe("pk");
    expect(first.rows).toHaveLength(10);
    expect(first.sql).toBe(
      'SELECT "customer_key", "customer_type", "customer_name", "customer_gender", "customer_city", "customer_state", "annual_income", "customer_since", "notes" FROM "public"."customer_dimension" ORDER BY "customer_key" ASC LIMIT 10 OFFSET 0',
    );
    const last = await page(connectionId, { offset: 55 });
    expect(last.rows.map((r) => r[0])).toEqual([56, 57, 58, 59, 60].map((v) => ({ t: "int", v })));
    const sorted = await page(connectionId, {
      sort: [{ column: "annual_income", dir: "desc", nulls: null }],
      columns: ["customer_key", "annual_income"],
      limit: 3,
    });
    expect(sorted.columns.map((c) => c.name)).toEqual(["customer_key", "annual_income"]);
    expect(sorted.sql).toContain('ORDER BY "annual_income" DESC LIMIT 3 OFFSET 0');
    // A table without a key pages too, but its rows cannot be edited.
    const fact = await page(connectionId, { table: sales, limit: 100, offset: 200 });
    expect(fact.identity).toBe("none");
    expect(fact.rows).toHaveLength(50);
    const companies = {
      conjunction: "and",
      items: [
        {
          kind: "condition",
          column: "customer_type",
          op: "eq",
          values: [{ t: "text", v: "Company" }],
        },
      ],
    };
    expect(
      await mockInvoke("count_table_rows", { connectionId, table: customers, filter: companies }),
    ).toEqual({ count: 15, exact: true });
    expect(
      await mockInvoke("count_table_rows", { connectionId, table: sales, filter: null }),
    ).toEqual({ count: 250, exact: true });
    const notes = await mockInvoke<{ t: string; v: string }>("fetch_cell", {
      connectionId,
      table: customers,
      key: key(8),
      column: "notes",
    });
    expect(notes.v).toMatch(/after 5 pm\.$/);
    // A raw WHERE that adds a statement or writes is refused before anything is sent.
    await expect(page(connectionId, { filter: raw("1=1; DELETE FROM t") })).rejects.toMatchObject({
      code: "INVALID_INPUT",
      message: SEVERAL_MESSAGE,
    });
    await expect(
      page(connectionId, { filter: raw("CLOSE_SESSION('x') IS NOT NULL") }),
    ).rejects.toMatchObject({ message: ADDED_WRITE_MESSAGE });
    expect((await page(connectionId, { filter: raw("annual_income > 0") })).rows).toHaveLength(10);
  });

  it("commits staged changes in one transaction, and a failing change rolls back all", async () => {
    const { connectionId } = await vmart();
    const req = (changes: object[]) => ({ table: customers, changes, returning: true });
    const good = [
      {
        op: "insert",
        rowId: "r2",
        values: {
          customer_key: { t: "int", v: 61 },
          customer_type: { t: "text", v: "Individual" },
          customer_name: { t: "text", v: "Hedy Lamarr" },
        },
      },
      {
        op: "update",
        rowId: "r1",
        key: key(1),
        changes: { customer_name: { t: "text", v: "Ada K. O'Neil" } },
      },
    ];
    // Deletes, then updates, then inserts; no RETURNING; values as `quote_exec` writes them.
    expect(await mockInvoke<string[]>("preview_changes", { connectionId, req: req(good) })).toEqual(
      [
        `UPDATE "public"."customer_dimension" SET "customer_name" = 'Ada K. O''Neil' WHERE "customer_key" = 1`,
        `INSERT INTO "public"."customer_dimension" ("customer_key", "customer_name", "customer_type") VALUES (61, 'Hedy Lamarr', 'Individual')`,
      ],
    );
    const ok = await mockInvoke<ApplyResult>("apply_changes", { connectionId, req: req(good) });
    expect(ok.success).toBe(true);
    expect(ok.results.map((r) => [r.rowId, r.affected, r.returned])).toEqual([
      ["r1", 1, null],
      ["r2", 1, null],
    ]);
    // The default the server fills in.
    expect((await page(connectionId, { offset: 60 })).rows[0]?.slice(0, 7)).toEqual([
      { t: "int", v: 61 },
      { t: "text", v: "Individual" },
      { t: "text", v: "Hedy Lamarr" },
      { t: "null" },
      { t: "null" },
      { t: "null" },
      { t: "int", v: 0 },
    ]);
    const bad = [
      {
        op: "update",
        rowId: "u1",
        key: key(2),
        changes: { annual_income: { t: "int", v: -5 } },
      },
      { op: "delete", rowId: "d1", key: key(61) },
    ];
    const failed = await mockInvoke<ApplyResult>("apply_changes", { connectionId, req: req(bad) });
    expect(failed.success).toBe(false);
    expect(failed.error).toMatchObject({
      code: "DB_ERROR",
      sqlstate: "23514",
      vendorCode: 7230,
      statementIndex: 1,
      message: expect.stringContaining(
        "Check constraint 'public.customer_dimension.C_INCOME_CHECK'",
      ),
    });
    expect(failed.results.map((r) => [r.rowId, r.error?.vendorCode ?? null])).toEqual([
      ["d1", null],
      ["u1", 7230],
    ]);
    // The DELETE before it was rolled back with it.
    expect((await page(connectionId, { offset: 60 })).rows).toHaveLength(1);
    // A NOT NULL column, in the server's words.
    const nulls = await mockInvoke<ApplyResult>("apply_changes", {
      connectionId,
      req: req([
        { op: "update", rowId: "n", key: key(3), changes: { customer_name: { t: "null" } } },
      ]),
    });
    expect(nulls.error).toMatchObject({
      sqlstate: "23502",
      message:
        "Cannot set a NOT NULL column (customer_name) to a NULL value in INSERT/UPDATE statement",
    });
    // A row that is gone fails `check_affected`.
    const gone = await mockInvoke<ApplyResult>("apply_changes", {
      connectionId,
      req: req([{ op: "delete", rowId: "x", key: key(999) }]),
    });
    expect(gone.error?.message).toBe(GUARD_MESSAGE);
    // No row id, no DEFAULT: the features the caps leave out.
    await expect(
      mockInvoke("apply_changes", {
        connectionId,
        req: {
          table: sales,
          returning: false,
          changes: [{ op: "delete", rowId: "k", key: { kind: "allColumns", value: {} } }],
        },
      }),
    ).rejects.toMatchObject({
      code: "UNSUPPORTED",
      message: "unsupported: Vertica: editing rows without a primary key is not available",
    });
    await expect(
      mockInvoke("preview_changes", {
        connectionId,
        req: req([{ op: "insert", rowId: "e", values: {}, exprs: { customer_since: "now" } }]),
      }),
    ).rejects.toMatchObject({
      message: "unsupported: Vertica: DEFAULT and server expressions is not available",
    });
  });

  it("runs the editor's statements one at a time, with Vertica's errors and DML counts", async () => {
    const { connectionId } = await vmart();
    const r = await run(
      connectionId,
      "SELECT version();\nSELECT customer_name FROM customer_dimension WHERE customer_state = 'TX' ORDER BY customer_key LIMIT 2;\nSELECT COUNT(*) FROM store.store_sales_fact;\nUPDATE public.customer_dimension SET annual_income = 1 WHERE customer_type = 'Company';\nSELECT * FROM nope",
    );
    expect(r.results.map((x) => x.kind)).toEqual(["rows", "rows", "rows", "affected", "empty"]);
    expect(r.rows[0]).toEqual([{ t: "text", v: "Vertica Analytic Database v24.1.0-0" }]);
    expect(r.rows.slice(1, 3)).toEqual([
      [{ t: "text", v: "Linus Allen" }],
      [{ t: "text", v: "Grace Allen" }],
    ]);
    expect(r.rows[3]).toEqual([{ t: "int", v: 250 }]);
    expect(r.results[3]?.affected).toBe(15);
    expect(r.results[4]?.error).toMatchObject({
      code: "DB_ERROR",
      sqlstate: "42V01",
      vendorCode: 4566,
      message: 'Relation "nope" does not exist',
    });
    const plan = await run(connectionId, "EXPLAIN SELECT * FROM store.store_sales_fact");
    expect(plan.results[0]?.columns.map((c) => c.name)).toEqual(["QUERY PLAN"]);
    expect(plan.rows.map((row) => (row[0] as { v: string }).v).join("\n")).toContain(
      "STORAGE ACCESS for store_sales_fact",
    );
    // A text of several statements never reaches a session's `run` (the editor splits it), and
    // the session refuses it as `refuse_several` does.
    const session = (await lane.drivers[0]!.connect(mockProfiles.get("p-mock-vertica")!)) as {
      run: (sql: string, emit: (e: QueryEvent) => void) => void;
    };
    expect(() => session.run("SELECT 1; SELECT 2", () => {})).toThrow(SEVERAL_MESSAGE);
  });

  it("lists sessions and kills a statement or a session", async () => {
    const { connectionId } = await vmart();
    const list = await mockInvoke<ProcessRow[]>("admin_list_processes", { connectionId });
    expect(list.map((p) => [p.user, p.state, p.command, p.current, p.own])).toEqual([
      ["dbadmin", "active", "Tabler", true, true],
      ["etl_loader", "active", "JDBC Driver", false, false],
      ["analyst", "active", "vsql", false, false],
      ["dbadmin", "idle", "ODBC Driver", false, false],
    ]);
    expect(list[0]?.application).toBe("Tabler");
    const kill = (ids: string[], scope: string) =>
      mockInvoke<KillOutcome[]>("admin_kill_processes", { connectionId, ids, scope });
    const [, etl, analyst, idle] = list.map((p) => p.id);
    expect(await kill([etl!, idle!, "nope"], "query")).toEqual([
      { id: etl, ok: true, message: null },
      { id: idle, ok: false, message: `session ${idle} runs no statement` },
      { id: "nope", ok: false, message: "not found: session nope" },
    ]);
    expect(await kill([analyst!], "connection")).toEqual([
      { id: analyst, ok: true, message: null },
    ]);
    const after = await mockInvoke<ProcessRow[]>("admin_list_processes", { connectionId });
    expect(after.map((p) => [p.id, p.state])).toEqual([
      [list[0]!.id, "active"],
      [etl, "idle"],
      [idle, "idle"],
    ]);
  });
});

describe("a read-only session", () => {
  it("refuses writes, sequence advances and SLEEP before anything is sent", async () => {
    const { connectionId } = await analytics();
    for (const sql of [
      "DELETE FROM public.customer_dimension",
      "SELECT NEXTVAL('s')",
      "SELECT s.NEXTVAL",
      'SELECT "s"."nextval"',
      "SELECT SLEEP(10)",
      "SELECT CLOSE_SESSION('x')",
      // The KELVIN SIGN folds to K: `KMEANS` trains a model.
      "SELECT KMEANS('m', 't', '*', 3)",
      // LONG S folds to s: `CLOSE_SESSION` ends a session.
      "SELECT CLOſE_SESSION('x')",
      "COMMIT",
    ])
      await expect(run(connectionId, sql), sql).rejects.toMatchObject({
        code: "INVALID_INPUT",
        message: READ_ONLY_MESSAGE,
      });
    expect((await run(connectionId, "SELECT COUNT(*) FROM public.date_dimension")).rows).toEqual([
      [{ t: "int", v: 31 }],
    ]);
    // In the grid's reads too, and in staged changes (the command refuses those first).
    await expect(page(connectionId, { filter: raw("x = s.NEXTVAL") })).rejects.toMatchObject({
      message: READ_ONLY_MESSAGE,
    });
    for (const cmd of ["preview_changes", "apply_changes"])
      await expect(
        mockInvoke(cmd, {
          connectionId,
          req: {
            table: customers,
            returning: false,
            changes: [{ op: "delete", rowId: "d", key: key(1) }],
          },
        }),
      ).rejects.toMatchObject({ code: "UNSUPPORTED", message: READ_ONLY_EDIT });
    await expect(
      mockInvoke("admin_kill_processes", { connectionId, ids: ["x"], scope: "connection" }),
    ).rejects.toMatchObject({ code: "UNSUPPORTED" });
    expect((await page(connectionId)).rows).toHaveLength(10);
  });
});

describe("the backend's classifier in the mock", () => {
  it("folds lookalike letters as `fold_ascii_lookalikes` does", () => {
    expect(foldAsciiLookalikes("café K")).toBe("café K");
    expect(foldAsciiLookalikes("SELECT 1")).toBe("SELECT 1");
    expect(foldAsciiLookalikes("ſı")).toBe("SI");
    expect(foldAsciiLookalikes("ſı")).toBe("SI");
    // The shared classifier (Safe Mode's) reads the raw text only: the backend refuses this on a
    // read-only session, Safe Mode does not ask (docs/drivers/vertica.md, Limits).
    expect(classifyDialect(LEX, "SELECT KMEANS('m', 't', '*', 3)")).toBe("read");
    expect(classifyVertica("SELECT KMEANS('m', 't', '*', 3)")).toBe("write");
    expect(classifyVertica("SELECT 'K'")).toBe("read");
  });

  it("finds a sequence's next value in either form, as `advances_sequence` does", () => {
    for (const sql of [
      "SELECT s.NEXTVAL",
      "SELECT public.s.nextval FROM t",
      'SELECT "s"."NEXTVAL"',
      'SELECT s.U&"\\004EEXTVAL"',
    ])
      expect(advancesSequence(sql) || refusedReadOnly(sql), sql).toBe(true);
    for (const sql of ["SELECT nextval_col FROM t", "SELECT 's.NEXTVAL'", "SELECT s.CURRVAL"])
      expect(advancesSequence(sql), sql).toBe(false);
  });
});
