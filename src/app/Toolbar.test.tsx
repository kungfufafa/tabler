import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { actions, type ActionId } from "@/features/palette/actions";
import { useConnectionDialog } from "@/features/connections/store";
import { SafetyHost } from "@/features/safety/SafetyHost";
import { emptyStaging, insertRow } from "@/features/staging/model";
import { useStructureStore } from "@/features/structure/store";
import { ipc, type ConnectionInfo, type ConnectionProfile } from "@/lib/ipc";
import { getPrefs, resetPrefsForTests, setPref } from "@/stores/prefs";
import { useConnectionsStore, type OpenConnection } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useUiStore } from "@/stores/ui";
import { databaseLabel, serverLabel } from "./connectionLabels";
import { pillModel, pillText, tagLabel, transportLabel } from "./statusPillModel";
import { DEFAULT_TOOLBAR_ITEMS } from "./toolbarLayout";
import { Toolbar } from "./Toolbar";
import { WorkspaceRail } from "./WorkspaceRail";

function info(connectionId: string, patch: Partial<ConnectionInfo> = {}): ConnectionInfo {
  return {
    connectionId,
    profileId: `p-${connectionId}`,
    name: `Conn ${connectionId}`,
    color: "green",
    driver: "postgres",
    serverVersion: "PostgreSQL 17.6 (Homebrew) on aarch64-apple-darwin",
    currentDatabase: "tabler_dev",
    currentUser: "postgres",
    tunnel: null,
    readOnly: false,
    ...patch,
  };
}

const open = (i: ConnectionInfo, patch: Partial<OpenConnection> = {}): OpenConnection => ({
  info: i,
  status: "connected",
  message: null,
  currentSchema: "public",
  ...patch,
});

function wrap(children: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>{children}</TooltipProvider>
    </QueryClientProvider>,
  );
}

const users = { schema: "public", name: "users" };

function stageRows(tabId: string, n: number) {
  let staging = emptyStaging();
  for (let i = 0; i < n; i++) staging = insertRow(staging, `new:${i}`, []);
  useTableTabsStore.getState().update(tabId, { staging });
}

let unregister: Array<() => void> = [];
function spyAction(id: ActionId) {
  const fn = vi.fn();
  unregister.push(actions.register(id, fn));
  return fn;
}

beforeEach(() => {
  resetPrefsForTests();
  useTabsStore.setState({
    tabs: [],
    activeId: null,
    closed: [],
    queryCounter: 0,
    closeRequest: null,
    history: [],
    historyIndex: -1,
  });
  useTableTabsStore.setState({ slices: {} });
  useStructureStore.setState({ byTab: {} });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useUiStore.setState({
    view: "workspace",
    sidebarOpen: true,
    consoleOpen: false,
    inspectorOpen: false,
  });
  vi.spyOn(ipc.profiles, "list").mockResolvedValue([]);
});
afterEach(() => {
  unregister.forEach((u) => u());
  unregister = [];
  vi.restoreAllMocks();
});

describe("connection labels", () => {
  it("shortens server versions to product and number", () => {
    expect(serverLabel(info("a"))).toBe("PostgreSQL 17.6");
    expect(serverLabel(info("a", { driver: "mysql", serverVersion: "MySQL 9.6.0" }))).toBe(
      "MySQL 9.6.0",
    );
    expect(serverLabel(info("a", { driver: "mysql", serverVersion: "MariaDB 11.8.2" }))).toBe(
      "MariaDB 11.8.2",
    );
    expect(serverLabel(info("a", { driver: "sqlite", serverVersion: "SQLite 3.54.0" }))).toBe(
      "SQLite 3.54.0",
    );
    expect(serverLabel(info("a", { serverVersion: "17" }))).toBe("PostgreSQL 17");
    expect(serverLabel(info("a", { serverVersion: "" }))).toBe("PostgreSQL");
  });

  it("shows a SQLite database as its file name", () => {
    expect(
      databaseLabel(info("a", { driver: "sqlite", currentDatabase: "/tmp/dev/sample.sqlite" })),
    ).toBe("sample.sqlite");
    expect(databaseLabel(info("a"))).toBe("tabler_dev");
    expect(databaseLabel(info("a", { currentDatabase: null }))).toBe("");
  });
});

