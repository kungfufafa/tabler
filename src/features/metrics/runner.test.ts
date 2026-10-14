import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { guardExecution } from "@/features/safety/guard";
import { ipc, type MetricsResult } from "@/lib/ipc";
import {
  AUTOMATIC_WRITE_REFUSED,
  BoardQueryError,
  MAX_ROWS,
  runBoardQuery,
  WRITE_REFUSED,
  type BoardQuery,
} from "./runner";

vi.mock("@/features/safety/guard", () => ({ guardExecution: vi.fn() }));

const result: MetricsResult = {
  columns: [],
  rows: [],
  truncated: false,
  affected: null,
  durationMs: 1,
  sql: "",
};

const q = (patch: Partial<BoardQuery>): BoardQuery => ({
  connectionId: "c1",
  dialect: "postgres",
  sql: "SELECT 1",
  values: {},
  allowWrites: false,
  queryId: "metrics:o1:1",
  ...patch,
});

describe("runBoardQuery", () => {
  let run: ReturnType<typeof vi.spyOn>;
  beforeEach(() => {
    run = vi.spyOn(ipc.metrics, "run").mockResolvedValue(result);
    vi.mocked(guardExecution).mockReset();
  });
  afterEach(() => run.mockRestore());

  it("runs a read with its variables bound, without a safe-mode prompt", async () => {
    await runBoardQuery(
      q({
        sql: "SELECT * FROM t WHERE id = {{id}} AND role = {{role}}",
        values: { id: { t: "int", v: 7 }, role: { t: "text", v: "admin" } },
        quiet: true,
      }),
    );
    expect(guardExecution).not.toHaveBeenCalled();
    expect(run).toHaveBeenCalledWith(
      "c1",
      "metrics:o1:1",
      {
        parts: ["SELECT * FROM t WHERE id = ", " AND role = ", ""],
        values: [
          { t: "int", v: 7 },
          { t: "text", v: "admin" },
        ],
        allowWrites: false,
        maxRows: MAX_ROWS,
      },
      true,
    );
  });

  it("refuses a write unless the object allows data changes", async () => {
    for (const sql of [
      "DELETE FROM t WHERE id = {{id}}",
      "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d",
      "SELECT * INTO copy FROM t",
    ]) {
      const err = await runBoardQuery(q({ sql, values: { id: { t: "int", v: 1 } } })).catch(
        (e: unknown) => e,
      );
      expect(err).toBeInstanceOf(BoardQueryError);
      expect((err as BoardQueryError).message).toBe(WRITE_REFUSED);
    }
    expect(run).not.toHaveBeenCalled();
    expect(guardExecution).not.toHaveBeenCalled();
  });

  it("sends an allowed write through the safe-mode guard first", async () => {
    vi.mocked(guardExecution).mockResolvedValue(true);
    await runBoardQuery(q({ sql: "UPDATE t SET a = 1", allowWrites: true, dialect: "mysql" }));
    expect(guardExecution).toHaveBeenCalledWith("c1", {
      kind: "query",
      statements: ["UPDATE t SET a = 1"],
    });
    expect(run).toHaveBeenCalledTimes(1);
    expect(run.mock.calls[0]![2]).toMatchObject({ allowWrites: true });

    vi.mocked(guardExecution).mockResolvedValue(false);
    run.mockClear();
    await expect(
      runBoardQuery(q({ sql: "UPDATE t SET a = 1", allowWrites: true })),
    ).rejects.toMatchObject({ reason: "declined" });
    expect(run).not.toHaveBeenCalled();
  });

  it("never runs a write by itself: automatic runs refuse it before the guard and the server", async () => {
    vi.mocked(guardExecution).mockResolvedValue(true);
    for (const [sql, dialect] of [
      ["DELETE FROM users", "postgres"],
      ["UPDATE t SET a = 1", "mysql"],
      ["DROP SCHEMA public CASCADE", "postgres"],
      ["WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d", "postgres"],
    ] as const) {
      const err = await runBoardQuery(
        q({ sql, dialect, allowWrites: true, automatic: true, quiet: true }),
      ).catch((e: unknown) => e);
      expect(err).toBeInstanceOf(BoardQueryError);
      expect((err as BoardQueryError).reason).toBe("refused");
      expect((err as BoardQueryError).message).toBe(AUTOMATIC_WRITE_REFUSED);
    }
    expect(run).not.toHaveBeenCalled();
    expect(guardExecution).not.toHaveBeenCalled();
    // Without "Allow data changes" the first refusal still applies.
    await expect(
      runBoardQuery(q({ sql: "DELETE FROM t", allowWrites: false, automatic: true })),
    ).rejects.toMatchObject({ message: WRITE_REFUSED });
  });

  it("runs the same write when the user asked for it, through the guard", async () => {
    vi.mocked(guardExecution).mockResolvedValue(true);
    for (const automatic of [false, undefined]) {
      run.mockClear();
      vi.mocked(guardExecution).mockClear();
      await runBoardQuery(q({ sql: "DELETE FROM users", allowWrites: true, automatic }));
      expect(guardExecution).toHaveBeenCalledOnce();
      expect(run).toHaveBeenCalledOnce();
      expect(run.mock.calls[0]![2]).toMatchObject({ allowWrites: true });
    }
  });

  it("leaves reads alone when the run is automatic", async () => {
    await runBoardQuery(q({ sql: "SELECT count(*) FROM users", automatic: true, quiet: true }));
    expect(run).toHaveBeenCalledOnce();
    expect(guardExecution).not.toHaveBeenCalled();
  });

  it("sends the object's query timeout along", async () => {
    await runBoardQuery(q({ timeoutSecs: 90 }));
    expect(run.mock.calls[0]![2]).toMatchObject({ timeoutSecs: 90 });
    run.mockClear();
    await runBoardQuery(q({}));
    expect(run.mock.calls[0]![2]).not.toHaveProperty("timeoutSecs");
  });

  it("refuses several statements and unknown variables", async () => {
    await expect(runBoardQuery(q({ sql: "SELECT 1; SELECT 2" }))).rejects.toMatchObject({
      reason: "statements",
    });
    await expect(runBoardQuery(q({ sql: "SELECT {{nope}}" }))).rejects.toMatchObject({
      reason: "variables",
      message: "No input or dropdown sets {{nope}}.",
    });
    expect(run).not.toHaveBeenCalled();
  });

  it("runs against the mock backend", async () => {
    run.mockRestore();
    const r = await runBoardQuery(
      q({ connectionId: "nope", sql: "SELECT count(*) AS users FROM users" }),
    ).catch((e: unknown) => e);
    // Not connected in the mock: the backend's error comes back.
    expect(r).toMatchObject({ code: "NOT_CONNECTED" });
  });
});
