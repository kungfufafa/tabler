/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import type { ApplyRequest } from "../../../bindings/ApplyRequest";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { BrowseRequest } from "../../../bindings/BrowseRequest";
import type { ConnectionInfo } from "../../../bindings/ConnectionInfo";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { FilterGroup } from "../../../bindings/FilterGroup";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { Value } from "../../../bindings/Value";
import { mockProfiles } from "../../handlers";
import { mockInvoke } from "../../transport";
import { enableMockDrivers, resetMockBuild } from "..";
import type { MockSession } from "../types";
import lane, { bigqueryMock } from ".";
import { bigqueryCaps, parseProcessId, READ_ONLY_MESSAGE, resetProcesses } from "./session";

// The mock's BigQuery against the backend (drivers/bigquery/session.rs and its session tests):
// caps equal to the recorded ones (`session_tests::caps_match_the_recorded_fixture` writes the
// same file), projects and datasets, cursor pages (free plain pages, one job for a sorted or
// filtered page), counts and cells, the editor with its cost notices and cost guard, edits
// refused (EditRows is off), Running Queries, the dry run, the form's calls, and the refusals of
// a read-only profile.

const ROOT = path.resolve(import.meta.dirname, "../../../../../..");
const RECORDED = JSON.parse(
  readFileSync(path.join(ROOT, "src-tauri/tests/fixtures/bigquery/caps.json"), "utf8"),
) as Record<"editable" | "readOnly" | "noDefaultDataset", SessionCaps>;

const seed = (id: string): ConnectionProfile => {
  const p = bigqueryMock.seedProfiles().find((s) => s.id === id);
  if (!p) throw new Error(id);
  return p;
};

