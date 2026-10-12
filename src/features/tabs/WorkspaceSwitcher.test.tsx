import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import type { ConnectionInfo } from "@/lib/ipc";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { WorkspaceSwitcherHost } from "./WorkspaceSwitcher";
import { useWorkspaceSwitcher } from "./workspaces";

const info = (id: string, name: string, db: string): ConnectionInfo => ({
  connectionId: id,
  profileId: `p-${id}`,
  name,
  color: "green",
  driver: "postgres",
  serverVersion: "17",
  currentDatabase: db,
  currentUser: null,
  tunnel: null,
  readOnly: false,
});

describe("Show Workspaces", () => {
  let unregister: (() => void)[] = [];
  beforeEach(() => {
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    useTabsStore.setState({ tabs: [], activeId: null, closed: [], history: [], historyIndex: -1 });
    const s = useConnectionsStore.getState();
    s.add(info("a", "Local Postgres", "tabler_dev"));
    s.add(info("b", "Analytics", "warehouse"));
    s.setActive("a");
    useTabsStore.getState().newQuery("b");
    useTabsStore.getState().newQuery("b");
    useUiStore.setState({ view: "workspace" });
    useWorkspaceSwitcher.setState({ open: false });
  });
  afterEach(() => {
    unregister.forEach((u) => u());
    unregister = [];
  });

  const renderHost = () =>
    render(
      <KeyboardProvider>
        <WorkspaceSwitcherHost />
      </KeyboardProvider>,
    );

  it("opens with ⇧⌘\\ and lists workspaces with database and tab count", () => {
    renderHost();
    fireEvent.keyDown(document.body, {
      key: "\\",
      code: "Backslash",
      ctrlKey: true,
      shiftKey: true,
    });
    const list = screen.getByRole("listbox", { name: "Open workspaces" });
    const options = within(list).getAllByRole("option");
    expect(options.map((o) => o.textContent)).toEqual([
      expect.stringContaining("Local Postgrestabler_dev0 tabs"),
      expect.stringContaining("Analyticswarehouse2 tabs"),
      "Open Connection…",
    ]);
    expect(options[0]).toHaveAttribute("aria-selected", "true");
  });

  it("switches with the arrow keys and Return", () => {
    renderHost();
    act(() => useWorkspaceSwitcher.setState({ open: true }));
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Search workspaces" }), {
      key: "ArrowDown",
    });
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Search workspaces" }), { key: "Enter" });
    expect(useConnectionsStore.getState().activeId).toBe("b");
    expect(useWorkspaceSwitcher.getState().open).toBe(false);
  });

  it("filters, and its last row opens the connection manager", () => {
    const launcher = vi.fn();
    unregister.push(actions.register("ui.launcher", launcher));
    renderHost();
    act(() => useWorkspaceSwitcher.setState({ open: true }));
    const search = screen.getByRole("textbox", { name: "Search workspaces" });
    fireEvent.change(search, { target: { value: "ware" } });
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual([
      expect.stringContaining("Analytics"),
      "Open Connection…",
    ]);
    fireEvent.click(screen.getByRole("option", { name: /Open Connection/ }));
    expect(launcher).toHaveBeenCalledTimes(1);
  });
});
