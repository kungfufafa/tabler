/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { beforeEach, describe, expect, it } from "vitest";
import type { ApplyRequest } from "../../../bindings/ApplyRequest";
import type { BrowseRequest } from "../../../bindings/BrowseRequest";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { FilterGroup } from "../../../bindings/FilterGroup";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { Value } from "../../../bindings/Value";
import type { MockSession } from "../types";
import lane, { clickhouseMock } from ".";
import { resetServer } from "./data";
import { clickhouseCaps, READ_ONLY_MESSAGE, resetProcesses } from "./session";

// The mock's ClickHouse: caps equal to the backend's recorded ones
// (`drivers::clickhouse::session_tests::caps_match_the_recorded_fixture` writes the same file),
// the server as the one database and its databases as schemas, offset pages, counts, a commit
// with one failing change (applied one by one, so those before it stay), the editor in its own
// HTTP session, Running Queries, and the refusals of a read-only profile on every path.

const ROOT = path.resolve(import.meta.dirname, "../../../../../..");
const RECORDED = JSON.parse(
  readFileSync(path.join(ROOT, "src-tauri/tests/fixtures/clickhouse/caps.json"), "utf8"),
) as Record<"password" | "command" | "readOnly", SessionCaps>;

const seed = (id: string): ConnectionProfile => {
  const p = clickhouseMock.seedProfiles().find((s) => s.id === id);
  if (!p) throw new Error(id);
  return p;
};

function connect(
  options: Partial<ConnectionProfile["options"]> = {},
  fields: Partial<ConnectionProfile> = {},
): MockSession {
  const p = seed("p-mock-clickhouse");
  return clickhouseMock.connect({ ...p, ...fields, options: { ...p.options, ...options } });
}

async function run(s: MockSession, sql: string): Promise<QueryEvent[]> {
  const events: QueryEvent[] = [];
  await s.run(sql, (e) => events.push(e));
  return events;
}

const finished = (events: QueryEvent[]): StatementResult => {
  const e = events.find((x) => x.event === "finished");
  if (e?.event !== "finished") throw new Error(JSON.stringify(events));
  return e.data.result;
};

const failure = (events: QueryEvent[]) => {
  const e = events.find((x) => x.event === "failed");
  if (e?.event !== "failed") throw new Error(JSON.stringify(events));
  return e.data.error;
};

const rowsOf = (events: QueryEvent[]): Value[][] =>
  events.flatMap((e) => (e.event === "rows" ? e.data.rows : []));

const thrown = (f: () => unknown): { code: string; message: string } => {
  try {
    f();
  } catch (e) {
    return e as { code: string; message: string };
  }
  throw new Error("did not throw");
};

const orders = { schema: "shop", name: "orders" };
const page = (over: Partial<BrowseRequest> = {}): BrowseRequest => ({
  table: orders,
  columns: null,
  filter: null,
  sort: [],
  limit: 100,
  offset: 0,
  ...over,
});
const where = (items: FilterGroup["items"], conjunction: "and" | "or" = "and"): FilterGroup => ({
  conjunction,
  items,
});
const int = (v: number): Value => ({ t: "int", v });
const text = (v: string): Value => ({ t: "text", v });
const key = (id: number) => ({ kind: "pk" as const, value: { id: int(id) } });
const changes = (list: ApplyRequest["changes"], table = orders): ApplyRequest => ({
  table,
  changes: list,
  returning: false,
});

beforeEach(() => {
  resetServer();
  resetProcesses();
});

