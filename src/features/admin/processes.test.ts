import { describe, expect, it } from "vitest";
import type { ProcessRow } from "@/lib/ipc";
import {
  cellText,
  clickSelection,
  filterProcesses,
  formatDuration,
  isIdle,
  killSummary,
  processColumns,
  processDatabases,
  sortProcesses,
} from "./processes";

function row(id: number, patch: Partial<ProcessRow> = {}): ProcessRow {
  return {
    id,
    user: "app",
    database: "shop",
    application: null,
    client: null,
    state: "active",
    command: null,
    startedAt: null,
    durationMs: 1000,
    waitEvent: null,
    query: "select 1",
    current: false,
    own: false,
    ...patch,
  };
}

describe("process list", () => {
  it("uses each server's own columns", () => {
    expect(processColumns("postgres").map((c) => c.label)).toEqual([
      "pid",
      "User",
      "Database",
      "Application",
      "Client",
      "State",
      "Started",
      "Wait event",
      "Query",
    ]);
    expect(processColumns("mysql").map((c) => c.label)).toEqual([
      "Id",
      "User",
      "Host",
      "db",
      "Command",
      "Time",
      "State",
      "Info",
    ]);
  });

  it("sorts by any column, empty values last, ties by id", () => {
    const rows = [
      row(3, { durationMs: 50, user: "b" }),
      row(1, { durationMs: null, user: null }),
      row(2, { durationMs: 900, user: "a" }),
    ];
    expect(sortProcesses(rows, { column: "duration", desc: true }).map((r) => r.id)).toEqual([
      2, 3, 1,
    ]);
    expect(sortProcesses(rows, { column: "duration", desc: false }).map((r) => r.id)).toEqual([
      3, 2, 1,
    ]);
    expect(sortProcesses(rows, { column: "user", desc: false }).map((r) => r.id)).toEqual([
      2, 3, 1,
    ]);
    expect(sortProcesses(rows, { column: "id", desc: true }).map((r) => r.id)).toEqual([3, 2, 1]);
    // Numeric-aware text order.
    const named = [row(1, { client: "10.0.0.10" }), row(2, { client: "10.0.0.9" })];
    expect(sortProcesses(named, { column: "client", desc: false }).map((r) => r.id)).toEqual([
      2, 1,
    ]);
  });

  it("filters by database and hides idle sessions of either server", () => {
    const rows = [
      row(1, { state: "idle" }),
      row(2, { state: "idle in transaction" }),
      row(3, { state: null, command: "Sleep", database: "sys" }),
      row(4, { database: null }),
    ];
    expect(isIdle(rows[0]!)).toBe(true);
    expect(isIdle(rows[1]!)).toBe(false);
    expect(filterProcesses(rows, { database: null, hideIdle: true }).map((r) => r.id)).toEqual([
      2, 4,
    ]);
    expect(filterProcesses(rows, { database: "sys", hideIdle: false }).map((r) => r.id)).toEqual([
      3,
    ]);
    expect(processDatabases(rows)).toEqual(["shop", "sys"]);
  });

  it("formats durations and cells", () => {
    expect(formatDuration(null)).toBe("");
    expect(formatDuration(850)).toBe("850 ms");
    expect(formatDuration(12_400)).toBe("12 s");
    expect(formatDuration(184_000)).toBe("3 min 4 s");
    expect(formatDuration(120_000)).toBe("2 min");
    expect(formatDuration(7_500_000)).toBe("2 h 5 min");
    expect(formatDuration(3 * 86_400_000 + 4 * 3_600_000)).toBe("3 d 4 h");
    const r = row(7, { query: "select *\n  from t", waitEvent: "Lock: tuple" });
    expect(cellText(r, "query")).toBe("select * from t");
    expect(cellText(r, "id")).toBe("7");
    expect(cellText(r, "waitEvent")).toBe("Lock: tuple");
    expect(cellText(row(8, { client: null }), "client")).toBe("");
  });

  it("selects with click, ⌘-click and ⇧-click", () => {
    const ids = [1, 2, 3, 4];
    const none = new Set<number>();
    expect([...clickSelection(ids, none, null, 2, { toggle: false, range: false })]).toEqual([2]);
    expect([...clickSelection(ids, new Set([2]), 2, 4, { toggle: true, range: false })]).toEqual([
      2, 4,
    ]);
    expect([...clickSelection(ids, new Set([2, 4]), 4, 2, { toggle: true, range: false })]).toEqual(
      [4],
    );
    expect([...clickSelection(ids, new Set([3]), 3, 1, { toggle: false, range: true })]).toEqual([
      1, 2, 3,
    ]);
    // No anchor: a ⇧-click selects only that row.
    expect([...clickSelection(ids, none, null, 3, { toggle: false, range: true })]).toEqual([3]);
  });

  it("sums up kills", () => {
    expect(killSummary([{ id: 1, ok: true, message: null }], "query").title).toBe(
      "Cancelled the query of 1 process",
    );
    const mixed = killSummary(
      [
        { id: 1, ok: true, message: null },
        { id: 2, ok: false, message: "permission denied" },
      ],
      "connection",
    );
    expect(mixed.title).toBe("Killed the connection of 1 process, 1 failed");
    expect(mixed.failed).toEqual([{ id: 2, message: "permission denied" }]);
    expect(killSummary([{ id: 3, ok: false, message: null }], "query").title).toBe(
      "Could not kill 1 process",
    );
  });
});
