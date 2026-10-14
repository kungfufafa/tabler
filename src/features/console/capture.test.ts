import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { ipc, onSqlLog, type BrowseRequest, type SqlLogEvent, type TableRef } from "@/lib/ipc";
import { mockInvoke } from "@/lib/ipc/mock";
import { resetMockDdl } from "@/lib/ipc/mock/ddl";
import { resetMockQueryState } from "@/lib/ipc/mock/query";
import { resetMockTrace } from "@/lib/ipc/mock/trace";
import { resetTraceLedger } from "@/lib/ipc/sqlTrace";
import { useConnectionsStore } from "@/stores/connections";
import { filterEntries, useConsoleStore } from "./store";

// The console store subscribes to the facade's SQL log; these run against the in-memory mock.

const USERS: TableRef = { schema: "public", name: "users" };
let c = "";

const entries = () => useConsoleStore.getState().entries;

beforeAll(async () => {
  const info = await ipc.connections.connect("p-local-pg");
  c = info.connectionId;
  useConnectionsStore.getState().add(info);
});

afterAll(async () => {
  await ipc.connections.disconnect(c);
  useConnectionsStore.getState().remove(c);
});

beforeEach(() => {
  useConsoleStore.getState().clear();
  resetMockQueryState({ emptyHistory: true });
  // Nothing recorded, nothing cached: the first read of a table runs its structure queries.
  resetMockTrace();
  resetTraceLedger();
});

afterEach(() => resetMockDdl());

