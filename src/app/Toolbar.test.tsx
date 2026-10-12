import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { actions, type ActionId } from "@/features/palette/actions";
import { useConnectionDialog } from "@/features/connections/store";
import { emptyStaging, insertRow } from "@/features/staging/model";
import { useStructureStore } from "@/features/structure/store";
import { ipc, type ConnectionInfo, type ConnectionProfile } from "@/lib/ipc";
import { useConnectionsStore, type OpenConnection } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useUiStore } from "@/stores/ui";
import { databaseLabel, serverLabel } from "./connectionLabels";
import { pillModel, pillText } from "./statusPillModel";
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

  it("enables the action control only while the visible tab has staged work", () => {
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

    // The structure view acts on its staged DDL, not on the data view's rows.
    act(() => useTabsStore.getState().setView(t, "structure"));
    expect(within(group).getByRole("button", { name: "Commit changes" })).toBeDisabled();
    act(() => useStructureStore.getState().stage(t, () => [{ op: "rawSql", sql: "x" }]));
    expect(within(group).getByRole("button", { name: "Commit 1 change" })).toBeEnabled();
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

  it("shows the read-only lock; its popover explains the mode", async () => {
    useConnectionsStore.getState().add(info("a", { readOnly: true }));
    wrap(<Toolbar launcher={false} />);
    const lock = screen.getByRole("button", { name: "Safe mode: read-only" });
    expect(lock).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(lock);
    const popover = await screen.findByRole("dialog", { name: "Read-only connection" });
    expect(lock).toHaveAttribute("aria-expanded", "true");
    expect(popover).toHaveTextContent("Edits and DDL are disabled for this connection.");
    expect(within(popover).getByRole("button", { name: "Edit Connection…" })).toBeInTheDocument();
  });

  it("the unlocked popover's Edit Connection… opens the connection's form", async () => {
    const profile = { id: "p-a", name: "Conn a", options: {} } as ConnectionProfile;
    vi.mocked(ipc.profiles.list).mockResolvedValue([profile]);
    useConnectionsStore.getState().add(info("a"));
    useConnectionDialog.setState({ open: false, profile: null, prefill: null });
    wrap(<Toolbar launcher={false} />);
    fireEvent.click(screen.getByRole("button", { name: "Safe mode: off" }));
    const popover = await screen.findByRole("dialog", { name: "Normal mode" });
    expect(popover).toHaveTextContent(
      "Queries run without confirmation. Safe-mode levels are coming.",
    );
    const edit = within(popover).getByRole("button", { name: "Edit Connection…" });
    await vi.waitFor(() => expect(edit).toBeEnabled());
    fireEvent.click(edit);
    // The form is mounted app-wide: it opens over the workspace, on the profile.
    expect(useUiStore.getState().view).not.toBe("launcher");
    expect(useConnectionDialog.getState()).toMatchObject({ open: true, profile });
    expect(screen.queryByRole("dialog", { name: "Normal mode" })).toBeNull();
  });

  it("puts the connection tag in the pill and opens the connection switcher on click", async () => {
    vi.mocked(ipc.profiles.list).mockResolvedValue([
      { id: "p-a", options: { tag: "production" } } as ConnectionProfile,
    ]);
    useConnectionsStore.getState().add(info("a"));
    const switcher = spyAction("palette.connections");
    wrap(<Toolbar launcher={false} />);
    const tag = await screen.findByText("PRODUCTION");
    expect(tag).toHaveAttribute("data-slot", "status-pill-tag");
    const pill = screen.getByRole("button", { name: /^PRODUCTION │ PostgreSQL 17\.6 : Conn a/ });
    fireEvent.click(pill);
    expect(switcher).toHaveBeenCalledTimes(1);
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

describe("WorkspaceRail", () => {
  it("shows while any connection is open, even a single one", () => {
    const { container } = wrap(<WorkspaceRail />);
    expect(container).toBeEmptyDOMElement();
    act(() => useConnectionsStore.getState().add(info("a")));
    expect(screen.getByRole("navigation", { name: "Open connections" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Connection manager" })).toBeInTheDocument();
    act(() => useConnectionsStore.getState().add(info("b", { driver: "mysql" })));
    const rail = screen.getByRole("navigation", { name: "Open connections" });
    expect(within(rail).getByRole("button", { name: "Conn a" })).toHaveTextContent("PG");
    expect(within(rail).getByRole("button", { name: "Conn b" })).toHaveTextContent("MY");
    expect(within(rail).getByRole("button", { name: "Conn b" })).toHaveAttribute(
      "aria-current",
      "true",
    );
  });

  it("switches the active connection and shows its workspace", () => {
    useConnectionsStore.getState().add(info("a"));
    useConnectionsStore.getState().add(info("b"));
    useUiStore.setState({ view: "launcher" });
    wrap(<WorkspaceRail />);
    fireEvent.click(screen.getByRole("button", { name: "Conn a" }));
    expect(useConnectionsStore.getState().activeId).toBe("a");
    expect(useUiStore.getState().view).toBe("workspace");
    expect(screen.getByRole("button", { name: "Conn a" })).toHaveAttribute("aria-current", "true");
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
    expect(screen.getByRole("button", { name: "Conn a" })).not.toHaveAttribute("aria-current");
  });

  it("disconnects from the context menu", async () => {
    const disconnect = vi.spyOn(ipc.connections, "disconnect").mockResolvedValue(undefined);
    useConnectionsStore.getState().add(info("a"));
    useConnectionsStore.getState().add(info("b"));
    useTabsStore.getState().openTable("a", users);
    wrap(<WorkspaceRail />);
    fireEvent.contextMenu(screen.getByRole("button", { name: "Conn a" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Disconnect" }));
    await vi.waitFor(() => expect(disconnect).toHaveBeenCalledWith("a"));
    await vi.waitFor(() => expect(useConnectionsStore.getState().order).toEqual(["b"]));
    expect(useTabsStore.getState().tabs).toHaveLength(0);
  });
});
