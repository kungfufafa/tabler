import { QueryClient } from "@tanstack/react-query";
import { Gauge } from "lucide-react";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getEditorState } from "@/features/editor/editorStates";
import { usePaletteStore } from "@/features/palette/store";
import { connectWorkspace } from "@/features/sidebar/switchDatabase";
import { registerToolTab } from "@/features/tabs/toolTabs";
import { resetWindowLabelForTests } from "@/features/tabs/windowHandoff";
import { migrateConnection } from "@/hooks/useConnectionEvents";
import { ipc } from "@/lib/ipc";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import { mockEmit } from "@/lib/ipc/mock/transport";
import { mockSessionOwner } from "@/lib/ipc/mock/windows";
import { installActiveSync } from "./activeSync";
import { useConnectionsStore } from "./connections";
import { resetPrefsForTests, setPref } from "./prefs";
import { getQueryTab, useQueryTabsStore } from "./queryTabs";
import {
  parseSession,
  SESSION_KEY,
  SESSION_VERSION,
  type Session,
  type SessionWorkspace,
} from "./session";
import {
  currentSession,
  flushSession,
  installSessionPersistence,
  resetSessionForTests,
  setRemembered,
  survivesClose,
} from "./sessionPersist";
import {
  adoptSession,
  installSessionOwnership,
  openHandoff,
  resetSessionRestoreForTests,
  restoreAtStartup,
  restoreWorkspaceTabs,
  waitForProfileConnection,
} from "./sessionRestore";
import { newFilterRow, useTableTabsStore } from "./tableTabs";
import { useTabsStore, type Tab } from "./tabs";
import { useUiStore } from "./ui";

const users = { schema: "public", name: "users" };

function resetStores() {
  useTabsStore.setState({
    tabs: [],
    activeId: null,
    closed: [],
    queryCounter: 0,
    closeRequest: null,
    history: [],
    historyIndex: -1,
  });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useQueryTabsStore.setState({ slices: {} });
  useTableTabsStore.setState({ slices: {} });
  usePaletteStore.setState({ connecting: [] });
  useUiStore.setState({ view: "launcher" });
}

/** The app's open-profile flow, minus React Query: connect, register, show the workspace. */
function openProfile(profileId: string): void {
  if (!usePaletteStore.getState().beginConnect(profileId)) return;
  ipc.connections
    .connect(profileId)
    .then((info) => {
      useConnectionsStore.getState().add(info);
      useUiStore.getState().setView("workspace");
    })
    .catch(() => undefined)
    .finally(() => usePaletteStore.getState().endConnect(profileId));
}

async function connect(profileId: string): Promise<string> {
  const info = await ipc.connections.connect(profileId);
  useConnectionsStore.getState().add(info);
  return info.connectionId;
}

const workspace = (over: Partial<SessionWorkspace> = {}): SessionWorkspace => ({
  profileId: "p-local-pg",
  open: true,
  database: null,
  schema: null,
  tabs: [],
  activeTab: null,
  ...over,
});

const ofConnection = (connectionId: string) =>
  useTabsStore.getState().tabs.filter((t) => t.connectionId === connectionId);

let uninstallSync: () => void;

beforeEach(() => {
  resetStores();
  resetPrefsForTests();
  resetMockSettings();
  resetSessionForTests();
  resetSessionRestoreForTests();
  uninstallSync = installActiveSync();
});

