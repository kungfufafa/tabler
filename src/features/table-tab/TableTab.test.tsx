import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { DataGridProps } from "@/features/datagrid/DataGrid";
import { actions } from "@/features/palette/actions";
import { emptyStaging, insertRow, pendingCount } from "@/features/staging/model";
import { ipc, type ApplyResult, type TableRef } from "@/lib/ipc";
import { isMac } from "@/lib/keys/combo";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import { useConnectionsStore } from "@/stores/connections";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import { newFilterRow, useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { useExportStore } from "@/features/export/store";
import { useImportStore } from "@/features/import/store";
import { filterMemoryKey, rememberFilters, resetFilterMemoryForTests } from "./filterMemory";
import { TableTab } from "./TableTab";

const toast = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  message: vi.fn(),
}));
vi.mock("sonner", () => ({ toast }));

// The connection's safe-mode check (another package's): allowed unless a test says otherwise.
const guard = vi.hoisted(() => ({ allow: true, calls: [] as unknown[] }));
vi.mock("@/features/safety/guard", () => ({
  guardExecution: async (connectionId: string, request: unknown) => {
    guard.calls.push({ connectionId, request });
    return guard.allow;
  },
}));

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
  /** Open it as the sidebar's preview tab (single click). */
  preview?: boolean;
}

/** Opens a table (default public.users on the mock Postgres connection), optionally with one staged insert. */
async function setup({
  confirmCommit = true,
  profile = "p-local-pg",
  table = { schema: "public", name: "users" },
  stage = true,
  limit,
  preview = false,
}: SetupOptions = {}) {
  resetPrefsForTests({ ...DEFAULT_PREFS, confirmCommit });
  const info = await ipc.connections.connect(profile);
  useConnectionsStore.getState().add(info);
  const id = useTabsStore.getState().openTable(info.connectionId, table, "table", { preview });
  if (limit) useTableTabsStore.getState().ensure(id, { limit });
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
  const tabOf = () =>
    useTabsStore.getState().tabs.find((t) => t.id === id) as Exclude<Tab, { kind: "query" }>;
  return { id, info, slice, pending, tabOf };
}

const mod = { [isMac ? "metaKey" : "ctrlKey"]: true };
const colIndex = (name: string) => gridProps().columns.findIndex((c) => c.id === name);
const column = (name: string) => gridProps().columns[colIndex(name)]!;

