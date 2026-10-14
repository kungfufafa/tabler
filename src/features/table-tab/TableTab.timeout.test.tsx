import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { IpcError, ipc } from "@/lib/ipc";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import { useConnectionsStore } from "@/stores/connections";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { TableTab } from "./TableTab";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), message: vi.fn() } }));

const TIMEOUT = {
  code: "QUERY_TIMEOUT",
  message: "Query timed out after 300 s",
  hint: "The statement was cancelled. The limit is Settings ▸ Connections ▸ Query time out (0 = no limit).",
} as const;

async function open() {
  resetPrefsForTests({ ...DEFAULT_PREFS });
  const info = await ipc.connections.connect("p-local-pg");
  useConnectionsStore.getState().add(info);
  const id = useTabsStore
    .getState()
    .openTable(info.connectionId, { schema: "public", name: "users" }, "table");
  const tab = useTabsStore.getState().tabs.find((t) => t.id === id) as Extract<
    Tab,
    { kind: "table" }
  >;
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <KeyboardProvider>
      <QueryClientProvider client={qc}>
        <TooltipProvider>
          <TableTab tab={tab} />
        </TooltipProvider>
      </QueryClientProvider>
    </KeyboardProvider>,
  );
}

describe("a table load that the query time out stopped", () => {
  beforeEach(() => {
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  });
  afterEach(() => vi.restoreAllMocks());

  it("says so, with the hint that names the setting", async () => {
    vi.spyOn(ipc.browse, "rows").mockRejectedValue(new IpcError(TIMEOUT));
    await open();
    expect(await screen.findByText("Query failed")).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByText(/Query timed out after 300 s/)).toHaveTextContent(
        "Settings ▸ Connections ▸ Query time out",
      ),
    );
  });

  it("an ordinary load error stays just its message", async () => {
    vi.spyOn(ipc.browse, "rows").mockRejectedValue(
      new IpcError({ code: "DB_ERROR", message: 'relation "users" does not exist' }),
    );
    await open();
    expect(await screen.findByText('relation "users" does not exist')).toBeInTheDocument();
    expect(screen.queryByText(/Query time out/)).toBeNull();
  });
});
