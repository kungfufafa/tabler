import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { Gauge } from "lucide-react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { actions, useAction } from "@/features/palette/actions";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { mockWindowConnections } from "@/lib/ipc/mock/windows";
import { useConnectionsStore } from "@/stores/connections";
import { groupTabs, registerUnsavedSource, useTabsStore, type Tab } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { dropTab, groupDropId } from "./tabDrag";
import { registerToolTab, type ToolTabProps } from "./toolTabs";
import { UnsavedChangesDialog } from "./UnsavedChangesDialog";
import { installConnectionClaims } from "./windowHandoff";
import { WindowHost } from "./WindowHost";
import { WorkspaceGroups } from "./WorkspaceGroups";

const info: ConnectionInfo = {
  connectionId: "c1",
  profileId: "p1",
  name: "Local PG",
  color: null,
  driver: "postgres",
  serverVersion: "PostgreSQL 17",
  currentDatabase: "app",
  currentUser: "me",
  tunnel: null,
  readOnly: false,
};

const refreshed: string[] = [];

/** A light tab body: its title, and a ⌘R handler that says which tab answered. */
function Note({ tab }: ToolTabProps) {
  useAction("view.refresh", () => void refreshed.push(tab.title));
  return <p data-testid="note">{tab.title}</p>;
}

const note = (id: string, title = id): Tab => ({
  id,
  kind: "tool",
  connectionId: "c1",
  tool: "note",
  title,
  params: { id },
});

const state = () => useTabsStore.getState();
const group = (g: 0 | 1) => groupTabs(state(), "c1", g).map((t) => t.id);

function renderWorkspace() {
  return render(
    <TooltipProvider>
      <WorkspaceGroups />
      <WindowHost />
      <UnsavedChangesDialog />
    </TooltipProvider>,
  );
}

let unregister: () => void = () => {};
beforeEach(() => {
  useTabsStore.setState({
    tabs: [],
    activeId: null,
    closed: [],
    queryCounter: 0,
    closeRequest: null,
    history: [],
    historyIndex: -1,
    splits: {},
  });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useConnectionsStore.getState().add(info);
  useUiStore.setState({ view: "workspace" });
  refreshed.splice(0);
  unregister = registerToolTab("note", { component: Note, icon: Gauge, title: () => "Note" });
  if (!Element.prototype.scrollIntoView) Element.prototype.scrollIntoView = () => {};
});
afterEach(() => {
  unregister();
  vi.restoreAllMocks();
});

describe("split workspace", () => {
  it("Split Right (⇧⌘D) shows two groups; only the focused one answers shortcuts", async () => {
    state().open(note("a", "Alpha"));
    renderWorkspace();
    expect(screen.getAllByTestId("note").map((n) => n.textContent)).toEqual(["Alpha"]);
    await act(() => actions.run("workspace.split-right"));
    const groups = document.querySelectorAll("[data-slot=tab-group]");
    expect(groups).toHaveLength(2);
    expect(within(groups[1] as HTMLElement).getByTestId("note")).toHaveTextContent("Alpha");
    expect(screen.getByRole("tablist", { name: "Left tab group" })).toBeInTheDocument();
    expect(screen.getByRole("tablist", { name: "Right tab group" })).toBeInTheDocument();
    // The copy (right, focused) answers ⌘R; clicking into the left group moves the focus there.
    await act(() => actions.run("view.refresh"));
    fireEvent.pointerDown(within(groups[0] as HTMLElement).getByTestId("note"));
    expect(state().activeId).toBe("a");
    await act(() => actions.run("view.refresh"));
    expect(refreshed).toEqual(["Alpha", "Alpha"]);
    expect(groups[0]).toHaveAttribute("data-focused", "true");
    expect(groups[1]).not.toHaveAttribute("data-focused");
  });

  it("closes the split when a group's last tab closes", () => {
    state().open(note("a"));
    state().open(note("b"));
    state().moveToGroup("b", 1);
    renderWorkspace();
    expect(document.querySelectorAll("[data-slot=tab-group]")).toHaveLength(2);
    act(() => state().close("b"));
    expect(document.querySelectorAll("[data-slot=tab-group]")).toHaveLength(0);
    expect(screen.getByTestId("note")).toHaveTextContent("a");
  });

  it("drops a tab on the other group (on a tab, or on its bar) to move it there", () => {
    for (const id of ["a", "b", "c"]) state().open(note(id));
    state().moveToGroup("c", 1);
    dropTab("a", "c");
    expect(group(1)).toEqual(["a", "c"]);
    dropTab("b", groupDropId(1));
    // The left group emptied: one group again, in strip order.
    expect(state().splits.c1).toBeUndefined();
    state().moveToGroup("a", 1);
    // Within a group a drop reorders.
    expect(group(0)).toEqual(["c", "b"]);
    dropTab("c", "b");
    expect(group(0)).toEqual(["b", "c"]);
    dropTab("c", "c");
    dropTab("c", null);
    expect(group(0)).toEqual(["b", "c"]);
  });
});

describe("window actions", () => {
  it("New Window and Move Tab to New Window go through the backend", async () => {
    const open = vi.spyOn(ipc.windows, "open").mockResolvedValue("w-2");
    state().open(note("a", "Alpha"));
    renderWorkspace();
    await act(() => actions.run("window.new"));
    expect(open).toHaveBeenLastCalledWith(null);
    await act(() => actions.run("window.move-tab"));
    await vi.waitFor(() => expect(state().tabs).toEqual([]));
    expect(open).toHaveBeenLastCalledWith({
      profileId: "p1",
      database: "app",
      schema: null,
      tab: { kind: "tool", tool: "note", title: "Alpha", params: { id: "a" } },
    });
  });

  it("tells the backend which connections the window owns", async () => {
    const report = vi.fn(async () => {});
    const uninstall = installConnectionClaims(report);
    expect(report).toHaveBeenLastCalledWith(["c1"]);
    useConnectionsStore.getState().add({ ...info, connectionId: "c0" });
    expect(report).toHaveBeenLastCalledWith(["c0", "c1"]);
    useConnectionsStore.getState().setActive("c1");
    expect(report).toHaveBeenCalledTimes(2);
    uninstall();
    // The mock backend records them per window.
    const undo = installConnectionClaims();
    await vi.waitFor(() => expect(mockWindowConnections.get("main")).toEqual(["c0", "c1"]));
    undo();
  });

  it("keeping the work of a window close calls a quit off", async () => {
    const cancelQuit = vi.spyOn(ipc.windows, "cancelQuit").mockResolvedValue();
    const remove = registerUnsavedSource((t) => (t.kind === "tool" ? 1 : 0));
    state().open(note("a"));
    renderWorkspace();
    act(() => void state().requestWindowClose());
    fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    expect(cancelQuit).toHaveBeenCalledTimes(1);
    expect(state().closeRequest).toBeNull();
    remove();
  });
});