beforeEach(() => {
  grid.props = null;
  resetFilterMemoryForTests();
  guard.allow = true;
  guard.calls = [];
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
    // The statements are generated for the connection's safe-mode check, not shown.
    const preview = vi.spyOn(ipc.mutate, "preview");
    const apply = vi.spyOn(ipc.mutate, "apply").mockResolvedValue(ok);
    await act(() => actions.run("grid.commit"));
    expect(apply).toHaveBeenCalledOnce();
    expect(preview).toHaveBeenCalledOnce();
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

describe("TableTab column guard", () => {
  it("records the page's columns with the first staged edit and forgets them without edits", async () => {
    const { id, slice } = await setup();
    await waitFor(() => expect(slice().stagedOn?.split("\u0000")).toContain("email"));
    act(() => useTableTabsStore.getState().update(id, { staging: emptyStaging() }));
    await waitFor(() => expect(slice().stagedOn).toBeNull());
  });

  it("refuses to commit edits staged against other columns", async () => {
    const { id, slice, pending } = await setup({ confirmCommit: false });
    await waitFor(() => expect(slice().stagedOn).not.toBeNull());
    // The page was reloaded with another structure since the edit was staged.
    act(() => useTableTabsStore.getState().update(id, { stagedOn: "id\u0000gone" }));
    const apply = vi.spyOn(ipc.mutate, "apply");
    const preview = vi.spyOn(ipc.mutate, "preview");
    await act(() => actions.run("grid.commit"));
    await act(() => actions.run("grid.preview"));
    expect(apply).not.toHaveBeenCalled();
    expect(preview).not.toHaveBeenCalled();
    expect(toast.error).toHaveBeenCalledWith(
      "The table's columns changed since these edits were made",
      expect.anything(),
    );
    expect(pending()).toBe(1);
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

  it("stages pasted whole rows as new rows, the columns they leave out at their defaults", async () => {
    const { slice } = await setup({ stage: false });
    const name = colIndex("name");
    const email = colIndex("email");
    const before = gridProps().rowCount;
    act(() =>
      gridProps().onAction!({
        type: "insertRows",
        rows: [
          [
            { c: name, value: { t: "text", v: "Cy" } },
            { c: email, value: { t: "text", v: "cy@x.io" } },
          ],
          [{ c: name, value: { t: "text", v: "Dee" } }],
        ],
      }),
    );
    await waitFor(() => expect(gridProps().rowCount).toBe(before + 2));
    const [first, second] = slice().staging.insertOrder.map((k) => slice().staging.changes.get(k)!);
    expect(first!.status).toBe("inserted");
    expect(first!.patch.get(name)).toEqual({ t: "text", v: "Cy" });
    expect(first!.patch.get(email)).toEqual({ t: "text", v: "cy@x.io" });
    expect(second!.patch.get(name)).toEqual({ t: "text", v: "Dee" });
    // `id` is a serial key: DEFAULT, like a new row's.
    expect(second!.patch.get(colIndex("id"))).toEqual({ t: "default" });
    expect(gridProps().getRow(before + 1)?.status).toBe("inserted");
  });

  it("Discard Changes reverts the selected rows", async () => {
    const { pending } = await setup({ stage: false });
    const name = colIndex("name");
    act(() =>
      gridProps().onAction!({
        type: "setValues",
        edits: [
          { r: 0, c: name, value: { t: "text", v: "Ann" } },
          { r: 1, c: name, value: { t: "text", v: "Bob" } },
        ],
      }),
    );
    expect(pending()).toBe(2);
    act(() => gridProps().onAction!({ type: "revertRows", rows: [0] }));
    expect(pending()).toBe(1);
    expect(gridProps().getRow(0)?.status).toBeUndefined();
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

describe("TableTab views and preview tabs", () => {
  it("shows the structure in the same tab: bottom switch and ⌃⌘]", async () => {
    const { id, tabOf } = await setup({ stage: false });
    const back = () => act(() => useTabsStore.getState().setView(id, "table"));
    const bar = screen.getByRole("toolbar", { name: "Table data" });
    expect(within(bar).getByRole("radio", { name: "Data" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    fireEvent.click(within(bar).getByRole("radio", { name: "Structure" }));
    expect(tabOf().kind).toBe("structure");
    back();
    await waitFor(() => expect(actions.has("view.structure")).toBe(true));
    await act(() => actions.run("view.structure"));
    expect(tabOf().kind).toBe("structure");
    expect(useTabsStore.getState().tabs).toHaveLength(1);
  });

  it("pins its preview tab once it has staged edits", async () => {
    const { tabOf } = await setup({ preview: true, stage: true });
    await waitFor(() => expect(tabOf().preview).toBeUndefined());
  });

  it("pins its preview tab once a filter is applied", async () => {
    const { id, tabOf } = await setup({ preview: true, stage: false });
    expect(tabOf().preview).toBe(true);
    act(() => useTableTabsStore.getState().update(id, { filtersVisible: true }));
    expect(tabOf().preview).toBe(true);
    act(() =>
      useTableTabsStore.getState().update(id, { filters: [newFilterRow("name", "eq", "Ann")] }),
    );
    await waitFor(() => expect(tabOf().preview).toBeUndefined());
  });
});

describe("TableTab staging keys", () => {
  it("redoes an undone edit (⇧⌘Z) only while there is something to redo", async () => {
    const { pending, slice } = await setup({ stage: false });
    const name = colIndex("name");
    act(() =>
      gridProps().onAction!({
        type: "setValues",
        edits: [{ r: 0, c: name, value: { t: "text", v: "Zed" } }],
      }),
    );
    expect(actions.has("grid.redo")).toBe(false);
    act(() => gridProps().onAction!({ type: "undo" }));
    expect(pending()).toBe(0);
    await waitFor(() => expect(actions.has("grid.redo")).toBe(true));
    await act(() => actions.run("grid.redo"));
    expect(pending()).toBe(1);
    expect([...slice().staging.changes.values()][0]!.patch.get(name)).toEqual({
      t: "text",
      v: "Zed",
    });
    await waitFor(() => expect(actions.has("grid.redo")).toBe(false));
  });

  it("Set Value ▸ EMPTY stages '' in text columns only", async () => {
    const { pending, slice } = await setup({ stage: false });
    act(() =>
      gridProps().onAction!({
        type: "setEmpty",
        cells: [
          { r: 0, c: colIndex("name") },
          { r: 0, c: colIndex("role") },
        ],
      }),
    );
    expect(pending()).toBe(1);
    const change = [...slice().staging.changes.values()][0]!;
    expect(change.patch.get(colIndex("name"))).toEqual({ t: "text", v: "" });
    expect(change.patch.has(colIndex("role"))).toBe(false);
    expect(toast.message).not.toHaveBeenCalled();

    act(() => gridProps().onAction!({ type: "setEmpty", cells: [{ r: 1, c: colIndex("role") }] }));
    expect(pending()).toBe(1);
    expect(toast.message).toHaveBeenCalledWith("EMPTY applies to text columns only");
  });
});

describe("TableTab filters", () => {
  it("a quick filter replaces the filters, shows the bar and applies", async () => {
    const { slice } = await setup({ stage: false });
    act(() =>
      gridProps().onQuickFilter!({
        column: column("name"),
        op: "in",
        values: [
          { t: "text", v: "Ann" },
          { t: "text", v: "Smith, J" },
        ],
      }),
    );
    expect(slice().filters).toEqual([
      expect.objectContaining({ column: "name", op: "in", value: 'Ann, "Smith, J"' }),
    ]);
    expect(slice().filtersVisible).toBe(true);
    expect(await screen.findByDisplayValue('Ann, "Smith, J"')).toBeInTheDocument();

    // An empty string cannot be an = value (blank conditions are skipped): it becomes IN ("").
    act(() =>
      gridProps().onQuickFilter!({
        column: column("nickname"),
        op: "ne",
        values: [{ t: "text", v: "" }],
      }),
    );
    expect(slice().filters).toEqual([
      expect.objectContaining({ column: "nickname", op: "notIn", value: '""' }),
    ]);

    act(() => gridProps().onQuickFilter!({ column: column("nickname"), op: "isNull", values: [] }));
    expect(slice().filters).toEqual([
      expect.objectContaining({ column: "nickname", op: "isNull", value: "" }),
    ]);
  });

  it("a quick filter asks about pending edits first", async () => {
    const { slice } = await setup({ stage: true });
    act(() =>
      gridProps().onQuickFilter!({
        column: column("name"),
        op: "eq",
        values: [{ t: "text", v: "Ann" }],
      }),
    );
    expect(await screen.findByText("1 pending change")).toBeInTheDocument();
    expect(slice().filters).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Discard and continue" }));
    expect(slice().filters).toEqual([expect.objectContaining({ column: "name", value: "Ann" })]);
  });

  it("Filter with Column focuses a new condition; ⌘I / ⇧⌘I / ⎋ work in the bar", async () => {
    const { slice } = await setup({ stage: false });
    act(() => gridProps().onFilterColumn!(column("email")));
    expect(slice().filtersVisible).toBe(true);
    const value = await screen.findByRole("textbox", { name: "Filter value" });
    expect(value).toHaveFocus();
    expect(screen.getByRole("combobox", { name: "Filter column" })).toHaveTextContent("email");

    fireEvent.change(value, { target: { value: "a@x.io" } });
    fireEvent.keyDown(value, { key: "i", code: "KeyI", ...mod });
    let values = screen.getAllByRole("textbox", { name: "Filter value" });
    expect(values.map((v) => (v as HTMLInputElement).value)).toEqual(["a@x.io", "a@x.io"]);
    expect(values[1]).toHaveFocus();

    // ⇧⌘I removes the focused condition (the bar's "filter" scope wins over the global key).
    fireEvent.change(values[1]!, { target: { value: "b@x.io" } });
    await waitFor(() => expect(actions.has("filter.remove")).toBe(true));
    fireEvent.keyDown(values[1]!, { key: "I", code: "KeyI", ...mod, shiftKey: true });
    values = screen.getAllByRole("textbox", { name: "Filter value" });
    expect(values.map((v) => (v as HTMLInputElement).value)).toEqual(["a@x.io"]);
    expect(values[0]).toHaveFocus();
    expect(slice().filters).toEqual([]);

    fireEvent.keyDown(values[0]!, { key: "Enter", code: "Enter", ...mod });
    expect(slice().filters).toEqual([
      expect.objectContaining({ column: "email", value: "a@x.io" }),
    ]);

    fireEvent.keyDown(values[0]!, { key: "Escape", code: "Escape" });
    expect(slice().filtersVisible).toBe(false);
  });

  it("⌥⌘F opens and closes the Columns menu", async () => {
    await setup({ stage: false });
    await waitFor(() => expect(actions.has("grid.columns")).toBe(true));
    await act(() => actions.run("grid.columns"));
    expect(await screen.findByText("Show all")).toBeInTheDocument();
    await act(() => actions.run("grid.columns"));
    await waitFor(() => expect(screen.queryByText("Show all")).not.toBeInTheDocument());
  });
});

describe("TableTab bottom bar", () => {
  it("shows the page's place, the row count and the query time, with the SELECT as its tooltip", async () => {
    const { slice, info } = await setup({ stage: false, limit: 10 });
    const { count } = await ipc.browse.count(
      info.connectionId,
      { schema: "public", name: "users" },
      null,
      false,
    );
    const summary = await screen.findByTestId("row-summary");
    await waitFor(() =>
      expect(summary).toHaveTextContent(
        new RegExp(`^1–10 of ${count.toLocaleString()} rows\\s*· 4 ms$`),
      ),
    );
    expect(summary).toHaveAttribute("title", slice().page!.sql);
    // No header toolbar above the grid any more.
    expect(screen.queryByRole("button", { name: "Refresh" })).not.toBeInTheDocument();
  });

  it("says how many of the shown rows are selected while the grid has a selection", async () => {
    await setup({ stage: false, limit: 10 });
    const summary = await screen.findByTestId("row-summary");
    await waitFor(() => expect(summary).toHaveTextContent(/^1–10 of /));
    act(() => gridProps().onSelectionChange!(3));
    expect(summary).toHaveTextContent(/^3 of 10 rows selected\s*· 4 ms$/);
    // Staged new rows count among the rows shown.
    act(() => gridProps().onAction!({ type: "newRow" }));
    await waitFor(() => expect(summary).toHaveTextContent(/^3 of 11 rows selected/));
    act(() => gridProps().onSelectionChange!(0));
    expect(summary).toHaveTextContent(/^1–10 of /);
  });

  it("[+ Row] runs the grid's new-row action; it is disabled on read-only tables", async () => {
    const { pending } = await setup({ stage: false });
    const add = screen.getByRole("button", { name: "Add row" });
    expect(add).toBeEnabled();
    // The grid's own handler (it focuses the new row) when it has one…
    const handler = vi.fn();
    const unregister = actions.register("grid.new-row", handler);
    fireEvent.click(add);
    expect(handler).toHaveBeenCalledOnce();
    expect(pending()).toBe(0);
    unregister();
    // …else the row is inserted directly.
    fireEvent.click(add);
    expect(pending()).toBe(1);
  });

  it("[+ Row] is disabled on a view", async () => {
    await setup({ stage: false, table: { schema: "public", name: "active_users" } });
    expect(screen.getByRole("button", { name: "Add row" })).toBeDisabled();
  });

  it("[Filters] toggles the bar and counts the applied conditions", async () => {
    const { id, slice } = await setup({ stage: false });
    const filters = screen.getByRole("button", { name: "Filters" });
    expect(filters).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(filters);
    expect(slice().filtersVisible).toBe(true);
    expect(filters).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("textbox", { name: "Filter value" })).toBeInTheDocument();
    // A blank condition filters nothing, so it is not counted.
    act(() =>
      useTableTabsStore.getState().update(id, {
        filters: [newFilterRow("name", "eq", "Ann"), newFilterRow("email", "eq", "")],
      }),
    );
    expect(filters).toHaveTextContent(/1$/);
    fireEvent.click(filters);
    expect(slice().filtersVisible).toBe(false);
  });

  it("⌘F opens the bar on the last applied filter's value, and keeps it open", async () => {
    const { id, slice } = await setup({ stage: false });
    // Applied: name = Ann and email = x; the unchecked and the blank rows are not applied.
    const skipped = { ...newFilterRow("role", "eq", "admin"), enabled: false };
    act(() =>
      useTableTabsStore.getState().update(id, {
        filters: [
          newFilterRow("name", "eq", "Ann"),
          newFilterRow("email", "eq", "x"),
          skipped,
          newFilterRow("name", "eq", ""),
        ],
      }),
    );
    await waitFor(() => expect(actions.has("grid.filter")).toBe(true));
    await act(() => actions.run("grid.filter"));
    expect(slice().filtersVisible).toBe(true);
    const values = await screen.findAllByRole("textbox", { name: "Filter value" });
    await waitFor(() => expect(document.activeElement).toBe(values[1]));
    expect(values[1]).toHaveValue("x");
    // Already open: ⌘F does not close it, it goes back to that value.
    act(() => values[0]!.focus());
    await act(() => actions.run("grid.filter"));
    expect(slice().filtersVisible).toBe(true);
    expect(document.activeElement).toBe(values[1]);
    // Nothing applied: the last row.
    act(() => useTableTabsStore.getState().update(id, { filters: [skipped] }));
    await act(() => actions.run("grid.filter"));
    const [only] = screen.getAllByRole("textbox", { name: "Filter value" });
    expect(document.activeElement).toBe(only);
  });

  it("the ⚙ popover goes to an offset, to the last page, and counts exactly", async () => {
    // An estimated count (as for a big table), exact on request.
    const count = vi
      .spyOn(ipc.browse, "count")
      .mockImplementation(async (_c, _t, _f, exact) =>
        exact ? { count: 50, exact: true } : { count: 1000, exact: false },
      );
    const { slice, info } = await setup({ stage: false, limit: 10 });
    const users = { schema: "public", name: "users" };
    const summary = screen.getByTestId("row-summary");
    await waitFor(() => expect(summary).toHaveTextContent(/^1–10 of ~1,000 rows/));
    const settings = () => screen.getByRole("button", { name: "Pagination settings" });

    fireEvent.click(settings());
    const offset = await screen.findByRole("spinbutton", { name: "Offset" });
    expect(screen.getByRole("combobox", { name: "Limit" })).toHaveTextContent("10 rows");
    fireEvent.change(offset, { target: { value: "25" } });
    fireEvent.click(screen.getByRole("button", { name: "Go" }));
    await waitFor(() => expect(slice().offset).toBe(25));
    await waitFor(() => expect(screen.queryByRole("spinbutton")).not.toBeInTheDocument());

    fireEvent.click(settings());
    const last = await screen.findByRole("button", { name: "Last page" });
    await waitFor(() => expect(last).toBeEnabled());
    fireEvent.click(last);
    await waitFor(() => expect(slice().offset).toBe(990));

    fireEvent.click(settings());
    fireEvent.click(await screen.findByRole("button", { name: "First page" }));
    await waitFor(() => expect(slice().offset).toBe(0));

    fireEvent.click(settings());
    fireEvent.click(await screen.findByRole("button", { name: "Count rows exactly" }));
    expect(count).toHaveBeenCalledWith(info.connectionId, users, null, true);
    await waitFor(() => expect(summary).toHaveTextContent(/^1–10 of 50 rows/));
    fireEvent.click(settings());
    expect(await screen.findByRole("button", { name: "Count rows exactly" })).toBeDisabled();
  });

  it("an estimated count in the summary is exact on a click", async () => {
    const count = vi
      .spyOn(ipc.browse, "count")
      .mockImplementation(async (_c, _t, _f, exact) =>
        exact ? { count: 50, exact: true } : { count: 1000, exact: false },
      );
    await setup({ stage: false, limit: 10 });
    fireEvent.click(await screen.findByRole("button", { name: "~1,000" }));
    expect(count).toHaveBeenLastCalledWith(expect.anything(), expect.anything(), null, true);
    await waitFor(() =>
      expect(screen.getByTestId("row-summary")).toHaveTextContent(/^1–10 of 50 rows/),
    );
  });

  it("paging asks about pending edits first", async () => {
    const { slice } = await setup({ stage: true, limit: 10 });
    fireEvent.click(screen.getByRole("button", { name: "Pagination settings" }));
    fireEvent.change(await screen.findByRole("spinbutton", { name: "Offset" }), {
      target: { value: "30" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Go" }));
    expect(await screen.findByText("1 pending change")).toBeInTheDocument();
    expect(slice().offset).toBe(0);
    fireEvent.click(screen.getByRole("button", { name: "Discard and continue" }));
    expect(slice().offset).toBe(30);
  });
});

describe("TableTab row details", () => {
  async function focus(r: number, column: string) {
    const { useInspectorStore } = await import("@/stores/inspector");
    const c = colIndex(column);
    const row = gridProps().getRow(r)!;
    act(() =>
      gridProps().onFocusCell!({
        pos: { r, c: 0 },
        sourceIndex: c,
        column: gridProps().columns[c]!,
        value: row.cells[c]!,
        row,
      }),
    );
    return () => useInspectorStore.getState().cell!;
  }

  it("stages edits made in the sidebar like the grid's, and shows the staged values", async () => {
    const { pending, slice } = await setup({ stage: false });
    const name = colIndex("name");
    const cell = await focus(0, "email");
    expect(cell().row?.edit).toBeTypeOf("function");
    act(() => cell().row!.edit!(name, { t: "text", v: "Zed" }));
    expect(pending()).toBe(1);
    expect([...slice().staging.changes.values()][0]!.patch.get(name)).toEqual({
      t: "text",
      v: "Zed",
    });
    // The sidebar follows the staging (the grid probe does not report the focus again).
    await waitFor(() => expect(cell().row?.cells[name]).toEqual({ t: "text", v: "Zed" }));
    expect(cell().row?.changed?.has(name)).toBe(true);
    expect(cell().row?.focused).toBe(colIndex("email"));
    // Undo in the grid takes it back there too.
    act(() => gridProps().onAction!({ type: "undo" }));
    await waitFor(() => expect(cell().row?.cells[name]).not.toEqual({ t: "text", v: "Zed" }));
  });

  it("a row staged for deletion is read-only in the sidebar", async () => {
    const { pending } = await setup({ stage: false });
    const cell = await focus(1, "name");
    const edit = cell().row!.edit!;
    act(() => gridProps().onAction!({ type: "deleteRows", rows: [1] }));
    await waitFor(() => expect(cell().row?.status).toBe("deleted"));
    expect(cell().row?.edit).toBeUndefined();
    // A stale callback does nothing either.
    act(() => edit(colIndex("name"), { t: "text", v: "x" }));
    expect(pending()).toBe(1);
  });

  it("applies the grid's Set Value guards", async () => {
    const { pending } = await setup({
      profile: "p-sample-sqlite",
      table: { schema: "main", name: "users" },
      stage: false,
    });
    const cell = await focus(0, "name");
    const edit = cell().row!.edit!;
    const notNull = gridProps().columns.findIndex((c) => c.editable && !c.nullable);
    const role = colIndex("role");
    expect(gridProps().columns[role]!.hasDefault).toBe(true);
    act(() => edit(notNull, { t: "null" }));
    act(() => edit(role, { t: "default" }));
    expect(toast.message).toHaveBeenCalledWith("SQLite can't set an existing value to DEFAULT", {
      description: "DEFAULT works on new rows only.",
    });
    expect(pending()).toBe(0);
    act(() => edit(role, { t: "text", v: "admin" }));
    expect(pending()).toBe(1);
  });

  it("offers no editing on a read-only table", async () => {
    await setup({ stage: false, table: { schema: "public", name: "active_users" } });
    const cell = await focus(0, "email");
    expect(cell().row).toBeDefined();
    expect(cell().row?.edit).toBeUndefined();
  });
});

describe("TableTab M8 grid features", () => {
  async function focus(r: number, column: string) {
    const { useInspectorStore } = await import("@/stores/inspector");
    const c = colIndex(column);
    const row = gridProps().getRow(r)!;
    act(() =>
      gridProps().onFocusCell!({
        pos: { r, c: 0 },
        sourceIndex: c,
        column: gridProps().columns[c]!,
        value: row.cells[c]!,
        row,
      }),
    );
    return () => useInspectorStore.getState().cell!;
  }

  it("commits only after the connection's safe-mode check, with the generated SQL", async () => {
    const { pending } = await setup({ confirmCommit: false });
    vi.spyOn(ipc.mutate, "preview").mockResolvedValue(["INSERT INTO users DEFAULT VALUES"]);
    const apply = vi.spyOn(ipc.mutate, "apply").mockResolvedValue(ok);
    guard.allow = false;
    await act(() => actions.run("grid.commit"));
    expect(guard.calls).toEqual([
      {
        connectionId: expect.any(String),
        request: { kind: "commit", statements: ["INSERT INTO users DEFAULT VALUES"] },
      },
    ]);
    expect(apply).not.toHaveBeenCalled();
    expect(pending()).toBe(1);
    guard.allow = true;
    await act(() => actions.run("grid.commit"));
    expect(apply).toHaveBeenCalledOnce();
    expect(pending()).toBe(0);
  });

  it("the Preview SQL sheet's Commit goes through the check too", async () => {
    await setup({ confirmCommit: true });
    vi.spyOn(ipc.mutate, "preview").mockResolvedValue(["INSERT 1"]);
    const apply = vi.spyOn(ipc.mutate, "apply").mockResolvedValue(ok);
    guard.allow = false;
    await act(() => actions.run("grid.commit"));
    fireEvent.click(await screen.findByRole("button", { name: /^Apply/ }));
    await waitFor(() => expect(guard.calls).toHaveLength(1));
    expect(apply).not.toHaveBeenCalled();
    expect(screen.getByText("Preview changes")).toBeInTheDocument();
  });

  it("several selected rows make the sidebar a bulk edit", async () => {
    const { pending, slice } = await setup({ stage: false });
    act(() => gridProps().onSelectedRowsChange!([0, 1]));
    const cell = await focus(0, "name");
    const name = colIndex("name");
    const row = () => cell().row!;
    expect(row().bulk?.count).toBe(2);
    // Different names; the same (NULL) avatar.
    expect(row().bulk?.mixed.has(name)).toBe(true);
    expect(row().bulk?.mixed.has(colIndex("avatar"))).toBe(false);
    act(() => row().edit!(name, { t: "text", v: "Same" }));
    expect(pending()).toBe(2);
    expect(slice().staging.undo).toHaveLength(1);
    for (const change of slice().staging.changes.values())
      expect(change.patch.get(name)).toEqual({ t: "text", v: "Same" });
    await waitFor(() => expect(row().bulk?.mixed.has(name)).toBe(false));
    // Back to one row: a plain Details view.
    act(() => gridProps().onSelectedRowsChange!([1]));
    await waitFor(() => expect(row().bulk).toBeUndefined());
  });

  it("Set Value ▸ NOW() stages the current time; Add a File… a file's bytes", async () => {
    const { slice } = await setup({ stage: false });
    const created = colIndex("last_login");
    act(() => gridProps().onAction!({ type: "setNow", cells: [{ r: 0, c: created }] }));
    const [change] = [...slice().staging.changes.values()];
    expect(change!.patch.get(created)).toMatchObject({
      t: "timestamp",
      v: expect.stringMatching(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/),
    });
    const avatar = colIndex("avatar");
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/avatar.png");
    act(() => gridProps().onAction!({ type: "setFile", cells: [{ r: 1, c: avatar }] }));
    await waitFor(() => expect(slice().staging.changes.size).toBe(2));
    const blob = [...slice().staging.changes.values()][1]!.patch.get(avatar);
    expect(blob).toMatchObject({ t: "bytes", v: { truncated: false } });
    expect(blob?.t === "bytes" && atob(blob.v.base64).startsWith("\x89PNG")).toBe(true);
  });

  it("⌘← / ⌘→ turn the page", async () => {
    const { slice } = await setup({ stage: false, limit: 10 });
    await act(() => actions.run("grid.prev-page"));
    expect(slice().offset).toBe(0);
    await act(() => actions.run("grid.next-page"));
    expect(slice().offset).toBe(10);
    await act(() => actions.run("grid.prev-page"));
    expect(slice().offset).toBe(0);
  });

  it("counts with the 'Estimate count' preference", async () => {
    const count = vi.spyOn(ipc.browse, "count");
    await setup({ stage: false });
    expect(count).toHaveBeenLastCalledWith(
      expect.any(String),
      expect.anything(),
      null,
      false,
      500_000,
    );
  });

  it("restores a table's remembered filters, and remembers new ones", async () => {
    const info = await ipc.connections.connect("p-local-pg");
    const key = filterMemoryKey(info.profileId, info.currentDatabase, {
      schema: "public",
      name: "users",
    });
    const remembered = [newFilterRow("name", "contains", "a")];
    rememberFilters(key, { filters: remembered, conjunction: "or", visible: true });
    const { slice, id } = await setup({ stage: false });
    expect(slice().filters).toEqual(remembered);
    expect(slice().conjunction).toBe("or");
    expect(slice().filtersVisible).toBe(true);
    act(() => useTableTabsStore.getState().update(id, { filtersVisible: false, filters: [] }));
    const { recallFilters } = await import("./filterMemory");
    expect(recallFilters(key)).toBeNull();
  });

  it("⇧⌘↩ in the filter bar applies with OR", async () => {
    const { slice, id } = await setup({ stage: false });
    act(() => useTableTabsStore.getState().update(id, { filtersVisible: true }));
    const value = await screen.findByRole("textbox", { name: "Filter value" });
    fireEvent.change(value, { target: { value: "1" } });
    act(() => value.focus());
    fireEvent.keyDown(value, { key: "Enter", code: "Enter", ...mod, shiftKey: true });
    await waitFor(() => expect(slice().filters).toHaveLength(1));
    expect(slice().conjunction).toBe("or");
  });

  it("wires the cell menu's Import / Export Current Page… and the FK viewer", async () => {
    const { id } = await setup({ stage: false, table: { schema: "public", name: "posts" } });
    act(() => gridProps().onImportCsv!());
    expect(useImportStore.getState().request).toMatchObject({
      kind: "csv",
      table: { schema: "public", name: "posts" },
    });
    act(() => gridProps().onExportPage!());
    expect(useExportStore.getState()).toMatchObject({
      open: true,
      target: { source: { kind: "table", tabId: id } },
      draft: { scope: "page" },
    });
    const fk = column("user_id");
    expect(fk.fk).toMatchObject({ table: "users", column: "id" });
    const userId = { t: "int" as const, v: 3 };
    const ref = await gridProps().onLoadReferenced!(fk, userId);
    expect(ref.table).toBe("public.users");
    expect(ref.rows).toHaveLength(1);
    expect(ref.rows[0]![ref.columns.findIndex((c) => c.name === "id")]).toEqual(userId);
    expect(gridProps().quickLookKey).toBe(true);
    expect(gridProps().rowNumbers).toBe(false);
  });
});
