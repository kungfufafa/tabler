import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { ipc, type QueryEvent } from "@/lib/ipc";
import { resetMockQueryState } from "@/lib/ipc/mock/query";

let connectionId = "";

beforeAll(async () => {
  connectionId = (await ipc.connections.connect("p-local-pg")).connectionId;
});

beforeEach(() => resetMockQueryState({ emptyHistory: true }));

afterAll(async () => {
  await ipc.connections.disconnect(connectionId);
});

async function run(sql: string, options = {}) {
  const events: QueryEvent[] = [];
  const results = await ipc.query.run(connectionId, crypto.randomUUID(), sql, options, (e) =>
    events.push(e),
  );
  return { events, results };
}

describe("mock run_sql", () => {
  it("streams table rows in chunks with spans and a done event", async () => {
    const { events, results } = await run("select 1;\nselect id, email from users limit 5");
    expect(events.map((e) => e.event)).toEqual([
      "started",
      "columns",
      "rows",
      "finished",
      "started",
      "columns",
      "rows",
      "finished",
      "done",
    ]);
    const second = events[4];
    expect(second).toEqual({
      event: "started",
      data: {
        index: 1,
        sql: "select id, email from users limit 5",
        span: { start: 10, end: 45, line: 2 },
      },
    });
    expect(results[1]).toMatchObject({ kind: "rows", rowCount: 5, truncated: false });
    expect(results[1]?.columns.map((c) => c.name)).toEqual(["id", "email"]);
  });

  it("caps rows at maxRows and marks the result truncated", async () => {
    const { events, results } = await run("select * from events_big", { maxRows: 1200 });
    const chunks = events.filter((e) => e.event === "rows");
    expect(chunks.map((e) => (e.event === "rows" ? e.data.rows.length : 0))).toEqual([
      500, 500, 200,
    ]);
    expect(results[0]).toMatchObject({ rowCount: 1200, truncated: true });
  });

  it("reports syntax errors with a position relative to the whole text and stops", async () => {
    const { events, results } = await run("select 1;\n  selec 2;\nselect 3");
    const failed = events.find((e) => e.event === "failed");
    expect(failed?.event === "failed" && failed.data.error).toMatchObject({
      code: "DB_ERROR",
      sqlstate: "42601",
      message: 'syntax error at or near "selec"',
      position: { offset: 12, line: 2, column: 3 },
      statementIndex: 1,
    });
    expect(results).toHaveLength(2);
  });

  it("reports unknown tables and affected rows", async () => {
    const missing = await run("select * from nope");
    expect(missing.results[0]?.error?.message).toBe('relation "nope" does not exist');
    const dml = await run(
      "update posts set views = 0 where id = 1; insert into tags (name) values ('a'), ('b')",
    );
    expect(dml.results.map((r) => [r.kind, r.affected])).toEqual([
      ["affected", 3],
      ["affected", 2],
    ]);
  });

  it("selects literals without FROM", async () => {
    const { events } = await run("select 1 as one, 'it''s', true");
    const cols = events.find((e) => e.event === "columns");
    const rows = events.find((e) => e.event === "rows");
    expect(cols?.event === "columns" && cols.data.columns.map((c) => c.name)).toEqual([
      "one",
      "?column?",
      "bool",
    ]);
    expect(rows?.event === "rows" && rows.data.rows[0]).toEqual([
      { t: "int", v: 1 },
      { t: "text", v: "it's" },
      { t: "bool", v: true },
    ]);
  });

  it("cancels a sleeping statement", async () => {
    const queryId = crypto.randomUUID();
    const events: QueryEvent[] = [];
    const pending = ipc.query.run(connectionId, queryId, "select pg_sleep(5); select 2", {}, (e) =>
      events.push(e),
    );
    await new Promise((r) => setTimeout(r, 30));
    await ipc.query.cancel(connectionId, queryId);
    const results = await pending;
    expect(results).toHaveLength(1);
    expect(results[0]?.error).toMatchObject({ code: "CANCELLED", sqlstate: "57014" });
    expect(events.at(-1)?.event).toBe("done");
  });

  it("records history, searchable, deletable and clearable", async () => {
    await run("select 1; select * from users limit 2");
    let list = await ipc.history.list({ search: "USERS" });
    expect(list.map((h) => h.sql)).toEqual(["select * from users limit 2"]);
    expect(list[0]).toMatchObject({ success: true, rowCount: 2, connectionName: "Local Postgres" });
    await ipc.history.delete([list[0]!.id]);
    list = await ipc.history.list();
    expect(list.map((h) => h.sql)).toEqual(["select 1"]);
    await ipc.history.clear();
    expect(await ipc.history.list()).toEqual([]);
  });
});

describe("mock explain and files", () => {
  it("returns a textual plan", async () => {
    const plan = await ipc.query.explain(connectionId, "select * from users limit 10", "analyze");
    expect(plan.text).toContain("Seq Scan on users");
    expect(plan.text).toContain("Execution Time");
    expect(plan.rows.length).toBeGreaterThan(1);
  });

  it("rejects explain of invalid SQL with a position", async () => {
    await expect(ipc.query.explain(connectionId, "  frob x", "plan")).rejects.toMatchObject({
      payload: { code: "DB_ERROR", position: { offset: 2, line: 1, column: 3 } },
    });
  });

  it("reads and writes text files in memory", async () => {
    await ipc.files.writeText("/tmp/a.sql", "select 42;");
    expect(await ipc.files.readText("/tmp/a.sql")).toBe("select 42;");
    await expect(ipc.files.readText("/tmp/missing.sql")).rejects.toMatchObject({
      code: "NOT_FOUND",
    });
  });
});