afterEach(() => {
  uninstallSync();
  resetSessionForTests();
  resetSessionRestoreForTests();
  vi.restoreAllMocks();
});

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("restoreWorkspaceTabs", () => {
  it("reopens tables that still exist, query text, caret, files and the active tab", async () => {
    const qc = new QueryClient();
    const c = await connect("p-local-pg");
    await ipc.files.writeText("/q/report.sql", "select * from posts;");
    const result = await restoreWorkspaceTabs(
      qc,
      c,
      workspace({
        activeTab: 2,
        tabs: [
          { kind: "table", table: users, view: "structure", title: "People" },
          {
            kind: "table",
            table: { schema: "public", name: "ghost" },
            view: "table",
            title: "ghost",
          },
          {
            kind: "query",
            title: "Query 7",
            sql: "select 1;\nselect 2;",
            filePath: null,
            cursor: 12,
          },
          { kind: "query", title: "report.sql", sql: null, filePath: "/q/report.sql", cursor: 3 },
          { kind: "query", title: "gone.sql", sql: null, filePath: "/q/gone.sql", cursor: 0 },
        ],
      }),
      { tables: true },
    );
    expect(result).toEqual({ restored: 3, skipped: ["public.ghost", "gone.sql"] });
    const tabs = ofConnection(c);
    expect(tabs.map((t) => [t.kind, t.title])).toEqual([
      ["structure", "People"],
      ["query", "Query 7"],
      ["query", "report.sql"],
    ]);
    const [, untitled, file] = tabs as [Tab, Tab, Tab];
    expect(useTabsStore.getState().activeId).toBe(untitled.id);
    // Untitled text is unsaved work; a file reloads clean from disk.
    expect(untitled).toMatchObject({ dirty: true });
    expect(file.kind === "query" && file.dirty).toBeFalsy();
    const state = getEditorState(untitled.id)!;
    expect(state.doc.toString()).toBe("select 1;\nselect 2;");
    expect(state.selection.main.head).toBe(12);
    expect(getEditorState(file.id)!.doc.toString()).toBe("select * from posts;");
    expect(getQueryTab(file.id).filePath).toBe("/q/report.sql");
    // New query tabs continue the numbering.
    expect(useTabsStore.getState().newQuery(c)).toBeTruthy();
    expect(useTabsStore.getState().tabs.at(-1)?.title).toBe("Query 8");
  });

  it("does not read a big file at start-up: its clean tab is left out, a dirty one keeps its text", async () => {
    const qc = new QueryClient();
    const c = await connect("p-local-pg");
    const read = vi.spyOn(ipc.sqlFiles, "read");
    const big = "/Users/me/dumps/big-dump.sql";
    const result = await restoreWorkspaceTabs(
      qc,
      c,
      workspace({
        activeTab: 0,
        tabs: [
          { kind: "query", title: "big-dump.sql", sql: null, filePath: big, cursor: 0 },
          { kind: "query", title: "edited.sql", sql: "select 1;", filePath: big, cursor: 0 },
        ],
      }),
      { tables: true },
    );
    expect(read).not.toHaveBeenCalled();
    expect(result.skipped).toEqual(["big-dump.sql"]);
    const tabs = ofConnection(c);
    expect(tabs.map((t) => t.title)).toEqual(["edited.sql"]);
    expect(getEditorState(tabs[0]!.id)!.doc.toString()).toBe("select 1;");
  });

  it("brings two tabs of one table back as two tabs (a save / restore round trip)", async () => {
    const qc = new QueryClient();
    const c = await connect("p-local-pg");
    const first = useTabsStore.getState().openTable(c, users);
    const second = useTabsStore.getState().openTable(c, users, "table", { newTab: true });
    const structure = useTabsStore.getState().openTable(c, users, "structure", { newTab: true });
    expect(new Set([first, second, structure]).size).toBe(3);
    // Save: the live workspace as it would be written, through JSON like the settings store.
    const saved = parseSession(JSON.parse(JSON.stringify(currentSession())));
    const ws = saved.workspaces[0]!;
    expect(ws.tabs.map((t) => t.kind === "table" && [t.table.name, t.view])).toEqual([
      ["users", "table"],
      ["users", "table"],
      ["users", "structure"],
    ]);
    // Restore into a fresh window: nothing is merged into one tab.
    resetStores();
    const again = await connect("p-local-pg");
    const result = await restoreWorkspaceTabs(qc, again, ws, { tables: true });
    expect(result).toEqual({ restored: 3, skipped: [] });
    const tabs = ofConnection(again);
    expect(tabs.map((t) => [t.kind, t.title])).toEqual([
      ["table", "users"],
      ["table", "users"],
      ["structure", "users"],
    ]);
    expect(new Set(tabs.map((t) => t.id)).size).toBe(3);
  });

  it("seeds a handed-over table tab's filter bar before the tab mounts", async () => {
    const qc = new QueryClient();
    const c = await connect("p-local-pg");
    const row = { ...newFilterRow("id", "eq", "5"), enabled: true };
    const raw = { ...newFilterRow("__raw_sql__", "eq", "id > 1"), enabled: false };
    const result = await restoreWorkspaceTabs(
      qc,
      c,
      workspace({
        tabs: [
          {
            kind: "table",
            table: users,
            view: "table",
            title: "users",
            filters: [row, raw],
            filtersVisible: true,
            conjunction: "or",
          },
          {
            kind: "table",
            table: { schema: "public", name: "posts" },
            view: "table",
            title: "posts",
          },
        ],
      }),
      { tables: true, autoSave: false },
    );
    expect(result.restored).toBe(2);
    const [usersTab, postsTab] = ofConnection(c) as [Tab, Tab];
    const slice = useTableTabsStore.getState().slices[usersTab.id]!;
    expect(slice.filters).toEqual([row, raw]);
    expect(slice.filters[0]).not.toBe(row);
    expect(slice.filtersVisible).toBe(true);
    expect(slice.conjunction).toBe("or");
    expect(slice.offset).toBe(0);
    // A tab without filters has no state of its own yet.
    expect(useTableTabsStore.getState().slices[postsTab.id]).toBeUndefined();
  });

  it("restores a dirty file tab's edits and keeps it dirty against the file", async () => {
    const qc = new QueryClient();
    const c = await connect("p-local-pg");
    await ipc.files.writeText("/q/a.sql", "select 'disk';");
    await restoreWorkspaceTabs(
      qc,
      c,
      workspace({
        tabs: [
          { kind: "query", title: "a.sql", sql: "select 'mine';", filePath: "/q/a.sql", cursor: 0 },
        ],
      }),
      { tables: true },
    );
    const [tab] = ofConnection(c);
    expect(tab).toMatchObject({ kind: "query", dirty: true });
    expect(getEditorState(tab!.id)!.doc.toString()).toBe("select 'mine';");
  });

  it("follows the auto-save setting and can leave table tabs out", async () => {
    setPref("autoSaveQueryTabs", "first");
    const qc = new QueryClient();
    const c = await connect("p-local-pg");
    await restoreWorkspaceTabs(
      qc,
      c,
      workspace({
        tabs: [
          { kind: "table", table: users, view: "table", title: "users" },
          { kind: "query", title: "Query 1", sql: "one", filePath: null, cursor: 0 },
          { kind: "query", title: "Query 2", sql: "two", filePath: null, cursor: 0 },
        ],
      }),
      { tables: false },
    );
    expect(ofConnection(c).map((t) => t.title)).toEqual(["Query 1"]);
  });
});

