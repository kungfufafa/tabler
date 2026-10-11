import { afterEach, describe, expect, it, vi } from "vitest";
import type { ErrorPayload, QueryEvent, ResultColumn, StatementResult, Value } from "@/lib/ipc";
import {
  dropResults,
  flushResultVersions,
  resultData,
  ResultStore,
  useResultVersions,
} from "@/stores/resultData";
import { clampRange, errorRange, runDiagnostics, spanRange, tokenEnd } from "./errorPos";
import { runStatusText } from "./labels";
import {
  applyEvent,
  failRun,
  finalizeRun,
  firstError,
  newRun,
  normalizeError,
  resultId,
  summarize,
  type RunState,
} from "./runModel";

const col = (name: string): ResultColumn => ({
  name,
  dataType: "integer",
  kind: "int",
  nullable: null,
  table: null,
});
const rows = (n: number, from = 0): Value[][] =>
  Array.from({ length: n }, (_, i) => [{ t: "int", v: from + i }]);

function result(index: number, patch: Partial<StatementResult> = {}): StatementResult {
  return {
    index,
    sql: `select ${index}`,
    span: { start: 0, end: 8, line: 1 },
    kind: "rows",
    columns: [col("a")],
    resultSets: 1,
    rowCount: 0,
    truncated: false,
    affected: null,
    lastInsertId: null,
    startedAt: "2026-10-11T00:00:00Z",
    durationMs: 7,
    error: null,
    notices: [],
    ...patch,
  };
}

function error(patch: Partial<ErrorPayload> = {}): ErrorPayload {
  return normalizeError({ code: "DB_ERROR", message: "boom", ...patch });
}

function fold(run: RunState, events: QueryEvent[], sink = new ResultStore()) {
  return { run: events.reduce((r, e) => applyEvent(r, e, sink, 1000), run), sink };
}

describe("applyEvent", () => {
  it("streams one result set into the buffer and finishes", () => {
    const { run, sink } = fold(newRun("q", "select 1", 0, 0), [
      {
        event: "started",
        data: { index: 0, sql: "select 1", span: { start: 0, end: 8, line: 1 } },
      },
      { event: "columns", data: { index: 0, set: 0, columns: [col("a")] } },
      { event: "rows", data: { index: 0, set: 0, rows: rows(500) } },
      { event: "rows", data: { index: 0, set: 0, rows: rows(204, 500) } },
      { event: "finished", data: { index: 0, result: result(0, { rowCount: 704 }) } },
      { event: "done", data: { totalMs: 23 } },
    ]);
    const id = resultId("q", 0, 0);
    expect(run.results).toEqual([
      {
        id,
        statementIndex: 0,
        set: 0,
        columns: [col("a")],
        done: true,
        truncated: false,
        durationMs: 7,
      },
    ]);
    expect(sink.rowCount(id)).toBe(704);
    expect(sink.get(id)?.rows[703]).toEqual([{ t: "int", v: 703 }]);
    expect(run.statements[0]?.status).toBe("ok");
    expect(run.status).toBe("done");
    expect(run.totalMs).toBe(23);
    expect(run.finishedAt).toBe(1000);
  });

  it("returns the same object for row chunks of a known set", () => {
    const sink = new ResultStore();
    const base = fold(newRun("q", "", 0), [
      { event: "columns", data: { index: 0, set: 0, columns: [col("a")] } },
    ]).run;
    const after = applyEvent(
      base,
      { event: "rows", data: { index: 0, set: 0, rows: rows(3) } },
      sink,
    );
    expect(after).toBe(base);
  });

  it("keeps several result sets per statement apart and flags truncation on the last", () => {
    const { run, sink } = fold(newRun("q", "call p()", 0), [
      {
        event: "started",
        data: { index: 0, sql: "call p()", span: { start: 0, end: 8, line: 1 } },
      },
      { event: "columns", data: { index: 0, set: 0, columns: [col("a")] } },
      { event: "rows", data: { index: 0, set: 0, rows: rows(2) } },
      { event: "columns", data: { index: 0, set: 1, columns: [col("b"), col("c")] } },
      { event: "rows", data: { index: 0, set: 1, rows: rows(10) } },
      {
        event: "finished",
        data: { index: 0, result: result(0, { resultSets: 2, rowCount: 12, truncated: true }) },
      },
    ]);
    expect(run.results.map((r) => [r.id, r.truncated, r.done])).toEqual([
      ["q:0:0", false, true],
      ["q:0:1", true, true],
    ]);
    expect(sink.get("q:0:1")?.columns.map((c) => c.name)).toEqual(["b", "c"]);
    expect(sink.rowCount("q:0:0")).toBe(2);
  });

  it("creates an empty result tab for a row statement without row events", () => {
    const { run, sink } = fold(newRun("q", "", 0), [
      {
        event: "started",
        data: { index: 0, sql: "select 1 where false", span: { start: 0, end: 20, line: 1 } },
      },
      { event: "finished", data: { index: 0, result: result(0) } },
    ]);
    expect(run.results).toHaveLength(1);
    expect(sink.get("q:0:0")?.columns).toEqual([col("a")]);
  });

  it("does not create result tabs for affected / empty statements", () => {
    const { run } = fold(newRun("q", "", 0), [
      {
        event: "finished",
        data: { index: 0, result: result(0, { kind: "affected", affected: 3, columns: [] }) },
      },
    ]);
    expect(run.results).toEqual([]);
    expect(summarize(run).affected).toBe(3);
  });

  it("records failures, cancellations and the final status", () => {
    const { run } = fold(newRun("q", "select 1; selec 2; select pg_sleep(9)", 0), [
      {
        event: "started",
        data: { index: 0, sql: "select 1", span: { start: 0, end: 8, line: 1 } },
      },
      { event: "finished", data: { index: 0, result: result(0) } },
      {
        event: "started",
        data: { index: 1, sql: "selec 2", span: { start: 10, end: 17, line: 1 } },
      },
      {
        event: "failed",
        data: { index: 1, error: error({ position: { offset: 10, line: 1, column: 11 } }) },
      },
      {
        event: "started",
        data: { index: 2, sql: "select pg_sleep(9)", span: { start: 19, end: 37, line: 1 } },
      },
      {
        event: "failed",
        data: { index: 2, error: error({ code: "CANCELLED", message: "canceled" }) },
      },
      { event: "done", data: { totalMs: 900 } },
    ]);
    expect(run.statements.map((s) => s.status)).toEqual(["ok", "error", "cancelled"]);
    expect(run.status).toBe("cancelled");
    expect(summarize(run)).toMatchObject({ statements: 3, errors: 1, cancelled: true });
    expect(firstError(run)?.statement?.index).toBe(1);
  });
});

