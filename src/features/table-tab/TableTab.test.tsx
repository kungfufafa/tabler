import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { DataGridProps } from "@/features/datagrid/DataGrid";
import { actions } from "@/features/palette/actions";
import { emptyStaging, insertRow, pendingCount } from "@/features/staging/model";
import { ipc, type ApplyResult, type TableRef } from "@/lib/ipc";
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

// The grid is replaced by a probe: tests drive TableTab through the props it hands the grid.
const grid = vi.hoisted(() => ({ props: null as DataGridProps | null }));
vi.mock("@/features/datagrid/DataGrid", () => ({
  DataGrid: (props: DataGridProps) => {
    grid.props = props;
    return null;
  },
}));
const gridProps = () => grid.props!;

const ok: ApplyResult = {
  success: true,
  results: [],
  sql: ["INSERT 1"],
  error: null,
  durationMs: 3,
};

interface SetupOptions {
  confirmCommit?: boolean;
  profile?: string;
  table?: TableRef;
  /** Stage one new row (so pending > 0). */
  stage?: boolean;
  limit?: number;
}

/** Opens a table (default public.users on the mock Postgres connection), optionally with one staged insert. */
async function setup({
  confirmCommit = true,
  profile = "p-local-pg",
  table = { schema: "public", name: "users" },
  stage = true,
  limit,
}: SetupOptions = {}) {
  resetPrefsForTests({ ...DEFAULT_PREFS, confirmCommit });
  const info = await ipc.connections.connect(profile);
  useConnectionsStore.getState().add(info);
  const id = useTabsStore.getState().openTable(info.connectionId, table);
  if (limit) useTableTabsStore.getState().ensure(id, { limit });
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
  await waitFor(() => expect(grid.props?.rowCount ?? 0).toBeGreaterThan(0));
  if (stage) {
    act(() =>
      useTableTabsStore.getState().update(id, { staging: insertRow(emptyStaging(), "new:1", []) }),
    );
    await waitFor(() => expect(actions.has("grid.commit")).toBe(true));
  }
  const slice = () => useTableTabsStore.getState().slices[id]!;
  const pending = () => pendingCount(slice().staging);
  return { id, info, slice, pending };
}

beforeEach(() => {
  grid.props = null;
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
    await setup({ confirmCommit: true });
    const preview = vi.spyOn(ipc.mutate, "preview").mockResolvedValue(["INSERT 1"]);
    const apply = vi.spyOn(ipc.mutate, "apply");
    await act(() => actions.run("grid.commit"));
    expect(preview).toHaveBeenCalledOnce();
    expect(apply).not.toHaveBeenCalled();
    expect(await screen.findByText("Preview changes")).toBeInTheDocument();
  });

  it("⌘S applies straight away when it is off", async () => {
    const { pending } = await setup({ confirmCommit: false });
    const preview = vi.spyOn(ipc.mutate, "preview");
    const apply = vi.spyOn(ipc.mutate, "apply").mockResolvedValue(ok);
    await act(() => actions.run("grid.commit"));
    expect(apply).toHaveBeenCalledOnce();
    expect(preview).not.toHaveBeenCalled();
    expect(pending()).toBe(0);
    expect(toast.success).toHaveBeenCalled();
    expect(screen.queryByText("Preview changes")).not.toBeInTheDocument();
  });

  it("⌘S on a table without a primary key always previews, with a warning", async () => {
    await setup({ confirmCommit: false, table: { schema: "public", name: "audit_log" } });
    const preview = vi.spyOn(ipc.mutate, "preview").mockResolvedValue(["INSERT 1"]);
    const apply = vi.spyOn(ipc.mutate, "apply");
    await act(() => actions.run("grid.commit"));
    expect(preview).toHaveBeenCalledOnce();
    expect(apply).not.toHaveBeenCalled();
    expect(await screen.findByText("Preview changes")).toBeInTheDocument();
    expect(screen.getByText(/^No primary key: each UPDATE\/DELETE matches/)).toBeInTheDocument();
  });

  it("shows a failed direct apply in the sheet and keeps the edits", async () => {
    const { pending } = await setup({ confirmCommit: false });
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
    await setup({ confirmCommit: false });
    const preview = vi.spyOn(ipc.mutate, "preview").mockResolvedValue(["INSERT 1"]);
    const apply = vi.spyOn(ipc.mutate, "apply");
    await act(() => actions.run("grid.preview"));
    expect(preview).toHaveBeenCalledOnce();
    expect(apply).not.toHaveBeenCalled();
    expect(await screen.findByText("Preview changes")).toBeInTheDocument();
  });

  it("the unsaved-changes dialog's Commit… runs the interrupted page change after the apply", async () => {
    const { slice, pending } = await setup({ confirmCommit: false, limit: 10 });
    vi.spyOn(ipc.mutate, "preview").mockResolvedValue(["INSERT 1"]);
    const apply = vi.spyOn(ipc.mutate, "apply").mockResolvedValue(ok);
    const next = await screen.findByRole("button", { name: "Next page" });
    await waitFor(() => expect(next).toBeEnabled());
    fireEvent.click(next);
    expect(await screen.findByText("1 pending change")).toBeInTheDocument();
    expect(slice().offset).toBe(0);
    fireEvent.click(screen.getByRole("button", { name: "Commit…" }));
    const applyButton = await screen.findByRole("button", { name: /^Apply/ });
    await waitFor(() => expect(applyButton).toBeEnabled());
    await act(async () => fireEvent.click(applyButton));
    expect(apply).toHaveBeenCalledOnce();
    await waitFor(() => expect(slice().offset).toBe(10));
    expect(pending()).toBe(0);
  });

  it("drops the interrupted change when the preview is dismissed", async () => {
    const { slice } = await setup({ confirmCommit: false, limit: 10 });
    vi.spyOn(ipc.mutate, "preview").mockResolvedValue(["INSERT 1"]);
    const apply = vi.spyOn(ipc.mutate, "apply").mockResolvedValue(ok);
    const next = await screen.findByRole("button", { name: "Next page" });
    await waitFor(() => expect(next).toBeEnabled());
    fireEvent.click(next);
    fireEvent.click(await screen.findByRole("button", { name: "Commit…" }));
    fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByText("Preview changes")).not.toBeInTheDocument());
    // A later ⌘S applies, but the dismissed page change does not come back.
    await act(() => actions.run("grid.commit"));
    expect(apply).toHaveBeenCalledOnce();
    expect(slice().offset).toBe(0);
  });
});

