import { afterEach, describe, expect, it } from "vitest";
import type { ConnectionInfo } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import {
  appendCapped,
  CONSOLE_MAX_ENTRIES,
  filterEntries,
  useConsoleStore,
  type ConsoleEntry,
} from "./store";

const info = {
  connectionId: "c1",
  profileId: "p1",
  name: "Local Postgres",
  color: null,
  driver: "postgres",
  serverVersion: "17",
  currentDatabase: "tabler_dev",
  currentUser: "me",
  tunnel: null,
  readOnly: false,
} satisfies ConnectionInfo;

afterEach(() => {
  useConsoleStore.setState({ entries: [], filter: "all" });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
});

describe("appendCapped", () => {
  it("appends, and drops the oldest entries beyond the cap", () => {
    expect(appendCapped([1, 2], 3, 5)).toEqual([1, 2, 3]);
    expect(appendCapped([1, 2, 3], 4, 3)).toEqual([2, 3, 4]);
    // A list already over the cap (a lowered limit) is trimmed back to it.
    expect(appendCapped([1, 2, 3, 4, 5], 6, 3)).toEqual([4, 5, 6]);
  });

  it("returns a new array", () => {
    const before = [1];
    expect(appendCapped(before, 2)).not.toBe(before);
    expect(before).toEqual([1]);
  });
});

describe("console store", () => {
  it("stamps entries with an id, the time and the connection's name and database", () => {
    useConnectionsStore.getState().add(info);
    useConsoleStore
      .getState()
      .log({ connectionId: "c1", category: "data", sql: "SELECT 1", meta: false }, 42);
    useConsoleStore
      .getState()
      .log({ connectionId: "gone", category: "query", sql: "SELECT 2", meta: false });
    const [a, b] = useConsoleStore.getState().entries;
    expect(a).toMatchObject({
      at: 42,
      connectionName: "Local Postgres",
      database: "tabler_dev",
      category: "data",
      sql: "SELECT 1",
    });
    expect(b).toMatchObject({ connectionName: null, database: null, sql: "SELECT 2" });
    expect(b!.id).toBeGreaterThan(a!.id);
  });

  it(`keeps the newest ${CONSOLE_MAX_ENTRIES} entries`, () => {
    const { log } = useConsoleStore.getState();
    for (let i = 0; i < CONSOLE_MAX_ENTRIES + 5; i++)
      log({ connectionId: "c1", category: "data", sql: `SELECT ${i}`, meta: false });
    const { entries } = useConsoleStore.getState();
    expect(entries).toHaveLength(CONSOLE_MAX_ENTRIES);
    expect(entries[0]!.sql).toBe("SELECT 5");
    expect(entries.at(-1)!.sql).toBe(`SELECT ${CONSOLE_MAX_ENTRIES + 4}`);
  });

  it("clears and sets the filter", () => {
    const s = useConsoleStore.getState();
    s.log({ connectionId: "c1", category: "data", sql: "SELECT 1", meta: false });
    s.setFilter("query");
    expect(useConsoleStore.getState().filter).toBe("query");
    useConsoleStore.getState().clear();
    expect(useConsoleStore.getState().entries).toEqual([]);
  });
});

describe("filterEntries", () => {
  const entry = (id: number, category: ConsoleEntry["category"]): ConsoleEntry => ({
    id,
    at: 0,
    connectionId: "c1",
    connectionName: null,
    database: null,
    category,
    sql: "",
    meta: false,
  });
  const all = [entry(1, "data"), entry(2, "structure"), entry(3, "query"), entry(4, "data")];

  it("returns the same array for All", () => {
    expect(filterEntries(all, "all")).toBe(all);
  });

  it("keeps one category, in order", () => {
    expect(filterEntries(all, "data").map((e) => e.id)).toEqual([1, 4]);
    expect(filterEntries(all, "structure").map((e) => e.id)).toEqual([2]);
    expect(filterEntries(all, "query").map((e) => e.id)).toEqual([3]);
  });
});