describe("the clickhouse mock lane", () => {
  it("has one driver with a local and a read-only Cloud profile", () => {
    expect(lane.drivers).toEqual([clickhouseMock]);
    const [local, cloud] = clickhouseMock.seedProfiles();
    expect(local!).toMatchObject({ driver: "clickhouse", port: 8123, ssl: "disable" });
    expect(clickhouseMock.serverVersion(local!)).toBe("ClickHouse 25.8.2.29");
    expect(clickhouseMock.serverVersion(cloud!)).toBe("ClickHouse 25.10.1.3832");
    expect(cloud!).toMatchObject({ port: 8443, ssl: "verify-full" });
    expect(cloud!.options).toMatchObject({ readOnly: true, tag: "production" });
    expect(clickhouseMock.util).toBeUndefined();
  });

  it("gives a session the caps the backend records", () => {
    expect(clickhouseCaps("default", "password", false)).toEqual(RECORDED.password);
    expect(clickhouseCaps("default", "passcode", false)).toEqual(RECORDED.command);
    expect(clickhouseCaps("default", "password", true)).toEqual(RECORDED.readOnly);
    expect(connect({}, { database: null }).caps).toEqual(RECORDED.password);
    expect(
      connect(
        { passwordMode: "command", passwordCommand: "vault read -field=pw ch" },
        {
          database: null,
        },
      ).caps,
    ).toEqual(RECORDED.command);
    expect(connect({ readOnly: true }, { database: null }).caps).toEqual(RECORDED.readOnly);
    expect(connect().caps.namespaces.defaultSchema).toBe("shop");
  });

  it("offers no method the backend lacks", () => {
    const s = connect();
    for (const m of ["call", "listRoutines", "routineDefinition", "listUsers", "loadUser"] as const)
      expect(s[m], m).toBeUndefined();
    for (const m of ["listProcesses", "killProcesses", "sessionDetails", "tableDdl"] as const)
      expect(s[m], m).toBeTypeOf("function");
  });

  it("checks the profile as `validate` and `open` do", () => {
    expect(thrown(() => connect({}, { host: " " }))).toEqual({
      code: "INVALID_INPUT",
      message: "Host is required",
    });
    expect(
      thrown(() => connect({ driverParams: { settings: [{ name: "readonly", value: "0" }] } }))
        .message,
    ).toBe(
      "“readonly” is set by Tabler or changes how statements are read, so it cannot be a connection setting",
    );
    expect(
      thrown(() => connect({ driverParams: { settings: [{ name: "a-b", value: "1" }] } })).message,
    ).toBe("“a-b” is not a ClickHouse setting name");
    expect(thrown(() => connect({ driverParams: { roles: ["a\u0007"] } })).message).toBe(
      "ClickHouse: “a\u0007” is not a role name",
    );
    expect(thrown(() => connect({ driverParams: { sessionTimeoutSecs: 0 } })).message).toBe(
      "ClickHouse: the session timeout is 1 to 3600 seconds",
    );
    expect(thrown(() => connect({ sslCertPath: "/c.pem" })).message).toBe(
      "ClickHouse: a client certificate needs both its certificate and its key file",
    );
    expect(thrown(() => connect({ driverParams: { certificateAuth: true } })).message).toBe(
      "ClickHouse: signing in with a certificate needs HTTPS (turn SSL on)",
    );
    expect(
      thrown(() => connect({ driverParams: { certificateAuth: true } }, { ssl: "verify-full" }))
        .message,
    ).toBe("ClickHouse: signing in with a certificate needs the client certificate and key files");
    expect(thrown(() => connect({}, { database: "nowhere" }))).toEqual({
      code: "DB_ERROR",
      message: "Database nowhere does not exist. (UNKNOWN_DATABASE)",
    });
  });
});

