import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { useConsoleStore } from "@/features/console/store";
import { ipc, type BrowseRequest } from "@/lib/ipc";
import { registerMock } from "@/lib/ipc/mock";
import { drainMockTrace, resetMockTrace } from "@/lib/ipc/mock/trace";
import { useConnectionsStore } from "@/stores/connections";
import {
  SECRET_MASK,
  TWIN_WINDOW_MS,
  markTraceSeen,
  maskSecrets,
  noteLogged,
  normalizeSql,
  resetTraceLedger,
  sameStatement,
  takeTwin,
  traceClock,
  traceLoggedSince,
} from "./sqlTrace";

describe("normalizeSql / sameStatement", () => {
  it("ignores whitespace and trailing semicolons", () => {
    expect(normalizeSql("  SELECT  1\n  FROM t ;; ")).toBe("SELECT 1 FROM t");
    expect(sameStatement("SELECT 1", "SELECT 1")).toBe(true);
    expect(sameStatement("SELECT 1", "SELECT 2")).toBe(false);
  });

  it("matches a statement the backend capped with its marker", () => {
    const long = `INSERT INTO t VALUES ('${"x".repeat(9000)}')`;
    const capped = `${long.slice(0, 8192)} /* … 812 more characters */`;
    expect(sameStatement(normalizeSql(long), normalizeSql(capped))).toBe(true);
    expect(sameStatement(normalizeSql(capped), normalizeSql(long))).toBe(true);
    expect(sameStatement(normalizeSql(`INSERT INTO other ${"x".repeat(9000)}`), capped)).toBe(
      false,
    );
  });
});

describe("maskSecrets", () => {
  it("masks a password in every spelling the SQL could have used", () => {
    const secret = "it's a \\ secret";
    const text = [
      `CREATE ROLE r PASSWORD 'it''s a \\ secret'`,
      `ALTER USER u IDENTIFIED BY 'it''s a \\\\ secret'`,
      `bad value it's a \\ secret`,
    ].join("\n");
    const masked = maskSecrets(text, [secret]);
    expect(masked).not.toContain("secret");
    expect(masked.split("\n")).toEqual([
      `CREATE ROLE r PASSWORD '${SECRET_MASK}'`,
      `ALTER USER u IDENTIFIED BY '${SECRET_MASK}'`,
      `bad value ${SECRET_MASK}`,
    ]);
  });

  it("leaves text without the secret, and secrets too short to mask, alone", () => {
    expect(maskSecrets("SELECT 1", ["hunter2"])).toBe("SELECT 1");
    expect(maskSecrets("SELECT 'ab'", ["ab"])).toBe("SELECT 'ab'");
    expect(maskSecrets("SELECT 'hunter2'", undefined)).toBe("SELECT 'hunter2'");
    expect(maskSecrets("SELECT 'hunter2'", [])).toBe("SELECT 'hunter2'");
  });

  it("masks the longest spelling first", () => {
    expect(maskSecrets("x it''s y", ["it's"])).toBe(`x ${SECRET_MASK} y`);
  });
});

describe("the twin ledger", () => {
  beforeEach(() => {
    resetTraceLedger();
    vi.useFakeTimers();
  });
  afterEach(() => vi.useRealTimers());

  it("pairs a statement logged by the call with the one drained from the trace, once", () => {
    noteLogged("c", "SELECT 1", "own");
    // The same source never pairs with itself, and a different statement does not pair.
    expect(takeTwin("c", "SELECT 1", "own")).toBe(false);
    expect(takeTwin("c", "SELECT 2", "trace")).toBe(false);
    expect(takeTwin("other", "SELECT 1", "trace")).toBe(false);
    expect(takeTwin("c", "SELECT 1", "trace")).toBe(true);
    // One twin cancels one entry only.
    expect(takeTwin("c", "SELECT 1", "trace")).toBe(false);
  });

  it("does not pair a statement that already had its twin, nor one that is old", () => {
    noteLogged("c", "SELECT 1", "own", true);
    expect(takeTwin("c", "SELECT 1", "trace")).toBe(false);
    noteLogged("c", "SELECT 3", "trace");
    vi.advanceTimersByTime(TWIN_WINDOW_MS + 1);
    expect(takeTwin("c", "SELECT 3", "own")).toBe(false);
  });

  it("knows whether any statement of the trace was logged since a moment", () => {
    const before = traceClock();
    expect(traceLoggedSince("c", before)).toBe(false);
    noteLogged("c", "SELECT 1", "own");
    expect(traceLoggedSince("c", before)).toBe(false);
    vi.advanceTimersByTime(5);
    noteLogged("c", "SELECT 2", "trace");
    expect(traceLoggedSince("c", before)).toBe(true);
    expect(traceLoggedSince("other", before)).toBe(false);
    expect(traceLoggedSince("c", traceClock() + 1)).toBe(false);
    // A statement that was reported but not logged again (its twin was) counts as seen.
    vi.advanceTimersByTime(5);
    const later = traceClock();
    markTraceSeen("d");
    expect(traceLoggedSince("d", later)).toBe(true);
  });
});

