/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { ConnectionInfo } from "../../../bindings/ConnectionInfo";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { DatabaseInfo } from "../../../bindings/DatabaseInfo";
import type { ExplainResult } from "../../../bindings/ExplainResult";
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
import lane, { KEY_FINGERPRINT, resetSnowflake } from ".";
import {
  quotesNextval,
  setsAppParameter,
  setsAutocommit,
  setsIgnoreCase,
  writes,
} from "./classify";
import {
  ADDED_STATEMENT_MESSAGE,
  AUTOCOMMIT_MESSAGE,
  appParameterMessage,
  FEATURES,
  IGNORE_CASE_MESSAGE,
  FILTER_OPS,
  GUARD_MESSAGE,
  KILL_CONNECTION_MESSAGE,
  movedMessage,
  READ_ONLY_EDIT,
  READ_ONLY_MESSAGE,
  SEVERAL_MESSAGE,
  snowflakeCaps,
  TRANSACTION_OPEN_MESSAGE,
} from "./session";

// The mock's Snowflake against the backend's behaviour (drivers/snowflake/session.rs, editor.rs,
// entry.rs and their tests): caps, the form's calls, catalog through SHOW, offset pages, costly
// counts, a whole cell, a staged commit with a failing change, the editor's statements one at a
// time with transactions and AUTOCOMMIT kept on, the user's queries and Kill Query, a read-only
// session's refusals, and the shared session of a password + MFA sign-in.

beforeAll(async () => {
  await import("../..");
});

let added: string[] = [];

beforeEach(async () => {
  resetSnowflake();
  added = (await enableMockDrivers(["snowflake"])).map((p) => p.id);
});

afterEach(() => {
  resetMockBuild();
  for (const id of added) mockProfiles.delete(id);
});

const connect = (profileId: string) => mockInvoke<ConnectionInfo>("connect", { profileId });
const shop = () => connect("p-mock-snowflake");
const analytics = () => connect("p-mock-snowflake-ro");
const mfa = () => connect("p-mock-snowflake-mfa");

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

const customers = { schema: "PUBLIC", name: "CUSTOMERS" };
const events = { schema: "PUBLIC", name: "EVENTS_RAW" };
const page = (connectionId: string, req: Record<string, unknown> = {}) =>
  mockInvoke<BrowsePage>("browse_table", {
    connectionId,
    req: { table: customers, columns: null, filter: null, sort: [], limit: 10, offset: 0, ...req },
  });
const raw = (sql: string) => ({ conjunction: "and", items: [{ kind: "raw", sql }] });
const key = (n: number) => ({ kind: "pk", value: { ID: { t: "int", v: n } } });
const count = (connectionId: string, extra: Record<string, unknown> = {}) =>
  mockInvoke<{ count: number; exact: boolean }>("count_table_rows", {
    connectionId,
    table: customers,
    filter: null,
    ...extra,
  });
const util = (
  p: ConnectionProfile,
  method: string,
  args: unknown = null,
  password: string | null = null,
) =>
  mockInvoke("driver_util", {
    input: {
      profile: p,
      password,
      sshPassword: null,
      sshPassphrase: null,
      clearPassword: false,
      keyPassphrase: null,
      sessionToken: null,
    },
    method,
    args,
  });

/** A profile like `id`'s with `patch` over it, under a new id. */
function variant(id: string, newId: string, patch: (p: ConnectionProfile) => object) {
  const p = mockProfiles.get(id)!;
  mockProfiles.set(newId, { ...p, id: newId, ...patch(p) });
  added.push(newId);
}