describe("waitForProfileConnection", () => {
  it("resolves with the new connection, or null once the connect failed", async () => {
    openProfile("p-local-pg");
    const id = await waitForProfileConnection("p-local-pg");
    expect(id && useConnectionsStore.getState().byId[id]?.info.profileId).toBe("p-local-pg");
    usePaletteStore.getState().beginConnect("p-missing");
    const pending = waitForProfileConnection("p-missing");
    usePaletteStore.getState().endConnect("p-missing");
    await expect(pending).resolves.toBeNull();
  });
});

describe("restoreAtStartup", () => {
  const stored = (session: Omit<Session, "version">) =>
    ipc.settings.set(SESSION_KEY, { version: SESSION_VERSION, ...session });

  it("reconnects the open workspaces in order and shows the one that was active", async () => {
    await stored({
      activeProfileId: "p-local-pg",
      workspaces: [
        workspace({
          schema: "inventory",
          activeTab: 1,
          tabs: [
            { kind: "table", table: users, view: "table", title: "users" },
            { kind: "query", title: "Query 1", sql: "select now();", filePath: null, cursor: 4 },
          ],
        }),
        workspace({
          profileId: "p-shop-mysql",
          tabs: [
            { kind: "query", title: "Query 2", sql: "show tables;", filePath: null, cursor: 0 },
          ],
        }),
        workspace({ profileId: "p-sample-sqlite", open: false }),
      ],
    });
    await restoreAtStartup({
      qc: new QueryClient(),
      openProfile,
      prefsLoaded: async () => {},
    });
    const { order, byId, activeId } = useConnectionsStore.getState();
    expect(order.map((id) => byId[id]?.info.profileId)).toEqual(["p-local-pg", "p-shop-mysql"]);
    const pg = order[0]!;
    expect(activeId).toBe(pg);
    expect(byId[pg]?.currentSchema).toBe("inventory");
    expect(useUiStore.getState().view).toBe("workspace");
    const active = useTabsStore
      .getState()
      .tabs.find((t) => t.id === useTabsStore.getState().activeId);
    expect(active).toMatchObject({ connectionId: pg, title: "Query 1" });
    expect(ofConnection(order[1]!).map((t) => t.title)).toEqual(["Query 2"]);
  });

  it("does nothing with the preference off; a later connect brings the query tabs back", async () => {
    setPref("reopenWorkspaces", false);
    await stored({
      activeProfileId: null,
      workspaces: [
        workspace({
          tabs: [
            { kind: "table", table: users, view: "table", title: "users" },
            { kind: "query", title: "Query 3", sql: "select 3;", filePath: null, cursor: 0 },
          ],
        }),
      ],
    });
    await restoreAtStartup({ qc: new QueryClient(), openProfile, prefsLoaded: async () => {} });
    expect(useConnectionsStore.getState().order).toEqual([]);
    const c = await connect("p-local-pg");
    await vi.waitFor(() => expect(ofConnection(c).map((t) => t.title)).toEqual(["Query 3"]));
  });

  it("leaves a workspace whose connect fails closed and remembered", async () => {
    await stored({
      activeProfileId: null,
      workspaces: [workspace({ profileId: "p-deleted" }), workspace({ profileId: "p-prod-pg" })],
    });
    const failing = vi.fn((profileId: string) => {
      usePaletteStore.getState().beginConnect(profileId);
      queueMicrotask(() => usePaletteStore.getState().endConnect(profileId));
    });
    await restoreAtStartup({
      qc: new QueryClient(),
      openProfile: failing,
      prefsLoaded: async () => {},
    });
    // The deleted profile is dropped up front; the other one was tried and stays closed.
    expect(failing.mock.calls.map(([id]) => id)).toEqual(["p-prod-pg"]);
    expect(useConnectionsStore.getState().order).toEqual([]);
    expect(currentSession().workspaces).toMatchObject([{ profileId: "p-prod-pg", open: false }]);
  });
});

