import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { seedEditor } from "@/features/editor/editorStates";
import { emptyStaging, insertRow } from "@/features/staging/model";
import type { ConnectionInfo } from "@/lib/ipc";
import { mockOpenedWindows } from "@/lib/ipc/mock/windows";
import { useConnectionsStore, type OpenConnection } from "@/stores/connections";
import { updateQueryTab, useQueryTabsStore } from "@/stores/queryTabs";
import { newFilterRow, useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore, type Tab } from "@/stores/tabs";
import {
  buildHandoff,
  handoffTab,
  handoffWorkspace,
  moveRefusal,
  moveTabToNewWindow,
  openNewWindow,
  openProfileInNewWindow,
} from "./windowHandoff";

const toast = vi.hoisted(() => ({ error: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

const info: ConnectionInfo = {
  connectionId: "c1",
  profileId: "p1",
  name: "Local PG",
  color: null,
  driver: "postgres",
  serverVersion: "PostgreSQL 17",
  currentDatabase: "shop",
  currentUser: "me",
  tunnel: null,
  readOnly: false,
};
const conn: OpenConnection = { info, status: "connected", message: null, currentSchema: "sales" };
const users = { schema: "public", name: "users" };

beforeEach(() => {
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null, splits: {} });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTableTabsStore.setState({ slices: {} });
  useQueryTabsStore.setState({ slices: {} });
  mockOpenedWindows.splice(0);
});
afterEach(() => toast.error.mockReset());

describe("handoff payloads", () => {
  it("carry the tab's connection (profile, database, schema) and the tab in session form", () => {
    const table: Tab = { id: "t", kind: "structure", connectionId: "c1", table: users, title: "U" };
    expect(buildHandoff(conn, table)).toEqual({
      profileId: "p1",
      database: "shop",
      schema: "sales",
      tab: { kind: "table", table: users, view: "structure", title: "U" },
    });
    // SQLite has one database: the profile's.
    expect(buildHandoff({ ...conn, info: { ...info, driver: "sqlite" } }).database).toBeNull();
    expect(buildHandoff(conn).tab).toBeNull();
  });

  it("carry a table tab's filter bar (a Move Tab to New Window keeps its filters, not its staged edits)", () => {
    const a: Tab = { id: "t", kind: "table", connectionId: "c1", table: users, title: "users" };
    // No state yet: no filters in the payload.
    expect(handoffTab(a)).toEqual({ kind: "table", table: users, view: "table", title: "users" });
    const row = { ...newFilterRow("id", "eq", "5"), value2: "" };
    let staging = emptyStaging();
    staging = insertRow(staging, "new:0", []);
    useTableTabsStore.getState().update("t", {
      filters: [row],
      filtersVisible: true,
      conjunction: "or",
      staging,
    });
    const payload = handoffTab(a);
    expect(payload).toEqual({
      kind: "table",
      table: users,
      view: "table",
      title: "users",
      filters: [row],
      filtersVisible: true,
      conjunction: "or",
    });
    // A copy: the live rows are not shared with the payload.
    expect(payload.kind === "table" && payload.filters?.[0]).not.toBe(row);
    // Given rows win over the tab's own (a tab that is not open here: a link's); null: none.
    const given = { filters: [{ ...row, id: "x", value: "9" }], filtersVisible: false };
    expect(handoffTab(a, undefined, given)).toMatchObject({
      filters: [{ id: "x", value: "9" }],
      filtersVisible: false,
    });
    expect(handoffTab(a, undefined, null)).toEqual({
      kind: "table",
      table: users,
      view: "table",
      title: "users",
    });
    // And the new window reads it back.
    const ws = handoffWorkspace({ profileId: "p1", database: null, schema: null, tab: payload });
    expect(ws?.tabs[0]).toMatchObject({ kind: "table", filters: [{ id: row.id, value: "5" }] });
  });

  it("move a query's text, caret and file; a clean file tab reopens from its file", () => {
    const q: Tab = { id: "q", kind: "query", connectionId: "c1", title: "Query 3", dirty: true };
    expect(handoffTab(q, { text: "select 1", cursor: 99, filePath: null })).toEqual({
      kind: "query",
      title: "Query 3",
      sql: "select 1",
      filePath: null,
      cursor: 8,
    });
    const clean: Tab = { ...q, dirty: false };
    expect(handoffTab(clean, { text: "x", cursor: 0, filePath: "/a.sql" })).toMatchObject({
      sql: null,
      filePath: "/a.sql",
    });
    const tool: Tab = {
      id: "o",
      kind: "tool",
      connectionId: "c1",
      tool: "overview",
      title: "Overview: public",
      params: { schema: "public" },
    };
    expect(handoffTab(tool)).toEqual({
      kind: "tool",
      tool: "overview",
      title: "Overview: public",
      params: { schema: "public" },
    });
  });

  it("are read back as a one-tab workspace (nothing for an empty window)", () => {
    const sent = buildHandoff(conn, {
      id: "t",
      kind: "table",
      connectionId: "c1",
      table: users,
      title: "users",
    });
    expect(handoffWorkspace(JSON.parse(JSON.stringify(sent)))).toEqual({
      profileId: "p1",
      open: true,
      database: "shop",
      schema: "sales",
      tabs: [{ kind: "table", table: users, view: "table", title: "users" }],
      activeTab: 0,
    });
    expect(handoffWorkspace(null)).toBeNull();
    expect(
      handoffWorkspace({ profileId: null, database: null, schema: null, tab: null }),
    ).toBeNull();
    expect(
      handoffWorkspace({ profileId: "p1", database: null, schema: null, tab: { kind: "junk" } }),
    ).toMatchObject({ tabs: [], activeTab: null });
  });
});

describe("moving a tab to a new window", () => {
  beforeEach(() => useConnectionsStore.getState().add(info));

  it("hands the query over and closes it here", async () => {
    const id = useTabsStore.getState().newQuery("c1");
    seedEditor(id, "select 42;");
    updateQueryTab(id, { filePath: null });
    useTabsStore.getState().setDirty(id, true);
    expect(await moveTabToNewWindow(id)).toBe(true);
    expect(mockOpenedWindows).toHaveLength(1);
    expect(mockOpenedWindows[0]!.handoff).toMatchObject({
      profileId: "p1",
      database: "shop",
      tab: { kind: "query", sql: "select 42;" },
    });
    // No "Discard changes?": the text went along.
    expect(useTabsStore.getState().tabs).toEqual([]);
    expect(useTabsStore.getState().closeRequest).toBeNull();
  });

  it("moves a table tab with its filters", async () => {
    const id = useTabsStore.getState().openTable("c1", users);
    const row = newFilterRow("id", "eq", "5");
    useTableTabsStore.getState().update(id, { filters: [row], filtersVisible: true });
    await expect(moveTabToNewWindow(id)).resolves.toBe(true);
    expect(mockOpenedWindows[0]?.handoff?.tab).toMatchObject({
      kind: "table",
      filters: [{ id: row.id, column: "id", op: "eq", value: "5", enabled: true }],
      filtersVisible: true,
    });
    expect(useTabsStore.getState().tabs).toEqual([]);
  });

  it("refuses a tab with staged changes", async () => {
    const id = useTabsStore.getState().openTable("c1", users);
    useTableTabsStore.getState().update(id, { staging: insertRow(emptyStaging(), "new:0", []) });
    expect(moveRefusal(useTabsStore.getState().tabs[0]!)).toMatch(/1 uncommitted change/);
    expect(await moveTabToNewWindow()).toBe(false);
    expect(toast.error).toHaveBeenCalledTimes(1);
    expect(mockOpenedWindows).toHaveLength(0);
    expect(useTabsStore.getState().tabs).toHaveLength(1);
  });

  it("opens empty windows and connections in new windows", async () => {
    expect(await openNewWindow()).toBe(true);
    expect(await openProfileInNewWindow("p1")).toBe(true);
    expect(mockOpenedWindows.map((w) => w.handoff)).toEqual([
      null,
      { profileId: "p1", database: null, schema: null, tab: null },
    ]);
  });
});
