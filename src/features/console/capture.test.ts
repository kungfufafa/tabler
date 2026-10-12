import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { ipc, onSqlLog, type SqlLogEvent, type TableRef } from "@/lib/ipc";
import { resetMockDdl } from "@/lib/ipc/mock/ddl";
import { resetMockQueryState } from "@/lib/ipc/mock/query";
import { useConnectionsStore } from "@/stores/connections";
import { useConsoleStore } from "./store";

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
});

afterEach(() => resetMockDdl());

describe("console capture", () => {
  it("logs a browsed page's SELECT under data, with the connection and database", async () => {
    const page = await ipc.browse.rows(c, {
      table: USERS,
      columns: null,
      filter: null,
      sort: [],
      limit: 5,
      offset: 0,
    });
    expect(entries()).toHaveLength(1);
    expect(entries()[0]).toMatchObject({
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

  it("labels EXPLAIN runs, from the editor option or the explain command", async () => {
    await ipc.query.run(c, crypto.randomUUID(), "select 1", { explain: "plan" }, () => undefined);
    await ipc.query.explain(c, "select * from users", "analyze");
    expect(entries().map((e) => [e.category, e.label, e.sql])).toEqual([
      ["query", "EXPLAIN", "select 1"],
      ["query", "EXPLAIN ANALYZE", "select * from users"],
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

  it("logs introspection as described meta lines", async () => {
    await ipc.schema.tables(c, "public");
    await ipc.browse.count(c, USERS, null);
    expect(entries().map((e) => [e.category, e.sql, e.meta])).toEqual([
      ["structure", "-- list tables in public", true],
      ["data", "-- count rows of public.users", true],
    ]);
    expect(entries()[1]!.rows).toBeGreaterThan(0);
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
