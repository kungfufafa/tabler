import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { actions } from "@/features/palette/actions";
import { Sidebar } from "@/features/sidebar/Sidebar";
import {
  PANE_STORAGE_KEY,
  reloadSidebarPrefs,
  useSidebarPrefs,
} from "@/features/sidebar/sidebarPrefs";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { resetMockSavedQueries } from "@/lib/ipc/mock/queries";
import { useConnectionsStore } from "@/stores/connections";
import { QUERIES_PANE_KEY, reloadQueriesPane } from "./sidebarPane";
import { resetSavedQueriesStore } from "./store";

function connect() {
  const info: ConnectionInfo = {
    connectionId: "c1",
    profileId: "p-local-pg",
    name: "Local Postgres",
    color: null,
    driver: "postgres",
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

describe("sidebar panes: Items | Queries | History", () => {
  beforeEach(() => {
    vi.spyOn(ipc.schema, "schemas").mockResolvedValue([{ name: "public", isDefault: true }]);
    vi.spyOn(ipc.schema, "tables").mockResolvedValue([]);
    localStorage.removeItem(PANE_STORAGE_KEY);
    localStorage.removeItem(QUERIES_PANE_KEY);
    reloadSidebarPrefs();
    reloadQueriesPane();
    resetMockSavedQueries();
    resetSavedQueriesStore();
    connect();
  });
  afterEach(() => {
    vi.restoreAllMocks();
    localStorage.removeItem(QUERIES_PANE_KEY);
  });

  it("switches to Queries and back, and remembers it", async () => {
    const { unmount } = renderSidebar();
    expect(screen.getAllByRole("radio").map((r) => r.textContent)).toEqual([
      "Items",
      "Queries",
      "History",
    ]);
    fireEvent.click(radio("Queries"));
    expect(radio("Queries")).toHaveAttribute("aria-checked", "true");
    expect(await screen.findByRole("tree", { name: "Saved queries" })).toBeInTheDocument();
    expect(await screen.findByRole("treeitem", { name: "Recent users" })).toBeInTheDocument();
    unmount();

    reloadQueriesPane();
    renderSidebar();
    expect(radio("Queries")).toHaveAttribute("aria-checked", "true");
    fireEvent.click(radio("History"));
    expect(radio("History")).toHaveAttribute("aria-checked", "true");
    expect(useSidebarPrefs.getState().pane).toBe("history");
    expect(localStorage.getItem(QUERIES_PANE_KEY)).toBeNull();
    expect(screen.queryByRole("tree", { name: "Saved queries" })).toBeNull();
  });

  it("⇧⌘F goes back to the Items search from Queries", async () => {
    renderSidebar();
    fireEvent.click(radio("Queries"));
    await act(() => actions.run("sidebar.focus-search"));
    expect(radio("Items")).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("textbox", { name: "Search for item" })).toHaveFocus();
  });
});