describe("restoring onto another database", () => {
  const stored = (session: Omit<Session, "version">) =>
    ipc.settings.set(SESSION_KEY, { version: SESSION_VERSION, ...session });
  const savedTabs: SessionWorkspace["tabs"] = [
    { kind: "table", table: users, view: "table", title: "users" },
    { kind: "query", title: "Query 1", sql: "select 1;", filePath: null, cursor: 0 },
    { kind: "query", title: "Query 2", sql: "select 2;", filePath: null, cursor: 0 },
  ];
  const restoreOnce = () =>
    restoreAtStartup({ qc: new QueryClient(), openProfile, prefsLoaded: async () => {} });
  const titles = () => {
    const [id] = useConnectionsStore.getState().order;
    return ofConnection(id!).map((t) => t.title);
  };

  it("keeps the SQL tabs and says so when the saved database cannot be opened", async () => {
    const warning = vi.spyOn(toast, "warning");
    vi.spyOn(ipc.connections, "switchDatabase").mockRejectedValue(new Error("gone"));
    await stored({
      activeProfileId: "p-local-pg",
      workspaces: [workspace({ database: "analytics", tabs: savedTabs })],
    });
    await restoreOnce();
    // The text is not lost; the table tab is not opened on the wrong database.
    expect(titles()).toEqual(["Query 1", "Query 2"]);
    expect(warning).toHaveBeenCalledTimes(1);
    expect(warning).toHaveBeenCalledWith(
      "Restored 2 SQL tabs on tabler_dev (was analytics)",
      expect.anything(),
    );
  });

  it("says so too when the switch lands on another database", async () => {
    const warning = vi.spyOn(toast, "warning");
    const real = ipc.connections.switchDatabase;
    vi.spyOn(ipc.connections, "switchDatabase").mockImplementation((id) => real(id, "postgres"));
    await stored({
      activeProfileId: "p-local-pg",
      workspaces: [workspace({ database: "analytics", tabs: savedTabs.slice(0, 2) })],
    });
    await restoreOnce();
    expect(titles()).toEqual(["Query 1"]);
    expect(warning).toHaveBeenCalledWith(
      "Restored 1 SQL tab on postgres (was analytics)",
      expect.anything(),
    );
  });

  it("is quiet when the workspace is back on its database", async () => {
    const warning = vi.spyOn(toast, "warning");
    await stored({
      activeProfileId: "p-local-pg",
      workspaces: [workspace({ database: "postgres", tabs: savedTabs })],
    });
    await restoreOnce();
    expect(titles()).toEqual(["users", "Query 1", "Query 2"]);
    expect(warning).not.toHaveBeenCalled();
  });

  it("a later connect on another database brings back the SQL tabs only, and says so", async () => {
    setPref("reopenWorkspaces", false);
    const warning = vi.spyOn(toast, "warning");
    await stored({
      activeProfileId: null,
      workspaces: [workspace({ database: "analytics", tabs: savedTabs })],
    });
    await restoreOnce();
    setPref("reopenWorkspaces", true);
    const c = await connect("p-local-pg");
    await vi.waitFor(() =>
      expect(ofConnection(c).map((t) => t.title)).toEqual(["Query 1", "Query 2"]),
    );
    await vi.waitFor(() =>
      expect(warning).toHaveBeenCalledWith(
        "Restored 2 SQL tabs on tabler_dev (was analytics)",
        expect.anything(),
      ),
    );
  });
});