describe("the catalog", () => {
  it("lists the server as the database and its databases as schemas", () => {
    const s = connect();
    expect(s.listDatabases()).toEqual([
      { name: "127.0.0.1", owner: null, encoding: null, isCurrent: true },
    ]);
    expect(s.listSchemas()).toEqual([
      { name: "analytics", isDefault: false },
      { name: "default", isDefault: false },
      { name: "shop", isDefault: true },
      { name: "system", isDefault: false },
    ]);
    expect(s.listTables("shop").map((t) => [t.name, t.kind, t.rowEstimate])).toEqual([
      ["countries", "foreignTable", null],
      ["customer_revenue", "view", null],
      ["customers", "table", 12],
      ["daily_sales_mv", "materializedView", null],
      ["import_log", "table", 2],
      ["orders", "table", 240],
    ]);
    expect(s.listTables("system").every((t) => t.kind === "systemView")).toBe(true);
  });

  it("gives columns with their key roles, the keys as indexes, the engine and the server's DDL", () => {
    const s = connect();
    const meta = s.tableMeta(orders);
    expect(meta.primaryKey).toEqual(["id"]);
    expect(meta.columns.map((c) => [c.name, c.kind, c.extra])).toEqual([
      ["id", "bigInt", "primary key, sorting key"],
      ["customer_id", "int", null],
      ["status", "enum", null],
      ["total", "decimal", "CODEC(ZSTD(1))"],
      ["created_at", "timestamp", "partition key"],
      ["tags", "array", null],
      ["attrs", "json", null],
      ["note", "text", null],
      ["created_day", "date", "MATERIALIZED"],
    ]);
    expect(meta.columns[2]!.enumValues).toEqual([
      "new",
      "paid",
      "shipped",
      "delivered",
      "cancelled",
    ]);
    expect(meta.columns[3]).toMatchObject({ numericPrecision: 12, numericScale: 2 });
    expect(meta.columns[7]!.nullable).toBe(true);
    expect(meta.columns[8]!.isGenerated).toBe(true);
    expect(meta.indexes.map((i) => [i.name, i.method])).toEqual([
      ["PRIMARY", "primary key"],
      ["PARTITION BY", "partition by"],
      ["idx_status", "set(5)"],
      ["by_customer", "projection"],
    ]);
    expect(meta.options).toMatchObject({ sortKey: "id" });
    expect(meta.options?.engine).toContain("MergeTree PARTITION BY toYYYYMM(created_at)");
    expect(s.tableDdl!(orders)).toContain("CREATE TABLE shop.orders");
    expect(s.tableDdl!({ schema: "shop", name: "customer_revenue" })).toContain(
      "AS SELECT customer_id",
    );
    const views = s.tableMeta({ schema: "analytics", name: "page_views" });
    expect(views.primaryKey).toEqual(["site_id", "ts", "id"]);
    expect(views.columns[1]!.extra).toBe("primary key, sorting key, partition key");
    expect(thrown(() => s.tableMeta({ schema: "shop", name: "nope" })).code).toBe("NOT_FOUND");
  });
});

describe("the grid", () => {
  it("browses offset pages ordered by the key, with exact values", () => {
    const s = connect();
    const first = s.browse(page());
    expect(first.rows).toHaveLength(100);
    expect(first.identity).toBe("pk");
    expect(first.pkColumns).toEqual(["id"]);
    expect(first.rows[0]![0]).toEqual(int(1));
    expect(first.sql).toBe(
      "SELECT `id`, `customer_id`, `status`, `total`, `created_at`, `tags`, `attrs`, `note`, `created_day` FROM `shop`.`orders` ORDER BY `id` ASC LIMIT 100 OFFSET 0",
    );
    const last = s.browse(page({ offset: 200 }));
    expect(last.rows).toHaveLength(40);
    expect(last.rows.at(-1)![0]).toEqual(int(240));
    // Integers past 2^53 arrive as text, as `decode.rs` gives them.
    const views = s.browse(page({ table: { schema: "analytics", name: "page_views" }, limit: 1 }));
    expect(views.rows[0]![2]).toEqual({ t: "bigInt", v: "9007199254740992" });
    expect(views.rows[0]![4]).toEqual({ t: "other", v: { ty: "IPv4", text: "10.0.0.0" } });
    // A table without a primary key cannot be edited.
    expect(s.browse(page({ table: { schema: "shop", name: "import_log" } })).identity).toBe("none");
  });

  it("sorts, filters and counts, raw conditions read as the server reads them", () => {
    const s = connect();
    const desc = s.browse(
      page({ sort: [{ column: "total", dir: "desc", nulls: null }], limit: 3 }),
    );
    const totals = desc.rows.map((r) => Number((r[3] as { v: string }).v));
    expect(totals).toEqual([...totals].sort((a, b) => b - a));
    const paid = where([{ kind: "condition", column: "status", op: "eq", values: [text("paid")] }]);
    expect(s.count!(orders, paid)).toEqual({ count: 48, exact: true });
    const raw = where([{ kind: "raw", sql: "customer_id = 3 AND total > 100" }]);
    const n = s.count!(orders, raw).count;
    expect(n).toBeGreaterThan(0);
    expect(s.browse(page({ filter: raw })).rows.every((r) => (r[1] as { v: number }).v === 3)).toBe(
      true,
    );
    expect(s.count!(orders, null)).toEqual({ count: 240, exact: true });
    expect(thrown(() => s.count!(orders, where([{ kind: "raw", sql: "nope = 1" }])))).toEqual({
      code: "DB_ERROR",
      message: "Unknown expression identifier `nope` in scope SELECT. (UNKNOWN_IDENTIFIER)",
    });
    expect(
      thrown(() => s.browse(page({ filter: where([{ kind: "raw", sql: "1; DROP" }]) }))),
    ).toMatchObject({
      code: "DB_ERROR",
    });
  });

  it("fetches a cell by key", () => {
    const s = connect();
    expect(s.fetchCell!(orders, key(5), "note")).toEqual(text("Leave at the door (5)"));
    expect(thrown(() => s.fetchCell!(orders, key(999), "note")).code).toBe("NOT_FOUND");
  });
});

