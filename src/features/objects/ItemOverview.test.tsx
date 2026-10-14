import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { TabContent } from "@/features/tabs/TabContent";
import { ipc, type ConnectionInfo, type OverviewItem } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import "./ItemOverview";
import { openItemOverview, overviewParams } from "./store";

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

const item = (name: string, comment: string | null = null): OverviewItem => ({
  name,
  schema: "public",
  kind: "table",
  estimatedRows: 1200,
  totalSize: 65536,
  dataSize: 49152,
  indexSize: 16384,
  engine: null,
  charset: null,
  collation: null,
  owner: "tabler",
  comment,
});

function renderTab() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TabContent />
    </QueryClientProvider>,
  );
}

describe("Item Overview tab", () => {
  beforeEach(() => {
    useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null, splits: {} });
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    useConnectionsStore.getState().add(info);
    vi.restoreAllMocks();
  });

  it("opens as a tab of the workspace, once per schema", async () => {
    vi.spyOn(ipc.objects, "overview").mockResolvedValue([
      item("users", "Registered accounts"),
      item("posts"),
    ]);
    openItemOverview({ connectionId: "c1", dialect: "postgres", schema: "public" });
    openItemOverview({ connectionId: "c1", dialect: "postgres", schema: "public" });
    const { tabs } = useTabsStore.getState();
    expect(tabs).toMatchObject([
      { kind: "tool", tool: "overview", title: "Overview: public", params: { schema: "public" } },
    ]);
    renderTab();
    expect(await screen.findByText("Registered accounts")).toBeInTheDocument();
    expect(screen.getAllByText("64 KB").length).toBeGreaterThan(0);
    expect(ipc.objects.overview).toHaveBeenCalledWith("c1", "public");
    // Double-clicking a row opens the item; the overview tab stays.
    fireEvent.doubleClick(screen.getByText("posts"));
    const after = useTabsStore.getState();
    expect(after.tabs.map((t) => t.kind)).toEqual(["tool", "table"]);
    expect(after.tabs.find((t) => t.id === after.activeId)).toMatchObject({
      kind: "table",
      table: { schema: "public", name: "posts" },
    });
  });

  it("restores only usable params", () => {
    expect(overviewParams({ schema: "public" })).toEqual({ schema: "public" });
    expect(overviewParams({ schema: "" })).toBeNull();
    expect(overviewParams(null)).toBeNull();
  });
});