describe("session persistence", () => {
  it("writes the live workspaces to the app settings", async () => {
    installSessionPersistence();
    const c = await connect("p-local-pg");
    useTabsStore.getState().openTable(c, users, "structure");
    const q = useTabsStore.getState().newQuery(c);
    useTabsStore.getState().rename(q, "Daily report");
    await flushSession();
    const saved = (await ipc.settings.get())[SESSION_KEY] as Session;
    expect(saved.activeProfileId).toBe("p-local-pg");
    expect(saved.workspaces).toEqual([
      {
        profileId: "p-local-pg",
        open: true,
        database: "tabler_dev",
        schema: null,
        activeTab: 1,
        tabs: [
          { kind: "table", table: users, view: "structure", title: "users" },
          { kind: "query", title: "Daily report", sql: "", filePath: null, cursor: 0 },
        ],
      },
    ]);
  });

  it("keeps remembered workspaces of profiles that are not open", async () => {
    installSessionPersistence();
    setRemembered([workspace({ profileId: "p-shop-mysql" })]);
    await flushSession();
    const saved = (await ipc.settings.get())[SESSION_KEY] as Session;
    expect(saved.workspaces).toMatchObject([{ profileId: "p-shop-mysql" }]);
  });

  it("closes the window without asking about auto-saved query tabs, but asks about files", async () => {
    const c = await connect("p-local-pg");
    const first = useTabsStore.getState().newQuery(c);
    const second = useTabsStore.getState().newQuery(c);
    useTabsStore.getState().setDirty(first, true);
    useTabsStore.getState().setDirty(second, true);
    const tabs = useTabsStore.getState().tabs;
    const tab = (id: string) => tabs.find((t) => t.id === id)!;
    expect(survivesClose(tab(first), tabs)).toBe(true);
    setPref("autoSaveQueryTabs", "first");
    expect(survivesClose(tab(first), tabs)).toBe(true);
    expect(survivesClose(tab(second), tabs)).toBe(false);
    setPref("autoSaveQueryTabs", "all");

    // Installed, the window-close guard leaves auto-saved tabs alone and closes after the write.
    installSessionPersistence();
    expect(useTabsStore.getState().requestWindowClose()).toBe(false);
    expect(useTabsStore.getState().closeRequest).toBeNull();
    // A file with unsaved edits still asks.
    useQueryTabsStore.getState().update(second, { filePath: "/q/x.sql" });
    expect(useTabsStore.getState().requestWindowClose()).toBe(false);
    expect(useTabsStore.getState().closeRequest?.dirty.map((d) => d.id)).toEqual([second]);
  });
});

describe("several workspaces of one profile", () => {
  const stored = (session: Omit<Session, "version">) =>
    ipc.settings.set(SESSION_KEY, { version: SESSION_VERSION, ...session });

  it("reopens each on its own database and shows the one that was on screen", async () => {
    await stored({
      activeProfileId: "p-local-pg",
      activeDatabase: "ecommerce_sales",
      workspaces: [
        workspace({
          database: "tabler_dev",
          tabs: [{ kind: "query", title: "Dev", sql: "select 1;", filePath: null, cursor: 0 }],
        }),
        workspace({
          database: "ecommerce_sales",
          tabs: [{ kind: "query", title: "Sales", sql: "select 2;", filePath: null, cursor: 0 }],
          activeTab: 0,
        }),
      ],
    });
    await restoreAtStartup({ qc: new QueryClient(), openProfile, prefsLoaded: async () => {} });
    const { order, byId, activeId } = useConnectionsStore.getState();
    expect(order.map((id) => byId[id]?.info.currentDatabase)).toEqual([
      "tabler_dev",
      "ecommerce_sales",
    ]);
    expect(activeId).toBe(order[1]);
    expect(ofConnection(order[0]!).map((t) => t.title)).toEqual(["Dev"]);
    expect(ofConnection(order[1]!).map((t) => t.title)).toEqual(["Sales"]);
    // Both are saved again, each with its database.
    const session = currentSession();
    expect(session.workspaces.map((w) => [w.database, w.tabs.length])).toEqual([
      ["tabler_dev", 1],
      ["ecommerce_sales", 1],
    ]);
    expect(session.activeDatabase).toBe("ecommerce_sales");
  });
});

