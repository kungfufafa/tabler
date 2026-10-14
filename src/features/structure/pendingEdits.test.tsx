import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { DataGridProps } from "@/features/datagrid/DataGrid";
import { CommitTestHost } from "@/features/commit/testHost";
import { actions } from "@/features/palette/actions";
import { emptyStaging, insertRow, pendingCount } from "@/features/staging/model";
import { TableTab } from "@/features/table-tab/TableTab";
import { rowKeys } from "@/features/table-tab/useTableRows";
import { ipc, type TableRef } from "@/lib/ipc";
import { resetMockDdl } from "@/lib/ipc/mock/ddl";
import { useConnectionsStore } from "@/stores/connections";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { invalidateConnectionSchema, reportHeldTables } from "./invalidate";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

// The grid is replaced by a probe: the test drives TableTab through the props it hands the grid.
const grid = vi.hoisted(() => ({ props: null as DataGridProps | null }));
vi.mock("@/features/datagrid/DataGrid", () => ({
  DataGrid: (props: DataGridProps) => {
    grid.props = props;
    return null;
  },
}));

const users: TableRef = { schema: "public", name: "users" };

beforeEach(() => {
  grid.props = null;
  resetPrefsForTests({ ...DEFAULT_PREFS, confirmCommit: true });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useTableTabsStore.setState({ slices: {} });
});

afterEach(() => {
  vi.restoreAllMocks();
  Object.values(toast).forEach((fn) => fn.mockReset());
  resetPrefsForTests();
  resetMockDdl();
});

describe("a schema change from the SQL editor", () => {
  it("leaves a grid with staged edits alone, so they commit to the columns they were made on", async () => {
    const info = await ipc.connections.connect("p-local-pg");
    useConnectionsStore.getState().add(info);
    const id = useTabsStore.getState().openTable(info.connectionId, users);
    const tab = useTabsStore.getState().tabs.find((t) => t.id === id) as Extract<
      Tab,
      { kind: "table" }
    >;
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <TooltipProvider>
          <TableTab tab={tab} />
          <CommitTestHost />
        </TooltipProvider>
      </QueryClientProvider>,
    );
    await waitFor(() => expect(grid.props?.rowCount ?? 0).toBeGreaterThan(0));
    const before = grid.props!.columns.map((c) => c.name);
    expect(before.slice(0, 3)).toEqual(["id", "email", "name"]);

    // Stage `name` = 'X' on the first row (staged by column position).
    act(() => grid.props!.onEditCommit!(0, before.indexOf("name"), { t: "text", v: "X" }));
    const pending = () => pendingCount(useTableTabsStore.getState().slices[id]!.staging);
    expect(pending()).toBe(1);

    // The editor drops a column (the backend then drops its metadata cache), and the query tab
    // refreshes the connection's caches.
    await ipc.ddl.apply(info.connectionId, users, [
      { op: "dropColumn", table: users, name: "email" },
    ]);
    let held: TableRef[] = [];
    act(() => {
      held = invalidateConnectionSchema(qc, info.connectionId);
    });
    expect(held).toEqual([users]);
    const metaQuery = qc.getQueryCache().find({ queryKey: rowKeys.meta(info.connectionId, users) });
    expect(metaQuery?.state.isInvalidated).toBe(false);
    for (const prefix of ["rows", "count"])
      for (const q of qc.getQueryCache().findAll({ queryKey: [prefix, info.connectionId] }))
        expect(q.state.isInvalidated, prefix).toBe(false);

    // The user is told why the grid did not change.
    reportHeldTables(held);
    expect(toast.message).toHaveBeenCalledWith("Unsaved edits in users", expect.anything());

    // Commit: the change still targets `name`, not the column that moved into its position.
    const preview = vi.spyOn(ipc.workspace, "preview");
    await waitFor(() => expect(actions.has("grid.commit")).toBe(true));
    await act(() => actions.run("grid.commit"));
    await waitFor(() => expect(preview).toHaveBeenCalled());
    const group = preview.mock.calls[0]![1][0]!;
    if (group.kind !== "rows") throw new Error("not a rows group");
    const req = group.request;
    expect(req.changes).toHaveLength(1);
    expect(req.changes[0]).toMatchObject({
      op: "update",
      changes: { name: { t: "text", v: "X" } },
    });
    expect(grid.props!.columns.map((c) => c.name)).toEqual(before);
  });

  it("refreshes every other table's metadata, rows and count", () => {
    const c = "c1";
    const posts: TableRef = { schema: "public", name: "posts" };
    const withEdits = useTabsStore.getState().openTable(c, users);
    useTabsStore.getState().openTable(c, posts);
    useTableTabsStore
      .getState()
      .update(withEdits, { staging: insertRow(emptyStaging(), "new:1", []) });
    const qc = new QueryClient({
      defaultOptions: { queries: { retry: false, staleTime: Infinity } },
    });
    const page = { columns: null, filter: null, sort: [], limit: 100, offset: 0 };
    const keys = (t: TableRef) => ({
      table: rowKeys.meta(c, t),
      rows: rowKeys.page(c, t, page, 0),
      count: rowKeys.count(c, t, null, 0),
    });
    for (const t of [users, posts])
      for (const key of Object.values(keys(t))) qc.setQueryData(key, { seeded: true });
    const invalidated = (key: readonly unknown[]) =>
      qc.getQueryCache().find({ queryKey: key, exact: true })?.state.isInvalidated;

    expect(invalidateConnectionSchema(qc, c)).toEqual([users]);
    for (const [name, key] of Object.entries(keys(users)))
      expect(invalidated(key), `users ${name}`).toBe(false);
    for (const [name, key] of Object.entries(keys(posts)))
      expect(invalidated(key), `posts ${name}`).toBe(true);

    // Once the edits are committed or discarded, the next schema change reloads it too.
    useTableTabsStore.getState().update(withEdits, { staging: emptyStaging() });
    expect(invalidateConnectionSchema(qc, c)).toEqual([]);
    for (const [name, key] of Object.entries(keys(users)))
      expect(invalidated(key), `users ${name}`).toBe(true);
  });
});