describe("statements that another call drained first", () => {
  const PAGE: BrowseRequest = {
    table: { schema: "public", name: "users" },
    columns: null,
    filter: null,
    sort: [],
    limit: 5,
    offset: 0,
  };
  let c = "";
  const entries = () => useConsoleStore.getState().entries;
  const restoreDrain = () =>
    registerMock("drain_sql_trace", ({ connectionId }) => drainMockTrace(connectionId as string));

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
    resetMockTrace();
    resetTraceLedger();
  });
  afterEach(restoreDrain);

  it("logs a call's own SELECT once when the trace twin was already logged elsewhere", async () => {
    const warm = await ipc.browse.rows(c, PAGE);
    useConsoleStore.getState().clear();
    // Another call drained (and logged) the page's SELECT before this call could: its drain is
    // empty, and the ledger knows the statement is already in the log.
    noteLogged(c, normalizeSql(warm.sql), "trace");
    registerMock("drain_sql_trace", () => []);
    await ipc.browse.rows(c, PAGE);
    expect(entries()).toEqual([]);
  });

  it("drops a drained statement that another call already logged as its own", async () => {
    const own = "SELECT c.relname::text FROM pg_class c WHERE fx3 = 1";
    noteLogged(c, normalizeSql(own), "own");
    registerMock("drain_sql_trace", () => [
      {
        sql: own,
        category: "structure",
        durationMs: 2,
        rows: 1,
        affected: null,
        error: null,
      },
      {
        sql: "SELECT 2 FROM pg_class",
        category: "structure",
        durationMs: 1,
        rows: null,
        affected: null,
        error: null,
      },
    ]);
    await ipc.schema.tables(c, "public");
    expect(entries().map((e) => e.sql)).toEqual(["SELECT 2 FROM pg_class"]);
  });

  it("logs the backend's failed statement with its error, and a failed drain changes nothing", async () => {
    registerMock("drain_sql_trace", () => [
      {
        sql: "SELECT * FROM pg_class WHERE broken",
        category: "structure",
        durationMs: 4,
        rows: null,
        affected: null,
        error: 'column "broken" does not exist',
      },
    ]);
    await ipc.schema.tables(c, "public");
    expect(entries()).toEqual([
      expect.objectContaining({
        sql: "SELECT * FROM pg_class WHERE broken",
        category: "structure",
        meta: false,
        error: 'column "broken" does not exist',
      }),
    ]);
    useConsoleStore.getState().clear();
    registerMock("drain_sql_trace", () => {
      throw { code: "INTERNAL", message: "boom" };
    });
    // No recorded SQL: the call is described instead, and still succeeds.
    await expect(ipc.schema.tables(c, "public")).resolves.toBeInstanceOf(Array);
    expect(entries().map((e) => [e.sql, e.meta])).toEqual([["-- list tables in public", true]]);
  });

  it("a quiet call logs none of the statements it ran, and still empties the trace", async () => {
    // Metrics Board refreshes run quietly: their SQL stays out of the console, and out of the
    // next call's log too.
    let drained = 0;
    registerMock("drain_sql_trace", () => {
      drained++;
      return [
        {
          sql: "SELECT count(*) FROM orders",
          category: "data",
          durationMs: 2,
          rows: 1,
          affected: null,
          error: null,
        },
      ];
    });
    await ipc.metrics.run(
      c,
      crypto.randomUUID(),
      { parts: ["SELECT 1"], values: [], allowWrites: false, maxRows: 10 },
      true,
    );
    expect(drained).toBe(1);
    expect(entries()).toEqual([]);
  });
});