describe("a profile's second workspace and reconnects", () => {
  const stored = (session: Omit<Session, "version">) =>
    ipc.settings.set(SESSION_KEY, { version: SESSION_VERSION, ...session });
  const queryTab = (title: string, sql: string): SessionWorkspace["tabs"][number] => ({
    kind: "query",
    title,
    sql,
    filePath: null,
    cursor: 0,
  });

  /** "Reopen closed workspaces" off, two closed workspaces of p-local-pg remembered. */
  async function startWithTwoClosedWorkspaces(qc: QueryClient) {
    setPref("reopenWorkspaces", false);
    await stored({
      activeProfileId: null,
      workspaces: [
        workspace({
          open: false,
          database: "tabler_dev",
          tabs: [queryTab("Dev", "select 1;")],
        }),
        workspace({
          open: false,
          database: "ecommerce_sales",
          tabs: [queryTab("Sales", "select 2;")],
        }),
      ],
    });
    await restoreAtStartup({ qc, openProfile, prefsLoaded: async () => {} });
    expect(useConnectionsStore.getState().order).toEqual([]);
  }

  it("restores the second workspace's remembered tabs and keeps its saved entry", async () => {
    const qc = new QueryClient();
    await startWithTwoClosedWorkspaces(qc);
    const dev = await connect("p-local-pg");
    await vi.waitFor(() => expect(ofConnection(dev).map((t) => t.title)).toEqual(["Dev"]));

    // The open-database action: another workspace of the profile while the first stays open.
    const sales = await connectWorkspace(qc, "p-local-pg", "ecommerce_sales");
    expect(sales).not.toBeNull();
    expect(useConnectionsStore.getState().byId[sales!]?.info.currentDatabase).toBe(
      "ecommerce_sales",
    );
    await vi.waitFor(() => expect(ofConnection(sales!).map((t) => t.title)).toEqual(["Sales"]));
    expect(ofConnection(dev).map((t) => t.title)).toEqual(["Dev"]);

    // Both workspaces are live now and saved with their own tabs.
    const session = currentSession();
    expect(session.workspaces.map((w) => [w.database, w.open, w.tabs.map((t) => t.title)])).toEqual(
      [
        ["tabler_dev", true, ["Dev"]],
        ["ecommerce_sales", true, ["Sales"]],
      ],
    );
  });

  it("a third database opened while the profile is open takes no other workspace's tabs", async () => {
    const qc = new QueryClient();
    await startWithTwoClosedWorkspaces(qc);
    const dev = await connect("p-local-pg");
    await vi.waitFor(() => expect(ofConnection(dev).map((t) => t.title)).toEqual(["Dev"]));

    // A database with no remembered workspace: it starts empty, "Sales" is not for it.
    const analytics = await connectWorkspace(qc, "p-local-pg", "analytics");
    expect(analytics).not.toBeNull();
    expect(useConnectionsStore.getState().byId[analytics!]?.info.currentDatabase).toBe("analytics");
    await flush();
    await flush();
    expect(ofConnection(analytics!)).toEqual([]);
    expect(ofConnection(dev).map((t) => t.title)).toEqual(["Dev"]);
    // "Sales" is still remembered (closed), beside the live workspaces.
    expect(
      currentSession().workspaces.map((w) => [w.database, w.open, w.tabs.map((t) => t.title)]),
    ).toEqual([
      ["tabler_dev", true, ["Dev"]],
      ["analytics", true, []],
      ["ecommerce_sales", false, ["Sales"]],
    ]);

    // So opening its database later still brings its tabs back.
    const sales = await connectWorkspace(qc, "p-local-pg", "ecommerce_sales");
    expect(sales).not.toBeNull();
    await vi.waitFor(() => expect(ofConnection(sales!).map((t) => t.title)).toEqual(["Sales"]));
    expect(ofConnection(analytics!)).toEqual([]);
  });

  it("keeps the second workspace remembered when its connect brings nothing back yet", async () => {
    const qc = new QueryClient();
    await startWithTwoClosedWorkspaces(qc);
    const dev = await connect("p-local-pg");
    await vi.waitFor(() => expect(ofConnection(dev).map((t) => t.title)).toEqual(["Dev"]));
    // The second workspace is still remembered, closed, while only the first is open.
    expect(
      currentSession().workspaces.map((w) => [w.database, w.open, w.tabs.map((t) => t.title)]),
    ).toEqual([
      ["tabler_dev", true, ["Dev"]],
      ["ecommerce_sales", false, ["Sales"]],
    ]);
  });

  it("does not restore again on a reconnect (the old connection is swapped for a new one)", async () => {
    const qc = new QueryClient();
    await startWithTwoClosedWorkspaces(qc);
    const dev = await connect("p-local-pg");
    await vi.waitFor(() => expect(ofConnection(dev).map((t) => t.title)).toEqual(["Dev"]));

    // What the reconnect does: one store update removes the old id and adds the new one.
    const info = await ipc.connections.connect("p-local-pg");
    expect(migrateConnection(dev, info, qc)).toBe(true);
    await flush();
    await flush();
    // The tabs followed; the other workspace's "Sales" did not land on this connection.
    expect(ofConnection(info.connectionId).map((t) => t.title)).toEqual(["Dev"]);
    expect(
      currentSession().workspaces.map((w) => [w.database, w.open, w.tabs.map((t) => t.title)]),
    ).toEqual([
      ["tabler_dev", true, ["Dev"]],
      ["ecommerce_sales", false, ["Sales"]],
    ]);
  });

  it("does not restore twice for a second connection of the same workspace", async () => {
    const qc = new QueryClient();
    await startWithTwoClosedWorkspaces(qc);
    const dev = await connect("p-local-pg");
    await vi.waitFor(() => expect(ofConnection(dev).map((t) => t.title)).toEqual(["Dev"]));

    // Another connection on the same profile and database while the first one is open.
    const again = await connect("p-local-pg");
    await flush();
    await flush();
    expect(ofConnection(again)).toEqual([]);
    expect(currentSession().workspaces.find((w) => w.database === "ecommerce_sales")).toMatchObject(
      { open: false, tabs: [{ title: "Sales" }] },
    );
  });
});