describe("commits", () => {
  it("previews each change as the backend writes it: deletes, then updates, then inserts", () => {
    const s = connect();
    expect(
      s.preview!(
        changes([
          {
            op: "insert",
            rowId: "n1",
            values: { id: int(500), status: text("new"), total: text("9.5") },
          },
          {
            op: "update",
            rowId: "r2",
            key: key(2),
            changes: { status: text("paid"), note: text("it's") },
          },
          { op: "delete", rowId: "r3", key: key(3) },
        ]),
      ),
    ).toEqual([
      "DELETE FROM `shop`.`orders` WHERE `id` = 3",
      "ALTER TABLE `shop`.`orders` UPDATE `note` = 'it\\'s', `status` = 'paid' WHERE `id` = 2",
      "INSERT INTO `shop`.`orders` (`id`, `status`, `total`) SELECT 500, 'new', '9.5'",
    ]);
    // The clocks as the dialect writes them (`server_expr`).
    const customers = { schema: "shop", name: "customers" };
    expect(
      s.preview!(
        changes(
          [
            {
              op: "update",
              rowId: "c1",
              key: key(1),
              changes: {},
              exprs: { updated_at: "now", signed_up: "currentDate" },
            },
            { op: "insert", rowId: "c2", values: { id: int(50) }, exprs: { updated_at: "now(3)" } },
          ],
          customers,
        ),
      ),
    ).toEqual([
      "ALTER TABLE `shop`.`customers` UPDATE `signed_up` = today(), `updated_at` = now() WHERE `id` = 1",
      "INSERT INTO `shop`.`customers` (`id`, `updated_at`) SELECT 50, now64(3)",
    ]);
  });

  it("refuses what ClickHouse cannot change, before anything is sent", () => {
    const s = connect();
    const refusal = (c: ApplyRequest["changes"][number]) => thrown(() => s.apply!(changes([c])));
    expect(refusal({ op: "update", rowId: "r", key: key(1), changes: { id: int(9) } })).toEqual({
      code: "UNSUPPORTED",
      message:
        'unsupported: "id" is part of the table\'s key (ORDER BY, PARTITION BY, PRIMARY KEY or SAMPLE BY), which ClickHouse cannot update',
    });
    expect(
      refusal({
        op: "update",
        rowId: "r",
        key: key(1),
        changes: { created_at: text("2026-01-01 00:00:00") },
      }).message,
    ).toContain('"created_at" is part of the table\'s key');
    expect(
      refusal({
        op: "update",
        rowId: "r",
        key: key(1),
        changes: { created_day: text("2026-01-01") },
      }).message,
    ).toBe(
      'unsupported: "created_day" is computed by the server (MATERIALIZED or ALIAS) and cannot be set',
    );
    expect(
      refusal({ op: "update", rowId: "r", key: key(1), changes: {}, exprs: { note: "default" } })
        .message,
    ).toBe('unsupported: ClickHouse SQL cannot set "note" to DEFAULT; enter the value explicitly');
    expect(refusal({ op: "insert", rowId: "n", values: { note: { t: "default" } } }).message).toBe(
      "unsupported: ClickHouse SQL cannot insert a row of defaults only; enter at least one value",
    );
    expect(
      refusal({ op: "delete", rowId: "r", key: { kind: "allColumns", value: { id: int(1) } } })
        .message,
    ).toBe("unsupported: ClickHouse: editing rows without a primary key is not available");
    expect(s.count!(orders, null).count).toBe(240);
  });

  it("applies changes one by one: a failing one stops the rest, those before it stay", () => {
    const s = connect();
    const result = s.apply!(
      changes([
        { op: "update", rowId: "r1", key: key(1), changes: { status: text("delivered") } },
        {
          op: "insert",
          rowId: "n1",
          values: { id: int(241), customer_id: int(1), status: text("new"), total: text("12.5") },
        },
        {
          op: "insert",
          rowId: "n2",
          values: { id: int(242), customer_id: text("abc"), total: text("1") },
        },
        {
          op: "insert",
          rowId: "n3",
          values: { id: int(243), customer_id: int(2), total: text("1") },
        },
      ]),
    );
    expect(result.success).toBe(false);
    expect(result.error).toMatchObject({
      code: "DB_ERROR",
      message:
        "Cannot parse string 'abc' as UInt32: syntax error at begin of string. (CANNOT_PARSE_TEXT)",
      vendorCode: 6,
      statementIndex: 2,
    });
    expect(result.results.map((r) => [r.rowId, r.error?.message ?? null])).toEqual([
      ["r1", null],
      ["n1", null],
      ["n2", result.error!.message],
      ["n3", "not applied: an earlier change failed"],
    ]);
    // Applied before the failure: no transaction rolls them back.
    expect(s.fetchCell!(orders, key(1), "status")).toEqual({ t: "enum", v: "delivered" });
    const added = s.browse(page({ filter: where([{ kind: "raw", sql: "id >= 241" }]) })).rows;
    expect(added.map((r) => r[0])).toEqual([int(241)]);
    // The new row took its DEFAULT and MATERIALIZED columns, its decimal at the column's scale.
    expect(added[0]![3]).toEqual({ t: "decimal", v: "12.50" });
    expect(added[0]![4]!.t).toBe("timestamp");
    expect(added[0]![8]).toEqual({ t: "date", v: (added[0]![4] as { v: string }).v.slice(0, 10) });
  });

  it("refuses an update or delete whose key matches other than one row", () => {
    const s = connect();
    // Keys are not unique: a second row with id 7.
    expect(
      s.apply!(changes([{ op: "insert", rowId: "n", values: { id: int(7), total: text("1") } }]))
        .success,
    ).toBe(true);
    const result = s.apply!(changes([{ op: "delete", rowId: "r7", key: key(7) }]));
    expect(result.error).toMatchObject({
      code: "DB_ERROR",
      message:
        "the row's key matches 2 row(s), not 1: ClickHouse keys are not unique, so the change was not applied",
      statementIndex: 0,
    });
    expect(
      s.apply!(changes([{ op: "delete", rowId: "r", key: key(999) }])).error?.message,
    ).toContain("matches 0 row(s)");
    expect(s.apply!(changes([{ op: "delete", rowId: "r8", key: key(8) }])).success).toBe(true);
    expect(s.count!(orders, null).count).toBe(240);
  });

  it("stay on the server for the next connection", () => {
    connect().apply!(
      changes([{ op: "update", rowId: "r", key: key(9), changes: { note: text("x") } }]),
    );
    expect(connect().fetchCell!(orders, key(9), "note")).toEqual(text("x"));
  });
});