describe("failRun / finalizeRun", () => {
  it("marks a rejected command and its running statement", () => {
    const started = fold(newRun("q", "select 1", 0, 0), [
      {
        event: "started",
        data: { index: 0, sql: "select 1", span: { start: 0, end: 8, line: 1 } },
      },
    ]).run;
    const run = failRun(started, error({ code: "NOT_CONNECTED", message: "not connected" }), 50);
    expect(run.status).toBe("error");
    expect(run.statements[0]?.status).toBe("error");
    expect(run.error?.code).toBe("NOT_CONNECTED");
    expect(run.totalMs).toBe(50);
    expect(failRun(run, error(), 60)).toBe(run);
  });

  it("treats a CANCELLED rejection as a cancellation", () => {
    const run = failRun(newRun("q", "x", 0, 0), error({ code: "CANCELLED", message: "c" }), 5);
    expect(run.status).toBe("cancelled");
    expect(firstError(run)).toBeNull();
  });

  it("reconciles summaries when `done` never arrived", () => {
    const sink = new ResultStore();
    const run = finalizeRun(
      newRun("q", "select 1; delete from t", 0, 0),
      [result(0), result(1, { kind: "affected", affected: 2, columns: [] })],
      sink,
      40,
    );
    expect(run.status).toBe("done");
    expect(run.statements.map((s) => s.status)).toEqual(["ok", "ok"]);
    expect(run.results.map((r) => r.id)).toEqual(["q:0:0"]);
    expect(run.totalMs).toBe(40);
  });
});

describe("resultData", () => {
  afterEach(() => {
    dropResults(() => true);
    vi.useRealTimers();
  });

  it("bumps versions at most once per frame and drops buffers", () => {
    resultData.setColumns("x:0:0", [col("a")]);
    resultData.append("x:0:0", rows(500));
    resultData.append("x:0:0", rows(500));
    expect(resultData.rowCount("x:0:0")).toBe(1000);
    flushResultVersions();
    expect(useResultVersions.getState().versions["x:0:0"]).toBe(1);
    flushResultVersions();
    expect(useResultVersions.getState().versions["x:0:0"]).toBe(1);
    dropResults((id) => id.startsWith("x:"));
    expect(resultData.has("x:0:0")).toBe(false);
    expect(useResultVersions.getState().versions["x:0:0"]).toBeUndefined();
  });
});

