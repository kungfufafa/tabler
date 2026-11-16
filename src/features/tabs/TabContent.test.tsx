import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { TabContent } from "./TabContent";

describe("the empty workspace hint", () => {
  let unregister: Array<() => void> = [];
  beforeEach(() => {
    useTabsStore.setState({ tabs: [], activeId: null, splits: {} });
    useConnectionsStore.setState({ byId: {}, order: [], activeId: "c1" });
    useUiStore.setState({ sidebarOpen: true });
  });
  afterEach(() => {
    unregister.forEach((u) => u());
    unregister = [];
  });

  it("offers Open anything and New query, which run their actions", () => {
    const open = vi.fn();
    const newQuery = vi.fn();
    unregister.push(actions.register("palette.open", open));
    unregister.push(actions.register("tab.new-query", newQuery));
    render(<TabContent />);
    expect(screen.getByText("Select an item in the sidebar, or:")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Open anything/ }));
    fireEvent.click(screen.getByRole("button", { name: /New query/ }));
    expect(open).toHaveBeenCalledTimes(1);
    expect(newQuery).toHaveBeenCalledTimes(1);
    // Each shows its shortcut.
    expect(
      screen.getByRole("button", { name: /Open anything/ }).querySelector("kbd"),
    ).not.toBeNull();
    expect(screen.queryByRole("button", { name: /Show sidebar/ })).not.toBeInTheDocument();
  });

  it("does not point at a hidden sidebar, and offers to show it", () => {
    const toggle = vi.fn();
    unregister.push(actions.register("ui.toggle-sidebar", toggle));
    useUiStore.setState({ sidebarOpen: false });
    render(<TabContent />);
    expect(screen.queryByText(/sidebar, or/)).not.toBeInTheDocument();
    expect(screen.getByText("Nothing is open yet.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Show sidebar/ }));
    expect(toggle).toHaveBeenCalledTimes(1);
  });
});
