import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { ConnectionInfo } from "../../../bindings/ConnectionInfo";
import type { DatabaseInfo } from "../../../bindings/DatabaseInfo";
import type { KillOutcome } from "../../../bindings/KillOutcome";
import type { ProcessRow } from "../../../bindings/ProcessRow";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { RowCount } from "../../../bindings/RowCount";
import type { SchemaInfo } from "../../../bindings/SchemaInfo";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { TableSummary } from "../../../bindings/TableSummary";
import type { Value } from "../../../bindings/Value";
import { mockProfiles } from "../../handlers";
import { mockInvoke } from "../../transport";
import { enableMockDrivers, resetMockBuild } from "..";
import lane from ".";
import { databases, oidOf, resetDatabases } from "./data";
import { FILTER_OPS, mongoCaps, READ_ONLY_MESSAGE } from "./session";

// The mock's MongoDB against the backend (drivers/mongodb/session.rs, calls.rs and their
// session tests): caps, catalog, offset pages, counts, a commit with a failing change (in order
// on a standalone server, all or nothing on a replica set), the console, the calls, Running
// Queries, and the refusals of a read-only session.

beforeAll(async () => {
  await import("../..");
});

let added: string[] = [];

beforeEach(async () => {
  resetDatabases();
  added = (await enableMockDrivers(["mongodb"])).map((p) => p.id);
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

const orders = { schema: "shop", name: "orders" };
const page = (connectionId: string, req: Record<string, unknown> = {}) =>
  mockInvoke<BrowsePage>("browse_table", {
    connectionId,
    req: { table: orders, columns: null, filter: null, sort: [], limit: 25, offset: 0, ...req },
  });

const oid = (n: number): Value => ({ t: "other", v: { ty: "objectId", text: oidOf("6730", n) } });
const pk = (n: number) => ({ kind: "pk", value: { _id: oid(n) } });

/** The totals of the seeded orders. */
const totals = () =>
  databases
    .get("shop")!
    .get("orders")!
    .docs.map((d) => (d.total?.t === "int" || d.total?.t === "float" ? d.total.v : 0));

const apply = (connectionId: string, changes: unknown[]) =>
  mockInvoke<ApplyResult>("apply_changes", {
    connectionId,
    req: { table: orders, changes, returning: false },
  });

const failing = (n: number) => ({
  op: "update",
  rowId: "bad",
  key: pk(n),
  changes: { total: { t: "float", v: -5 } },
});

describe("the mock MongoDB driver", () => {
  it("is the lane's one driver, with three seed profiles", () => {
    expect(lane.drivers.map((d) => d.driver)).toEqual(["mongodb"]);
    expect(added).toEqual(["p-mock-mongodb", "p-mock-mongodb-rs", "p-mock-mongodb-ro"]);
  });

  it("connects with the backend's caps: sequential standalone, atomic replica set", async () => {
    const one = await connect("p-mock-mongodb");
    expect(one.serverVersion).toBe("MongoDB 8.0.4");
    expect(one.capabilities).toEqual(mongoCaps(false, "shop", "password"));
    expect(one.capabilities?.data.transactions).toBe("sequential");
    expect(one.capabilities?.data.filterOps).toEqual(FILTER_OPS);
    expect(FILTER_OPS).toHaveLength(23);
    const rs = await connect("p-mock-mongodb-rs");
    expect(rs.serverVersion).toBe("MongoDB 8.0.4 (replica set rs0)");
    expect(rs.capabilities?.data.transactions).toBe("atomic");
    expect(rs.capabilities?.namespaces.defaultSchema).toBeNull();
  });

  it("lists databases, the session's database as its schema, and collections", async () => {
    const { connectionId } = await connect("p-mock-mongodb");
    const dbs = await mockInvoke<DatabaseInfo[]>("list_databases", { connectionId });
    expect(dbs.map((d) => [d.name, d.isCurrent])).toEqual([
      ["admin", false],
      ["analytics", false],
      ["config", false],
      ["local", false],
      ["shop", true],
    ]);
    expect(await mockInvoke<SchemaInfo[]>("list_schemas", { connectionId })).toEqual([
      { name: "shop", isDefault: true },
    ]);
    const colls = await mockInvoke<TableSummary[]>("list_tables", { connectionId, schema: "shop" });
    expect(colls.map((c) => [c.name, c.kind, c.comment])).toEqual([
      ["customers", "table", null],
      ["orders", "table", null],
      ["paid_orders", "view", "view on orders"],
      ["products", "table", null],
      ["system.views", "systemView", null],
    ]);
    // Without a database every one but the server's own.
    const rs = await connect("p-mock-mongodb-rs");
    const schemas = await mockInvoke<SchemaInfo[]>("list_schemas", {
      connectionId: rs.connectionId,
    });
    expect(schemas.map((s) => s.name)).toEqual(["analytics", "shop"]);
  });

  it("samples a collection's columns, _id first as the key, with mixed and missing fields", async () => {
    const { connectionId } = await connect("p-mock-mongodb");
    const meta = await mockInvoke<TableMeta>("get_table", { connectionId, table: orders });
    expect(meta.primaryKey).toEqual(["_id"]);
    const col = (n: string) => meta.columns.find((c) => c.name === n)!;
    expect(meta.columns[0]).toMatchObject({ name: "_id", dataType: "objectId", isPk: true });
    expect(col("total")).toMatchObject({ dataType: "double|int", kind: "float", nullable: false });
    expect(col("status")).toMatchObject({ dataType: "string", nullable: true });
    expect(col("trace")).toMatchObject({ dataType: "long", kind: "bigInt" });
    expect(meta.checks[0]?.name).toBe("validator");
    expect(meta.ddl).toContain('db.createCollection("orders", {"validator"');
    const view = await mockInvoke<TableMeta>("get_table", {
      connectionId,
      table: { schema: "shop", name: "paid_orders" },
    });
    expect(view.primaryKey).toEqual([]);
  });

  it("browses by offset, sorted with _id breaking ties, filtered, counted and fetched", async () => {
    const { connectionId } = await connect("p-mock-mongodb");
    const first = await page(connectionId);
    expect(first.rows).toHaveLength(25);
    expect(first.nextCursor).toBeNull();
    expect(first.identity).toBe("pk");
    expect(first.sql).toBe('db.getSiblingDB("shop").runCommand({"find": "orders", "limit": 25})');
    const third = await page(connectionId, { offset: 50 });
    expect(third.rows).toHaveLength(10);
    // A missing field is `Other{missing}`, not NULL.
    const status = first.columns.findIndex((c) => c.name === "status");
    expect(first.rows[6]![status]).toEqual({ t: "other", v: { ty: "missing", text: "" } });
    // Values keep their types: 2^53 + 1 as BigInt text.
    const trace = first.columns.findIndex((c) => c.name === "trace");
    expect(first.rows[0]![trace]).toEqual({ t: "bigInt", v: "9007199254740993" });
    const sorted = await page(connectionId, {
      sort: [{ column: "total", dir: "desc", nulls: null }],
    });
    const total = sorted.columns.findIndex((c) => c.name === "total");
    expect((sorted.rows[0]![total] as { v: number }).v).toBe(Math.max(...totals()));
    expect(sorted.sql).toContain('"sort": {"total": -1, "_id": 1}');
    const filter = {
      conjunction: "and",
      items: [
        { kind: "condition", column: "status", op: "eq", values: [{ t: "text", v: "paid" }] },
      ],
    };
    const paid = await page(connectionId, { filter });
    expect(paid.rows.length).toBeGreaterThan(0);
    const raw = { conjunction: "and", items: [{ kind: "raw", sql: "{total: {$gte: 500}}" }] };
    expect((await page(connectionId, { filter: raw, limit: 100 })).rows.length).toBe(
      totals().filter((t) => t >= 500).length,
    );
    // Unfiltered: the collection's own count; filtered: the documents counted.
    expect(
      await mockInvoke<RowCount>("count_table_rows", { connectionId, table: orders, filter: null }),
    ).toEqual({ count: 60, exact: false });
    const counted = await mockInvoke<RowCount>("count_table_rows", {
      connectionId,
      table: orders,
      filter,
    });
    expect(counted.exact).toBe(true);
    expect(counted.count).toBe(paid.rows.length);
    const items = await mockInvoke<Value>("fetch_cell", {
      connectionId,
      table: orders,
      key: pk(1),
      column: "items",
    });
    expect(items.t).toBe("json");
  });

  it("applies changes one after the other on a standalone server, stopping at a failure", async () => {
    const { connectionId } = await connect("p-mock-mongodb");
    const changes = [
      { op: "update", rowId: "r1", key: pk(1), changes: { status: { t: "text", v: "shipped" } } },
      failing(2),
      { op: "delete", rowId: "r3", key: pk(3) },
    ];
    const preview = await mockInvoke<string[]>("preview_changes", {
      connectionId,
      req: { table: orders, changes, returning: false },
    });
    expect(preview[0]).toBe(
      `db.runCommand({"update": "orders", "updates": [{"q": {"_id": {"$oid": "${oidOf("6730", 1)}"}}, "u": {"$set": {"status": "shipped"}}, "multi": false}], "ordered": true})`,
    );
    const result = await apply(connectionId, changes);
    expect(result.success).toBe(false);
    expect(result.error).toMatchObject({ message: "Document failed validation", vendorCode: 121 });
    expect(result.results.map((r) => [r.rowId, r.affected, r.error?.message ?? null])).toEqual([
      ["r1", 1, null],
      ["bad", 0, "Document failed validation"],
      ["r3", 0, "not applied: an earlier change failed"],
    ]);
    const docs = databases.get("shop")!.get("orders")!.docs;
    expect(docs[0]!.status).toEqual({ t: "text", v: "shipped" });
    expect(docs).toHaveLength(60);
  });

  it("applies all or nothing on a replica set", async () => {
    const { connectionId } = await connect("p-mock-mongodb-rs");
    const before = databases.get("shop")!.get("orders")!.docs[0]!.status;
    const result = await apply(connectionId, [
      { op: "update", rowId: "r1", key: pk(1), changes: { status: { t: "text", v: "x" } } },
      failing(2),
    ]);
    expect(result.success).toBe(false);
    expect(result.results.map((r) => [r.affected, r.error?.message ?? null])).toEqual([
      [0, null],
      [0, "Document failed validation"],
    ]);
    expect(databases.get("shop")!.get("orders")!.docs[0]!.status).toEqual(before);
    const ok = await apply(connectionId, [
      { op: "insert", rowId: "n1", values: { total: { t: "int", v: 3 } }, exprs: { at: "now" } },
      {
        op: "update",
        rowId: "r2",
        key: pk(2),
        changes: { note: { t: "other", v: { ty: "missing", text: "" } } },
      },
    ]);
    expect(ok.success).toBe(true);
    const docs = databases.get("shop")!.get("orders")!.docs;
    expect(docs).toHaveLength(61);
    expect(docs[60]!.at?.t).toBe("timestampTz");
  });

  it("runs the console, streaming rows and counting writes", async () => {
    const { connectionId } = await connect("p-mock-mongodb");
    const found = await run(
      connectionId,
      "db.orders.find({status: 'paid'}).sort({total: -1}).limit(3)",
    );
    expect(found.results[0]?.kind).toBe("rows");
    expect(found.rows).toHaveLength(3);
    const counted = await run(connectionId, "db.orders.countDocuments({total: {$gte: 500}})");
    expect(counted.rows).toEqual([[{ t: "int", v: totals().filter((t) => t >= 500).length }]]);
    const updated = await run(
      connectionId,
      "db.orders.updateMany({status: 'new'}, {$set: {status: 'open'}})",
    );
    expect(updated.results[0]).toMatchObject({ kind: "affected", affected: 13 });
    expect(updated.results[0]?.notices).toEqual(["matched 13, modified 13, upserted 0"]);
    const bad = await run(connectionId, "db.orders.mapReduce('f', 'g')");
    expect(bad.results[0]?.error?.message).toContain("not supported by the mock");
    const used = await run(connectionId, "use analytics\ndb.events.estimatedDocumentCount()");
    expect(used.results[0]?.notices).toEqual(["switched to db analytics"]);
    expect(used.rows).toEqual([[{ t: "int", v: 250 }]]);
  });

  it("answers the calls, previews a write's command, and refuses unknown ones", async () => {
    const { connectionId } = await connect("p-mock-mongodb");
    const at = { database: "shop", collection: "orders" };
    const stats = await mockInvoke<{ count: number }>("session_call", {
      connectionId,
      method: "collectionStats",
      args: at,
    });
    expect(stats.count).toBe(60);
    const preview = await mockInvoke<{ command: string }>("session_call", {
      connectionId,
      method: "preview",
      args: { method: "dropCollection", args: at },
    });
    expect(preview.command).toBe('db.getSiblingDB("shop").runCommand({"drop": "orders"})');
    // The shell text the user typed comes back as the parsed document (`calls.rs`'s tests).
    const previewOf = (method: string, args: Record<string, unknown>) =>
      mockInvoke<{ command: string }>("session_call", {
        connectionId,
        method: "preview",
        args: { method, args },
      }).then((r) => r.command);
    expect(
      await previewOf("createIndex", { ...at, keys: "{a: 1, b: -1}", options: "{unique: true}" }),
    ).toBe(
      'db.getSiblingDB("shop").runCommand({"createIndexes": "orders", "indexes": [{"key": {"a": 1, "b": -1}, "name": "a_1_b_-1", "unique": true}]})',
    );
    expect(
      await previewOf("setValidator", {
        ...at,
        validator: "{$jsonSchema: {required: ['a']}}",
        level: "moderate",
      }),
    ).toBe(
      'db.getSiblingDB("shop").runCommand({"collMod": "orders", "validator": {"$jsonSchema": {"required": ["a"]}}, "validationLevel": "moderate"})',
    );
    await expect(previewOf("setValidator", { ...at, level: "loose" })).rejects.toMatchObject({
      code: "INVALID_INPUT",
      message: expect.stringContaining("one of strict, moderate, off"),
    });
    await expect(previewOf("createIndex", { ...at, keys: "{}" })).rejects.toMatchObject({
      code: "INVALID_INPUT",
    });
    // `mongo::create_collection` extends the command with the options (calls.rs's test).
    const coll = { database: "shop", name: "c" };
    expect(await previewOf("createCollection", coll)).toBe(
      'db.getSiblingDB("shop").runCommand({"create": "c"})',
    );
    expect(
      await previewOf("createCollection", { ...coll, options: "{capped: true, size: 1024}" }),
    ).toBe('db.getSiblingDB("shop").runCommand({"create": "c", "capped": true, "size": 1024})');
    await expect(
      previewOf("createCollection", { ...coll, options: "{capped: }" }),
    ).rejects.toMatchObject({
      code: "INVALID_INPUT",
      message: expect.stringMatching(/^MongoDB options: .* \(line 1, column \d+\)$/),
    });
    await expect(previewOf("createCollection", { ...coll, options: "[1]" })).rejects.toMatchObject({
      message: "MongoDB options: expected a document `{ … }` (line 1, column 1)",
    });
    const view = { database: "shop", name: "big", viewOn: "orders", pipeline: "[{$match: {}}]" };
    const created = await mockInvoke<{ command: string }>("session_call", {
      connectionId,
      method: "createView",
      args: view,
    });
    expect(created.command).toBe(
      'db.getSiblingDB("shop").runCommand({"create": "big", "viewOn": "orders", "pipeline": [{"$match": {}}]})',
    );
    expect(databases.get("shop")!.get("big")).toMatchObject({ kind: "view", viewOn: "orders" });
    await expect(
      mockInvoke("session_call", {
        connectionId,
        method: "createView",
        args: { ...view, name: "bad", pipeline: "{}" },
      }),
    ).rejects.toMatchObject({ code: "INVALID_INPUT" });
    await mockInvoke("session_call", { connectionId, method: "truncateCollection", args: at });
    expect(databases.get("shop")!.get("orders")!.docs).toHaveLength(0);
    await expect(
      mockInvoke("session_call", { connectionId, method: "frobnicate", args: at }),
    ).rejects.toMatchObject({ code: "UNSUPPORTED" });
    await expect(
      mockInvoke("driver_util", {
        input: { profile: mockProfiles.get("p-mock-mongodb") },
        method: "listDatabases",
        args: null,
      }),
    ).rejects.toMatchObject({ code: "UNSUPPORTED" });
  });

  it("lists and kills operations, and refuses Kill Connection", async () => {
    const { connectionId } = await connect("p-mock-mongodb");
    const ops = await mockInvoke<ProcessRow[]>("admin_list_processes", { connectionId });
    expect(ops.map((p) => [p.id, p.current])).toEqual([
      ["4011", false],
      ["4012", false],
      ["4013", true],
    ]);
    const killed = await mockInvoke<KillOutcome[]>("admin_kill_processes", {
      connectionId,
      ids: ["4011"],
      scope: "query",
    });
    expect(killed).toEqual([{ id: "4011", ok: true, message: null }]);
    const conn = await mockInvoke<KillOutcome[]>("admin_kill_processes", {
      connectionId,
      ids: ["4012"],
      scope: "connection",
    });
    expect(conn[0]?.ok).toBe(false);
  });

  it("refuses every write of a read-only session, and still reads", async () => {
    const { connectionId, readOnly } = await connect("p-mock-mongodb-ro");
    expect(readOnly).toBe(true);
    const result = await apply(connectionId, [{ op: "delete", rowId: "r1", key: pk(1) }]);
    expect(result).toMatchObject({ success: false, error: { message: READ_ONLY_MESSAGE } });
    expect(databases.get("shop")!.get("orders")!.docs).toHaveLength(60);
    const write = await run(connectionId, "db.orders.deleteMany({})");
    expect(write.results[0]?.error?.message).toBe(READ_ONLY_MESSAGE);
    const out = await run(connectionId, "db.orders.aggregate([{$match: {}}, {$out: 'copy'}])");
    expect(out.results[0]?.error?.message).toBe(READ_ONLY_MESSAGE);
    const read = await run(connectionId, "db.orders.find().limit(2)");
    expect(read.rows).toHaveLength(2);
    await expect(
      mockInvoke("session_call", {
        connectionId,
        method: "dropCollection",
        args: { database: "shop", collection: "orders" },
      }),
    ).rejects.toMatchObject({ message: READ_ONLY_MESSAGE });
    await expect(
      mockInvoke("session_call", {
        connectionId,
        method: "createView",
        args: { database: "shop", name: "v", viewOn: "orders", pipeline: "[]" },
      }),
    ).rejects.toMatchObject({ message: READ_ONLY_MESSAGE });
    expect(
      await mockInvoke<{ count: number }>("session_call", {
        connectionId,
        method: "collectionStats",
        args: { database: "shop", collection: "orders" },
      }),
    ).toMatchObject({ count: 60 });
  });
});