describe("console capture", () => {
  const PAGE: BrowseRequest = {
    table: USERS,
    columns: null,
    filter: null,
    sort: [],
    limit: 5,
    offset: 0,
  };

  it("logs a browsed page's SELECT under data, with the connection and database", async () => {
    const page = await ipc.browse.rows(c, PAGE);
    // The first read of a table also reads its structure: those queries are logged too (below).
    const data = entries().filter((e) => e.category === "data");
    expect(data).toHaveLength(1);
    expect(data[0]).toMatchObject({
      connectionId: c,
      connectionName: "Local Postgres",
      database: "tabler_dev",
      category: "data",
      sql: page.sql,
      meta: false,
      durationMs: page.durationMs,
      rows: 5,
    });
    expect(page.sql).toMatch(/^SELECT .* FROM "public"."users"/);
  });

  it("logs a browsed page once: the call's own entry, not the backend's record of the same SELECT", async () => {
    await ipc.browse.rows(c, PAGE);
    useConsoleStore.getState().clear();
    // The structure is cached now: the page is the only statement.
    const page = await ipc.browse.rows(c, PAGE);
    expect(entries().map((e) => [e.category, e.sql, e.meta])).toEqual([["data", page.sql, false]]);
    // The backend's record of the same SELECT was drained and dropped, not left for the next call.
    expect(await mockInvoke("drain_sql_trace", { connectionId: c })).toEqual([]);
  });

  it("logs the structure queries that reading a table ran, before its page", async () => {
    const page = await ipc.browse.rows(c, PAGE);
    expect(entries().map((e) => [e.category, e.meta])).toEqual([
      ["structure", false],
      ["structure", false],
      ["structure", false],
      ["data", false],
    ]);
    expect(entries()[0]!.sql).toMatch(/^SELECT .* FROM pg_class c JOIN pg_namespace n/);
    expect(entries().at(-1)!.sql).toBe(page.sql);
  });

  it("logs a failed call as a described request with the error", async () => {
    await expect(
      ipc.browse.rows(c, {
        table: { schema: "public", name: "nope" },
        columns: null,
        filter: null,
        sort: [],
        limit: 5,
        offset: 0,
      }),
    ).rejects.toMatchObject({ code: "NOT_FOUND" });
    expect(entries()).toEqual([
      expect.objectContaining({
        category: "data",
        sql: "-- browse public.nope",
        meta: true,
        error: expect.stringContaining("nope"),
      }),
    ]);
  });

  it("logs each statement of an editor run with its rows, and the failing one with its error", async () => {
    await ipc.query.run(
      c,
      crypto.randomUUID(),
      "select id, email from users limit 3;\nselect * from nosuch",
      {},
      () => undefined,
    );
    expect(entries().map((e) => [e.category, e.sql, e.rows, e.error])).toEqual([
      ["query", "select id, email from users limit 3", 3, undefined],
      ["query", "select * from nosuch", undefined, 'relation "nosuch" does not exist'],
    ]);
    expect(entries()[0]!.durationMs).toBeGreaterThan(0);
  });

  it("labels EXPLAIN runs of the editor, and logs the explain command as what really ran", async () => {
    await ipc.query.run(c, crypto.randomUUID(), "select 1", { explain: "plan" }, () => undefined);
    await ipc.query.explain(c, "select * from users", "analyze");
    // The editor's run is the statement the user submitted, labelled; the explain command is the
    // statements the backend ran (the wrapped EXPLAIN, in a transaction it rolls back).
    expect(entries().map((e) => [e.category, e.label, e.sql, e.meta])).toEqual([
      ["query", "EXPLAIN", "select 1", false],
      ["data", undefined, "BEGIN", false],
      [
        "data",
        undefined,
        "EXPLAIN (ANALYZE, BUFFERS, VERBOSE, FORMAT TEXT) select * from users",
        false,
      ],
      ["data", undefined, "ROLLBACK", false],
    ]);
  });

  it("logs applied row edits as one batch with the affected count", async () => {
    const result = await ipc.mutate.apply(c, {
      table: USERS,
      returning: true,
      changes: [
        {
          op: "update",
          rowId: "r1",
          key: { kind: "pk", value: { id: { t: "int", v: 1 } } },
          changes: { bio: { t: "text", v: "hello" } },
        },
      ],
    });
    expect(result.success).toBe(true);
    expect(entries()).toEqual([
      expect.objectContaining({ category: "data", sql: result.sql[0], meta: false, affected: 1 }),
    ]);
  });

  it("logs applied DDL under structure, and a rolled-back plan with its error", async () => {
    const ok = await ipc.ddl.apply(c, USERS, [
      { op: "renameColumn", table: USERS, name: "nickname", newName: "nick" },
    ]);
    await ipc.ddl.apply(c, USERS, [
      { op: "alterColumnType", table: USERS, name: "bio", newType: "nosuchtype", using: null },
    ]);
    expect(entries().map((e) => [e.category, e.sql, e.error])).toEqual([
      ["structure", ok.executed[0], undefined],
      ["structure", "-- apply schema changes to public.users", 'type "nosuchtype" does not exist'],
    ]);
  });

  it("logs introspection as the SQL the backend ran, not a description", async () => {
    await ipc.schema.tables(c, "public");
    expect(entries()).toHaveLength(1);
    const [tables] = entries();
    expect(tables).toMatchObject({ category: "structure", meta: false });
    expect(tables!.sql).toMatch(
      /^SELECT .* FROM pg_class c JOIN pg_namespace n .* n\.nspname = 'public'/,
    );
    expect(tables!.sql.startsWith("--")).toBe(false);
    expect(tables!.durationMs).toBeGreaterThanOrEqual(0);
    expect(tables!.rows).toBeGreaterThan(0);

    await ipc.schema.databases(c);
    await ipc.schema.schemas(c);
    await ipc.schema.completion(c);
    expect(entries().every((e) => e.category === "structure" && !e.meta)).toBe(true);
    expect(entries().some((e) => e.sql.includes("FROM pg_database d"))).toBe(true);
    expect(entries().some((e) => e.sql.includes("FROM pg_namespace n"))).toBe(true);
    expect(entries().some((e) => e.sql.includes("FROM pg_attribute a"))).toBe(true);
  });

  it("Meta queries only shows the catalog SELECTs, Data queries only does not", async () => {
    await ipc.schema.tables(c, "public");
    await ipc.browse.count(c, USERS, null);
    const all = entries();
    const meta = filterEntries(all, "meta");
    expect(meta.length).toBeGreaterThan(0);
    expect(meta.every((e) => e.category === "structure" && !e.sql.startsWith("--"))).toBe(true);
    expect(meta.every((e) => /pg_/.test(e.sql))).toBe(true);
    expect(filterEntries(all, "data").map((e) => e.sql)).toEqual([
      'SELECT COUNT(*) FROM "public"."users"',
    ]);
    expect(filterEntries(all, "data").some((e) => e.sql.includes("pg_"))).toBe(false);
  });

  it("logs a row count as its COUNT statement, not a description", async () => {
    const counted = await ipc.browse.count(c, USERS, null);
    const data = entries().filter((e) => e.category === "data");
    expect(data).toEqual([
      expect.objectContaining({
        sql: 'SELECT COUNT(*) FROM "public"."users"',
        meta: false,
        rows: 1,
      }),
    ]);
    expect(counted.exact).toBe(true);
    expect(entries().some((e) => e.meta)).toBe(false);
  });

  it("logs a fetched cell as its SELECT", async () => {
    await ipc.browse.cell(c, USERS, { kind: "pk", value: { id: { t: "int", v: 1 } } }, "bio");
    const data = entries().filter((e) => e.category === "data");
    expect(data).toHaveLength(1);
    expect(data[0]!.sql).toMatch(/^SELECT "bio" FROM "public"."users" WHERE/);
    expect(data[0]!.meta).toBe(false);
  });

  it("still describes a call that ran no SQL (a cached answer), and never as SQL", async () => {
    await ipc.schema.table(c, USERS);
    useConsoleStore.getState().clear();
    // The structure is cached: nothing ran on the server.
    await ipc.schema.table(c, USERS);
    expect(entries()).toEqual([
      expect.objectContaining({
        category: "structure",
        sql: "-- load the structure of public.users",
        meta: true,
      }),
    ]);
    // A refresh reads it again: the queries replace the description.
    useConsoleStore.getState().clear();
    await ipc.schema.table(c, USERS, true);
    expect(entries().length).toBeGreaterThan(1);
    expect(entries().some((e) => e.meta)).toBe(false);
  });

  it("logs concurrent calls without duplicates or descriptions", async () => {
    const [page] = await Promise.all([
      ipc.browse.rows(c, PAGE),
      ipc.browse.count(c, USERS, null),
      ipc.schema.tables(c, "public"),
    ]);
    const data = entries()
      .filter((e) => e.category === "data")
      .map((e) => e.sql);
    expect(data.sort()).toEqual([page.sql, 'SELECT COUNT(*) FROM "public"."users"'].sort());
    expect(entries().some((e) => e.meta)).toBe(false);
    const listed = entries().filter((e) => e.sql.includes("FROM pg_class c JOIN pg_namespace n"));
    // The table list and the cold table structure each ran their own pg_class query.
    expect(listed.length).toBeGreaterThanOrEqual(1);
    expect(new Set(entries().map((e) => e.sql)).size).toBe(entries().length);
  });

  it("never lets a throwing listener break the call", async () => {
    const seen: SqlLogEvent[] = [];
    const offBad = onSqlLog(() => {
      throw new Error("boom");
    });
    const offGood = onSqlLog((e) => seen.push(e));
    await expect(ipc.schema.schemas(c)).resolves.toBeInstanceOf(Array);
    offBad();
    offGood();
    expect(seen).toHaveLength(1);
    expect(entries()).toHaveLength(1);
  });
});
