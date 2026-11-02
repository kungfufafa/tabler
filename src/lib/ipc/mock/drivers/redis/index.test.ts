import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { ConnectionInfo } from "../../../bindings/ConnectionInfo";
import type { DatabaseInfo } from "../../../bindings/DatabaseInfo";
import type { FilterGroup } from "../../../bindings/FilterGroup";
import type { FilterOp } from "../../../bindings/FilterOp";
import type { KillOutcome } from "../../../bindings/KillOutcome";
import type { ProcessRow } from "../../../bindings/ProcessRow";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { RedisEditResult } from "../../../bindings/RedisEditResult";
import type { RedisKeyPage } from "../../../bindings/RedisKeyPage";
import type { RedisServerInfo } from "../../../bindings/RedisServerInfo";
import type { RowCount } from "../../../bindings/RowCount";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { TableSummary } from "../../../bindings/TableSummary";
import type { Value } from "../../../bindings/Value";
import { mockProfiles } from "../../handlers";
import { mockInvoke } from "../../transport";
import { enableMockDrivers, resetMockBuild } from "..";
import lane from ".";
import { READ_ONLY, runConsole } from "./console";
import { globMatch, resetServer, server } from "./data";
import { keyFilter, REDIS_CAPS } from "./session";

// The mock's Redis against the backend (drivers/redis/session.rs, grid.rs, apply.rs, calls.rs and
// their session tests): caps, the databases as tables, SCAN cursor pages with the filter bar as
// globs, counts, whole cells, a commit with a failing change (the others applied), the console
// with SELECT and MULTI, the value editor's calls, the clients, and a read-only session's
// refusals.

beforeAll(async () => {
  await import("../..");
});

let added: string[] = [];