describe("the editor", () => {
  it("runs reads, keeps its SETs in its session, and reports inserts as affected rows", async () => {
    const s = connect();
    const r = await run(s, "SELECT status, count() AS n FROM orders GROUP BY status");
    expect(finished(r)).toMatchObject({ kind: "rows", rowCount: 5, resultSets: 1 });
    expect(rowsOf(r)[0]).toEqual([{ t: "enum", v: "paid" }, int(48)]);
    expect(rowsOf(await run(s, "SELECT version(), currentDatabase()"))).toEqual([
      [text("25.8.2.29"), text("shop")],
    ]);
    expect(rowsOf(await run(s, "SELECT number FROM numbers(3)"))).toEqual([
      [int(0)],
      [int(1)],
      [int(2)],
    ]);
    expect(finished(await run(s, "SET max_threads = 2")).kind).toBe("empty");
    expect(rowsOf(await run(s, "SELECT getSetting('max_threads')"))).toEqual([[text("2")]]);
    const ins = await run(
      s,
      "INSERT INTO customers (id, name, email, country, signed_up) VALUES (13, 'Ida', 'ida@x', 'SE', '2026-01-02'), (14, 'Jo', 'jo@x', 'NO', '2026-01-03')",
    );
    expect(finished(ins)).toMatchObject({ kind: "affected", affected: 2 });
    expect(
      finished(await run(s, "ALTER TABLE customers UPDATE name = 'Ida B' WHERE id = 13")).kind,
    ).toBe("empty");
    expect(rowsOf(await run(s, "SELECT name FROM shop.customers WHERE id = 13"))).toEqual([
      [text("Ida B")],
    ]);
    expect(rowsOf(await run(s, "SHOW TABLES FROM analytics"))).toEqual([[text("page_views")]]);
    expect(
      rowsOf(await run(s, "SELECT * FROM customer_revenue WHERE customer_id = 1"))[0]![1],
    ).toEqual(int(20));
    expect(finished(await run(s, "EXPLAIN SELECT * FROM orders LIMIT 1")).rowCount).toBe(4);
  });

  it("fails with the server's errors in its words", async () => {
    const s = connect();
    expect(failure(await run(s, "SELECT * FROM nope"))).toMatchObject({
      code: "DB_ERROR",
      message: "Table shop.nope does not exist. (UNKNOWN_TABLE)",
      vendorCode: 60,
    });
    expect(failure(await run(s, "SELEC 1"))).toMatchObject({ code: "DB_ERROR", vendorCode: 62 });
    expect(failure(await run(s, "ALTER TABLE orders UPDATE id = 1 WHERE id = 2")).message).toBe(
      "Cannot UPDATE key column `id`. (CANNOT_UPDATE_COLUMN)",
    );
  });
});

