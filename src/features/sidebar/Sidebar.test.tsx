import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { actions } from "@/features/palette/actions";
import { ipc, type ConnectionInfo, type Driver, type TableSummary } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { pinScope } from "@/stores/pins";
import { recordRecent, useRecentStore } from "@/stores/recent";
import { useTabsStore } from "@/stores/tabs";
import { Sidebar } from "./Sidebar";
import {
  COLLAPSED_STORAGE_KEY,
  LEGACY_OPTIONS_STORAGE_KEY,
  OPTIONS_STORAGE_KEY,
  PANE_STORAGE_KEY,
  reloadSidebarPrefs,
  setSidebarPane,
  useSidebarPrefs,
} from "./sidebarPrefs";

function table(name: string, kind: TableSummary["kind"] = "table"): TableSummary {
  return { schema: "public", name, kind, rowEstimate: 10, sizeBytes: null, comment: null };
}

const TABLES = [table("users"), table("posts"), table("tags"), table("user_stats", "view")];

function connect(driver: Driver = "postgres") {
  const info: ConnectionInfo = {
    connectionId: "c1",
    profileId: "p1",
    name: "Local",
    color: null,
    driver,
    serverVersion: "17.6",
    currentDatabase: "tabler_dev",
    currentUser: "tabler",
    tunnel: null,
    readOnly: false,
  };
  useConnectionsStore.setState({
    byId: { c1: { info, status: "connected", message: null, currentSchema: null } },
    order: ["c1"],
    activeId: "c1",
  });
}

function renderSidebar() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <Sidebar />
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

const radio = (name: string) => screen.getByRole("radio", { name });
const itemsSearch = () => screen.getByRole("textbox", { name: "Search for item" });
/** A table row of the tree (its accessible name ends with the row estimate). */
const tableRow = (name: string) =>
  screen.getAllByRole("button", { name: new RegExp(`^${name}\\d*$`) }).at(-1)!;
const openMenu = (label: string) =>
  fireEvent.keyDown(screen.getByRole("button", { name: label }), { key: "Enter" });

describe("Sidebar", () => {
  beforeEach(() => {
    vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(400);
    vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(240);
    vi.spyOn(ipc.schema, "schemas").mockResolvedValue([
      { name: "public", isDefault: true },
      { name: "inventory", isDefault: false },
    ]);
    vi.spyOn(ipc.schema, "tables").mockResolvedValue(TABLES);
    for (const key of [
      PANE_STORAGE_KEY,
      COLLAPSED_STORAGE_KEY,
      OPTIONS_STORAGE_KEY,
      LEGACY_OPTIONS_STORAGE_KEY,
    ])
      localStorage.removeItem(key);
    reloadSidebarPrefs();
    useRecentStore.setState({ recent: {} });
    useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
    connect();
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("asks for a connection when none is open", () => {
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    renderSidebar();
    expect(screen.getByText("No connection")).toBeInTheDocument();
    expect(screen.queryByRole("radiogroup")).toBeNull();
  });

  it("switches between Items and History and remembers the pane", async () => {
    const { unmount } = renderSidebar();
    expect(radio("Items")).toHaveAttribute("aria-checked", "true");
    expect(await screen.findByRole("button", { name: /^Tables/ })).toBeInTheDocument();
    fireEvent.click(radio("History"));
    expect(radio("History")).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("textbox", { name: "Search history" })).toBeInTheDocument();
    // The tree stays mounted (scroll position, search) but hidden.
    expect(screen.queryByRole("textbox", { name: "Search for item" })).toBeNull();
    expect(JSON.parse(localStorage.getItem(PANE_STORAGE_KEY)!)).toBe("history");

    unmount();
    reloadSidebarPrefs();
    renderSidebar();
    expect(radio("History")).toHaveAttribute("aria-checked", "true");
  });

  it("⇧⌘F focuses the Items search, from the History pane too", async () => {
    setSidebarPane("history");
    renderSidebar();
    await act(() => actions.run("sidebar.focus-search"));
    expect(radio("Items")).toHaveAttribute("aria-checked", "true");
    expect(itemsSearch()).toHaveFocus();
  });

  it("filters the tree, ↓ from the search focuses the first table, Esc clears", async () => {
    renderSidebar();
    await screen.findByRole("button", { name: /^Tables/ });
    fireEvent.change(itemsSearch(), { target: { value: "user" } });
    expect(screen.queryByRole("button", { name: /^posts/ })).toBeNull();
    fireEvent.keyDown(itemsSearch(), { key: "ArrowDown" });
    expect(tableRow("users")).toHaveFocus();
    // Only focus moves: nothing opens until the user picks a table.
    expect(useTabsStore.getState().tabs).toEqual([]);

    fireEvent.keyDown(itemsSearch(), { key: "Escape" });
    expect(itemsSearch()).toHaveValue("");
    expect(tableRow("posts")).toBeInTheDocument();
  });

  it("hides Recent and the row counts by default; ⚙ turns them on", async () => {
    recordRecent(pinScope("p1", "tabler_dev"), { schema: "public", name: "tags" });
    renderSidebar();
    await screen.findByRole("button", { name: /^Tables/ });
    expect(screen.queryByRole("button", { name: /^Recent/ })).toBeNull();
    expect(tableRow("users")).toHaveTextContent(/^users$/);

    openMenu("Sidebar options");
    const showRecent = screen.getByRole("menuitemcheckbox", { name: "Show Recent" });
    expect(showRecent).toHaveAttribute("aria-checked", "false");
    fireEvent.click(showRecent);
    expect(screen.getByRole("button", { name: /^Recent/ })).toBeInTheDocument();
    expect(useSidebarPrefs.getState().options.showRecent).toBe(true);

    openMenu("Sidebar options");
    const rowCounts = screen.getByRole("menuitemcheckbox", { name: "Show row counts" });
    expect(rowCounts).toHaveAttribute("aria-checked", "false");
    fireEvent.click(rowCounts);
    expect(tableRow("users")).toHaveTextContent("users10");
    expect(JSON.parse(localStorage.getItem(OPTIONS_STORAGE_KEY)!)).toMatchObject({
      showRecent: true,
      showRowCounts: true,
    });
  });

  it("+ offers New Table… and New Query", async () => {
    const newQuery = vi.fn();
    const unregister = actions.register("tab.new-query", newQuery);
    try {
      renderSidebar();
      await screen.findByRole("button", { name: /^Tables/ });
      openMenu("New item");
      fireEvent.click(screen.getByRole("menuitem", { name: /New Query/ }));
      expect(newQuery).toHaveBeenCalledOnce();

      openMenu("New item");
      fireEvent.click(screen.getByRole("menuitem", { name: "New Table…" }));
      expect(await screen.findByRole("dialog", { name: "New table" })).toBeInTheDocument();
    } finally {
      unregister();
    }
  });

  it("shows the schema picker only when there is a schema to pick", async () => {
    const { unmount } = renderSidebar();
    const picker = await screen.findByRole("combobox", { name: "Schema" });
    await waitFor(() => expect(picker).toHaveTextContent("public"));
    unmount();

    vi.spyOn(ipc.schema, "schemas").mockResolvedValue([{ name: "tabler_dev", isDefault: true }]);
    connect("mysql");
    renderSidebar();
    await screen.findByRole("button", { name: /^Tables/ });
    expect(screen.queryByRole("combobox")).toBeNull();
  });
});