describe("tool tabs in the session", () => {
  let unregister: () => void = () => {};
  beforeEach(() => {
    unregister = registerToolTab<{ schema: string }>("overview-test", {
      component: () => null,
      icon: Gauge,
      title: (p) => `Overview: ${p.schema}`,
      persist: true,
      restore: (raw) =>
        typeof (raw as { schema?: unknown } | null)?.schema === "string"
          ? { schema: (raw as { schema: string }).schema }
          : null,
    });
  });
  afterEach(() => unregister());

  it("are saved when their tool keeps them and reopened with the table tabs", async () => {
    const qc = new QueryClient();
    const c = await connect("p-local-pg");
    useTabsStore.getState().open({
      id: "o1",
      kind: "tool",
      connectionId: c,
      tool: "overview-test",
      title: "Overview: public",
      params: { schema: "public" },
    });
    const saved = currentSession().workspaces[0]!;
    expect(saved.tabs).toEqual([
      {
        kind: "tool",
        tool: "overview-test",
        title: "Overview: public",
        params: { schema: "public" },
      },
    ]);
    useTabsStore.getState().forceClose(["o1"]);
    const result = await restoreWorkspaceTabs(
      qc,
      c,
      {
        ...saved,
        tabs: [
          ...saved.tabs,
          { kind: "tool", tool: "overview-test", title: "Bad", params: { schema: 4 } },
          { kind: "tool", tool: "unknown", title: "Gone", params: null },
        ],
      },
      { tables: true },
    );
    expect(result).toEqual({ restored: 1, skipped: ["Bad", "Gone"] });
    expect(ofConnection(c)).toMatchObject([
      { kind: "tool", tool: "overview-test", params: { schema: "public" } },
    ]);
    // Off the saved database (tables: false) they stay closed.
    useTabsStore.getState().forceClose(ofConnection(c).map((t) => t.id));
    await restoreWorkspaceTabs(qc, c, saved, { tables: false });
    expect(ofConnection(c)).toEqual([]);
  });
});