function connect(
  id = "p-mock-bigquery",
  options: Partial<ConnectionProfile["options"]> = {},
  params: Record<string, unknown> = {},
): MockSession {
  const p = seed(id);
  return bigqueryMock.connect({
    ...p,
    options: { ...p.options, ...options, driverParams: { ...p.options.driverParams, ...params } },
  });
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

const thrown = (f: () => unknown): { code: string; message: string; hint?: string } => {
  try {
    f();
  } catch (e) {
    return e as { code: string; message: string };
  }
  throw new Error("did not throw");
};

const orders = { schema: "shop", name: "orders" };
const events = { schema: "analytics", name: "events" };
const page = (over: Partial<BrowseRequest> = {}): BrowseRequest => ({
  table: orders,
  columns: null,
  filter: null,
  sort: [],
  limit: 100,
  offset: 0,
  ...over,
});
const where = (items: FilterGroup["items"]): FilterGroup => ({ conjunction: "and", items });
const int = (v: number): Value => ({ t: "int", v });
const text = (v: string): Value => ({ t: "text", v });

beforeEach(() => resetProcesses());

describe("the bigquery mock lane", () => {
  it("has one driver with a key-file, a read-only ADC and a token profile", () => {
    expect(lane.drivers).toEqual([bigqueryMock]);
    const [sa, ro, token] = bigqueryMock.seedProfiles();
    expect(sa!).toMatchObject({
      driver: "bigquery",
      database: "demo-proj",
      host: null,
      port: null,
    });
    expect(sa!.options.driverParams).toMatchObject({
      auth: "serviceAccount",
      defaultDataset: "shop",
    });
    expect(ro!.options).toMatchObject({ readOnly: true, tag: "production", passwordMode: "none" });
    expect(token!.options).toMatchObject({
      passwordMode: "command",
      passwordCommand: "gcloud auth print-access-token",
      driverParams: { auth: "accessToken", maximumBytesBilled: null },
    });
    expect(bigqueryMock.serverVersion(sa!)).toBe("BigQuery");
  });

  it("gives a session the caps the backend records", () => {
    expect(bigqueryCaps("shop")).toEqual(RECORDED.editable);
    expect(bigqueryCaps(null)).toEqual(RECORDED.noDefaultDataset);
    expect(connect().caps).toEqual(RECORDED.editable);
    expect(connect("p-mock-bigquery", { readOnly: true }).caps).toEqual(RECORDED.readOnly);
    expect(connect("p-mock-bigquery", {}, { defaultDataset: "" }).caps).toEqual(
      RECORDED.noDefaultDataset,
    );
    const { features } = RECORDED.editable;
    for (const f of ["editRows", "insertRows", "deleteRows", "explain", "switchDatabase", "format"])
      expect(features, f).not.toContain(f);
  });

  it("offers the backend's hooks and no edits", () => {
    const s = connect();
    for (const m of ["preview", "apply", "listUsers", "loadUser", "sessionDetails"] as const)
      expect(s[m], m).toBeUndefined();
    for (const m of [
      "tableDdl",
      "count",
      "fetchCell",
      "call",
      "listProcesses",
      "killProcesses",
      "listRoutines",
      "routineDefinition",
    ] as const)
      expect(s[m], m).toBeTypeOf("function");
  });

  it("checks the profile as `validate` and the sign-in do, in their words", () => {
    expect(thrown(() => connect("p-mock-bigquery", {}, { keyFilePath: "" }))).toEqual({
      code: "INVALID_INPUT",
      message: "BigQuery: choose the service account's key file (JSON).",
    });
    expect(thrown(() => connect("p-mock-bigquery", {}, { maximumBytesBilled: 0 })).message).toBe(
      "BigQuery: Maximum bytes billed must be more than 0 (or empty for no limit).",
    );
    expect(thrown(() => connect("p-mock-bigquery", {}, { defaultDataset: "a-b" })).message).toBe(
      "BigQuery: “a-b” is no dataset name (letters, digits and underscores).",
    );
    expect(thrown(() => connect("p-mock-bigquery", {}, { labels: { Team: "x" } })).message).toMatch(
      /^BigQuery: the label “Team” must use lower-case/,
    );
    expect(
      thrown(() => connect("p-mock-bigquery", {}, { apiEndpoint: "http://10.0.0.1" })).message,
    ).toMatch(/must be an https URL/);
    expect(
      thrown(() => connect("p-mock-bigquery-token", {}, { apiEndpoint: "https://bq.example/v2#x" }))
        .message,
    ).toMatch(/must be a base URL, without a query/);
    // An imported profile must not send the key file's or ADC's token anywhere but Google.
    expect(
      thrown(() => connect("p-mock-bigquery", {}, { apiEndpoint: "https://evil.example/v2" }))
        .message,
    ).toMatch(/must be a googleapis\.com host/);
    expect(
      thrown(() =>
        connect("p-mock-bigquery", {}, { auth: "applicationDefault", apiEndpoint: "https://e.x" }),
      ).code,
    ).toBe("INVALID_INPUT");
    expect(
      connect("p-mock-bigquery-token", {}, { apiEndpoint: "https://evil.example/v2" }),
    ).toBeTruthy();
    expect(
      thrown(() =>
        connect("p-mock-bigquery", {}, { keyFilePath: "\\\\attacker.example\\s\\k.json" }),
      ).message,
    ).toMatch(/must be a full path on this machine/);
    expect(thrown(() => connect("p-mock-bigquery-token", { passwordMode: "none" })).code).toBe(
      "AUTH_FAILED",
    );
  });

  it("lists projects as databases and datasets as schemas", () => {
    const s = connect();
    expect(s.listDatabases().map((d) => [d.name, d.isCurrent])).toEqual([
      ["demo-proj", true],
      ["bigquery-public-data", false],
      ["acme-analytics", false],
    ]);
    expect(s.listSchemas()).toEqual([
      { name: "analytics", isDefault: false },
      { name: "shop", isDefault: true },
    ]);
    expect(s.listTables("analytics").map((t) => [t.name, t.kind, t.rowEstimate])).toEqual([
      ["daily_revenue", "materializedView", null],
      ["events", "table", null],
      ["gcs_exports", "foreignTable", null],
    ]);
    expect(s.listTables("shop").map((t) => t.name)).toEqual([
      "customers",
      "orders",
      "v_big_spenders",
    ]);
    expect(thrown(() => s.listTables("nope")).code).toBe("NOT_FOUND");
  });

  it("describes a table as `tables.get` does, with stand-in indexes and Show DDL", () => {
    const s = connect();
    const meta = s.tableMeta(orders);
    expect(meta).toMatchObject({ kind: "table", primaryKey: ["order_id"], rowEstimate: 250 });
    expect(meta.columns.map((c) => [c.name, c.dataType, c.kind, c.nullable])).toEqual([
      ["order_id", "INT64", "bigInt", false],
      ["customer_id", "INT64", "bigInt", true],
      ["status", "STRING", "text", true],
      ["total", "NUMERIC(10, 2)", "decimal", true],
      ["created_at", "TIMESTAMP", "timestampTz", true],
      ["items", "ARRAY<STRUCT<sku STRING, qty INT64>>", "json", false],
    ]);
    expect(meta.columns[3]).toMatchObject({ numericPrecision: 10, numericScale: 2 });
    expect(meta.indexes.map((i) => i.name)).toEqual(["PRIMARY KEY", "partitioning", "clustering"]);
    expect(meta.foreignKeys[0]).toMatchObject({ refTable: { schema: "shop", name: "customers" } });
    expect(s.tableMeta({ schema: "shop", name: "v_big_spenders" }).rowEstimate).toBeNull();
    // An unqualified table resolves in the default dataset.
    expect(s.tableMeta({ schema: null, name: "customers" }).table).toEqual({
      schema: "shop",
      name: "customers",
    });
    expect(
      thrown(() =>
        connect("p-mock-bigquery", {}, { defaultDataset: "" }).tableMeta({
          schema: null,
          name: "x",
        }),
      ),
    ).toEqual({ code: "INVALID_INPUT", message: "BigQuery: choose a dataset first" });
    expect(s.tableDdl!(orders)).toMatch(/^CREATE TABLE `demo-proj.shop.orders`/);
    expect(s.tableDdl!(orders)).toContain("PRIMARY KEY (order_id) NOT ENFORCED");
    expect(s.listRoutines!("shop").map((r) => [r.name, r.kind])).toEqual([
      ["archive_orders", "procedure"],
      ["net_total", "function"],
    ]);
    expect(
      s.routineDefinition!({
        schema: "shop",
        name: "net_total",
        kind: "function",
        identityArguments: "",
      }),
    ).toMatch(/^CREATE FUNCTION/);
  });

  it("reads plain pages for free and follows their cursor", () => {
    const s = connect();
    const first = s.browse(page());
    expect(first.rows).toHaveLength(100);
    expect(first.sql).toBe(
      "SELECT * FROM `demo-proj`.`shop`.`orders` -- read with tabledata.list: no job, nothing billed",
    );
    expect(first.identity).toBe("pk");
    expect(first.rows[0]!.slice(0, 4)).toEqual([
      int(1),
      int(1),
      text("new"),
      { t: "decimal", v: "10.00" },
    ]);
    expect(first.rows[0]![4]).toEqual({ t: "timestampTz", v: "2024-01-01 00:00:00+00" });
    const second = s.browse(page({ cursor: first.nextCursor! }));
    expect(second.rows[0]![0]).toEqual(int(101));
    const third = s.browse(page({ cursor: second.nextCursor! }));
    expect(third.rows).toHaveLength(50);
    expect(third.nextCursor).toBeNull();
    // No offsets, and a cursor of another table is stale.
    expect(thrown(() => s.browse(page({ offset: 100 }))).message).toBe(
      "BigQuery pages follow one another: there is no offset to start from",
    );
    expect(
      thrown(() =>
        s.browse(page({ table: { schema: "shop", name: "customers" }, cursor: first.nextCursor! })),
      ).message,
    ).toBe(
      "BigQuery: this page no longer matches the table, filter or sort; reload the first page",
    );
  });

  it("reads a sorted or filtered page with one job whose pages follow, and values as decode.rs makes them", () => {
    const s = connect();
    const paid = where([{ kind: "condition", column: "status", op: "eq", values: [text("paid")] }]);
    const sorted = s.browse(
      page({ filter: paid, sort: [{ column: "order_id", dir: "desc", nulls: null }], limit: 50 }),
    );
    expect(sorted.sql).toMatch(
      /^SELECT `order_id`, .* FROM `shop`\.`orders` WHERE .* ORDER BY `order_id` DESC LIMIT 1000000$/,
    );
    expect(sorted.rows[0]![0]).toEqual(int(250));
    expect(sorted.rows.every((r) => r[2]!.t === "text" && r[2]!.v === "paid")).toBe(true);
    const next = s.browse(
      page({
        filter: paid,
        sort: [{ column: "order_id", dir: "desc", nulls: null }],
        limit: 50,
        cursor: sorted.nextCursor!,
      }),
    );
    expect(next.rows).toHaveLength(13);
    expect(next.nextCursor).toBeNull();
    // Another sort with the old cursor is stale.
    expect(thrown(() => s.browse(page({ cursor: sorted.nextCursor! }))).code).toBe("INVALID_INPUT");
    // A view is always read with a query.
    expect(s.browse(page({ table: { schema: "shop", name: "v_big_spenders" } })).sql).toMatch(
      /^SELECT /,
    );
    // The events table: GEOGRAPHY as WKT, BYTES, INT64 beyond 2^53 as text, JSON raw.
    const e = s.browse(page({ table: events, limit: 1 }));
    expect(e.identity).toBe("none");
    expect(e.rows[0]!.slice(4)).toEqual([
      { t: "json", v: '{"path":"/p/0","mobile":true}' },
      { t: "other", v: { ty: "geometry", text: "POINT(106.80 -6.2)" } },
      { t: "bytes", v: { base64: btoa("evt-0"), len: 5, truncated: false } },
      { t: "bigInt", v: "9007199254740993" },
    ]);
  });

  it("refuses a sorted page that would bill more than the cost guard, and a filter that runs more", () => {
    const s = connect();
    const e = thrown(() =>
      s.browse(page({ table: events, sort: [{ column: "event_ts", dir: "asc", nulls: null }] })),
    );
    expect(e).toMatchObject({
      code: "DB_ERROR",
      hint: "The query would bill more bytes than the connection's Maximum bytes billed allows.",
    });
    expect(e.message).toMatch(/^Query exceeded limit for bytes billed: 1073741824\./);
    // Without a guard it runs.
    const free = connect("p-mock-bigquery-token");
    expect(
      free.browse(page({ table: events, sort: [{ column: "event_ts", dir: "asc", nulls: null }] }))
        .rows,
    ).toHaveLength(100);
    for (const sql of [
      "1=1; DROP TABLE shop.orders",
      "status IN (SELECT x FROM EXTERNAL_QUERY('c', 'q'))",
    ])
      expect(thrown(() => s.browse(page({ filter: where([{ kind: "raw", sql }]) }))).message).toBe(
        "A filter can only read rows: this one would run another statement",
      );
  });

  it("counts, and reads a whole cell by its key", () => {
    const s = connect();
    expect(s.count!(orders, null)).toEqual({ count: 250, exact: true });
    const paid = where([{ kind: "condition", column: "status", op: "eq", values: [text("paid")] }]);
    expect(s.count!(orders, paid)).toEqual({ count: 63, exact: true });
    // CountMode::Costly, once the shared handler passes `exact`: numRows until asked, and a
    // filter or a view (which has no numRows) always counts.
    const costly = s.count as unknown as (t: unknown, f: unknown, exact: boolean) => unknown;
    expect(costly(orders, null, false)).toEqual({ count: 250, exact: false });
    expect(costly(orders, paid, false)).toEqual({ count: 63, exact: true });
    expect(costly({ schema: "shop", name: "v_big_spenders" }, null, false)).toMatchObject({
      exact: true,
    });
    expect(s.fetchCell!(orders, { kind: "pk", value: { order_id: int(2) } }, "items")).toEqual({
      t: "json",
      v: '[{"sku":"SKU-200","qty":2},{"sku":"SKU-300","qty":3}]',
    });
    expect(
      thrown(() => s.fetchCell!(orders, { kind: "pk", value: { order_id: int(999) } }, "status")),
    ).toEqual({
      code: "NOT_FOUND",
      message: "not found: the row no longer exists",
    });
  });

  it("runs the editor's statements with what each processed and billed", async () => {
    const s = connect();
    const read = await run(
      s,
      "SELECT order_id, total FROM orders WHERE status = 'paid' ORDER BY total DESC LIMIT 3",
    );
    const r = finished(read);
    expect(r).toMatchObject({ kind: "rows", rowCount: 3 });
    expect(r.columns.map((c) => [c.name, c.dataType])).toEqual([
      ["order_id", "INT64"],
      ["total", "NUMERIC(10, 2)"],
    ]);
    expect(r.notices).toEqual(["BigQuery processed 31.3 KB, billed 10.0 MB"]);
    // The same query again is the cache's: free.
    expect(
      finished(
        await run(
          s,
          "SELECT order_id, total FROM orders WHERE status = 'paid' ORDER BY total DESC LIMIT 3",
        ),
      ).notices,
    ).toEqual(["BigQuery processed 0 B, billed 0 B (cached result)"]);
    expect(rowsOf(await run(s, "SELECT COUNT(*) AS n FROM `demo-proj.shop.customers`"))).toEqual([
      [int(40)],
    ]);
    expect(rowsOf(await run(s, "SELECT 1 AS one, 'a' AS s"))).toEqual([[int(1), text("a")]]);
    // A statement that bills too much fails as its job would.
    const big = failure(await run(s, "SELECT * FROM analytics.events"));
    expect(big).toMatchObject({
      code: "DB_ERROR",
      vendorCode: 400,
      detail: "reason: bytesBilledLimitExceeded",
    });
    const missing = failure(await run(s, "SELECT * FROM shop.nope"));
    expect(missing.message).toBe(
      "Not found: Table demo-proj:shop.nope was not found in location US",
    );
    expect(failure(await run(s, "CREATE TEMP TABLE t AS SELECT 1")).message).toMatch(
      /^The mock BigQuery runs/,
    );
  });

  it("applies the editor's DML whole, and a failing statement changes nothing", async () => {
    const s = connect();
    const ins = finished(
      await run(
        s,
        "INSERT INTO shop.customers (customer_id, name, vip) VALUES (41, 'Ina', TRUE), (42, 'Joko', FALSE)",
      ),
    );
    expect(ins).toMatchObject({ kind: "affected", affected: 2 });
    expect(ins.notices[0]).toMatch(/^BigQuery processed /);
    expect(
      finished(await run(s, "UPDATE shop.customers SET country = 'ID' WHERE customer_id >= 41"))
        .affected,
    ).toBe(2);
    // One bad row: nothing of the statement is stored.
    const bad = failure(
      await run(s, "INSERT INTO shop.customers (customer_id, name) VALUES (43, 'x'), (NULL, 'y')"),
    );
    expect(bad.message).toBe("Required field customer_id cannot be null");
    expect(s.count!({ schema: "shop", name: "customers" }, null).count).toBe(42);
    expect(
      finished(await run(s, "DELETE FROM shop.customers WHERE customer_id = 42")).affected,
    ).toBe(1);
    expect(failure(await run(s, "DELETE FROM shop.customers")).message).toBe(
      "DELETE must have a WHERE clause",
    );
    expect(
      failure(await run(s, "DELETE FROM shop.v_big_spenders WHERE customer_id = 1")).message,
    ).toMatch(/is a view, which cannot be the target of a DML statement$/);
  });

  it("dry-runs any statement without running it, and knows no other call", () => {
    const s = connect();
    expect(s.call!("dryRun", { sql: "SELECT * FROM analytics.events" })).toEqual({
      bytesProcessed: "3435973837",
      columns: expect.arrayContaining([{ name: "geo", dataType: "GEOGRAPHY" }]),
    });
    const before = s.count!({ schema: "shop", name: "customers" }, null).count;
    expect(
      s.call!("dryRun", { sql: "DELETE FROM shop.customers WHERE customer_id = 1" }),
    ).toMatchObject({
      columns: [],
    });
    expect(s.count!({ schema: "shop", name: "customers" }, null).count).toBe(before);
    expect(thrown(() => s.call!("dryRun", {}))).toEqual({
      code: "INVALID_INPUT",
      message: "BigQuery: dryRun needs the sql",
    });
    // Estimate Full Scan…: the table by name, the statement built on the backend's side.
    expect(s.call!("dryRun", { table: { schema: "analytics", name: "events" } })).toEqual(
      s.call!("dryRun", { sql: "SELECT * FROM analytics.events" }),
    );
    expect(thrown(() => s.call!("dryRun", { table: { name: "events" } }))).toEqual({
      code: "INVALID_INPUT",
      message: "BigQuery: dryRun needs the table's dataset and name",
    });
    expect(thrown(() => s.call!("dropDataset", {}))).toEqual({
      code: "UNSUPPORTED",
      message: "unsupported: BigQuery: unknown call “dropDataset”",
    });
  });

  it("lists running jobs and cancels them by their full id", () => {
    const s = connect();
    const jobs = s.listProcesses!();
    expect(jobs.map((j) => [j.state, j.own])).toEqual([
      ["RUNNING", false],
      ["PENDING", false],
    ]);
    expect(parseProcessId("example.com:proj:EU.job_1")).toEqual({
      project: "example.com:proj",
      location: "EU",
      job: "job_1",
    });
    expect(s.killProcesses!([jobs[0]!.id, "nonsense"], "query")).toEqual([
      { id: jobs[0]!.id, ok: true, message: null },
      { id: "nonsense", ok: false, message: "“nonsense” is not a BigQuery job" },
    ]);
    expect(s.listProcesses!()).toHaveLength(1);
  });

  it("refuses every write of a read-only profile before it runs, and still reads", async () => {
    const s = connect("p-mock-bigquery-ro");
    const refused = failure(await run(s, "DELETE FROM shop.orders WHERE order_id = 1"));
    expect(refused).toMatchObject({ code: "INVALID_INPUT", message: READ_ONLY_MESSAGE });
    for (const sql of [
      "EXECUTE IMMEDIATE 'DELETE FROM shop.orders WHERE true'",
      "CALL shop.archive_orders('2024-01-01')",
      "SELECT * FROM EXTERNAL_QUERY('c', 'SELECT 1')",
      "BEGIN SELECT 1; END",
    ])
      expect(failure(await run(s, sql)).message, sql).toBe(READ_ONLY_MESSAGE);
    expect(s.count!({ schema: "shop", name: "orders" }, null).count).toBe(250);
    expect(finished(await run(s, "SELECT COUNT(*) FROM shop.orders")).kind).toBe("rows");
    expect(s.killProcesses!([s.listProcesses!()[0]!.id], "query")[0]).toMatchObject({
      ok: false,
      message: READ_ONLY_MESSAGE,
    });
    // A dry run runs nothing: a read-only session may estimate.
    expect(s.call!("dryRun", { sql: "SELECT * FROM shop.orders" })).toHaveProperty(
      "bytesProcessed",
    );
  });
});

describe("the bigquery mock through the app's commands", () => {
  beforeAll(async () => {
    await import("../..");
  });

  let added: string[] = [];
  beforeEach(async () => {
    added = (await enableMockDrivers(["bigquery"])).map((p) => p.id);
  });
  afterEach(() => {
    resetMockBuild();
    for (const id of added) mockProfiles.delete(id);
  });

  const open = (profileId: string) => mockInvoke<ConnectionInfo>("connect", { profileId });

  it("connects the seed profiles with the recorded caps", async () => {
    expect(added).toEqual(["p-mock-bigquery", "p-mock-bigquery-ro", "p-mock-bigquery-token"]);
    const c = await open("p-mock-bigquery");
    expect(c).toMatchObject({ serverVersion: "BigQuery", currentDatabase: "demo-proj" });
    expect(c.capabilities).toEqual(RECORDED.editable);
    const ro = await open("p-mock-bigquery-ro");
    expect(ro.readOnly).toBe(true);
  });

  it("browses by cursor, refuses edits and serves the dry run and the form's calls", async () => {
    const { connectionId } = await open("p-mock-bigquery");
    const first = await mockInvoke<BrowsePage>("browse_table", { connectionId, req: page() });
    expect(first.nextCursor).toBeTruthy();
    const req: ApplyRequest = {
      table: orders,
      changes: [{ op: "delete", rowId: 0, key: { kind: "pk", value: { order_id: int(1) } } }],
      returning: false,
    } as unknown as ApplyRequest;
    await expect(mockInvoke("preview_changes", { connectionId, req })).rejects.toMatchObject({
      code: "UNSUPPORTED",
    });
    await expect(mockInvoke("apply_changes", { connectionId, req })).rejects.toMatchObject({
      code: "UNSUPPORTED",
    });
    expect(
      await mockInvoke("session_call", {
        connectionId,
        method: "dryRun",
        args: { sql: "SELECT order_id FROM shop.orders" },
      }),
    ).toMatchObject({ bytesProcessed: "10667" });
    const input = { profile: seed("p-mock-bigquery"), password: null };
    expect(await mockInvoke("driver_util", { input, method: "listDatasets", args: null })).toEqual([
      { name: "analytics", location: "US" },
      { name: "shop", location: "US" },
    ]);
    expect(await mockInvoke("driver_util", { input, method: "listProjects", args: null })).toEqual([
      { id: "demo-proj", name: "Demo Project" },
      { id: "acme-analytics", name: "Acme Analytics" },
    ]);
    await expect(
      mockInvoke("driver_util", { input, method: "deleteProject", args: null }),
    ).rejects.toMatchObject({ code: "UNSUPPORTED" });
  });
});
