import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { ConnectionInfo } from "../../../bindings/ConnectionInfo";
import type { DatabaseInfo } from "../../../bindings/DatabaseInfo";
import type { KillOutcome } from "../../../bindings/KillOutcome";
import type { ProcessRow } from "../../../bindings/ProcessRow";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { RowCount } from "../../../bindings/RowCount";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { TableSummary } from "../../../bindings/TableSummary";
import type { Value } from "../../../bindings/Value";
import { mockProfiles } from "../../handlers";
import { mockInvoke } from "../../transport";
import { enableMockDrivers, resetMockBuild } from "..";
import lane from ".";
import { CONSOLE_READ_ONLY } from "./console";
import { esCaps, FILTER_OPS, readOnlyMessage, resetClusters } from "./session";

// The mock's Elasticsearch against the backend (drivers/elasticsearch/session.rs and its session
// tests): caps, catalog, cursor pages, counts, fetch, a guarded bulk commit with one failing
// change, the console with SQL, the calls, the cluster's tasks, and the refusals of a read-only
// session.

beforeAll(async () => {
  await import("../..");
});

let added: string[] = [];

beforeEach(async () => {
  resetClusters();
  added = (await enableMockDrivers(["elasticsearch"])).map((p) => p.id);
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
  const columns = events.flatMap((e) =>
    e.event === "columns" ? [e.data.columns.map((c) => c.name)] : [],
  );
  return { results, rows, columns };
}

const logs = { schema: null, name: "logs-2026.10" };
const page = (connectionId: string, req: Record<string, unknown> = {}) =>
  mockInvoke<BrowsePage>("browse_table", {
    connectionId,
    req: { table: logs, columns: null, filter: null, sort: [], limit: 50, offset: 0, ...req },
  });

const text = (v: string): Value => ({ t: "text", v });
const int = (v: number): Value => ({ t: "int", v });
const key = (id: string, seq: number, term = 1) => ({
  kind: "pk",
  value: { _id: text(id), _seq_no: int(seq), _primary_term: int(term) },
});

const apply = (connectionId: string, changes: unknown[]) =>
  mockInvoke<ApplyResult>("apply_changes", {
    connectionId,
    req: { table: logs, changes, returning: true },
  });

describe("the mock Elasticsearch driver", () => {
  it("is the lane's one driver, with three seed profiles", () => {
    expect(lane.drivers.map((d) => d.driver)).toEqual(["elasticsearch"]);
    expect(added).toEqual(["p-mock-elasticsearch", "p-mock-opensearch", "p-mock-elasticsearch-ro"]);
  });

  it("connects with the backend's caps and versions", async () => {
    const es = await connect("p-mock-elasticsearch");
    expect(es.serverVersion).toBe("Elasticsearch 9.1.5");
    const all = { seqNoGuards: true, tasks: true, caseInsensitive: true };
    expect(es.capabilities).toEqual(esCaps(all));
    expect(es.capabilities?.features).toEqual([
      "editor",
      "cancelOnServer",
      "editRows",
      "insertRows",
      "deleteRows",
      "returning",
      "exactCount",
      "structureView",
      "viewDefinition",
      "processList",
      "killProcess",
    ]);
    expect(es.capabilities?.data).toMatchObject({
      paging: "cursor",
      count: "costly",
      rawFilter: "queryString",
      transactions: "batch",
      affected: "exact",
    });
    expect(es.capabilities?.data.filterOps).toEqual(FILTER_OPS);
    expect(es.capabilities?.safety).toEqual({
      readOnly: "classifierOnly",
      hardened: false,
      confirm: "passcode",
    });
    // Without `if_seq_no` guards or the task API, those features go.
    const old = esCaps({ seqNoGuards: false, tasks: false, caseInsensitive: false });
    expect(old.features).not.toContain("editRows");
    expect(old.features).not.toContain("processList");
    expect(old.data.filterOps).not.toContain("ilike");
    const os = await connect("p-mock-opensearch");
    expect(os.serverVersion).toBe("OpenSearch 2.19.1");
  });

  it("lists the cluster, its indices, aliases and data streams", async () => {
    const { connectionId } = await connect("p-mock-elasticsearch");
    const dbs = await mockInvoke<DatabaseInfo[]>("list_databases", { connectionId });
    expect(dbs.map((d) => d.name)).toEqual(["docker-cluster"]);
    const tables = await mockInvoke<TableSummary[]>("list_tables", { connectionId, schema: "" });
    expect(tables.map((t) => [t.name, t.kind])).toEqual([
      ["logs-2026.10", "table"],
      ["logs-2026.09", "table"],
      ["products", "table"],
      ["logs", "view"],
      ["metrics-app", "partitioned"],
    ]);
    expect(tables.find((t) => t.name === "logs")?.comment).toBe(
      "alias of logs-2026.10, logs-2026.09",
    );
  });

  it("describes an index by its flattened mapping, the hit's metadata as the key", async () => {
    const { connectionId } = await connect("p-mock-elasticsearch");
    const meta = await mockInvoke<TableMeta>("get_table", { connectionId, table: logs });
    expect(meta.primaryKey).toEqual(["_id", "_seq_no", "_primary_term"]);
    expect(meta.columns.map((c) => c.name)).toContain("user.name");
    expect(meta.columns.find((c) => c.name === "message")?.extra).toBe("not sortable");
    const alias = await mockInvoke<TableMeta>("get_table", {
      connectionId,
      table: { schema: null, name: "logs" },
    });
    expect(alias.primaryKey).toEqual(["_index", "_id", "_seq_no", "_primary_term"]);
    expect(alias.kind).toBe("view");
  });

  it("pages by cursor, values typed as the backend's, missing fields missing", async () => {
    const { connectionId } = await connect("p-mock-elasticsearch");
    const first = await page(connectionId);
    expect(first.rows).toHaveLength(50);
    expect(first.nextCursor).toBe("pos:50");
    expect(first.sql).toMatch(
      /^POST \/logs-2026\.10\/_search\?preference=tabler-mock\d+\n\{"from":0,"size":50/,
    );
    const col = (name: string) => first.columns.findIndex((c) => c.name === name);
    const row = first.rows[0]!;
    expect(row[col("_id")]).toEqual(text("log-0001"));
    expect(row[col("@timestamp")]).toEqual({ t: "timestampTz", v: "2026-10-01T08:00:00.000Z" });
    expect(row[col("user.id")]).toEqual(int(1000));
    expect(row[col("tags")]).toEqual({ t: "json", v: '["web"]' });
    expect(first.rows[6]![col("tags")]).toEqual({ t: "other", v: { ty: "missing", text: "" } });
    const third = await page(connectionId, { cursor: "pos:100" });
    expect(third.rows).toHaveLength(20);
    expect(third.nextCursor).toBeNull();
    // Big integers travel as BigInt.
    const products = await page(connectionId, { table: { schema: null, name: "products" } });
    const barcode = products.columns.findIndex((c) => c.name === "barcode");
    expect(products.rows[0]![barcode]).toEqual({ t: "bigInt", v: "9007199254740993" });
  });

  it("filters, sorts by sortable fields only, and counts at a cost", async () => {
    const { connectionId } = await connect("p-mock-elasticsearch");
    const filter = {
      conjunction: "and",
      items: [{ kind: "condition", column: "level", op: "eq", values: [text("error")] }],
    };
    const errors = await page(connectionId, { filter });
    expect(errors.rows).toHaveLength(30);
    expect(errors.sql).toContain('"term":{"level":"error"}');
    const counted = await mockInvoke<RowCount>("count_table_rows", {
      connectionId,
      table: logs,
      filter,
    });
    expect(counted).toEqual({ count: 30, exact: true });
    expect(
      await mockInvoke<RowCount>("count_table_rows", { connectionId, table: logs, filter: null }),
    ).toEqual({ count: 120, exact: false });
    const sorted = await page(connectionId, {
      sort: [{ column: "status", dir: "desc", nulls: null }],
    });
    expect(sorted.rows[0]![sorted.columns.findIndex((c) => c.name === "status")]).toEqual(int(500));
    await expect(
      page(connectionId, { sort: [{ column: "message", dir: "asc", nulls: null }] }),
    ).rejects.toMatchObject({ message: expect.stringMatching(/cannot be sorted/) });
    // A raw filter: a Lucene query string, or DSL that may not call a model.
    const raw = (sql: string) => ({ conjunction: "and", items: [{ kind: "raw", sql }] });
    expect((await page(connectionId, { filter: raw("user.name:ada") })).rows).toHaveLength(24);
    await expect(
      page(connectionId, { filter: raw('{"semantic":{"field":"f","query":"q"}}') }),
    ).rejects.toMatchObject({ message: expect.stringMatching(/calls a model/) });
  });

  it("fetches a cell by the row's _id", async () => {
    const { connectionId } = await connect("p-mock-elasticsearch");
    const cell = await mockInvoke<Value>("fetch_cell", {
      connectionId,
      table: logs,
      key: key("log-0002", 1),
      column: "message",
    });
    expect(cell).toEqual(text("request 2 served"));
    await expect(
      mockInvoke("fetch_cell", {
        connectionId,
        table: logs,
        key: key("nope", 1),
        column: "message",
      }),
    ).rejects.toMatchObject({ code: "NOT_FOUND" });
  });

  it("previews one guarded _bulk action per change, and applies each on its own", async () => {
    const { connectionId } = await connect("p-mock-elasticsearch");
    const changes = [
      {
        op: "update",
        rowId: "a",
        key: key("log-0001", 0),
        changes: { "user.name": text("grace"), status: text("201") },
      },
      // `abc` in a long field: the cluster refuses this one change.
      { op: "update", rowId: "b", key: key("log-0002", 1), changes: { status: text("abc") } },
      // Read before someone else changed it: a version conflict.
      { op: "delete", rowId: "c", key: key("log-0003", 99) },
      {
        op: "insert",
        rowId: "d",
        values: { _id: text("new-1"), level: text("info"), status: text("200") },
      },
    ];
    const preview = await mockInvoke<string[]>("preview_changes", {
      connectionId,
      req: { table: logs, changes, returning: true },
    });
    expect(preview[0]).toBe(
      'POST /_bulk?refresh=wait_for\n{"update":{"_index":"logs-2026.10","_id":"log-0001","if_seq_no":0,"if_primary_term":1}}\n{"doc":{"user":{"name":"grace"},"status":201}}',
    );
    expect(preview[3]).toBe(
      'POST /_bulk?refresh=wait_for\n{"create":{"_index":"logs-2026.10","_id":"new-1"}}\n{"level":"info","status":200}',
    );
    const r = await apply(connectionId, changes);
    expect(r.success).toBe(false);
    expect(r.sql).toEqual(preview);
    expect(r.results.map((x) => [x.rowId, x.affected, x.error?.detail ?? null])).toEqual([
      ["a", 1, null],
      ["b", 0, "document_parsing_exception"],
      ["c", 0, "version_conflict_engine_exception"],
      ["d", 1, null],
    ]);
    // The applied ones return their new guard, which the next edit sends back.
    expect(r.results[0]!.returned).toMatchObject({ _id: text("log-0001"), _seq_no: int(120) });
    const again = await apply(connectionId, [changes[0]]);
    expect(again.results[0]!.error?.detail).toBe("version_conflict_engine_exception");
    const fetched = await mockInvoke<Value>("fetch_cell", {
      connectionId,
      table: logs,
      key: key("log-0001", 120),
      column: "user.name",
    });
    expect(fetched).toEqual(text("grace"));
    // Metadata and removed fields are refused before anything is sent.
    await expect(
      mockInvoke("preview_changes", {
        connectionId,
        req: {
          table: logs,
          changes: [
            { op: "update", rowId: "x", key: key("log-0004", 3), changes: { _id: text("y") } },
          ],
          returning: false,
        },
      }),
    ).rejects.toMatchObject({ message: expect.stringMatching(/metadata cannot be edited/) });
  });

  it("runs console requests and SQL as grids", async () => {
    const { connectionId } = await connect("p-mock-elasticsearch");
    const cat = await run(connectionId, "GET _cat/indices?v");
    expect(cat.columns[0]).toEqual([
      "health",
      "status",
      "index",
      "pri",
      "rep",
      "docs.count",
      "store.size",
    ]);
    const search = await run(connectionId, "GET /logs/_search?q=level:error&size=5");
    expect(search.columns[0]!.slice(0, 3)).toEqual(["_index", "_id", "_score"]);
    expect(search.rows).toHaveLength(5);
    expect(search.results[0]!.notices).toEqual(["37 hits", "showing the first 5 (size)"]);
    const sql = await run(connectionId, "SELECT level, status FROM logs-2026.10 LIMIT 3");
    expect(sql.columns[0]).toEqual(["level", "status"]);
    expect(sql.rows).toEqual([
      [text("info"), int(200)],
      [text("warn"), int(200)],
      [text("error"), int(201)],
    ]);
    const count = await run(connectionId, "GET /products/_count");
    expect(String(count.rows[0]![0]!.t === "json" && count.rows[0]![0]!.v)).toContain('"count": 8');
    const bad = await run(connectionId, "FETCH /logs");
    expect(bad.results[0]!.error?.code).toBe("INVALID_INPUT");
  });

  it("answers the session's calls and the form's", async () => {
    const { connectionId } = await connect("p-mock-elasticsearch");
    const settings = await mockInvoke<string>("session_call", {
      connectionId,
      method: "indexSettings",
      args: { index: "products" },
    });
    expect(settings).toContain('"max_result_window": "10000"');
    const action = await mockInvoke<{ request: string }>("session_call", {
      connectionId,
      method: "indexAction",
      args: { index: "products", action: "clear_cache" },
    });
    expect(action.request).toBe("POST /products/_cache/clear");
    await expect(
      mockInvoke("session_call", { connectionId, method: "truncate", args: { index: "logs-*" } }),
    ).rejects.toMatchObject({ message: expect.stringMatching(/names several indices/) });
    await expect(
      mockInvoke("session_call", { connectionId, method: "frobnicate", args: {} }),
    ).rejects.toMatchObject({ code: "UNSUPPORTED" });
    const p = mockProfiles.get("p-mock-elasticsearch")!;
    const util = (method: string, args: unknown) =>
      mockInvoke("driver_util", { input: { profile: p }, method, args });
    expect(await util("cloudIdUrl", { cloudId: "x:" + btoa("cloud.es.io$abc") })).toEqual({
      url: "https://abc.cloud.es.io:443",
    });
    expect(await util("awsEndpoint", { host: "search-x.eu-west-1.es.amazonaws.com" })).toEqual({
      region: "eu-west-1",
      service: "es",
    });
  });

  it("lists and cancels the cluster's tasks", async () => {
    const { connectionId } = await connect("p-mock-elasticsearch");
    const tasks = await mockInvoke<ProcessRow[]>("admin_list_processes", { connectionId });
    expect(tasks.map((t) => [t.command, t.current])).toEqual([
      ["indices:data/write/reindex", false],
      ["indices:data/read/search", false],
      ["cluster:monitor/tasks/lists", true],
    ]);
    const out = await mockInvoke<KillOutcome[]>("admin_kill_processes", {
      connectionId,
      ids: [tasks[0]!.id, "bad id"],
      scope: "query",
    });
    expect(out.map((o) => o.ok)).toEqual([true, false]);
  });

  it("refuses every write on a read-only connection", async () => {
    const { connectionId } = await connect("p-mock-elasticsearch-ro");
    // Reads still work.
    expect((await page(connectionId)).rows).toHaveLength(50);
    expect((await run(connectionId, "GET /logs/_search")).results[0]!.error).toBeNull();
    // The console refuses what the classifier does not call a read, before anything runs.
    for (const cmd of ["DELETE /logs-2026.10", "POST /logs/_refresh", "GET /logs/_flush"]) {
      const r = await run(connectionId, cmd);
      expect(r.results[0]!.error?.message, cmd).toBe(CONSOLE_READ_ONLY);
    }
    await expect(
      mockInvoke("preview_changes", {
        connectionId,
        req: {
          table: logs,
          changes: [{ op: "delete", rowId: "x", key: key("log-0001", 0) }],
          returning: false,
        },
      }),
    ).rejects.toMatchObject({ message: readOnlyMessage("editing rows") });
    const r = await apply(connectionId, [{ op: "delete", rowId: "x", key: key("log-0001", 0) }]);
    expect(r.results[0]!.error?.message).toBe(readOnlyMessage("editing rows"));
    await expect(
      mockInvoke("session_call", {
        connectionId,
        method: "deleteIndex",
        args: { index: "products" },
      }),
    ).rejects.toMatchObject({ message: readOnlyMessage("deleteIndex") });
    expect(
      await mockInvoke("session_call", { connectionId, method: "clusterHealth", args: null }),
    ).toContain('"status": "green"');
    await expect(
      mockInvoke("admin_kill_processes", { connectionId, ids: ["n:1"], scope: "query" }),
    ).rejects.toMatchObject({ code: "UNSUPPORTED" });
  });
});