describe("pillModel", () => {
  it("describes the connection and the visible table", () => {
    const conn = open(info("a", { tunnel: { localPort: 50000 } }));
    const tab = {
      id: "t",
      kind: "table" as const,
      connectionId: "a",
      table: users,
      title: "users",
    };
    const model = pillModel(conn, tab, "local", false);
    expect(pillText(model)).toBe(
      "LOCAL │ PostgreSQL 17.6 : SSH : Conn a : tabler_dev : public.users",
    );
    expect(pillText(pillModel(conn, { ...tab, kind: "structure" }, null, false))).toBe(
      "PostgreSQL 17.6 : SSH : Conn a : tabler_dev : public.users (Structure)",
    );
  });

  it("shows the schema and SQL Query for a query tab, only the schema without a tab", () => {
    const conn = open(info("a"));
    const q = { id: "q", kind: "query" as const, connectionId: "a", title: "Query 1" };
    expect(pillText(pillModel(conn, q, "development", false))).toBe(
      "DEV │ PostgreSQL 17.6 : Conn a : tabler_dev : public : SQL Query",
    );
    expect(pillText(pillModel(conn, null, null, false))).toBe(
      "PostgreSQL 17.6 : Conn a : tabler_dev : public",
    );
    // Another connection's tab says nothing about this connection.
    expect(pillText(pillModel(conn, { ...q, connectionId: "b" }, null, false))).toBe(
      "PostgreSQL 17.6 : Conn a : tabler_dev : public",
    );
  });

  it("does not call a tool tab (Metrics Board, Item Overview, SQL Viewer) a SQL Query", () => {
    const conn = open(info("a"));
    for (const tool of ["metrics", "item-overview", "sqlviewer"]) {
      const tab = {
        id: `tool-${tool}`,
        kind: "tool" as const,
        connectionId: "a",
        tool,
        title: tool,
      };
      expect(pillText(pillModel(conn, tab, null, false))).toBe(
        "PostgreSQL 17.6 : Conn a : tabler_dev : public",
      );
    }
  });

  it("adds SSH / TLS / SSH/TLS after the server", () => {
    expect(transportLabel(false, false)).toBeNull();
    expect(transportLabel(true, false)).toBe("SSH");
    expect(transportLabel(false, true)).toBe("TLS");
    expect(transportLabel(true, true)).toBe("SSH/TLS");
    const tls = pillModel(open(info("a")), null, null, false, true);
    expect(pillText(tls)).toBe("PostgreSQL 17.6 : TLS : Conn a : tabler_dev : public");
    expect(tls.segments[1]).toEqual({ role: "transport", text: "TLS" });
    const both = pillModel(
      open(info("a", { tunnel: { localPort: 1 } })),
      null,
      "staging",
      false,
      true,
    );
    expect(pillText(both)).toBe(
      "STAGING │ PostgreSQL 17.6 : SSH/TLS : Conn a : tabler_dev : public",
    );
  });

  it("labels custom tags with their name", () => {
    expect(tagLabel("development")).toBe("DEV");
    expect(tagLabel("qa")).toBe("QA");
  });

  it("leads with the connection problem", () => {
    const lost = open(info("a"), { status: "lost" });
    const model = pillModel(lost, null, null, false);
    expect(model.problem).toBe("lost");
    expect(model.segments.slice(0, 3)).toMatchObject([
      { role: "state", text: "Connection lost" },
      { role: "server" },
      { role: "name" },
    ]);
    expect(pillModel(lost, null, null, true).problem).toBe("reconnecting");
    expect(pillModel(open(info("a"), { status: "reconnecting" }), null, null, false).problem).toBe(
      "reconnecting",
    );
  });
});