describe("the mock driver", () => {
  it("grants what the backend's session.rs lists", () => {
    // The constant arrays of `drivers/snowflake/session.rs`, read from the source, so that a
    // change there fails here.
    const rs = readFileSync(
      path.resolve(
        import.meta.dirname,
        "../../../../../../src-tauri/src/drivers/snowflake/session.rs",
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
    const caps = snowflakeCaps("PUBLIC");
    expect(caps.features).toHaveLength(18);
    // `SessionCaps::with` sorts them in `Feature`'s order.
    expect([...caps.features].sort()).toEqual(listed("FEATURES", "Feature").sort());
    expect(caps.data.filterOps).toEqual(listed("FILTER_OPS", "FilterOp"));
    for (const line of [
      "paging: Paging::Offset,",
      "count: CountMode::Costly,",
      "read_only: ReadOnlyMode::ClassifierOnly,",
      "hardened: false,",
      "keepalive_secs: None,",
      "ping_billed: false,",
      "raw_filter: Some(RawFilter::SqlWhere),",
      "transactions: TxMode::Atomic,",
      "affected: Affected::Exact,",
      "default_keyword: false,",
      "upsert: false,",
    ])
      expect(rs, line).toContain(line);
    for (const message of [ADDED_STATEMENT_MESSAGE, TRANSACTION_OPEN_MESSAGE])
      expect(rs.replace(/"\s*\n\s*"/g, "").replace(/ \\\n\s+/g, " ")).toContain(message);
    expect(rs).toContain("Snowflake cancels one query, not a whole session: use Kill Query");
  });

  it("is Snowflake's, with seed profiles for ?mockDrivers=snowflake", () => {
    expect(lane.drivers.map((d) => d.driver)).toEqual(["snowflake"]);
    expect(added).toEqual(["p-mock-snowflake", "p-mock-snowflake-ro", "p-mock-snowflake-mfa"]);
  });

  it("connects with the backend's version and caps", async () => {
    const info = await shop();
    expect(info.serverVersion).toBe("Snowflake 9.38.1");
    expect(info.currentDatabase).toBe("TABLER_DEMO");
    expect(info.capabilities).toEqual(snowflakeCaps("PUBLIC"));
    const caps = info.capabilities!;
    expect(caps.dialect).toBe("snowflake");
    expect(caps.features).toEqual(FEATURES);
    for (const f of [
      "returning",
      "editWithoutKey",
      "serverExpressions",
      "explainAnalyze",
      "structureEdit",
      "copyAsInsert",
      "users",
      "sequences",
      "exportTables",
    ])
      expect(caps.features, f).not.toContain(f);
    expect(caps.namespaces).toMatchObject({
      databases: true,
      schemas: "schemas",
      defaultSchema: "PUBLIC",
    });
    expect(caps.data).toMatchObject({
      paging: "offset",
      count: "costly",
      rawFilter: "sqlWhere",
      transactions: "atomic",
      affected: "exact",
      maxParams: 1000,
    });
    expect(caps.data.filterOps).toEqual(FILTER_OPS);
    expect(caps.safety).toEqual({
      readOnly: "classifierOnly",
      hardened: false,
      confirm: "passcode",
    });
    expect(caps.session).toEqual({ keepaliveSecs: null, pingBilled: false });
    expect((await analytics()).readOnly).toBe(true);
  });

  it("refuses what the backend's validate and sign-in refuse, in their words", async () => {
    const refused = async (patch: (p: ConnectionProfile) => object, message: string) => {
      const id = `p-x${added.length}`;
      variant("p-mock-snowflake", id, patch);
      await expect(connect(id), message).rejects.toMatchObject({ message });
    };
    const params = (p: ConnectionProfile, over: object) => ({
      options: { ...p.options, driverParams: { ...p.options.driverParams, ...over } },
    });
    await refused(() => ({ user: "" }), "User is required");
    await refused((p) => params(p, { account: "" }), "Snowflake: account is required");
    await refused(
      (p) => params(p, { auth: "sso" }),
      "Snowflake settings: unknown variant `sso`, expected one of `keyPair`, `pat`, `oauth`, `password`",
    );
    await refused(
      (p) => params(p, { account: "x;DROP" }),
      "“x;DROP” is no Snowflake account identifier (orgname-account_name or locator.region)",
    );
    await refused(
      (p) => params(p, { privateKeyPath: "" }),
      "Snowflake: key pair sign-in needs the private key file",
    );
    await refused(
      (p) => params(p, { baseUrl: "http://example.com" }),
      "Snowflake endpoint “http://example.com”: use https:// (plain http only reaches a test server on this machine)",
    );
    await refused(
      (p) => params(p, { sessionParameters: { "X; DROP": 1 } }),
      "“X; DROP” is no Snowflake session parameter name",
    );
    await refused(
      (p) => ({ options: { ...params(p, { auth: "pat" }).options, passwordMode: "none" } }),
      "authentication failed: enter the Snowflake programmatic access token",
    );
    await refused(
      (p) => ({ options: { ...p.options, startupSql: "ALTER SESSION SET AUTOCOMMIT = FALSE" } }),
      AUTOCOMMIT_MESSAGE,
    );
    // The startup commands run on the app's session too: its quoted names keep their case.
    await refused(
      (p) => ({
        options: {
          ...p.options,
          startupSql: "ALTER SESSION SET QUOTED_IDENTIFIERS_IGNORE_CASE = TRUE",
        },
      }),
      IGNORE_CASE_MESSAGE,
    );
    // A connections file is imported without the form's checks: a share is never opened, and
    // the key's JWT never goes to another host.
    for (const share of ["\\\\attacker\\share\\k.p8", "/\\attacker\\share\\k.p8"])
      await refused(
        (p) => params(p, { privateKeyPath: share }),
        `Snowflake: the key file “${share}” must be a full path on this machine (C:\\keys\\rsa_key.p8 or /keys/rsa_key.p8), not a network share`,
      );
    await refused(
      (p) => params(p, { baseUrl: "https://attacker.example" }),
      "Snowflake endpoint “https://attacker.example”: a key pair signs in only at Snowflake's own endpoint (*.snowflakecomputing.com) or a test server on this machine (another host could sign in as you with the token)",
    );
    // A read-only profile refuses a startup command that writes, before signing in.
    const id = `p-ro${added.length}`;
    variant("p-mock-snowflake-ro", id, (p) => ({
      options: { ...p.options, startupSql: "USE ROLE ACCOUNTADMIN" },
    }));
    await expect(connect(id)).rejects.toMatchObject({
      code: "INVALID_INPUT",
      message:
        "Snowflake: this connection is read-only, and its startup commands write (or are not known to only read)",
    });
  });

  it("answers the form's calls: the key's fingerprint and what the sign-in may pick", async () => {
    const p = mockProfiles.get("p-mock-snowflake")!;
    expect(await util(p, "keyFingerprint")).toEqual({ fingerprint: KEY_FINGERPRINT });
    await expect(
      util(p, "keyFingerprint", { privateKeyPath: "/k/rsa_key_encrypted.p8" }),
    ).rejects.toMatchObject({
      message: "Snowflake: the private key is encrypted; its fingerprint shows once it signs in",
    });
    // `/\host` is `\\host` on Windows: a share is never opened.
    await expect(
      util(p, "keyFingerprint", { privateKeyPath: "/\\host\\share\\k.p8" }),
    ).rejects.toMatchObject({
      code: "INVALID_INPUT",
      message: expect.stringContaining("not a network share"),
    });
    expect(await util(p, "listWarehouses")).toEqual(["COMPUTE_WH", "ETL_WH", "REPORTING_WH"]);
    expect(await util(p, "listRoles")).toContain("SYSADMIN");
    expect(await util(p, "listDatabases")).toEqual(["ANALYTICS", "SNOWFLAKE", "TABLER_DEMO"]);
    // A PAT sign-in needs the token; the read-only profile may still list (all reads).
    const ro = mockProfiles.get("p-mock-snowflake-ro")!;
    await expect(util(ro, "listRoles")).rejects.toMatchObject({
      code: "AUTH_FAILED",
      message: "authentication failed: enter the Snowflake programmatic access token",
    });
    expect(await util(ro, "listRoles", null, "pat-1")).toContain("ANALYST");
    // An unknown call writes: refused read-only before it runs, unknown otherwise.
    await expect(util(ro, "dropEverything")).rejects.toMatchObject({ message: READ_ONLY_MESSAGE });
    await expect(util(p, "dropEverything")).rejects.toMatchObject({
      code: "UNSUPPORTED",
      message: "unsupported: Snowflake: unknown call “dropEverything”",
    });
    expect(lane.drivers[0]!.utilKind!("listWarehouses")).toBe("read");
    expect(lane.drivers[0]!.utilKind!("dropEverything")).toBe("write");
  });
});

describe("a session", () => {
  it("lists through SHOW: databases, schemas, tables then views, structure, DDL, routines", async () => {
    const { connectionId } = await shop();
    const dbs = await mockInvoke<DatabaseInfo[]>("list_databases", { connectionId });
    expect(dbs.map((d) => [d.name, d.owner, d.isCurrent])).toEqual([
      ["ANALYTICS", "SYSADMIN", false],
      ["SNOWFLAKE", null, false],
      ["TABLER_DEMO", "SYSADMIN", true],
    ]);
    const schemas = await mockInvoke<SchemaInfo[]>("list_schemas", { connectionId });
    expect(schemas).toEqual([
      { name: "INFORMATION_SCHEMA", isDefault: false },
      { name: "PUBLIC", isDefault: true },
      { name: "SALES", isDefault: false },
    ]);
    const tables = await mockInvoke<TableSummary[]>("list_tables", {
      connectionId,
      schema: "PUBLIC",
    });
    expect(tables.map((t) => [t.name, t.kind, t.rowEstimate])).toEqual([
      ["CUSTOMERS", "table", 60],
      ["EVENTS_RAW", "table", 25],
      ["ORDERS", "table", 140],
      ["CUSTOMER_TOTALS", "view", null],
    ]);
    const meta = await mockInvoke<TableMeta>("get_table", {
      connectionId,
      table: { schema: "PUBLIC", name: "ORDERS" },
    });
    expect(meta.primaryKey).toEqual(["ORDER_ID"]);
    expect(meta.foreignKeys.map((f) => [f.columns, f.refTable.name, f.refColumns])).toEqual([
      [["CUSTOMER_ID"], "CUSTOMERS", ["ID"]],
    ]);
    expect(meta.columns.map((c) => [c.name, c.dataType, c.kind])).toEqual([
      ["ORDER_ID", "NUMBER(38,0)", "int"],
      ["CUSTOMER_ID", "NUMBER(38,0)", "int"],
      ["AMOUNT", "NUMBER(12,2)", "decimal"],
      ["STATUS", "VARCHAR(16)", "text"],
      ["ORDERED_AT", "TIMESTAMP_TZ(3)", "timestampTz"],
      ["TRACKING_ID", "NUMBER(38,0)", "int"],
    ]);
    const c = await mockInvoke<TableMeta>("get_table", { connectionId, table: customers });
    expect(c.indexes.map((i) => [i.name, i.columns, i.primary])).toEqual([
      ["SYS_CONSTRAINT_PK_CUSTOMERS", ["ID"], true],
      ["UQ_CUSTOMERS_EMAIL", ["EMAIL"], false],
    ]);
    expect(c.columns[0]).toMatchObject({ name: "ID", isPk: true, isAutoIncrement: true });
    const ddl = await mockInvoke<string>("get_table_ddl", { connectionId, table: customers });
    expect(ddl).toMatch(/^create or replace TABLE CUSTOMERS \(\n/);
    expect(ddl).toContain("\tprimary key (ID)");
    expect(
      await mockInvoke<string | null>("get_view_definition", {
        connectionId,
        table: { schema: "PUBLIC", name: "CUSTOMER_TOTALS" },
      }),
    ).toMatch(/^create or replace view CUSTOMER_TOTALS as/);
    const routines = await mockInvoke<RoutineSummary[]>("list_routines", {
      connectionId,
      schema: "PUBLIC",
    });
    expect(routines.map((r) => [r.name, r.kind, r.arguments])).toEqual([
      ["ORDER_TOTAL", "function", "CUSTOMER NUMBER"],
      ["ARCHIVE_ORDERS", "procedure", "BEFORE DATE"],
    ]);
    expect(
      await mockInvoke<string>("get_routine_definition", {
        connectionId,
        routine: {
          schema: "PUBLIC",
          name: "ARCHIVE_ORDERS",
          kind: "procedure",
          identityArguments: "BEFORE DATE",
        },
      }),
    ).toMatch(/^CREATE OR REPLACE PROCEDURE "ARCHIVE_ORDERS"/);
    expect(await mockInvoke<SessionDetails>("session_details", { connectionId })).toEqual({
      tls: true,
      tlsVersion: null,
      tlsCipher: null,
      encoding: "UTF-8",
    });
  });

  it("switches database by connecting again", async () => {
    const { connectionId } = await shop();
    const info = await mockInvoke<ConnectionInfo>("switch_database", {
      connectionId,
      database: "ANALYTICS",
    });
    expect(info.currentDatabase).toBe("ANALYTICS");
    const tables = await mockInvoke<TableSummary[]>("list_tables", {
      connectionId,
      schema: "PUBLIC",
    });
    expect(tables.map((t) => t.name)).toEqual(["DAILY_REVENUE"]);
  });

  it("pages by offset, sorts, filters, counts (the metadata's until asked) and fetches a cell", async () => {
    const { connectionId } = await shop();
    const first = await page(connectionId);
    expect(first.identity).toBe("pk");
    expect(first.rows).toHaveLength(10);
    expect(first.sql).toBe(
      'SELECT "ID", "NAME", "EMAIL", "COUNTRY", "SIGNED_UP", "PROFILE" FROM "PUBLIC"."CUSTOMERS" ORDER BY "ID" ASC LIMIT 10 OFFSET 0',
    );
    const last = await page(connectionId, { offset: 55 });
    expect(last.rows.map((r) => r[0])).toEqual([56, 57, 58, 59, 60].map((v) => ({ t: "int", v })));
    const sorted = await page(connectionId, {
      sort: [{ column: "NAME", dir: "desc", nulls: null }],
      columns: ["ID", "NAME"],
      limit: 3,
    });
    expect(sorted.columns.map((c) => c.name)).toEqual(["ID", "NAME"]);
    expect(sorted.sql).toContain('ORDER BY "NAME" DESC LIMIT 3 OFFSET 0');
    // VARIANT cells are JSON; a NUMBER past 2^53 travels as text.
    expect(first.rows[1]?.[5]).toMatchObject({ t: "json" });
    const orders = await page(connectionId, {
      table: { schema: "PUBLIC", name: "ORDERS" },
      limit: 1,
    });
    expect(orders.rows[0]?.[5]).toEqual({ t: "bigInt", v: "9007199254740993" });
    // No key: pages, but not editable.
    expect((await page(connectionId, { table: events })).identity).toBe("none");
    // Costly: the metadata's row count, exact only on request.
    expect(await count(connectionId)).toEqual({ count: 60, exact: false });
    expect(await count(connectionId, { exact: true })).toEqual({ count: 60, exact: true });
    const indonesia = {
      conjunction: "and",
      items: [{ kind: "condition", column: "COUNTRY", op: "eq", values: [{ t: "text", v: "ID" }] }],
    };
    expect(await count(connectionId, { filter: indonesia })).toEqual({ count: 8, exact: true });
    const email = await mockInvoke<{ t: string; v: string }>("fetch_cell", {
      connectionId,
      table: customers,
      key: key(2),
      column: "EMAIL",
    });
    expect(email.v).toBe("budi.2@example.com");
    // A raw WHERE holding a second statement (`;` or `->>`) is refused before anything is sent.
    for (const filter of ["1=1; DELETE FROM t", "1=1 ->> DELETE FROM t"])
      await expect(page(connectionId, { filter: raw(filter) }), filter).rejects.toMatchObject({
        code: "INVALID_INPUT",
        message: ADDED_STATEMENT_MESSAGE,
      });
    expect((await page(connectionId, { filter: raw("ID > 0") })).rows).toHaveLength(10);
  });

  it("commits staged changes in one transaction, and a failing change rolls back all", async () => {
    const { connectionId } = await shop();
    const req = (changes: object[]) => ({ table: customers, changes, returning: false });
    const good = [
      {
        op: "insert",
        rowId: "r2",
        values: {
          NAME: { t: "text", v: "Lestari Putri" },
          PROFILE: { t: "json", v: '{"tier":"gold"}' },
        },
      },
      {
        op: "update",
        rowId: "r1",
        key: key(1),
        changes: { NAME: { t: "text", v: "Ayu O'Neil \\ Santoso" } },
      },
    ];
    // Deletes, then updates, then inserts; backslash-escaped strings; a VARIANT through
    // INSERT … SELECT (Snowflake refuses functions in VALUES).
    expect(await mockInvoke<string[]>("preview_changes", { connectionId, req: req(good) })).toEqual(
      [
        `UPDATE "PUBLIC"."CUSTOMERS" SET "NAME" = 'Ayu O\\'Neil \\\\ Santoso' WHERE "ID" = 1`,
        `INSERT INTO "PUBLIC"."CUSTOMERS" ("NAME", "PROFILE") SELECT 'Lestari Putri', PARSE_JSON('{"tier":"gold"}')`,
      ],
    );
    const ok = await mockInvoke<ApplyResult>("apply_changes", { connectionId, req: req(good) });
    expect(ok.success).toBe(true);
    expect(ok.results.map((r) => [r.rowId, r.affected, r.returned])).toEqual([
      ["r1", 1, null],
      ["r2", 1, null],
    ]);
    // The identity and the defaults Snowflake fills in.
    expect((await page(connectionId, { offset: 60 })).rows[0]?.slice(0, 4)).toEqual([
      { t: "int", v: 61 },
      { t: "text", v: "Lestari Putri" },
      { t: "null" },
      { t: "text", v: "ID" },
    ]);
    const bad = [
      { op: "update", rowId: "u1", key: key(2), changes: { NAME: { t: "null" } } },
      { op: "delete", rowId: "d1", key: key(61) },
    ];
    const failed = await mockInvoke<ApplyResult>("apply_changes", { connectionId, req: req(bad) });
    expect(failed.success).toBe(false);
    expect(failed.error).toMatchObject({
      code: "DB_ERROR",
      sqlstate: "22000",
      vendorCode: 100072,
      statementIndex: 1,
      message: "NULL result in a non-nullable column 'NAME'",
    });
    expect(failed.results.map((r) => [r.rowId, r.error?.vendorCode ?? null])).toEqual([
      ["d1", null],
      ["u1", 100072],
    ]);
    // The DELETE before it was rolled back with it.
    expect((await page(connectionId, { offset: 60 })).rows).toHaveLength(1);
    // A row that is gone fails `expect_affected`.
    const gone = await mockInvoke<ApplyResult>("apply_changes", {
      connectionId,
      req: req([{ op: "delete", rowId: "x", key: key(999) }]),
    });
    expect(gone.error?.message).toBe(GUARD_MESSAGE);
    await expect(
      mockInvoke("apply_changes", {
        connectionId,
        req: {
          table: events,
          returning: false,
          changes: [{ op: "delete", rowId: "k", key: { kind: "allColumns", value: {} } }],
        },
      }),
    ).rejects.toMatchObject({
      code: "UNSUPPORTED",
      message: "unsupported: Snowflake: editing rows without a primary key is not available",
    });
  });

  it("runs the editor's statements one at a time, with Snowflake's errors and DML counts", async () => {
    const { connectionId } = await shop();
    const r = await run(
      connectionId,
      "SELECT CURRENT_VERSION(), CURRENT_DATABASE();\nSELECT NAME FROM CUSTOMERS WHERE COUNTRY = 'SG' ORDER BY ID LIMIT 2;\nSELECT COUNT(*) FROM TABLER_DEMO.PUBLIC.ORDERS;\nUPDATE ORDERS SET STATUS = 'PAID' WHERE CUSTOMER_ID = 1;\nSELECT * FROM NOPE",
    );
    expect(r.results.map((x) => x.kind)).toEqual(["rows", "rows", "rows", "affected", "empty"]);
    expect(r.rows[0]).toEqual([
      { t: "text", v: "9.38.1" },
      { t: "text", v: "TABLER_DEMO" },
    ]);
    expect(r.rows.slice(1, 3)).toEqual([
      [{ t: "text", v: "Farah de Vries" }],
      [{ t: "text", v: "Dewi de Vries" }],
    ]);
    expect(r.rows[3]).toEqual([{ t: "int", v: 140 }]);
    expect(r.results[3]?.affected).toBe(3);
    expect(r.results[4]?.error).toMatchObject({
      code: "DB_ERROR",
      sqlstate: "42S02",
      vendorCode: 2003,
      message: "SQL compilation error:\nObject 'NOPE' does not exist or not authorized.",
    });
    // A transaction lasts across runs; ROLLBACK undoes it.
    await run(connectionId, "BEGIN");
    expect((await run(connectionId, "DELETE FROM ORDERS")).results[0]?.affected).toBe(140);
    await run(connectionId, "ROLLBACK");
    expect((await run(connectionId, "SELECT COUNT(*) FROM ORDERS")).rows).toEqual([
      [{ t: "int", v: 140 }],
    ]);
    // AUTOCOMMIT stays on: an editor statement naming it is refused before it is sent.
    await expect(run(connectionId, "ALTER SESSION SET AUTOCOMMIT = FALSE")).rejects.toMatchObject({
      code: "INVALID_INPUT",
      message: AUTOCOMMIT_MESSAGE,
    });
    // Explain is EXPLAIN USING TEXT.
    const plan = await mockInvoke<ExplainResult>("explain", {
      connectionId,
      sql: "SELECT * FROM ORDERS",
      mode: "plan",
    });
    expect(plan.text).toContain("->TableScan  TABLER_DEMO.PUBLIC.ORDERS");
    // A text of several statements never reaches a session's `run` (the editor splits it), and
    // Snowflake refuses it (MULTI_STATEMENT_COUNT = 1).
    const session = lane.drivers[0]!.connect(mockProfiles.get("p-mock-snowflake")!);
    const failed: QueryEvent[] = [];
    void session.run("SELECT 1 ->> SELECT 2", (e) => failed.push(e));
    expect(failed.at(-1)).toMatchObject({
      event: "failed",
      data: { error: { vendorCode: 8, message: SEVERAL_MESSAGE } },
    });
    // `USE` moves the editor's session.
    await run(connectionId, "USE SCHEMA SALES");
    expect((await run(connectionId, "SELECT COUNT(*) FROM REGIONS")).rows).toEqual([
      [{ t: "int", v: 3 }],
    ]);
  });

  it("lists the user's queries and cancels one, never a whole session", async () => {
    const { connectionId } = await shop();
    const list = await mockInvoke<ProcessRow[]>("admin_list_processes", { connectionId });
    expect(list.map((p) => [p.state, p.application])).toEqual([
      ["RUNNING", "ETL_WH"],
      ["RESUMING_WAREHOUSE", "REPORTING_WH"],
    ]);
    const kill = (ids: string[], scope: string) =>
      mockInvoke<KillOutcome[]>("admin_kill_processes", { connectionId, ids, scope });
    expect(await kill([list[1]!.id], "connection")).toEqual([
      { id: list[1]!.id, ok: false, message: KILL_CONNECTION_MESSAGE },
    ]);
    expect(await kill([list[0]!.id], "query")).toEqual([
      { id: list[0]!.id, ok: true, message: null },
    ]);
    const after = await mockInvoke<ProcessRow[]>("admin_list_processes", { connectionId });
    expect(after.map((p) => p.id)).toEqual([list[1]!.id]);
  });
});

describe("a read-only session", () => {
  it("refuses what the classifier does not call a read, before anything is sent", async () => {
    const { connectionId } = await analytics();
    for (const sql of [
      "DELETE FROM PUBLIC.DAILY_REVENUE",
      "SELECT SYSTEM$CANCEL_ALL_QUERIES(1)",
      "USE DATABASE TABLER_DEMO",
      "CALL archive_orders('2026-01-01')",
      "EXECUTE IMMEDIATE $$DELETE FROM t$$",
      "SELECT seq.NEXTVAL",
      'SELECT seq."NEXTVAL"',
      "LIST @my_stage",
      "BEGIN",
    ])
      await expect(run(connectionId, sql), sql).rejects.toMatchObject({
        code: "INVALID_INPUT",
        message: READ_ONLY_MESSAGE,
      });
    expect((await run(connectionId, "SELECT COUNT(*) FROM PUBLIC.DAILY_REVENUE")).rows).toEqual([
      [{ t: "int", v: 31 }],
    ]);
    const revenue = { schema: "PUBLIC", name: "DAILY_REVENUE" };
    // In the grid's reads too, and in staged changes.
    await expect(
      page(connectionId, { table: revenue, filter: raw("SYSTEM$WAIT(10) IS NULL") }),
    ).rejects.toMatchObject({ message: READ_ONLY_MESSAGE });
    for (const cmd of ["preview_changes", "apply_changes"])
      await expect(
        mockInvoke(cmd, {
          connectionId,
          req: {
            table: revenue,
            returning: false,
            changes: [
              {
                op: "delete",
                rowId: "d",
                key: { kind: "pk", value: { DAY: { t: "date", v: "2026-10-01" } } },
              },
            ],
          },
        }),
      ).rejects.toMatchObject({ code: "UNSUPPORTED", message: READ_ONLY_EDIT });
    await expect(
      mockInvoke("admin_kill_processes", { connectionId, ids: ["x"], scope: "query" }),
    ).rejects.toMatchObject({ code: "UNSUPPORTED" });
    expect((await page(connectionId, { table: revenue })).rows).toHaveLength(10);
    // The connection's session refuses the kill itself too (`SYSTEM$CANCEL_QUERY` writes).
    const session = lane.drivers[0]!.connect(mockProfiles.get("p-mock-snowflake-ro")!);
    expect(session.killProcesses!(["x"], "query")).toEqual([
      { id: "x", ok: false, message: READ_ONLY_MESSAGE },
    ]);
  });
});

describe("the shared session of a password + MFA sign-in", () => {
  it("keeps the grid out of the editor's transaction and away from another database", async () => {
    const { connectionId } = await mfa();
    const req = {
      table: customers,
      returning: false,
      changes: [
        { op: "update", rowId: "u", key: key(3), changes: { COUNTRY: { t: "text", v: "SG" } } },
      ],
    };
    await run(connectionId, "BEGIN");
    await expect(mockInvoke("apply_changes", { connectionId, req })).rejects.toMatchObject({
      code: "INVALID_INPUT",
      message: TRANSACTION_OPEN_MESSAGE,
    });
    await run(connectionId, "COMMIT");
    expect((await mockInvoke<ApplyResult>("apply_changes", { connectionId, req })).success).toBe(
      true,
    );
    await run(connectionId, "USE DATABASE ANALYTICS");
    const moved = movedMessage("ANALYTICS", "TABLER_DEMO");
    expect(moved).toBe(
      'Snowflake: the editor moved this session to database ANALYTICS (the app shares it, as the sign-in asks MFA), and the app\'s statements would land there: run USE DATABASE "TABLER_DEMO" in the editor first',
    );
    await expect(page(connectionId)).rejects.toMatchObject({ message: moved });
    await expect(count(connectionId, { exact: true })).rejects.toMatchObject({ message: moved });
    await run(connectionId, "USE DATABASE TABLER_DEMO");
    expect((await page(connectionId)).rows).toHaveLength(10);
    // The grid's quoted names keep their case: the editor may not change it on this session.
    for (const sql of [
      "ALTER SESSION SET QUOTED_IDENTIFIERS_IGNORE_CASE = TRUE",
      "alter session unset quoted_identifiers_ignore_case",
    ])
      await expect(run(connectionId, sql), sql).rejects.toMatchObject({
        code: "INVALID_INPUT",
        message: IGNORE_CASE_MESSAGE,
      });
    // Nor the input formats the grid's dates, times and timestamps are read with.
    await expect(
      run(connectionId, "ALTER SESSION SET DATE_INPUT_FORMAT = 'YYYY-DD-MM'"),
    ).rejects.toMatchObject({
      code: "INVALID_INPUT",
      message: appParameterMessage("DATE_INPUT_FORMAT"),
    });
  });

  it("lets the editor's own session match names in any case", async () => {
    const { connectionId } = await shop();
    const r = await run(connectionId, "ALTER SESSION SET QUOTED_IDENTIFIERS_IGNORE_CASE = TRUE");
    expect(r.results[0]?.error).toBeNull();
  });
});

describe("the backend's classifier in the mock", () => {
  it("reads writes, a quoted NEXTVAL and AUTOCOMMIT as the backend does", () => {
    expect(writes('SELECT seq."NEXTVAL"')).toBe(true);
    expect(quotesNextval("SELECT 'NEXTVAL'")).toBe(false);
    expect(writes("SELECT * FROM t")).toBe(false);
    for (const sql of [
      "ALTER SESSION SET AUTOCOMMIT = FALSE",
      "alter session set autocommit=false",
      "CALL set_autocommit(false) /* AUTOCOMMIT */",
    ])
      expect(setsAutocommit(sql), sql).toBe(true);
    for (const sql of [
      "ALTER USER u SET AUTOCOMMIT = FALSE",
      "ALTER ACCOUNT SET AUTOCOMMIT = FALSE",
      "SELECT 'AUTOCOMMIT'",
      "ALTER SESSION SET AUTOCOMMIT_X = 1",
    ])
      expect(setsAutocommit(sql), sql).toBe(false);
    expect(setsIgnoreCase("ALTER SESSION SET QUOTED_IDENTIFIERS_IGNORE_CASE = TRUE")).toBe(true);
    expect(setsIgnoreCase("ALTER USER u SET QUOTED_IDENTIFIERS_IGNORE_CASE = TRUE")).toBe(false);
    expect(setsIgnoreCase("SELECT 'QUOTED_IDENTIFIERS_IGNORE_CASE'")).toBe(false);
    expect(setsAppParameter("alter session set timestamp_input_format = 'DD/MM/YYYY'")).toBe(
      "TIMESTAMP_INPUT_FORMAT",
    );
    expect(setsAppParameter("ALTER SESSION SET QUOTED_IDENTIFIERS_IGNORE_CASE = TRUE")).toBe(
      "QUOTED_IDENTIFIERS_IGNORE_CASE",
    );
    // The text only, as the backend's check; the backend sets the parameters back after these.
    expect(setsAppParameter("EXECUTE IMMEDIATE $q")).toBeNull();
    expect(setsAppParameter("ALTER SESSION SET TIMEZONE = 'UTC'")).toBeNull();
  });
});
