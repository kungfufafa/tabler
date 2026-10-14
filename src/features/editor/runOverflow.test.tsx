import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { QueryEvent, ResultColumn, StatementResult, Value } from "@/lib/ipc";
import { resultData, ResultStore } from "@/stores/resultData";
import { MessagesView } from "./MessagesView";
import { formatPlainResult, PLAIN_ROWS_MAX } from "./plainResult";
import { allResultSets, applyEvent, finalizeRun, MAX_RESULT_TABS, newRun } from "./runModel";

const col = (name: string): ResultColumn => ({
  name,
  dataType: "integer",
  kind: "int",
  nullable: null,
  table: null,
});

function result(index: number): StatementResult {
  return {
    index,
    sql: `select ${index}`,
    span: { start: 0, end: 8, line: 1 },
    kind: "rows",
    columns: [col("n")],
    resultSets: 1,
    rowCount: 1,
    truncated: false,
    affected: null,
    lastInsertId: null,
    startedAt: "2026-10-11T00:00:00Z",
    durationMs: 1,
    error: null,
    notices: [],
  };
}

/** `n` statements, each returning one row (`select i`). */
function events(n: number): QueryEvent[] {
  return Array.from({ length: n }, (_, index): QueryEvent[] => [
    {
      event: "started",
      data: { index, sql: `select ${index}`, span: { start: 0, end: 8, line: 1 } },
    },
    { event: "columns", data: { index, set: 0, columns: [col("n")] } },
    { event: "rows", data: { index, set: 0, rows: [[{ t: "int", v: index }]] } },
    { event: "finished", data: { index, result: result(index) } },
  ]).flat();
}

describe("very large result counts", () => {
  it("keeps the first 100 result sets as tabs and the rest as overflow", () => {
    const sink = new ResultStore();
    let run = newRun("r1", "…", 0);
    for (const e of events(130)) run = applyEvent(run, e, sink, 1000);
    run = applyEvent(run, { event: "done", data: { totalMs: 9 } }, sink, 1000);
    expect(run.results).toHaveLength(MAX_RESULT_TABS);
    expect(run.overflow).toHaveLength(30);
    expect(run.overflow!.every((r) => r.done)).toBe(true);
    expect(allResultSets(run)).toHaveLength(130);
    expect(sink.get(run.overflow![0]!.id)?.rows).toEqual([[{ t: "int", v: 100 }]]);
    // A run within the limit has no overflow at all.
    let small = newRun("r2", "…", 0);
    for (const e of events(3)) small = applyEvent(small, e, sink, 1000);
    expect(small.overflow).toBeUndefined();
    // Statements reconciled from the response (channel lagging) overflow the same way.
    const late = finalizeRun(
      { ...newRun("r3", "…", 0) },
      Array.from({ length: 102 }, (_, i) => result(i)),
      sink,
    );
    expect(late.results).toHaveLength(100);
    expect(late.overflow).toHaveLength(2);
  });

  it("formats a result set as a plain-text table", () => {
    const rows: Value[][] = [
      [
        { t: "int", v: 1 },
        { t: "text", v: "alice" },
      ],
      [{ t: "int", v: 22 }, { t: "null" }],
    ];
    expect(formatPlainResult(["id", "name"], rows)).toBe(
      [" id | name", "----+-------", " 1  | alice", " 22 | NULL", "(2 rows)"].join("\n"),
    );
    const many = Array.from({ length: PLAIN_ROWS_MAX + 5 }, (_, i): Value[] => [
      { t: "int", v: i },
    ]);
    const text = formatPlainResult(["n"], many, { truncated: true });
    expect(text.split("\n")).toHaveLength(PLAIN_ROWS_MAX + 3);
    expect(text).toMatch(/\(55 rows, 5 not shown, row limit reached\)$/);
  });

  it("lists the overflow under its statements in Messages", () => {
    let run = newRun("r-msg", "…", 0);
    for (const e of events(MAX_RESULT_TABS + 2)) run = applyEvent(run, e, resultData, 1000);
    run = applyEvent(run, { event: "done", data: { totalMs: 9 } }, resultData, 1000);
    render(<MessagesView tabId="t-msg" run={run} />);
    expect(
      screen.getByText(/100 result sets have tabs; the other 2 are listed here as text/),
    ).toBeTruthy();
    const blocks = document.querySelectorAll("[data-slot=plain-result]");
    expect(blocks).toHaveLength(2);
    expect(blocks[0]!.textContent).toContain(" 100");
    expect(blocks[1]!.textContent).toContain("(1 row)");
  });
});
