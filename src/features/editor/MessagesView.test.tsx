import { render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { QueryEvent } from "@/lib/ipc";
import { ResultStore } from "@/stores/resultData";
import { firstLine } from "./labels";
import { MessagesView, VIRTUALIZE_FROM } from "./MessagesView";
import { applyEvent, newRun, type RunState } from "./runModel";

// Counts row renders: every StatementRow render calls `firstLine` once.
vi.mock("./labels", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./labels")>();
  return { ...actual, firstLine: vi.fn(actual.firstLine) };
});
const rowRenders = vi.mocked(firstLine);

const sink = new ResultStore();

function started(index: number): QueryEvent {
  return {
    event: "started",
    data: {
      index,
      sql: `insert into t values (${index})`,
      span: { start: index * 26, end: index * 26 + 25, line: index + 1 },
    },
  };
}

function finished(index: number): QueryEvent {
  return {
    event: "finished",
    data: {
      index,
      result: {
        index,
        sql: `insert into t values (${index})`,
        span: { start: index * 26, end: index * 26 + 25, line: index + 1 },
        kind: "affected",
        columns: [],
        resultSets: 0,
        rowCount: 0,
        truncated: false,
        affected: 1,
        lastInsertId: null,
        startedAt: "2026-10-11T00:00:00Z",
        durationMs: 1,
        error: null,
        notices: [],
      },
    },
  };
}

function runOf(n: number): RunState {
  let run = newRun("r", "insert …", 0);
  for (let i = 0; i < n; i++)
    run = applyEvent(applyEvent(run, started(i), sink), finished(i), sink);
  return run;
}

beforeEach(() => {
  rowRenders.mockClear();
});

describe("MessagesView", () => {
  it("re-renders only the statements an event changed", () => {
    let run = runOf(50);
    const { rerender } = render(<MessagesView tabId="t-messages" run={run} />);
    expect(rowRenders).toHaveBeenCalledTimes(50);

    rowRenders.mockClear();
    run = applyEvent(run, started(50), sink);
    rerender(<MessagesView tabId="t-messages" run={run} />);
    expect(rowRenders).toHaveBeenCalledTimes(1);

    rowRenders.mockClear();
    run = applyEvent(run, finished(50), sink);
    rerender(<MessagesView tabId="t-messages" run={run} />);
    expect(rowRenders).toHaveBeenCalledTimes(1);

    // `done` leaves finished statements alone.
    rowRenders.mockClear();
    run = applyEvent(run, { event: "done", data: { totalMs: 60 } }, sink);
    rerender(<MessagesView tabId="t-messages" run={run} />);
    expect(rowRenders).toHaveBeenCalledTimes(0);
  });

  it("renders only the visible rows of a long script", () => {
    // jsdom has no layout: give the scroller (and every row) a size.
    const height = vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(300);
    const width = vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(600);
    try {
      const run = runOf(2000);
      const { container } = render(<MessagesView tabId="t-messages" run={run} />);
      const rows = container.querySelectorAll("li").length;
      expect(rows).toBeGreaterThan(0);
      expect(rows).toBeLessThan(100);
      expect(rowRenders.mock.calls.length).toBeLessThan(200);
      expect(container.querySelector("li")).toHaveTextContent("insert into t values (0)");
    } finally {
      height.mockRestore();
      width.mockRestore();
    }
  });

  it("renders every row of a short script", () => {
    const run = runOf(VIRTUALIZE_FROM - 1);
    const { container } = render(<MessagesView tabId="t-messages" run={run} />);
    expect(container.querySelectorAll("li")).toHaveLength(VIRTUALIZE_FROM - 1);
  });
});