beforeEach(async () => {
  resetServer();
  added = (await enableMockDrivers(["redis"])).map((p) => p.id);
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

const db0 = { schema: null, name: "db0" };
const text = (v: string): Value => ({ t: "text", v });
const page = (connectionId: string, req: Record<string, unknown> = {}) =>
  mockInvoke<BrowsePage>("browse_table", {
    connectionId,
    req: { table: db0, columns: null, filter: null, sort: [], limit: 25, offset: 0, ...req },
  });
const pk = (key: string, type: string) => ({
  kind: "pk",
  value: { key: text(key), type: text(type) },
});
const apply = (connectionId: string, changes: unknown[]) =>
  mockInvoke<ApplyResult>("apply_changes", {
    connectionId,
    req: { table: db0, changes, returning: false },
  });
const call = <T>(connectionId: string, method: string, args: unknown) =>
  mockInvoke<T>("session_call", { connectionId, method, args });
const keys = (db = 0) => server().dbs.get(db)!;
const cond = (column: string, op: FilterOp, v: string): FilterGroup => ({
  conjunction: "and",
  items: [{ kind: "condition", column, op, values: [text(v)] }],
});

describe("the mock Redis driver", () => {
  it("is the lane's one driver, with three seed profiles", () => {
    expect(lane.drivers.map((d) => d.driver)).toEqual(["redis"]);
    expect(
      lane.drivers[0]!.seedProfiles().map((p) => [p.id, p.database, p.options.readOnly]),
    ).toEqual([
      ["p-mock-redis", null, false],
      ["p-mock-redis-db1", "1", false],
      ["p-mock-redis-ro", null, true],
    ]);
  });

  it("connects with the backend's caps and lists every database as a table of keys", async () => {
    const info = await connect("p-mock-redis");
    expect(info.serverVersion).toBe("Redis 7.4.1");
    expect(info.currentDatabase).toBe("db0");
    expect(info.capabilities).toEqual(REDIS_CAPS);
    expect(REDIS_CAPS.data.paging).toBe("cursor");
    expect(REDIS_CAPS.data.transactions).toBe("batch");
    expect(REDIS_CAPS.data.sort).toBe(false);
    expect(REDIS_CAPS.features).not.toContain("cancelOnServer");
    const { connectionId } = info;
    const dbs = await mockInvoke<DatabaseInfo[]>("list_databases", { connectionId });
    expect(dbs).toHaveLength(16);
    expect(dbs.filter((d) => d.isCurrent).map((d) => d.name)).toEqual(["db0"]);
    const tables = await mockInvoke<TableSummary[]>("list_tables", { connectionId, schema: "" });
    expect(tables.slice(0, 3).map((t) => [t.name, t.rowEstimate, t.comment])).toEqual([
      ["db0", keys(0).size, "10 keys with a TTL"],
      ["db1", 5, "5 keys with a TTL"],
      ["db2", 0, null],
    ]);
    const meta = await mockInvoke<TableMeta>("get_table", { connectionId, table: db0 });
    expect(meta.primaryKey).toEqual(["key", "type"]);
    expect(meta.columns.filter((c) => c.isGenerated).map((c) => c.name)).toEqual([
      "type",
      "size",
      "memory",
    ]);
    const def = await mockInvoke<string>("get_view_definition", { connectionId, table: db0 });
    expect(JSON.parse(def)).toMatchObject({ database: 0, primaryKey: ["key", "type"] });
    // The DB index of the profile picks the current database.
    expect((await connect("p-mock-redis-db1")).currentDatabase).toBe("db1");
  });

  it("browses by SCAN cursor, page by page, without losing or repeating a key", async () => {
    const { connectionId } = await connect("p-mock-redis");
    const seen: string[] = [];
    let cursor: string | null = null;
    let pages = 0;
    do {
      const p: BrowsePage = await page(connectionId, { cursor });
      expect(p.pkColumns).toEqual(["key", "type"]);
      expect(p.sql).toMatch(/^SCAN \d+ COUNT 25/);
      seen.push(...p.rows.map((r) => (r[0] as { v: string }).v));
      cursor = p.nextCursor ?? null;
      pages++;
    } while (cursor !== null);
    expect(pages).toBe(Math.ceil(keys().size / 25));
    expect(new Set(seen).size).toBe(keys().size);
    const first = await page(connectionId);
    const user = first.rows.find((r) => (r[0] as { v: string }).v === "user:1")!;
    expect(user.slice(0, 4)).toEqual([
      text("user:1"),
      text("hash"),
      { t: "null" },
      { t: "int", v: 3 },
    ]);
    expect(user[5]).toMatchObject({ t: "json" });
  });

  it("filters by glob, exact key and type, as the backend maps the filter bar", async () => {
    const { connectionId } = await connect("p-mock-redis");
    const users = await page(connectionId, {
      filter: cond("key", "hasPrefix", "user:1"),
      limit: 100,
    });
    expect(users.rows.map((r) => (r[0] as { v: string }).v).sort()).toEqual(
      ["user:1", ...Array.from({ length: 10 }, (_, i) => `user:1${i}`)].sort(),
    );
    expect(users.sql).toContain("MATCH user:1*");
    const exact = await page(connectionId, { filter: cond("key", "eq", "my key with spaces") });
    expect(exact.sql).toBe('TYPE "my key with spaces"');
    expect(exact.rows).toHaveLength(1);
    const streams = await page(connectionId, { filter: cond("type", "eq", "stream") });
    expect(streams.rows.map((r) => r[0])).toEqual([text("events:orders")]);
    expect(keyFilter(cond("key", "containsCase", "Ab*"))).toEqual({
      exact: null,
      pattern: "*[aA][bB]\\**",
      type: null,
    });
    expect(keyFilter(cond("key", "like", "us_r:%"))).toMatchObject({ pattern: "us?r:*" });
    expect(globMatch("*[aA][bB]\\**", "xAb*y")).toBe(true);
    expect(globMatch("user:[0-3]", "user:2")).toBe(true);
    expect(globMatch("user:[^0-3]", "user:2")).toBe(false);
    await expect(page(connectionId, { filter: cond("ttl", "eq", "1") })).rejects.toMatchObject({
      code: "UNSUPPORTED",
    });
  });

  it("counts exactly and fetches whole cells, refusing a key whose type changed", async () => {
    const { connectionId } = await connect("p-mock-redis");
    const count = (filter: unknown) =>
      mockInvoke<RowCount>("count_table_rows", { connectionId, table: db0, filter });
    expect(await count(null)).toEqual({ count: keys().size, exact: true });
    expect(await count(cond("key", "eq", "greeting"))).toEqual({ count: 1, exact: true });
    expect(await count(cond("key", "hasPrefix", "session:"))).toEqual({ count: 10, exact: true });
    const blob = (await page(connectionId, { filter: cond("key", "eq", "big:blob") })).rows[0]!;
    expect(blob[5]).toMatchObject({ t: "bytes", v: { len: 40_000, truncated: true } });
    const whole = await mockInvoke<Value>("fetch_cell", {
      connectionId,
      table: db0,
      key: pk("big:blob", "string"),
      column: "value",
    });
    expect(whole).toEqual(text("x".repeat(40_000)));
    await expect(
      mockInvoke("fetch_cell", {
        connectionId,
        table: db0,
        key: pk("big:blob", "hash"),
        column: "value",
      }),
    ).rejects.toMatchObject({ message: "the key was changed by someone else" });
  });

  it("previews and applies a commit change by change: one fails, the others apply", async () => {
    const { connectionId } = await connect("p-mock-redis");
    const changes = [
      {
        op: "update",
        rowId: "r1",
        key: pk("greeting", "string"),
        changes: { value: text("hi"), ttl: text("60") },
      },
      // A rename onto a key that exists: RENAMENX refuses it, after its TTL edit applied.
      {
        op: "update",
        rowId: "r2",
        key: pk("counter:visits", "string"),
        changes: { ttl: text("30"), key: text("greeting") },
      },
      {
        op: "insert",
        rowId: "r3",
        values: {
          key: text("new:key"),
          type: { t: "other", v: { ty: "missing", text: "" } },
          value: text("v"),
        },
      },
      { op: "delete", rowId: "r4", key: pk("tags:popular", "set") },
    ];
    const preview = await mockInvoke<string[]>("preview_changes", {
      connectionId,
      req: { table: db0, changes, returning: false },
    });
    expect(preview).toEqual([
      "WATCH greeting",
      "MULTI",
      "SET greeting hi XX KEEPTTL",
      "PEXPIRE greeting 60000",
      "EXEC",
      "WATCH counter:visits",
      "MULTI",
      "PEXPIRE counter:visits 30000",
      "RENAMENX counter:visits greeting",
      "EXEC",
      "SET new:key v NX",
      "WATCH tags:popular",
      "MULTI",
      "UNLINK tags:popular",
      "EXEC",
    ]);
    const result = await apply(connectionId, changes);
    expect(result.success).toBe(false);
    expect(result.results.map((r) => [r.rowId, r.affected, r.error?.message ?? null])).toEqual([
      ["r1", 1, null],
      ["r2", 0, "a key with this name already exists"],
      ["r3", 1, null],
      ["r4", 1, null],
    ]);
    expect(keys().get("greeting")).toMatchObject({ value: "hi", pttl: 60_000 });
    // No rollback: the failed change's TTL edit stays applied, as Redis's MULTI leaves it.
    expect(keys().get("counter:visits")).toMatchObject({ pttl: 30_000 });
    expect(keys().has("new:key")).toBe(true);
    expect(keys().has("tags:popular")).toBe(false);
    // A container's value is edited in the value editor; NULL is refused (TablePlus deleted the key).
    await expect(
      apply(connectionId, [
        { op: "update", rowId: "x", key: pk("user:1", "hash"), changes: { value: text("{}") } },
      ]),
    ).rejects.toMatchObject({ message: "a hash is edited element by element in the value editor" });
    await expect(
      apply(connectionId, [
        {
          op: "update",
          rowId: "x",
          key: pk("greeting", "string"),
          changes: { value: { t: "null" } },
        },
      ]),
    ).rejects.toMatchObject({
      message: "Redis has no NULL value: set an empty string, or delete the key",
    });
    await expect(
      apply(connectionId, [{ op: "insert", rowId: "x", values: { key: text("") } }]),
    ).rejects.toMatchObject({ message: "a new key needs a name" });
    // A key replaced meanwhile by another type fails its type guard; nothing of it is sent.
    keys().set("greeting", { type: "list", value: ["a"], pttl: null });
    const stale = await apply(connectionId, [
      { op: "delete", rowId: "d", key: pk("greeting", "string") },
    ]);
    expect(stale.results[0]?.error?.message).toBe("the key was changed by someone else");
    expect(keys().has("greeting")).toBe(true);
  });

  it("runs the console with SELECT and MULTI on its own connection, shaped as the backend's grids", async () => {
    const { connectionId } = await connect("p-mock-redis");
    const got = await run(
      connectionId,
      "HGETALL user:1\nZRANGE leaderboard 0 1 WITHSCORES\nLRANGE queue:emails 0 1\nGET greeting",
    );
    expect(got.results.map((r) => r.columns.map((c) => c.name))).toEqual([
      ["field", "value"],
      ["member", "score"],
      ["#", "value"],
      ["value"],
    ]);
    expect(got.rows[0]).toEqual([text("name"), text("Grace 1")]);
    const scan = await run(connectionId, "SCAN 0 MATCH user:* COUNT 5");
    expect(scan.results[0]?.notices).toEqual(["next cursor: 5"]);
    const info = await run(connectionId, "INFO server");
    expect(info.results[0]?.columns.map((c) => c.name)).toEqual(["section", "field", "value"]);
    // SELECT changes the console's database only; MULTI queues until EXEC.
    await run(connectionId, "SELECT 2\nMULTI\nSET a 1\nSET b 2");
    expect(keys(2).size).toBe(0);
    const exec = await run(connectionId, "EXEC");
    expect(exec.rows).toEqual([
      [{ t: "int", v: 1 }, text("OK")],
      [{ t: "int", v: 2 }, text("OK")],
    ]);
    expect([...keys(2).keys()]).toEqual(["a", "b"]);
    expect((await page(connectionId)).rows.length).toBeGreaterThan(0);
    const bad = await run(connectionId, 'GET "unclosed');
    expect(bad.results[0]?.error?.message).toBe("Invalid argument(s): unbalanced quotes");
    const blocked = await run(connectionId, "BLPOP queue:emails 0");
    expect(blocked.results[0]?.error).toMatchObject({ code: "UNSUPPORTED" });
    // The grid still reads db0; the console goes back to it with SELECT.
    const wrong = await run(connectionId, "SELECT 0\nGET user:1");
    expect(wrong.results[0]?.error).toBeNull();
    expect(wrong.results[1]?.error?.message).toMatch(/^WRONGTYPE/);
    const unknown = await run(connectionId, "FROBNICATE x");
    expect(unknown.results[0]?.error?.message).toMatch(/^ERR unknown command 'FROBNICATE'/);
  });

  it("serves the value editor's calls: pages, previews and guarded edits with a result each", async () => {
    const { connectionId } = await connect("p-mock-redis");
    const first = await call<RedisKeyPage>(connectionId, "keyPage", {
      db: 0,
      key: text("queue:emails"),
      type: "list",
      cursor: null,
      count: 50,
    });
    expect(first.elements).toHaveLength(50);
    expect(first.elements[0]).toEqual({ name: { t: "int", v: 0 }, value: text("email-job-1") });
    expect(first.nextCursor).toBe("50");
    expect(first.size).toEqual({ t: "int", v: 120 });
    await expect(
      call(connectionId, "keyPage", {
        db: 0,
        key: text("queue:emails"),
        type: "hash",
        cursor: null,
        count: 5,
      }),
    ).rejects.toMatchObject({ message: "the key was changed by someone else" });
    const args = {
      db: 0,
      key: text("user:1"),
      type: "hash",
      edits: [
        { op: "hashSet", field: text("plan"), value: text("pro") },
        {
          op: "hashRename",
          from: text("email"),
          to: text("name"),
          value: text("grace1@example.com"),
        },
      ],
    };
    const preview = await call<string[]>(connectionId, "previewEdits", args);
    expect(preview).toEqual([
      "WATCH user:1",
      "MULTI",
      "HSET user:1 plan pro",
      "EXEC",
      "WATCH user:1",
      "MULTI",
      "HSETNX user:1 name grace1@example.com",
      "HDEL user:1 email",
      "EXEC",
    ]);
    const r = await call<RedisEditResult>(connectionId, "editKey", args);
    expect(r.results).toEqual([
      { ok: true, error: null },
      { ok: false, error: "the field already exists" },
    ]);
    const h = keys().get("user:1") as { value: Map<string, string> };
    expect(h.value.get("plan")).toBe("pro");
    expect(h.value.get("email")).toBe("grace1@example.com");
    const created = await call<RedisEditResult>(connectionId, "newKey", {
      db: 0,
      key: text("zs"),
      ttl: { t: "int", v: 10 },
      value: { type: "zset", members: [{ member: text("m"), score: "1.5" }] },
    });
    expect(created).toEqual({
      results: [{ ok: true, error: null }],
      preview: ["WATCH zs", "MULTI", "ZADD zs 1.5 m", "PEXPIRE zs 10000", "EXEC"],
    });
    const again = await call<RedisEditResult>(connectionId, "newKey", {
      db: 0,
      key: text("zs"),
      ttl: null,
      value: { type: "set", members: [text("x")] },
    });
    expect(again.results[0]).toEqual({ ok: false, error: "a key with this name already exists" });
    const copied = await call<RedisEditResult>(connectionId, "copyKey", {
      db: 0,
      from: text("zs"),
      to: text("greeting"),
    });
    expect(copied.results[0]?.ok).toBe(false);
    const info = await call<RedisServerInfo>(connectionId, "serverInfo", null);
    expect(info).toMatchObject({
      product: "Redis",
      version: "7.4.1",
      databases: 16,
      readOnly: false,
    });
    expect(info.sections.map((s) => s.name)).toContain("Keyspace");
    await call(connectionId, "flushDb", { db: 1 });
    expect(keys(1).size).toBe(0);
    await expect(call(connectionId, "frobnicate", null)).rejects.toMatchObject({
      code: "UNSUPPORTED",
      message: "unsupported: Redis: unknown call “frobnicate”",
    });
  });

  it("keeps a leading BOM as a byte of a word or a value, as the backend reads bytes", async () => {
    const BOM = "\uFEFF";
    const { connectionId } = await connect("p-mock-redis");
    // console argv: a BOM-prefixed command name is not SET. Run on the console itself: run_sql's
    // one-line fallback split (the lane module is not loaded here) would trim the BOM away.
    const events: QueryEvent[] = [];
    const state = { server: server(), readOnly: false, db: 0, queue: null };
    runConsole(state, `${BOM}SET k v`, (e) => events.push(e));
    const failed = events.find((e) => e.event === "failed");
    expect(failed?.event === "failed" ? failed.data.error.message : undefined).toMatch(
      new RegExp(`^ERR unknown command '${BOM}SET'`),
    );
    expect(keys().has("k")).toBe(false);
    // engine GETRANGE: the BOM of a stored value comes back.
    await run(connectionId, `SET bom ${BOM}v`);
    expect((await run(connectionId, "GETRANGE bom 0 -1")).rows).toEqual([[text(`${BOM}v`)]]);
    // plan value decode: a bytes cell edit keeps its BOM.
    const raw = new TextEncoder().encode(`${BOM}x`);
    const bytes: Value = {
      t: "bytes",
      v: { base64: btoa(String.fromCharCode(...raw)), len: raw.length, truncated: false },
    };
    const r = await apply(connectionId, [
      { op: "update", rowId: "r1", key: pk("greeting", "string"), changes: { value: bytes } },
    ]);
    expect(r.success).toBe(true);
    expect(keys().get("greeting")).toMatchObject({ type: "string", value: `${BOM}x` });
  });

  it("lists the clients as the process list and kills one", async () => {
    const { connectionId } = await connect("p-mock-redis");
    const rows = await mockInvoke<ProcessRow[]>("admin_list_processes", { connectionId });
    expect(rows.map((p) => [p.id, p.database, p.current])).toEqual([
      ["3", "db0", false],
      ["4", "db1", false],
      ["10", "db0", true],
    ]);
    const killed = await mockInvoke<KillOutcome[]>("admin_kill_processes", {
      connectionId,
      ids: ["4", "99"],
      scope: "connection",
    });
    expect(killed).toEqual([
      { id: "4", ok: true, message: null },
      { id: "99", ok: false, message: "no such client (it may have disconnected)" },
    ]);
  });

  it("refuses every write of a read-only session, and still reads", async () => {
    const { connectionId, readOnly } = await connect("p-mock-redis-ro");
    expect(readOnly).toBe(true);
    const size = keys().size;
    await expect(
      apply(connectionId, [{ op: "delete", rowId: "r1", key: pk("greeting", "string") }]),
    ).rejects.toMatchObject({ message: READ_ONLY });
    for (const write of [
      "DEL greeting",
      "FLUSHALL",
      "KEYS *",
      "CONFIG SET maxmemory 1",
      "FT.DICTADD d w",
    ]) {
      const r = await run(connectionId, write);
      expect(r.results[0]?.error?.message, write).toBe(READ_ONLY);
    }
    for (const [method, args] of [
      [
        "editKey",
        {
          db: 0,
          key: text("user:1"),
          type: "hash",
          edits: [{ op: "hashDelete", field: text("name") }],
        },
      ],
      ["newKey", { db: 0, key: text("k"), ttl: null, value: { type: "string", value: text("v") } }],
      ["copyKey", { db: 0, from: text("greeting"), to: text("g2") }],
      ["flushDb", { db: 0 }],
    ] as const)
      await expect(call(connectionId, method, args), method).rejects.toMatchObject({
        message: READ_ONLY,
      });
    // calls.rs counts an unknown call as a write, so read-only refuses it before dispatch.
    await expect(call(connectionId, "frobnicate", {})).rejects.toMatchObject({
      code: "INVALID_INPUT",
      message: READ_ONLY,
    });
    // The app refuses a kill on a read-only connection before the session (admin::kill).
    await expect(
      mockInvoke("admin_kill_processes", { connectionId, ids: ["3"], scope: "connection" }),
    ).rejects.toMatchObject({ code: "UNSUPPORTED" });
    expect(keys().size).toBe(size);
    const read = await run(connectionId, "GET greeting");
    expect(read.rows).toEqual([[text("hello, world")]]);
    expect(
      await call<string[]>(connectionId, "previewEdits", {
        db: 0,
        key: text("tags:popular"),
        type: "set",
        edits: [{ op: "setAdd", member: text("x") }],
      }),
    ).toEqual(["WATCH tags:popular", "MULTI", "SADD tags:popular x", "EXEC"]);
    expect((await page(connectionId)).rows).toHaveLength(25);
  });

  it("refuses a DB index the server lacks at connect, and the form has no calls", () => {
    const driver = lane.drivers[0]!;
    const p = { ...driver.seedProfiles()[0]!, database: "99" };
    expect(() => driver.connect(p)).toThrow();
    expect(() => driver.util?.("anything", null, { profile: p } as never)).toThrow();
  });
});
