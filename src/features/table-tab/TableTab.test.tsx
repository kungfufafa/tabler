import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { actions } from "@/features/palette/actions";
import { emptyStaging, insertRow, pendingCount } from "@/features/staging/model";
import { ipc, type ApplyResult } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { TableTab } from "./TableTab";

const toast = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  message: vi.fn(),
}));
vi.mock("sonner", () => ({ toast }));

const ok: ApplyResult = {
  success: true,
  results: [],
  sql: ["INSERT 1"],
  error: null,
  durationMs: 3,
};

/** Opens public.users on a mock Postgres connection with one staged insert. */
async function setup(confirmCommit: boolean) {
  resetPrefsForTests({ ...DEFAULT_PREFS, confirmCommit });
  const info = await ipc.connections.connect("p-local-pg");
  useConnectionsStore.getState().add(info);
  const id = useTabsStore
    .getState()
    .openTable(info.connectionId, { schema: "public", name: "users" });
  const tab = useTabsStore.getState().tabs.find((t) => t.id === id) as Extract<
    Tab,
    { kind: "table" }
  >;
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <TableTab tab={tab} />
      </TooltipProvider>
    </QueryClientProvider>,
  );
  await waitFor(() => expect(useTableTabsStore.getState().slices[id]?.page).not.toBeNull());
  act(() =>
    useTableTabsStore.getState().update(id, { staging: insertRow(emptyStaging(), "new:1", []) }),
  );
  await waitFor(() => expect(actions.has("grid.commit")).toBe(true));
  const pending = () => pendingCount(useTableTabsStore.getState().slices[id]!.staging);
  return { pending };
}

beforeEach(() => {
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useTableTabsStore.setState({ slices: {} });
});

afterEach(() => {
  vi.restoreAllMocks();
  Object.values(toast).forEach((fn) => fn.mockReset());
  resetPrefsForTests();
});

describe("TableTab commit", () => {
  it("⌘S opens the Preview SQL sheet while 'Confirm before commit' is on", async () => {
    await setup(true);
    const preview = vi.spyOn(ipc.mutate, "preview").mockResolvedValue(["INSERT 1"]);
    const apply = vi.spyOn(ipc.mutate, "apply");
    await act(() => actions.run("grid.commit"));
    expect(preview).toHaveBeenCalledOnce();
    expect(apply).not.toHaveBeenCalled();
    expect(await screen.findByText("Preview changes")).toBeInTheDocument();
  });

  it("⌘S applies straight away when it is off", async () => {
    const { pending } = await setup(false);
    const preview = vi.spyOn(ipc.mutate, "preview");
    const apply = vi.spyOn(ipc.mutate, "apply").mockResolvedValue(ok);
    await act(() => actions.run("grid.commit"));
    expect(apply).toHaveBeenCalledOnce();
    expect(preview).not.toHaveBeenCalled();
    expect(pending()).toBe(0);
    expect(toast.success).toHaveBeenCalled();
    expect(screen.queryByText("Preview changes")).not.toBeInTheDocument();
  });

  it("shows a failed direct apply in the sheet and keeps the edits", async () => {
    const { pending } = await setup(false);
    vi.spyOn(ipc.mutate, "apply").mockResolvedValue({
      ...ok,
      success: false,
      sql: ["INSERT INTO users DEFAULT VALUES"],
      error: {
        code: "QUERY_FAILED",
        message: "null value in column email",
        detail: null,
        hint: null,
        sqlstate: "23502",
        vendorCode: null,
        position: null,
        statementIndex: 0,
        data: null,
      },
    });
    await act(() => actions.run("grid.commit"));
    expect(await screen.findByText("Preview changes")).toBeInTheDocument();
    expect(screen.getByText("INSERT INTO users DEFAULT VALUES")).toBeInTheDocument();
    expect(screen.getByText("null value in column email")).toBeInTheDocument();
    expect(pending()).toBe(1);
  });

  it("'grid.preview' always opens the sheet (the unsaved dialog's Review…)", async () => {
    await setup(false);
    const preview = vi.spyOn(ipc.mutate, "preview").mockResolvedValue(["INSERT 1"]);
    const apply = vi.spyOn(ipc.mutate, "apply");
    await act(() => actions.run("grid.preview"));
    expect(preview).toHaveBeenCalledOnce();
    expect(apply).not.toHaveBeenCalled();
    expect(await screen.findByText("Preview changes")).toBeInTheDocument();
  });
});