describe("TableTab refresh", () => {
  it("⌘R drops the backend's cached metadata for the table", async () => {
    const { info, slice } = await setup({ stage: false });
    await waitFor(() => expect(actions.has("view.refresh")).toBe(true));
    const invalidate = vi.spyOn(ipc.schema, "invalidate");
    const before = slice().refreshToken;
    await act(() => actions.run("view.refresh"));
    expect(invalidate).toHaveBeenCalledWith(info.connectionId, { schema: "public", name: "users" });
    await waitFor(() => expect(slice().refreshToken).toBe(before + 1));
  });

  it("still reloads when the backend invalidation fails", async () => {
    const { slice } = await setup({ stage: false });
    await waitFor(() => expect(actions.has("view.refresh")).toBe(true));
    vi.spyOn(ipc.schema, "invalidate").mockRejectedValue(new Error("gone"));
    const before = slice().refreshToken;
    await act(() => actions.run("view.refresh"));
    await waitFor(() => expect(slice().refreshToken).toBe(before + 1));
  });
});

describe("TableTab grid actions", () => {
  const colIndex = (name: string) => gridProps().columns.findIndex((c) => c.id === name);

  it("stages a paste (setValues) with a single undo step", async () => {
    const { slice, pending } = await setup({ stage: false });
    const name = colIndex("name");
    const nickname = colIndex("nickname");
    act(() =>
      gridProps().onAction!({
        type: "setValues",
        edits: [
          { r: 0, c: name, value: { t: "text", v: "Ann" } },
          { r: 0, c: nickname, value: { t: "null" } },
          { r: 1, c: name, value: { t: "text", v: "Bob" } },
        ],
      }),
    );
    expect(pending()).toBe(2);
    expect(slice().staging.undo).toHaveLength(1);
    const row0 = [...slice().staging.changes.values()][0]!;
    expect(row0.patch.get(name)).toEqual({ t: "text", v: "Ann" });
    act(() => gridProps().onAction!({ type: "undo" }));
    expect(pending()).toBe(0);
  });

  it("multi-cell Set NULL is one undo step", async () => {
    const { slice, pending } = await setup({ stage: false });
    const nickname = colIndex("nickname");
    const bio = colIndex("bio");
    act(() =>
      gridProps().onAction!({
        type: "setNull",
        cells: [
          { r: 0, c: nickname },
          { r: 0, c: bio },
          { r: 1, c: nickname },
        ],
      }),
    );
    expect(slice().staging.undo).toHaveLength(1);
    act(() => gridProps().onAction!({ type: "undo" }));
    expect(pending()).toBe(0);
  });

  it("skips Set DEFAULT on saved SQLite rows with a note, but allows it on new rows", async () => {
    const { slice, pending } = await setup({
      profile: "p-sample-sqlite",
      table: { schema: "main", name: "users" },
      stage: false,
    });
    const role = colIndex("role");
    act(() => gridProps().onAction!({ type: "setDefault", cells: [{ r: 0, c: role }] }));
    expect(toast.message).toHaveBeenCalledWith("SQLite can't set an existing value to DEFAULT", {
      description: "DEFAULT works on new rows only.",
    });
    expect(pending()).toBe(0);

    toast.message.mockReset();
    act(() => gridProps().onAction!({ type: "newRow" }));
    await waitFor(() => expect(gridProps().rowCount).toBeGreaterThan(0));
    const last = gridProps().rowCount - 1;
    act(() => gridProps().onAction!({ type: "setDefault", cells: [{ r: last, c: role }] }));
    expect(toast.message).not.toHaveBeenCalled();
    const inserted = slice().staging.changes.get(slice().staging.insertOrder[0]!)!;
    expect(inserted.patch.get(role)).toEqual({ t: "default" });
  });

  it("tells the inspector about the whole row and how to fetch the full value", async () => {
    const { info } = await setup({ stage: false });
    const { useInspectorStore } = await import("@/stores/inspector");
    const avatar = colIndex("avatar");
    const row = gridProps().getRow(0)!;
    act(() =>
      gridProps().onFocusCell!({
        pos: { r: 0, c: 0 },
        sourceIndex: avatar,
        column: gridProps().columns[avatar]!,
        value: row.cells[avatar]!,
        row,
      }),
    );
    const cell = useInspectorStore.getState().cell!;
    expect(cell.row?.focused).toBe(avatar);
    expect(cell.row?.columns).toBe(gridProps().columns);
    expect(cell.fetch).toEqual({
      connectionId: info.connectionId,
      table: { schema: "public", name: "users" },
      key: expect.objectContaining({ kind: "pk" }),
      column: "avatar",
    });
  });
});
