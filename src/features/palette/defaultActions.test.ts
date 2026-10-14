import { QueryClient } from "@tanstack/react-query";
import { renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setCell } from "@/features/staging/model";
import { getToolTab } from "@/features/tabs/toolTabs";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { actions } from "./actions";
import { openEditorFromTab, reloadWorkspace, useDefaultActions } from "./defaultActions";
import { usePaletteStore } from "./store";

const windowClose = vi.hoisted(() => vi.fn(async () => true));
vi.mock("@/hooks/useUnsavedGuard", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useUnsavedGuard")>()),
  requestWindowClose: windowClose,
}));

beforeEach(() => {
  windowClose.mockClear();
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTableTabsStore.setState({ slices: {} });
  useUiStore.setState({ view: "workspace" });
});

afterEach(() => {
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const info = (id: string): ConnectionInfo => ({
  connectionId: id,
  profileId: `p-${id}`,
  name: id,
  color: null,
  driver: "postgres",
  serverVersion: "17",
  currentDatabase: "app",
  currentUser: "me",
  tunnel: null,
  readOnly: false,
});

describe("default tab.close (⌘W: close tab / window)", () => {
  const inDialog = () => {
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    const input = document.createElement("input");
    dialog.append(input);
    document.body.append(dialog);
    input.focus();
    return input;
  };

  it("closes the active tab, but not from inside another modal", async () => {
    const { unmount } = renderHook(() => useDefaultActions());
    const id = useTabsStore.getState().newQuery("c1");

    const input = inDialog();
    await actions.run("tab.close");
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual([id]);
    // Inside a dialog it neither closes the tab nor the window.
    expect(windowClose).not.toHaveBeenCalled();

    input.blur();
    await actions.run("tab.close");
    expect(useTabsStore.getState().tabs).toEqual([]);
    expect(windowClose).not.toHaveBeenCalled();
    unmount();
  });

  it("closes the window when there is no active tab", async () => {
    const { unmount } = renderHook(() => useDefaultActions());
    useConnectionsStore.getState().add(info("c1"));
    // An empty workspace.
    await actions.run("tab.close");
    expect(windowClose).toHaveBeenCalledTimes(1);

    // The welcome screen (no connection at all) too.
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    await actions.run("tab.close");
    expect(windowClose).toHaveBeenCalledTimes(2);
    unmount();
  });

  it("closes the window, not a hidden tab, on the Connection Manager", async () => {
    const { unmount } = renderHook(() => useDefaultActions());
    useConnectionsStore.getState().add(info("c1"));
    const id = useTabsStore.getState().newQuery("c1");
    useUiStore.setState({ view: "launcher" });
    await actions.run("tab.close");
    expect(windowClose).toHaveBeenCalledTimes(1);
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual([id]);

    // Back in the workspace the tab closes, the window stays.
    useUiStore.setState({ view: "workspace" });
    await actions.run("tab.close");
    expect(useTabsStore.getState().tabs).toEqual([]);
    expect(windowClose).toHaveBeenCalledTimes(1);
    unmount();
  });

  it("does not close a tab of another workspace that is not the one on screen", async () => {
    const { unmount } = renderHook(() => useDefaultActions());
    useConnectionsStore.getState().add(info("c1"));
    useConnectionsStore.getState().add(info("c2"));
    useConnectionsStore.getState().setActive("c1");
    const other = useTabsStore.getState().newQuery("c2");
    await actions.run("tab.close");
    expect(windowClose).toHaveBeenCalledTimes(1);
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual([other]);
    unmount();
  });

  it("closes a table tab with an active tab in the workspace, asking about unsaved work", async () => {
    const { unmount } = renderHook(() => useDefaultActions());
    useConnectionsStore.getState().add(info("c1"));
    useTabsStore.getState().openTable("c1", { schema: "public", name: "users" });
    await actions.run("tab.close");
    expect(useTabsStore.getState().tabs).toEqual([]);
    expect(windowClose).not.toHaveBeenCalled();
    unmount();
  });
});

describe("Reload Workspace (⌘R)", () => {
  it("reloads the active tab and the sidebar, refetches the other clean table tabs and the schema", async () => {
    useConnectionsStore.getState().add(info("c1"));
    const tabs = useTabsStore.getState();
    const clean = tabs.openTable("c1", { schema: "public", name: "users" });
    const dirty = tabs.openTable("c1", { schema: "public", name: "posts" });
    const active = tabs.openTable("c1", { schema: "public", name: "tags" });
    const elsewhere = tabs.openTable("c2", { schema: "public", name: "users" });
    tabs.activate(active);
    const slices = useTableTabsStore.getState();
    for (const id of [clean, dirty, active, elsewhere]) slices.ensure(id);
    slices.update(dirty, (s) => ({
      staging: setCell(s.staging, "k1", 0, { t: "text", v: "x" }, [{ t: "text", v: "y" }]),
    }));
    const refresh = vi.fn();
    const sidebar = vi.fn();
    const offs = [
      actions.register("view.refresh", refresh),
      actions.observe("view.refresh", sidebar),
    ];
    const invalidate = vi.spyOn(ipc.schema, "invalidate").mockResolvedValue(undefined);
    const qc = new QueryClient();
    const invalidated = vi.spyOn(qc, "invalidateQueries");

    await reloadWorkspace(qc);

    expect(refresh).toHaveBeenCalledTimes(1);
    expect(sidebar).toHaveBeenCalledTimes(1);
    const token = (id: string) => useTableTabsStore.getState().slices[id]?.refreshToken;
    expect(token(clean)).toBe(1);
    // Staged edits keep their rows; the active tab reloads itself; other workspaces stay.
    expect(token(dirty)).toBe(0);
    expect(token(active)).toBe(0);
    expect(token(elsewhere)).toBe(0);
    expect(invalidate).toHaveBeenCalledWith("c1");
    const keys = invalidated.mock.calls.map(([f]) => JSON.stringify(f?.queryKey));
    expect(keys).toEqual(expect.arrayContaining(['["tables","c1"]', '["completion","c1"]']));
    offs.forEach((off) => off());
  });

  it("is ⌘R's action, and ⌥⌘R's stays the current tab's", async () => {
    const { unmount } = renderHook(() => useDefaultActions());
    expect(actions.has("workspace.reload")).toBe(true);
    const refresh = vi.fn();
    const off = actions.register("view.refresh", refresh);
    await actions.run("workspace.reload");
    expect(refresh).toHaveBeenCalledTimes(1);
    off();
    unmount();
  });
});

describe("⌘↩ outside the editor", () => {
  it("opens the SQL editor from a non-SQL tab and runs the current statement in a SQL tab", async () => {
    const newQuery = vi.fn();
    const runCurrent = vi.fn();
    const offs = [
      actions.register("tab.new-query", newQuery),
      actions.register("query.run-current", runCurrent),
    ];
    const tabs = useTabsStore.getState();
    tabs.activate(tabs.openTable("c1", { schema: "public", name: "users" }));
    await openEditorFromTab();
    expect(newQuery).toHaveBeenCalledTimes(1);
    tabs.activate(tabs.newQuery("c1"));
    await openEditorFromTab();
    expect(runCurrent).toHaveBeenCalledTimes(1);
    expect(newQuery).toHaveBeenCalledTimes(1);
    // Not from the welcome screen.
    useUiStore.setState({ view: "launcher" });
    await openEditorFromTab();
    expect(newQuery).toHaveBeenCalledTimes(1);
    offs.forEach((off) => off());
  });
});

describe("File ▸ New Workspace / New SQL Viewer", () => {
  it("opens the connection picker, and a SQL Viewer tab in the active workspace", async () => {
    await import("@/features/sqlviewer/SqlViewerTab");
    const { unmount } = renderHook(() => useDefaultActions());
    await actions.run("workspace.new");
    expect(usePaletteStore.getState()).toMatchObject({ open: true, mode: "connections" });
    usePaletteStore.getState().setOpen(false);

    useConnectionsStore.getState().add(info("c1"));
    await actions.run("sqlviewer.new");
    await actions.run("sqlviewer.new");
    const viewers = useTabsStore.getState().tabs.filter((t) => t.kind === "tool");
    // Every ⇧⌘O is a new, untitled viewer.
    expect(viewers).toHaveLength(2);
    expect(viewers[0]).toMatchObject({
      connectionId: "c1",
      tool: "sqlviewer",
      title: "SQL Viewer",
      params: { path: null },
    });
    expect(getToolTab("sqlviewer")).not.toBeNull();
    unmount();
  });
});