describe("Running Queries", () => {
  it("lists the server's queries with the listing itself, and kills by query id", () => {
    const s = connect();
    const list = s.listProcesses!();
    expect(list.map((p) => [p.user, p.current, p.own])).toEqual([
      ["etl", false, false],
      ["grafana", false, false],
      ["default", true, true],
    ]);
    expect(list[0]!.durationMs).toBe(84200);
    const id = list[1]!.id;
    expect(s.killProcesses!([id], "query")).toEqual([{ id, ok: true, message: null }]);
    expect(s.listProcesses!().some((p) => p.id === id)).toBe(false);
    // `KILL QUERY … ASYNC` finds nothing to stop and says nothing.
    expect(s.killProcesses!(["gone"], "query")[0]!.ok).toBe(true);
    expect(s.killProcesses!([id], "connection")[0]).toEqual({
      id,
      ok: false,
      message:
        "unsupported: ClickHouse stops queries, not connections (each request is one): use Kill Query",
    });
  });

  it("says whether the session is over HTTPS, in UTF-8", () => {
    expect(connect().sessionDetails!()).toEqual({
      tls: false,
      tlsVersion: null,
      tlsCipher: null,
      encoding: "UTF-8",
    });
    expect(clickhouseMock.connect(seed("p-mock-clickhouse-cloud")).sessionDetails!().tls).toBe(
      true,
    );
  });
});

describe("a read-only profile", () => {
  const ro = () => clickhouseMock.connect(seed("p-mock-clickhouse-cloud"));

  it("reads, with `readonly=2` hardening in its caps", async () => {
    const s = ro();
    expect(s.caps.safety).toEqual({ readOnly: "perRequest", hardened: true, confirm: "password" });
    expect(
      s.browse(page({ table: { schema: "analytics", name: "page_views" } })).rows,
    ).toHaveLength(60);
    expect(finished(await run(s, "SELECT count() FROM page_views")).rowCount).toBe(1);
  });

  it("refuses every write before anything runs", async () => {
    const s = ro();
    for (const sql of [
      "INSERT INTO shop.customers (id) VALUES (99)",
      "SET max_threads = 1",
      "SELECT * FROM shop.orders SETTINGS readonly = 0",
      "SELECT * FROM url('http://x', CSV)",
      "frobnicate",
    ])
      expect(failure(await run(s, sql)), sql).toEqual(
        expect.objectContaining({ code: "INVALID_INPUT", message: READ_ONLY_MESSAGE }),
      );
    expect(thrown(() => s.preview!(changes([{ op: "delete", rowId: "r", key: key(1) }])))).toEqual({
      code: "UNSUPPORTED",
      message: "unsupported: this connection is read-only",
    });
    expect(
      thrown(() => s.apply!(changes([{ op: "delete", rowId: "r", key: key(1) }]))).message,
    ).toBe("unsupported: this connection is read-only");
    expect(
      thrown(() =>
        s.count!(orders, where([{ kind: "raw", sql: "id IN (SELECT 1 FROM url('x', CSV))" }])),
      ),
    ).toEqual({ code: "INVALID_INPUT", message: READ_ONLY_MESSAGE });
    const id = s.listProcesses!()[0]!.id;
    expect(s.killProcesses!([id], "query")).toEqual([
      { id, ok: false, message: READ_ONLY_MESSAGE },
    ]);
    expect(connect().count!(orders, null).count).toBe(240);
  });
});