describe("error positions", () => {
  const sql = "select 1;\nselec 2 from t";

  it("maps a position to the offending token, shifted by the selection offset", () => {
    const pos = { offset: 10, line: 2, column: 1 };
    expect(errorRange({ position: pos }, { sql, baseOffset: 0 })).toEqual({ from: 10, to: 15 });
    // Same text submitted as a selection starting at offset 42 of the document.
    expect(errorRange({ position: pos }, { sql, baseOffset: 42 })).toEqual({ from: 52, to: 57 });
  });

  it("converts character offsets past astral characters", () => {
    const text = "select '🙂', x frm t";
    // `frm` is character 14 but UTF-16 index 15.
    expect(
      errorRange({ position: { offset: 14, line: 1, column: 15 } }, { sql: text, baseOffset: 3 }),
    ).toEqual({
      from: 18,
      to: 21,
    });
  });

  it("falls back to the statement span without a position", () => {
    const span = { start: 10, end: 24, line: 2 };
    expect(errorRange({ position: null }, { sql, baseOffset: 5 }, span)).toEqual({
      from: 15,
      to: 29,
    });
    expect(spanRange(span, { sql, baseOffset: 0 })).toEqual({ from: 10, to: 24 });
    expect(errorRange({ position: null }, { sql, baseOffset: 0 })).toBeNull();
  });

  it("covers quoted tokens and single characters", () => {
    expect(tokenEnd(`a "x y" b`, 2)).toBe(7);
    expect(tokenEnd("a + b", 2)).toBe(3);
    expect(tokenEnd("abc", 3)).toBe(3);
    expect(clampRange({ from: 5, to: 50 }, 10)).toEqual({ from: 5, to: 10 });
  });

  it("builds lint diagnostics for failed statements only", () => {
    const run: RunState = {
      ...newRun("q", sql, 100),
      statements: [
        {
          index: 1,
          sql: "selec 2 from t",
          span: { start: 10, end: 24, line: 2 },
          status: "error",
          result: null,
          error: error({
            message: 'syntax error at or near "selec"',
            sqlstate: "42601",
            position: { offset: 10, line: 2, column: 1 },
          }),
        },
        {
          index: 2,
          sql: "select pg_sleep(1)",
          span: { start: 0, end: 1, line: 1 },
          status: "cancelled",
          result: null,
          error: error({ code: "CANCELLED" }),
        },
      ],
    };
    expect(runDiagnostics(run, 1000)).toEqual([
      {
        from: 110,
        to: 115,
        severity: "error",
        source: "SQLSTATE 42601",
        message: 'syntax error at or near "selec"',
      },
    ]);
  });
});

describe("Stop and the run outcome", () => {
  const started = (index: number): QueryEvent => ({
    event: "started",
    data: { index, sql: `update t set a = ${index}`, span: { start: 0, end: 5, line: 1 } },
  });
  const finished = (index: number): QueryEvent => ({
    event: "finished",
    data: { index, result: result(index, { kind: "affected", affected: 3, columns: [] }) },
  });

  it("keeps a run that finished before Stop took effect successful", () => {
    const sink = new ResultStore();
    const ran = fold(newRun("q", "update t set a = 0", 0), [started(0), finished(0)], sink).run;
    // Stop pressed while the trailing events were still being delivered.
    const stopping = { ...ran, cancelling: true };
    const run = applyEvent(stopping, { event: "done", data: { totalMs: 5 } }, sink, 1000);
    expect(run.status).toBe("done");
    expect(run.cancelling).toBe(false);
    expect(run.statements.map((s) => s.status)).toEqual(["ok"]);
    expect(runStatusText(summarize(run))).toBe("1 statement · 5 ms");
    // Same through the reconciliation path.
    const reconciled = finalizeRun({ ...stopping }, [], sink, 1000);
    expect(reconciled.status).toBe("done");
  });

  it("counts a statement that never reported back after Stop as cancelled", () => {
    const sink = new ResultStore();
    const ran = fold(newRun("q", "x", 0), [started(0), finished(0), started(1)], sink).run;
    const run = applyEvent(
      { ...ran, cancelling: true },
      { event: "done", data: { totalMs: 9 } },
      sink,
    );
    expect(run.statements.map((s) => s.status)).toEqual(["ok", "cancelled"]);
    expect(run.status).toBe("cancelled");
    // Without Stop, such a statement is taken as done.
    const plain = applyEvent(ran, { event: "done", data: { totalMs: 9 } }, sink);
    expect(plain.statements.map((s) => s.status)).toEqual(["ok", "ok"]);
    expect(plain.status).toBe("done");
  });

  it("keeps the identity of statements an event does not touch", () => {
    const sink = new ResultStore();
    const a = fold(newRun("q", "x", 0), [started(0), finished(0)], sink).run;
    const b = applyEvent(a, started(1), sink);
    expect(b.statements[0]).toBe(a.statements[0]);
    expect(b.results).toBe(a.results);
    const c = applyEvent(b, finished(1), sink);
    expect(c.statements[0]).toBe(a.statements[0]);
    expect(c.statements.map((s) => s.index)).toEqual([0, 1]);
    // Out-of-order arrival still ends up sorted.
    const d = applyEvent(applyEvent(newRun("q", "x", 0), started(2), sink), started(1), sink);
    expect(d.statements.map((s) => s.index)).toEqual([1, 2]);
  });
});

describe("runDiagnostics with edits after submit", () => {
  it("maps or drops each range through `locate`", () => {
    const run: RunState = {
      ...newRun("q", "select 1;\nselec 2", 0),
      statements: [
        {
          index: 1,
          sql: "selec 2",
          span: { start: 10, end: 17, line: 2 },
          status: "error",
          result: null,
          error: error({ position: { offset: 10, line: 2, column: 1 } }),
        },
      ],
    };
    const shifted = runDiagnostics(run, 100, (r) => ({ from: r.from + 8, to: r.to + 8 }));
    expect(shifted.map((d) => [d.from, d.to])).toEqual([[18, 23]]);
    expect(runDiagnostics(run, 100, () => null)).toEqual([]);
  });
});