describe("a window's handoff", () => {
  it("connects to the handed-over database and rebuilds the moved tab, whatever auto-save says", async () => {
    setPref("autoSaveQueryTabs", "none");
    vi.spyOn(ipc.windows, "takeHandoff").mockResolvedValue({
      profileId: "p-local-pg",
      database: "ecommerce_sales",
      schema: "inventory",
      tab: { kind: "query", title: "Moved", sql: "select 42;", filePath: null, cursor: 3 },
    });
    const id = await openHandoff({ qc: new QueryClient(), prefsLoaded: async () => {} });
    expect(id).not.toBeNull();
    const conn = useConnectionsStore.getState().byId[id!]!;
    expect(conn.info.currentDatabase).toBe("ecommerce_sales");
    expect(conn.currentSchema).toBe("inventory");
    const [tab] = ofConnection(id!);
    expect(tab).toMatchObject({ kind: "query", title: "Moved", dirty: true });
    expect(getEditorState(tab!.id)?.doc.toString()).toBe("select 42;");
    expect(useTabsStore.getState().activeId).toBe(tab!.id);
  });

  it("shows a handed-over table with the link's filter applied", async () => {
    const row = { ...newFilterRow("id", "eq", "5"), value2: "" };
    vi.spyOn(ipc.windows, "takeHandoff").mockResolvedValue({
      profileId: "p-local-pg",
      database: null,
      schema: "public",
      tab: {
        kind: "table",
        table: users,
        view: "table",
        title: "users",
        filters: [row],
        filtersVisible: true,
      },
    });
    const id = await openHandoff({ qc: new QueryClient(), prefsLoaded: async () => {} });
    const [tab] = ofConnection(id!);
    expect(tab).toMatchObject({ kind: "table", title: "users" });
    expect(useTableTabsStore.getState().slices[tab!.id]).toMatchObject({
      filters: [{ column: "id", op: "eq", value: "5", enabled: true }],
      filtersVisible: true,
      offset: 0,
    });
  });

  it("opens nothing for an empty window", async () => {
    vi.spyOn(ipc.windows, "takeHandoff").mockResolvedValue(null);
    await expect(
      openHandoff({ qc: new QueryClient(), prefsLoaded: async () => {} }),
    ).resolves.toBeNull();
    expect(useConnectionsStore.getState().order).toEqual([]);
  });
});

describe("another window keeps the session once the main one closed", () => {
  const stored = (session: Omit<Session, "version">) =>
    ipc.settings.set(SESSION_KEY, { version: SESSION_VERSION, ...session });

  afterEach(() => {
    mockSessionOwner.label = "main";
    resetWindowLabelForTests();
  });

  it("keeps the closed window's workspaces remembered but closed, and its own live", async () => {
    // What the main window wrote as it closed.
    await stored({
      activeProfileId: "p-shop-mysql",
      workspaces: [workspace({ profileId: "p-shop-mysql" }), workspace({ open: false })],
    });
    const c = await connect("p-sample-sqlite");
    useTabsStore.getState().newQuery(c);
    const qc = new QueryClient();
    await expect(adoptSession(qc)).resolves.toBe(true);
    // Once per window.
    await expect(adoptSession(qc)).resolves.toBe(false);
    await flushSession();
    const saved = (await ipc.settings.get())[SESSION_KEY] as Session;
    expect(saved.workspaces.map((w) => [w.profileId, w.open])).toEqual([
      ["p-sample-sqlite", true],
      ["p-shop-mysql", false],
      ["p-local-pg", false],
    ]);
    expect(saved.activeProfileId).toBe("p-sample-sqlite");
  });

  it("takes over when told, or at startup when the backend says it is the keeper", async () => {
    await stored({ activeProfileId: null, workspaces: [workspace({ profileId: "p-shop-mysql" })] });
    const qc = new QueryClient();
    // The main window (this test's) is not told anything at startup: it restores itself.
    const owner = vi.spyOn(ipc.windows, "isSessionOwner");
    const uninstall = installSessionOwnership(qc);
    await vi.waitFor(() => expect(owner).not.toHaveBeenCalled());
    await connect("p-local-pg");
    await vi.waitFor(async () => {
      // Told once its listener is up (adopting twice is a no-op).
      mockEmit("window:session-owner", null);
      await flushSession();
      const saved = (await ipc.settings.get())[SESSION_KEY] as Session;
      expect(saved.workspaces.map((w) => [w.profileId, w.open])).toEqual([
        ["p-local-pg", true],
        ["p-shop-mysql", false],
      ]);
    });
    uninstall();

    // A window opened later that the backend names the keeper adopts it at startup.
    resetSessionForTests();
    resetSessionRestoreForTests();
    resetWindowLabelForTests();
    vi.spyOn(ipc.windows, "label").mockResolvedValue("w-2");
    mockSessionOwner.label = "w-2";
    owner.mockResolvedValue(true);
    const again = installSessionOwnership(qc);
    await vi.waitFor(() => expect(owner).toHaveBeenCalled());
    await vi.waitFor(async () => {
      await flushSession();
      const saved = (await ipc.settings.get())[SESSION_KEY] as Session;
      expect(saved.workspaces.every((w) => !w.open || w.profileId === "p-local-pg")).toBe(true);
      expect(saved.workspaces.some((w) => w.profileId === "p-shop-mysql")).toBe(true);
    });
    again();
  });
});
