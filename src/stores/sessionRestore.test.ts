import { QueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getEditorState } from "@/features/editor/editorStates";
import { usePaletteStore } from "@/features/palette/store";
import { ipc } from "@/lib/ipc";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import { installActiveSync } from "./activeSync";
import { useConnectionsStore } from "./connections";
import { resetPrefsForTests, setPref } from "./prefs";
import { getQueryTab, useQueryTabsStore } from "./queryTabs";
import { SESSION_KEY, SESSION_VERSION, type Session, type SessionWorkspace } from "./session";
import {
  currentSession,
  flushSession,
  installSessionPersistence,
  resetSessionForTests,
  setRemembered,
  survivesClose,
} from "./sessionPersist";
import {
  resetSessionRestoreForTests,
  restoreAtStartup,
  restoreWorkspaceTabs,
  waitForProfileConnection,
} from "./sessionRestore";
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