describe("Toolbar", () => {
  it("shows only the title in the launcher", () => {
    wrap(<Toolbar launcher />);
    expect(screen.getByText("Tabler")).toBeInTheDocument();
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });

  it("enables the action control while the workspace has pending changes, in any of its tabs", () => {
    useConnectionsStore.getState().add(info("a"));
    const t = useTabsStore.getState().openTable("a", users);
    const commit = spyAction("grid.commit");
    const discard = spyAction("grid.discard");
    const preview = spyAction("grid.preview");
    wrap(<Toolbar launcher={false} />);
    const group = screen.getByRole("group", { name: "Staged changes" });
    for (const b of within(group).getAllByRole("button")) expect(b).toBeDisabled();

    act(() => stageRows(t, 3));
    fireEvent.click(within(group).getByRole("button", { name: "Commit 3 changes" }));
    fireEvent.click(within(group).getByRole("button", { name: "Discard changes" }));
    fireEvent.click(within(group).getByRole("button", { name: "Preview SQL" }));
    expect(commit).toHaveBeenCalledTimes(1);
    expect(discard).toHaveBeenCalledTimes(1);
    expect(preview).toHaveBeenCalledTimes(1);
    expect(document.querySelector("[data-slot=pending-badge]")).toHaveTextContent("3");

    // One commit covers the tab's other view too: its staged DDL joins the rows.
    act(() => useTabsStore.getState().setView(t, "structure"));
    expect(within(group).getByRole("button", { name: "Commit 3 changes" })).toBeEnabled();
    act(() => useStructureStore.getState().stage(t, () => [{ op: "rawSql", sql: "x" }]));
    expect(within(group).getByRole("button", { name: "Commit 4 changes" })).toBeEnabled();

    // ... and the changes of a tab that is not in front.
    const other = useTabsStore.getState().openTable("a", { schema: "public", name: "posts" });
    act(() => useTabsStore.getState().activate(other));
    expect(within(group).getByRole("button", { name: "Commit 4 changes" })).toBeEnabled();
    act(() => stageRows(other, 2));
    expect(within(group).getByRole("button", { name: "Commit 6 changes" })).toBeEnabled();
  });

  it("highlights the open panels and toggles them", () => {
    useConnectionsStore.getState().add(info("a"));
    const toggleConsole = spyAction("ui.toggle-console");
    wrap(<Toolbar launcher={false} />);
    const panels = screen.getByRole("group", { name: "Panels" });
    expect(within(panels).getByRole("button", { name: "Left sidebar" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(within(panels).getByRole("button", { name: "Console log" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    fireEvent.click(within(panels).getByRole("button", { name: "Console log" }));
    expect(toggleConsole).toHaveBeenCalledTimes(1);
    act(() => useUiStore.setState({ inspectorOpen: true }));
    expect(within(panels).getByRole("button", { name: "Right sidebar" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  });

  it("runs the toolbar actions", () => {
    useConnectionsStore.getState().add(info("a"));
    const ids: ActionId[] = [
      "ui.toggle-sidebar",
      "palette.databases",
      "tab.new-query",
      "view.refresh",
      "palette.open",
    ];
    const spies = ids.map(spyAction);
    wrap(<Toolbar launcher={false} />);
    for (const name of ["Toggle sidebar", "Switch database", "New query", "Refresh"])
      fireEvent.click(screen.getByRole("button", { name }));
    fireEvent.click(screen.getByRole("button", { name: "Open anything" }));
    for (const spy of spies) expect(spy).toHaveBeenCalledTimes(1);
  });

  it("shows the safe-mode level; its menu lists the five levels and the read-only note", async () => {
    vi.mocked(ipc.profiles.list).mockResolvedValue([
      { id: "p-a", name: "Conn a", options: { safeMode: "alertNonSelect" } } as ConnectionProfile,
    ]);
    useConnectionsStore.getState().add(info("a", { readOnly: true }));
    wrap(<Toolbar launcher={false} />);
    const lock = await screen.findByRole("button", { name: "Safe mode: Alert Mode 2 (read-only)" });
    expect(lock).toHaveAttribute("data-level", "alertNonSelect");
    expect(lock).toHaveAttribute("aria-expanded", "false");
    fireEvent.keyDown(lock, { key: "Enter" });
    const menu = await screen.findByRole("menu", { name: /^Safe mode: / });
    expect(lock).toHaveAttribute("aria-expanded", "true");
    const radios = within(menu).getAllByRole("menuitemradio");
    expect(radios.map((r) => r.textContent)).toEqual([
      "Silent ModeSend queries to the server without any warnings",
      "Alert Mode 1Warn before sending queries to the server",
      "Alert Mode 2Warn before sending queries to the server, except SELECT/EXPLAIN/SHOW queries",
      "Safe Mode 1Prompt for password before sending queries to the server",
      "Safe Mode 2Prompt for password before sending queries to the server, except SELECT/EXPLAIN/SHOW queries",
    ]);
    expect(radios.map((r) => r.getAttribute("aria-checked"))).toEqual([
      "false",
      "false",
      "true",
      "false",
      "false",
    ]);
    expect(menu).toHaveTextContent("Read-only connection: edits and DDL are disabled.");
    expect(within(menu).getByRole("menuitem", { name: "Edit Connection…" })).toBeInTheDocument();
  });

  it("treats a profile without a level as Silent Mode (open lock)", async () => {
    vi.mocked(ipc.profiles.list).mockResolvedValue([
      { id: "p-a", name: "Conn a", options: { safeMode: null } } as ConnectionProfile,
    ]);
    useConnectionsStore.getState().add(info("a"));
    wrap(<Toolbar launcher={false} />);
    const lock = await screen.findByRole("button", { name: "Safe mode: Silent Mode" });
    expect(lock).toHaveAttribute("data-level", "silent");
    expect(document.querySelector("[data-slot=read-only-dot]")).toBeNull();
  });

  it("choosing a level saves the connection's profile", async () => {
    const profile = {
      id: "p-a",
      name: "Conn a",
      options: { safeMode: "silent" },
    } as ConnectionProfile;
    vi.mocked(ipc.profiles.list).mockResolvedValue([profile]);
    vi.spyOn(ipc.profiles, "get").mockResolvedValue(profile);
    const save = vi.spyOn(ipc.profiles, "save").mockImplementation(async (input) => {
      vi.mocked(ipc.profiles.list).mockResolvedValue([input.profile]);
      return input.profile;
    });
    useConnectionsStore.getState().add(info("a"));
    wrap(<Toolbar launcher={false} />);
    const lock = await screen.findByRole("button", { name: "Safe mode: Silent Mode" });
    fireEvent.keyDown(lock, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitemradio", { name: /^Alert Mode 1/ }));
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save.mock.calls[0]![0]).toMatchObject({
      profile: { id: "p-a", options: { safeMode: "alert" } },
      password: null,
      clearPassword: false,
    });
    expect(await screen.findByRole("button", { name: "Safe mode: Alert Mode 1" })).toHaveAttribute(
      "data-level",
      "alert",
    );
  });

  it("asks for the password before lowering a Safe level", async () => {
    const profile = {
      id: "p-a",
      name: "Conn a",
      options: { safeMode: "safe" },
    } as ConnectionProfile;
    vi.mocked(ipc.profiles.list).mockResolvedValue([profile]);
    vi.spyOn(ipc.profiles, "get").mockResolvedValue(profile);
    vi.spyOn(ipc.profiles, "secretStatus").mockResolvedValue({
      hasPassword: true,
      hasSshPassword: false,
      hasSshPassphrase: false,
    });
    const verify = vi
      .spyOn(ipc.safety, "verifyPassword")
      .mockImplementation(async (_id, password) => password === "pw");
    const save = vi.spyOn(ipc.profiles, "save").mockImplementation(async (i) => i.profile);
    useConnectionsStore.getState().add(info("a"));
    wrap(
      <>
        <Toolbar launcher={false} />
        <SafetyHost />
      </>,
    );
    const lock = await screen.findByRole("button", { name: "Safe mode: Safe Mode 1" });
    expect(lock).toHaveAttribute("data-level", "safe");
    fireEvent.keyDown(lock, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitemradio", { name: /^Silent Mode/ }));
    const dialog = await screen.findByRole("alertdialog", {
      name: "Enter the password of Conn a to continue",
    });
    const field = within(dialog).getByLabelText("Password");
    fireEvent.change(field, { target: { value: "nope" } });
    fireEvent.submit(field.closest("form")!);
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Wrong password");
    expect(save).not.toHaveBeenCalled();
    fireEvent.change(field, { target: { value: "pw" } });
    fireEvent.submit(field.closest("form")!);
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(verify).toHaveBeenLastCalledWith("p-a", "pw");
    expect(save.mock.calls[0]![0].profile.options.safeMode).toBe("silent");
  });

  it("the menu's Edit Connection… opens the connection's form", async () => {
    const profile = { id: "p-a", name: "Conn a", options: {} } as ConnectionProfile;
    vi.mocked(ipc.profiles.list).mockResolvedValue([profile]);
    useConnectionsStore.getState().add(info("a"));
    useConnectionDialog.setState({ open: false, profile: null, prefill: null });
    wrap(<Toolbar launcher={false} />);
    const lock = screen.getByRole("button", { name: "Safe mode: Silent Mode" });
    fireEvent.keyDown(lock, { key: "Enter" });
    const menu = await screen.findByRole("menu", { name: /^Safe mode: / });
    const edit = within(menu).getByRole("menuitem", { name: "Edit Connection…" });
    await vi.waitFor(() => expect(edit).not.toHaveAttribute("data-disabled"));
    fireEvent.click(edit);
    // The form is mounted app-wide: it opens over the workspace, on the profile.
    expect(useUiStore.getState().view).not.toBe("launcher");
    expect(useConnectionDialog.getState()).toMatchObject({ open: true, profile });
    await vi.waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
  });

  it("puts the connection tag in the pill; a click shows the details, which switch connection", async () => {
    vi.mocked(ipc.profiles.list).mockResolvedValue([
      {
        id: "p-a",
        driver: "postgres",
        host: "db.example.com",
        port: 5432,
        ssh: null,
        options: { tag: "production" },
      } as unknown as ConnectionProfile,
    ]);
    vi.spyOn(ipc.security, "sessionDetails").mockResolvedValue({
      tls: true,
      tlsVersion: "TLSv1.3",
      tlsCipher: "TLS_AES_256_GCM_SHA384",
      encoding: "UTF8",
    });
    useConnectionsStore.getState().add(info("a"));
    const switcher = spyAction("palette.connections");
    wrap(<Toolbar launcher={false} />);
    const tag = await screen.findByText("PRODUCTION");
    expect(tag).toHaveAttribute("data-slot", "status-pill-tag");
    const pill = await screen.findByRole("button", {
      name: /^PRODUCTION │ PostgreSQL 17\.6 : TLS : Conn a/,
    });
    fireEvent.click(pill);
    const details = await screen.findByRole("dialog");
    expect(details).toHaveTextContent("db.example.com:5432");
    expect(details).toHaveTextContent("TLSv1.3 · TLS_AES_256_GCM_SHA384");
    expect(details).toHaveTextContent("UTF8");
    expect(details).toHaveTextContent("postgres");
    fireEvent.click(within(details).getByRole("button", { name: "Switch Connection…" }));
    expect(switcher).toHaveBeenCalledTimes(1);
  });

  it("recolours the connection from the details", async () => {
    const profile = {
      id: "p-a",
      name: "Conn a",
      color: "green",
      driver: "postgres",
      options: {},
    } as unknown as ConnectionProfile;
    vi.mocked(ipc.profiles.list).mockResolvedValue([profile]);
    vi.spyOn(ipc.profiles, "get").mockResolvedValue(profile);
    const save = vi.spyOn(ipc.profiles, "save").mockImplementation(async (i) => i.profile);
    vi.spyOn(ipc.security, "sessionDetails").mockResolvedValue({
      tls: false,
      tlsVersion: null,
      tlsCipher: null,
      encoding: null,
    });
    useConnectionsStore.getState().add(info("a"));
    wrap(<Toolbar launcher={false} />);
    fireEvent.click(await screen.findByRole("button", { name: /connection details/ }));
    const colours = await screen.findByRole("radiogroup", { name: "Connection colour" });
    expect(within(colours).getByRole("radio", { name: "green" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    fireEvent.click(within(colours).getByRole("radio", { name: "red" }));
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save.mock.calls[0]![0].profile).toMatchObject({ id: "p-a", color: "red" });
    await vi.waitFor(() => expect(useConnectionsStore.getState().byId.a!.info.color).toBe("red"));
    expect(screen.getByRole("dialog")).toHaveTextContent("Not encrypted");
  });

  it("tints the pill with the warning colour while the connection is lost", () => {
    useConnectionsStore.getState().add(info("a"));
    useConnectionsStore.getState().setStatus("a", "lost");
    wrap(<Toolbar launcher={false} />);
    const pill = document.querySelector("[data-slot=status-pill]") as HTMLElement;
    expect(pill).toHaveAttribute("data-state", "lost");
    expect(pill.getAttribute("style")).toContain("var(--warning)");
    expect(pill).toHaveAccessibleName(/^Connection lost : PostgreSQL 17\.6/);
  });

  it("says No connection without one", () => {
    wrap(<Toolbar launcher={false} />);
    expect(screen.getByRole("button", { name: /No connection/ })).toBeInTheDocument();
  });
});

describe("customisable toolbar", () => {
  const order = () =>
    Array.from(document.querySelectorAll("[data-slot=toolbar] button[aria-label]")).map((b) =>
      b.getAttribute("aria-label"),
    );

  it("renders the stored layout, spaces included, and follows the preference", () => {
    useConnectionsStore.getState().add(info("a"));
    wrap(<Toolbar launcher={false} />);
    expect(order()).toContain("Refresh");
    act(() => setPref("toolbarItems", ["refresh", "flexibleSpace", "openAnything", "space"]));
    expect(order()).toEqual(["Refresh", "Open anything"]);
    expect(document.querySelector("[data-slot=status-pill]")).toBeNull();
    expect(document.querySelector("[data-slot=toolbar-flexible-space]")).not.toBeNull();
    expect(document.querySelector("[data-slot=toolbar-space]")).not.toBeNull();
    act(() => setPref("toolbarItems", ["openAnything", "status", "refresh"]));
    expect(order()[0]).toBe("Open anything");
    expect(order().at(-1)).toBe("Refresh");
    expect(document.querySelector("[data-slot=status-pill]")).not.toBeNull();
  });

  it("shows the Metrics Board only while something handles it", () => {
    useConnectionsStore.getState().add(info("a"));
    wrap(<Toolbar launcher={false} />);
    expect(screen.queryByRole("button", { name: "Metrics board" })).toBeNull();
    let metrics = vi.fn();
    act(() => {
      metrics = spyAction("metrics.open");
    });
    const button = screen.getByRole("button", { name: "Metrics board" });
    fireEvent.click(button);
    expect(metrics).toHaveBeenCalledTimes(1);
    act(() => unregister.pop()!());
    expect(screen.queryByRole("button", { name: "Metrics board" })).toBeNull();
  });

  it("right-click offers Icon Only / Icon and Text", { timeout: 15_000 }, async () => {
    useConnectionsStore.getState().add(info("a"));
    wrap(<Toolbar launcher={false} />);
    fireEvent.contextMenu(document.querySelector("[data-slot=toolbar]")!);
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByRole("menuitemradio", { name: "Icon Only" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    fireEvent.click(within(menu).getByRole("menuitemradio", { name: "Icon and Text" }));
    expect(getPrefs().toolbarDisplay).toBe("iconAndText");
    expect(screen.getByRole("button", { name: "Refresh" })).toHaveTextContent("Refresh");
  });

  it(
    "Customize Toolbar… edits the layout: remove, add, repeat spaces, restore",
    { timeout: 15_000 },
    async () => {
      useConnectionsStore.getState().add(info("a"));
      wrap(<Toolbar launcher={false} />);
      fireEvent.contextMenu(document.querySelector("[data-slot=toolbar]")!);
      fireEvent.click(await screen.findByRole("menuitem", { name: "Customize Toolbar…" }));
      const sheet = await screen.findByRole("dialog", { name: "Customize Toolbar" });
      const strip = within(sheet).getByRole("list", { name: "Toolbar items" });
      expect(
        within(strip)
          .getAllByRole("listitem")
          .map((i) => i.getAttribute("data-item")),
      ).toEqual(DEFAULT_TOOLBAR_ITEMS);
      // ⌫ on a strip item removes it; the palette offers it again; double-click adds it back.
      const refresh = within(strip).getByRole("listitem", { name: "Refresh" });
      fireEvent.keyDown(refresh, { key: "Backspace" });
      expect(getPrefs().toolbarItems).not.toContain("refresh");
      fireEvent.doubleClick(within(sheet).getByRole("button", { name: "Add Refresh" }));
      expect(getPrefs().toolbarItems.at(-1)).toBe("refresh");
      // Spaces may repeat.
      fireEvent.doubleClick(within(sheet).getByRole("button", { name: "Add Flexible Space" }));
      fireEvent.doubleClick(within(sheet).getByRole("button", { name: "Add Flexible Space" }));
      expect(getPrefs().toolbarItems.filter((i) => i === "flexibleSpace")).toHaveLength(2);
      fireEvent.click(within(sheet).getByRole("button", { name: "Restore Default Set" }));
      expect(getPrefs().toolbarItems).toEqual(DEFAULT_TOOLBAR_ITEMS);
    },
  );
});

describe("WorkspaceRail", () => {
  it("shows while any connection is open, even a single one", () => {
    const { container } = wrap(<WorkspaceRail />);
    expect(container).toBeEmptyDOMElement();
    act(() => useConnectionsStore.getState().add(info("a")));
    expect(screen.getByRole("navigation", { name: "Open connections" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Connection manager" })).toBeInTheDocument();
    act(() => useConnectionsStore.getState().add(info("b", { driver: "mysql" })));
    const rail = screen.getByRole("navigation", { name: "Open connections" });
    // One icon per workspace: the database's initials (tabler_dev → TD).
    expect(within(rail).getByRole("button", { name: /Conn a/ })).toHaveTextContent("TD");
    expect(within(rail).getByRole("button", { name: /Conn b/ })).toHaveTextContent("TD");
    expect(within(rail).getByRole("button", { name: /Conn b/ })).toHaveAttribute(
      "aria-current",
      "true",
    );
  });

  it("switches the active connection and shows its workspace", () => {
    useConnectionsStore.getState().add(info("a"));
    useConnectionsStore.getState().add(info("b"));
    useUiStore.setState({ view: "launcher" });
    wrap(<WorkspaceRail />);
    fireEvent.click(screen.getByRole("button", { name: /Conn a/ }));
    expect(useConnectionsStore.getState().activeId).toBe("a");
    expect(useUiStore.getState().view).toBe("workspace");
    expect(screen.getByRole("button", { name: /Conn a/ })).toHaveAttribute("aria-current", "true");
  });

  it("opens the connection manager from +, highlighted while it shows", () => {
    useConnectionsStore.getState().add(info("a"));
    const launcher = spyAction("ui.launcher");
    wrap(<WorkspaceRail />);
    const plus = screen.getByRole("button", { name: "Connection manager" });
    expect(plus).not.toHaveAttribute("aria-current");
    fireEvent.click(plus);
    expect(launcher).toHaveBeenCalledTimes(1);
    act(() => useUiStore.setState({ view: "launcher" }));
    expect(plus).toHaveAttribute("aria-current", "true");
    expect(screen.getByRole("button", { name: /Conn a/ })).not.toHaveAttribute("aria-current");
  });

  it("disconnects from the context menu", async () => {
    const disconnect = vi.spyOn(ipc.connections, "disconnect").mockResolvedValue(undefined);
    useConnectionsStore.getState().add(info("a"));
    useConnectionsStore.getState().add(info("b"));
    useTabsStore.getState().openTable("a", users);
    wrap(<WorkspaceRail />);
    fireEvent.contextMenu(screen.getByRole("button", { name: /Conn a/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Disconnect" }));
    await vi.waitFor(() => expect(disconnect).toHaveBeenCalledWith("a"));
    await vi.waitFor(() => expect(useConnectionsStore.getState().order).toEqual(["b"]));
    expect(useTabsStore.getState().tabs).toHaveLength(0);
  });
});
