import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import {
  ipc,
  type ConnectionInfo,
  type ExportProgress,
  type ExportRequest,
  type ExportResult,
  type ResultColumn,
  type Value,
} from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import { newFilterRow, useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";
import { newRun, resultId, type RunState } from "@/features/editor/runModel";
import { updateQueryTab, useQueryTabsStore } from "@/stores/queryTabs";
import { resultData } from "@/stores/resultData";
import { openExportCurrentPage } from "@/features/table-tab/gridActions";
import { runResultsOf } from "./allResults";
import { NotifyHost } from "@/features/notify/NotifyHost";
import { ExportHost } from "./ExportHost";
import { EXPORT_OPTIONS_KEY } from "./options";
import {
  closeExport,
  openExport,
  openExportForActiveTab,
  openExportTables,
  resetExportStore,
  useExportStore,
  type ExportSourceSpec,
} from "./store";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

let info: ConnectionInfo;

/** A connected mock Postgres with `table` open in a tab whose state the grid would have written. */
async function setup(table = "users") {
  info = await ipc.connections.connect("p-local-pg");
  useConnectionsStore.getState().add(info);
  const tabId = useTabsStore
    .getState()
    .openTable(info.connectionId, { schema: "public", name: table });
  useTableTabsStore.getState().ensure(tabId);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <ExportHost />
    </QueryClientProvider>,
  );
  await waitFor(() => expect(actions.has("export.open")).toBe(true));
  return tabId;
}

/** The table tab's Export button: exports that table's rows with its filter, sort and page. */
async function openFromShortcut() {
  await act(async () => {
    openExportForActiveTab();
  });
  return screen.findByRole("dialog");
}

/** ⇧⌘E / File ▸ Export…: the multi-table export. */
async function openExportKey() {
  await act(() => actions.run("export.open"));
  return screen.findByRole("dialog");
}

/** Spies on the export call (the mock still runs) and answers the save prompt with `path`. */
function spyRun(path = "/Users/me/Downloads/out.file") {
  vi.spyOn(window, "prompt").mockReturnValue(path);
  return vi.spyOn(ipc.exports, "run");
}

const requestOf = (spy: ReturnType<typeof spyRun>) => spy.mock.calls[0]![1] as ExportRequest;

/** An export that runs until the test finishes it, reporting progress on demand. */
function controlledRun(path = "/Users/me/Downloads/out.csv") {
  vi.spyOn(window, "prompt").mockReturnValue(path);
  const ctl = {
    progress: (_p: ExportProgress) => {},
    finish: (_r: ExportResult) => {},
  };
  const spy = vi.spyOn(ipc.exports, "run").mockImplementation(
    (_connectionId, _req, onProgress) =>
      new Promise<ExportResult>((resolve) => {
        ctl.progress = onProgress;
        ctl.finish = resolve;
      }),
  );
  return { spy, ctl };
}

const exportButton = (dialog: HTMLElement) =>
  within(dialog).getByRole("button", { name: /^Export…/ });

/** The progress bar's fill (its width is the percentage done). */
const progressFill = () =>
  within(screen.getByRole("dialog")).getByRole("status").querySelector<HTMLElement>("[style]");

beforeAll(() => {
  // jsdom lacks these; Radix Select needs them to open and scroll to the selected item.
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

beforeEach(() => {
  localStorage.clear();
  // The success toast (the "open containing folder" popup has tests of its own).
  resetPrefsForTests({ ...DEFAULT_PREFS, exportRevealPopup: false });
  resetExportStore();
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useTableTabsStore.setState({ slices: {} });
});

afterEach(() => {
  vi.restoreAllMocks();
  Object.values(toast).forEach((fn) => fn.mockReset());
});

describe("ExportDialog", () => {
  it("the table tab\u2019s Export button exports its rows with its filter and sort", async () => {
    const user = userEvent.setup();
    const tabId = await setup();
    useTableTabsStore.getState().update(tabId, {
      filters: [newFilterRow("id", "gt", "10")],
      sort: [{ column: "name", dir: "desc", nulls: null }],
      count: { count: 40, exact: true },
    });
    const dialog = await openFromShortcut();
    expect(within(dialog).getByText("public.users")).toBeInTheDocument();
    expect(within(dialog).getByText(/1 filter/)).toBeInTheDocument();
    expect(await within(dialog).findByText("40 rows match the filter")).toBeInTheDocument();

    await user.click(within(dialog).getByRole("radio", { name: "JSON" }));
    await user.click(within(dialog).getByRole("radio", { name: "NDJSON" }));
    const run = spyRun("/Users/me/Downloads/users.ndjson");
    await user.click(within(dialog).getByRole("button", { name: /^Export…/ }));

    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(window.prompt).toHaveBeenCalledWith(
      "Save export as",
      expect.stringMatching(/^\/Users\/me\/Downloads\/users_\d{8}-\d{4}\.ndjson$/),
    );
    const req = requestOf(run);
    expect(req.path).toBe("/Users/me/Downloads/users.ndjson");
    expect(req.format).toEqual({ format: "json", ndjson: true, pretty: false });
    expect(req.maxRows).toBeNull();
    expect(req.source).toMatchObject({
      kind: "table",
      table: { schema: "public", name: "users" },
      filter: {
        conjunction: "and",
        items: [{ column: "id", op: "gt", values: [{ t: "int", v: 10 }] }],
      },
      sort: [{ column: "name", dir: "desc", nulls: null }],
      columns: null,
    });
    const [title, opts] = toast.success.mock.calls[0]!;
    expect(title).toBe("Exported 40 rows");
    expect(opts.description).toMatch(/^users\.ndjson · /);
    expect(opts.action.label).toBe("Show in Finder");
    // Revealing is a no-op in the browser mock.
    expect(() => opts.action.onClick()).not.toThrow();
    // The dialog closes and the choices are remembered.
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(JSON.parse(localStorage.getItem(EXPORT_OPTIONS_KEY)!)).toMatchObject({
      format: "json",
      json: { ndjson: true },
    });
  });

  it("reopens with the last used format and exports only the current page", async () => {
    const user = userEvent.setup();
    localStorage.setItem(
      EXPORT_OPTIONS_KEY,
      JSON.stringify({
        format: "csv",
        csv: { delimiter: "\t", header: true, quoteAll: true, nullAs: "NULL" },
      }),
    );
    const tabId = await setup();
    useTableTabsStore.getState().update(tabId, {
      limit: 20,
      offset: 0,
      page: { rows: 20, durationMs: 1, sql: 'SELECT * FROM "users" LIMIT 20 OFFSET 0' },
      count: { count: 50, exact: true },
    });
    const dialog = await openFromShortcut();
    expect(within(dialog).getByRole("radio", { name: "Tab" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    await user.click(within(dialog).getByRole("radio", { name: "Current page" }));
    expect(within(dialog).getByText("Rows 1–20")).toBeInTheDocument();
    const run = spyRun();
    await user.click(within(dialog).getByRole("button", { name: /^Export…/ }));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(window.prompt).toHaveBeenCalledWith("Save export as", expect.stringMatching(/\.tsv$/));
    const req = requestOf(run);
    expect(req.maxRows).toBe(20);
    expect(req.source.kind).toBe("table");
    // Options stored by an earlier version keep their meaning.
    expect(req.format).toEqual({
      format: "csv",
      delimiter: "\t",
      header: true,
      quote: "double",
      nullAs: "NULL",
      lineBreak: "\n",
      decimal: ".",
      lineBreakToSpace: false,
    });
    expect(toast.success.mock.calls[0]![0]).toBe("Exported 20 rows");
  });

  it("shows progress and cancels a big export with a neutral toast", async () => {
    const user = userEvent.setup();
    await setup("events_big");
    const dialog = await openFromShortcut();
    spyRun();
    await user.click(within(dialog).getByRole("button", { name: /^Export…/ }));
    await waitFor(() => expect(useExportStore.getState().run?.rows ?? 0).toBeGreaterThan(0));
    expect(within(dialog).getByRole("status")).toHaveTextContent(/rows/);
    await user.click(within(dialog).getByRole("button", { name: "Cancel export" }));
    await waitFor(() =>
      expect(toast.message).toHaveBeenCalledWith("Export cancelled", {
        description: "No file was written.",
      }),
    );
    expect(toast.success).not.toHaveBeenCalled();
    expect(useExportStore.getState().run).toBeNull();
    // Still open, ready for another try.
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: /^Export…/ })).toBeEnabled();
  });

  it("exports a query passed by openExport and shows backend errors inline", async () => {
    const user = userEvent.setup();
    await setup();
    act(() =>
      openExport({
        connectionId: info.connectionId,
        source: { kind: "sql", sql: "SELECT 1;\nSELECT 2" },
      }),
    );
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Query result")).toBeInTheDocument();
    expect(within(dialog).queryByRole("radio", { name: "Current page" })).not.toBeInTheDocument();

    // INSERTs of a query go into `untitled` unless a table is named.
    await user.click(within(dialog).getByRole("radio", { name: "SQL" }));
    const exportButton = within(dialog).getByRole("button", { name: /^Export…/ });
    expect(exportButton).toBeEnabled();
    expect(within(dialog).getByLabelText("Table name")).toHaveAttribute("placeholder", "untitled");
    await user.type(within(dialog).getByLabelText("Table name"), "archive");
    expect(exportButton).toBeEnabled();

    const run = spyRun("/Users/me/Downloads/q.sql");
    await user.click(exportButton);
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "an export runs a single statement, but the SQL contains 2",
    );
    expect(requestOf(run).format).toMatchObject({
      format: "sqlInsert",
      tableName: "archive",
      batchSize: 100,
    });
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("does nothing when the save panel is dismissed", async () => {
    const user = userEvent.setup();
    await setup();
    const dialog = await openFromShortcut();
    vi.spyOn(window, "prompt").mockReturnValue(null);
    const run = vi.spyOn(ipc.exports, "run");
    await user.click(within(dialog).getByRole("button", { name: /^Export…/ }));
    expect(run).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("exports a page after the first with its hidden columns left out", async () => {
    const user = userEvent.setup();
    const tabId = await setup();
    const table = { schema: "public", name: "users" };
    // The grid's browse statement selects every column.
    const page = await ipc.browse.rows(info.connectionId, {
      table,
      columns: null,
      filter: null,
      sort: [],
      limit: 5,
      offset: 5,
    });
    expect(page.sql).toContain('"email"');
    useTableTabsStore.getState().update(tabId, {
      limit: 5,
      offset: 5,
      columnState: { widths: {}, order: [], hidden: ["email", "name"] },
      page: { rows: page.rows.length, durationMs: 1, sql: page.sql },
      count: { count: 40, exact: true },
    });
    const dialog = await openFromShortcut();
    const visibleOnly = await within(dialog).findByRole("checkbox", {
      name: /Only visible columns \(14 of 16\)/,
    });
    expect(visibleOnly).toBeChecked();
    await user.click(within(dialog).getByRole("radio", { name: "Current page" }));
    // Nothing blocks it: the page is a table query that starts at the offset.
    expect(within(dialog).queryByText(/Pages after the first/)).not.toBeInTheDocument();
    expect(exportButton(dialog)).toBeEnabled();
    expect(within(dialog).getByText(/^Rows 6–/)).toBeInTheDocument();
    const run = spyRun();
    await user.click(exportButton(dialog));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    const source = requestOf(run).source;
    expect(source).toMatchObject({ kind: "table", table, offset: 5 });
    const columns = source.kind === "table" ? source.columns : null;
    expect(columns).toHaveLength(14);
    expect(columns).not.toContain("email");
    expect(columns).not.toContain("name");
    expect(requestOf(run).maxRows).toBe(5);
  });

  it("exports a later page even while it is still loading", async () => {
    const user = userEvent.setup();
    const tabId = await setup("post_tags");
    const browse = (offset: number) =>
      `SELECT "post_id", "tag_id" FROM "public"."post_tags" LIMIT 200 OFFSET ${offset}`;
    // "Next" was clicked: the offset moved, the tab still holds page 1 while page 2 loads.
    useTableTabsStore.getState().update(tabId, {
      limit: 200,
      offset: 200,
      page: { rows: 200, durationMs: 1, sql: browse(0) },
      count: { count: 400, exact: true },
    });
    const dialog = await openFromShortcut();
    await user.click(within(dialog).getByRole("radio", { name: "Current page" }));
    // The rows of page 1 are not page 2's: the hint does not count them.
    expect(within(dialog).getByText("Up to 200 rows from row 201")).toBeVisible();
    expect(exportButton(dialog)).toBeEnabled();

    act(() =>
      useTableTabsStore.getState().update(tabId, {
        page: { rows: 200, durationMs: 1, sql: browse(200) },
      }),
    );
    expect(await within(dialog).findByText("Rows 201–400")).toBeInTheDocument();
    const run = spyRun();
    await user.click(exportButton(dialog));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(requestOf(run).source).toMatchObject({ kind: "table", offset: 200 });
    expect(requestOf(run).maxRows).toBe(200);
  });

  it("leaves generated columns out of an INSERT export of the visible columns", async () => {
    const user = userEvent.setup();
    const table = ipc.schema.table;
    vi.spyOn(ipc.schema, "table").mockImplementation(async (...args) => {
      const meta = await table(...args);
      return {
        ...meta,
        columns: meta.columns.map((c) => (c.name === "score" ? { ...c, isGenerated: true } : c)),
      };
    });
    const tabId = await setup();
    useTableTabsStore.getState().update(tabId, {
      columnState: { widths: {}, order: [], hidden: ["bio"] },
    });
    const dialog = await openFromShortcut();
    await user.click(within(dialog).getByRole("radio", { name: "SQL" }));
    const run = spyRun();
    await user.click(exportButton(dialog));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    const source = requestOf(run).source;
    const columns = source.kind === "table" ? source.columns : null;
    expect(columns).toContain("id");
    // Hidden, and generated (an INSERT of it would fail on import).
    expect(columns).not.toContain("bio");
    expect(columns).not.toContain("score");
  });

  it("reopens a running page export as it was started, with its progress", async () => {
    const user = userEvent.setup();
    const tabId = await setup();
    useTableTabsStore.getState().update(tabId, {
      limit: 20,
      offset: 0,
      page: { rows: 20, durationMs: 1, sql: 'SELECT * FROM "users" LIMIT 20 OFFSET 0' },
      count: { count: 1_000_000, exact: true },
    });
    const { spy, ctl } = controlledRun();
    let dialog = await openFromShortcut();
    await user.click(within(dialog).getByRole("radio", { name: "Current page" }));
    await user.click(exportButton(dialog));
    await waitFor(() => expect(spy).toHaveBeenCalledOnce());
    expect(requestOf(spy).maxRows).toBe(20);
    act(() => ctl.progress({ rows: 10, bytes: 840 }));
    expect(progressFill()!.style.width).toBe("50%");

    // Close while it runs, then reopen (⌘⇧E / the toolbar button).
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(useExportStore.getState().run).not.toBeNull();
    dialog = await openFromShortcut();
    expect(within(dialog).getByRole("radio", { name: "Current page" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    // 10 of the page's 20 rows, not of the table's million.
    expect(progressFill()!.style.width).toBe("50%");

    act(() =>
      ctl.finish({ path: "/Users/me/Downloads/out.csv", rows: 20, bytes: 1680, durationMs: 9 }),
    );
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
  });

  it("reopens a running query export with its INSERT table name", async () => {
    const user = userEvent.setup();
    await setup();
    const source = { kind: "sql" as const, sql: "SELECT 1" };
    act(() => openExport({ connectionId: info.connectionId, source }));
    let dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("radio", { name: "SQL" }));
    await user.type(within(dialog).getByLabelText("Table name"), "archive");
    const { spy, ctl } = controlledRun("/Users/me/Downloads/q.sql");
    await user.click(exportButton(dialog));
    await waitFor(() => expect(spy).toHaveBeenCalledOnce());
    expect(requestOf(spy).format).toMatchObject({ tableName: "archive" });

    act(() => closeExport());
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    act(() => openExport({ connectionId: info.connectionId, source }));
    dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByLabelText("Table name")).toHaveValue("archive");
    expect(within(dialog).queryByText(/needs a target table name/)).not.toBeInTheDocument();

    act(() => ctl.finish({ path: "/Users/me/Downloads/q.sql", rows: 1, bytes: 40, durationMs: 2 }));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    // The next export starts from a fresh form.
    act(() => openExport({ connectionId: info.connectionId, source }));
    dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByLabelText("Table name")).toHaveValue("");
  });

  describe("a result shown in the SQL editor", () => {
    const columns: ResultColumn[] = [
      { name: "id", dataType: "int8", kind: "int", nullable: false, table: "users" },
      { name: "email", dataType: "text", kind: "text", nullable: true, table: "users" },
    ];
    const rows: Value[][] = [
      [
        { t: "int", v: 1 },
        { t: "text", v: "ana@example.com" },
      ],
      [{ t: "int", v: 2 }, { t: "null" }],
    ];
    const held = (patch: Partial<Extract<ExportSourceSpec, { kind: "rows" }>> = {}) =>
      ({
        kind: "rows",
        columns,
        rows,
        label: "Result 2",
        table: "users",
        sql: "select id, email from users",
        truncated: false,
        stopped: null,
        ...patch,
      }) as const;

    it("writes the rows as they are, with no row or column choices", async () => {
      const user = userEvent.setup();
      await setup();
      act(() => openExport({ connectionId: info.connectionId, source: held() }));
      const dialog = await screen.findByRole("dialog");
      expect(within(dialog).getByText("Result 2")).toBeInTheDocument();
      expect(within(dialog).getByText("· 2 rows")).toBeInTheDocument();
      expect(within(dialog).getByText("select id, email from users")).toBeInTheDocument();
      expect(within(dialog).queryByRole("radio", { name: "Current page" })).toBeNull();
      expect(within(dialog).queryByRole("radio", { name: "All rows" })).toBeNull();
      expect(within(dialog).queryByText(/Only visible columns/)).toBeNull();

      // INSERTs go into the result's table unless another is named.
      await user.click(within(dialog).getByRole("radio", { name: "SQL" }));
      expect(within(dialog).getByLabelText("Table name")).toHaveAttribute("placeholder", "users");
      expect(exportButton(dialog)).toBeEnabled();
      const run = spyRun("/Users/me/Downloads/users.sql");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
      expect(window.prompt).toHaveBeenCalledWith(
        "Save export as",
        expect.stringMatching(/\/users_\d{8}-\d{4}\.sql$/),
      );
      const req = requestOf(run);
      expect(req.source).toEqual({ kind: "rows", columns, rows, table: "users" });
      expect(req.format).toMatchObject({ format: "sqlInsert", tableName: "", batchSize: 100 });
      expect(req.maxRows).toBeNull();
      expect(toast.success).toHaveBeenCalledWith("Exported 2 rows", expect.anything());
    });

    it("inserts into untitled when the result's table is unknown", async () => {
      const user = userEvent.setup();
      await setup();
      act(() =>
        openExport({ connectionId: info.connectionId, source: held({ table: null, sql: null }) }),
      );
      const dialog = await screen.findByRole("dialog");
      await user.click(within(dialog).getByRole("radio", { name: "SQL" }));
      // A join or a computed column: nothing to type, nothing blocks the export.
      expect(exportButton(dialog)).toBeEnabled();
      expect(within(dialog).queryByText(/needs a target table name/)).toBeNull();
      expect(within(dialog).getByLabelText("Table name")).toHaveAttribute(
        "placeholder",
        "untitled",
      );
      const run = spyRun("/Users/me/Downloads/join.sql");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
      expect(requestOf(run).format).toMatchObject({ format: "sqlInsert", tableName: "untitled" });
      vi.restoreAllMocks();

      // A name typed overrides it.
      act(() =>
        openExport({ connectionId: info.connectionId, source: held({ table: null, sql: null }) }),
      );
      const again = await screen.findByRole("dialog");
      await user.click(within(again).getByRole("radio", { name: "SQL" }));
      await user.type(within(again).getByLabelText("Table name"), "archive");
      const second = spyRun("/Users/me/Downloads/join2.sql");
      await user.click(exportButton(again));
      await waitFor(() => expect(toast.success).toHaveBeenCalledTimes(2));
      expect(requestOf(second).format).toMatchObject({ tableName: "archive" });
    });

    it("offers all rows of a cut-off result by running its statement again", async () => {
      const user = userEvent.setup();
      await setup();
      const source = held({ truncated: true, sql: "select * from events_big" });
      act(() => openExport({ connectionId: info.connectionId, source }));
      let dialog = await screen.findByRole("dialog");
      const shown = within(dialog).getByRole("radio", { name: "The 2 rows shown" });
      expect(shown).toHaveAttribute("aria-checked", "true");

      // Default: the rows shown.
      let run = spyRun("/Users/me/Downloads/shown.csv");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
      expect(requestOf(run).source).toMatchObject({ kind: "rows", rows });
      vi.restoreAllMocks();

      act(() => openExport({ connectionId: info.connectionId, source }));
      dialog = await screen.findByRole("dialog");
      await user.click(within(dialog).getByRole("radio", { name: "All rows" }));
      expect(within(dialog).getByText(/Runs the statement again/)).toBeInTheDocument();
      run = spyRun("/Users/me/Downloads/all.csv");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(toast.success).toHaveBeenCalledTimes(2));
      expect(requestOf(run).source).toEqual({ kind: "sql", sql: "select * from events_big" });
    });

    it("says when the statement stopped early, and offers to run it again", async () => {
      const user = userEvent.setup();
      await setup();
      const source = held({ stopped: "cancelled", sql: "select * from events_big" });
      act(() => openExport({ connectionId: info.connectionId, source }));
      const dialog = await screen.findByRole("dialog");
      expect(within(dialog).getByText("· 2 rows shown")).toBeInTheDocument();
      expect(within(dialog).getByText("· cancelled")).toBeInTheDocument();
      expect(within(dialog).getByText(/statement was cancelled/)).toBeInTheDocument();
      // Default: the rows that arrived, as shown; "All rows" runs the statement again.
      expect(within(dialog).getByRole("radio", { name: "The 2 rows shown" })).toHaveAttribute(
        "aria-checked",
        "true",
      );
      await user.click(within(dialog).getByRole("radio", { name: "All rows" }));
      const run = spyRun("/Users/me/Downloads/all.csv");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
      expect(requestOf(run).source).toEqual({ kind: "sql", sql: "select * from events_big" });
    });

    it("a result that failed partway says so too", async () => {
      await setup();
      act(() =>
        openExport({ connectionId: info.connectionId, source: held({ stopped: "error" }) }),
      );
      const dialog = await screen.findByRole("dialog");
      expect(within(dialog).getByText("· failed partway")).toBeInTheDocument();
      expect(within(dialog).getByText(/failed partway: only the rows/)).toBeInTheDocument();
      expect(within(dialog).getByRole("radio", { name: "All rows" })).toBeInTheDocument();
    });

    it("never runs a writing statement again", async () => {
      const user = userEvent.setup();
      await setup();
      const sql = "delete from users where id < 10 returning id, email";
      act(() =>
        openExport({ connectionId: info.connectionId, source: held({ truncated: true, sql }) }),
      );
      const dialog = await screen.findByRole("dialog");
      expect(within(dialog).getByText(sql)).toBeInTheDocument();
      expect(within(dialog).queryByRole("radio", { name: "All rows" })).toBeNull();
      expect(within(dialog).getByText(/only the rows shown are written/)).toBeInTheDocument();
      const run = spyRun("/Users/me/Downloads/deleted.csv");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
      expect(requestOf(run).source).toMatchObject({ kind: "rows", rows });
    });

    const capped: Value = { t: "bytes", v: { base64: "", len: 70_000, truncated: true } };

    it("exports a result with a cut binary value by running its statement again", async () => {
      const user = userEvent.setup();
      await setup();
      act(() =>
        openExport({
          connectionId: info.connectionId,
          source: held({ rows: [[{ t: "int", v: 1 }, capped]] }),
        }),
      );
      const dialog = await screen.findByRole("dialog");
      // Not cut at the row limit, yet the rows shown can't be written: "All rows" is the default.
      expect(within(dialog).getByRole("radio", { name: "All rows" })).toHaveAttribute(
        "aria-checked",
        "true",
      );
      expect(within(dialog).getByText(/binary values in full/)).toBeInTheDocument();
      const run = spyRun("/Users/me/Downloads/blob.csv");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
      expect(requestOf(run).source).toEqual({ kind: "sql", sql: "select id, email from users" });
      vi.restoreAllMocks();

      // The rows shown can't be exported as they are.
      act(() =>
        openExport({
          connectionId: info.connectionId,
          source: held({ rows: [[{ t: "int", v: 1 }, capped]] }),
        }),
      );
      const again = await screen.findByRole("dialog");
      await user.click(within(again).getByRole("radio", { name: "The 1 rows shown" }));
      expect(within(again).getByText(/cut to its 64 KiB preview/)).toBeInTheDocument();
      expect(exportButton(again)).toBeDisabled();
    });

    it("points to the table when running the statement again can't help", async () => {
      await setup();
      // MySQL caps blobs in query exports too; and a later result set has no statement to rerun.
      const mysql = await ipc.connections.connect("p-shop-mysql");
      useConnectionsStore.getState().add(mysql);
      act(() =>
        openExport({
          connectionId: mysql.connectionId,
          source: held({ rows: [[{ t: "int", v: 1 }, capped]] }),
        }),
      );
      let dialog = await screen.findByRole("dialog");
      expect(within(dialog).queryByRole("radio", { name: "All rows" })).toBeNull();
      expect(within(dialog).getByText(/Export its table instead/)).toBeInTheDocument();
      expect(exportButton(dialog)).toBeDisabled();

      act(() => closeExport());
      await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
      act(() =>
        openExport({
          connectionId: info.connectionId,
          source: held({ rows: [[{ t: "int", v: 1 }, capped]], sql: null }),
        }),
      );
      dialog = await screen.findByRole("dialog");
      expect(within(dialog).getByText(/Export its table instead/)).toBeInTheDocument();
      expect(exportButton(dialog)).toBeDisabled();
      await ipc.connections.disconnect(mysql.connectionId);
    });

    it("lets go of the result's rows once the dialog has closed", async () => {
      const user = userEvent.setup();
      await setup();
      act(() => openExport({ connectionId: info.connectionId, source: held() }));
      let dialog = await screen.findByRole("dialog");
      await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
      await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
      await waitFor(() => expect(useExportStore.getState().target).toBeNull());

      // Closed while the export runs: released when it ends.
      act(() => openExport({ connectionId: info.connectionId, source: held() }));
      dialog = await screen.findByRole("dialog");
      const { ctl } = controlledRun("/Users/me/Downloads/r.csv");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(useExportStore.getState().run).not.toBeNull());
      act(() => closeExport());
      await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
      expect(useExportStore.getState().target).not.toBeNull();
      act(() =>
        ctl.finish({ path: "/Users/me/Downloads/r.csv", rows: 2, bytes: 40, durationMs: 2 }),
      );
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
      expect(useExportStore.getState().target).toBeNull();
    });
  });

  /** The checkbox of the table labelled `label` in `list`. */
  const tableBox = (list: HTMLElement, label: string) =>
    within(within(list).getByText(label).closest("label")!).getByRole("checkbox");

  it("without a table tab, ⌘⇧E exports the current schema's tables", async () => {
    const user = userEvent.setup();
    await setup();
    act(() => {
      useConnectionsStore.getState().setActive(info.connectionId);
      useConnectionsStore.getState().setCurrentSchema(info.connectionId, "public");
      useTabsStore.getState().newQuery(info.connectionId);
    });
    const dialog = await openExportKey();
    // Every database is listed; the current one is open with the tables of all its schemas.
    const dbs = await within(dialog).findByRole("list", { name: "Databases" });
    expect(within(dbs).getByText("ecommerce_sales")).toBeInTheDocument();
    const list = await within(dialog).findByRole("list", { name: "Tables of tabler_dev" });
    await within(list).findByText("public.users");
    await waitFor(() =>
      expect(tableBox(list, "public.users")).toHaveAttribute("aria-checked", "true"),
    );
    // Only the current schema's tables are checked.
    expect(tableBox(list, "public.posts")).toHaveAttribute("aria-checked", "true");
    expect(tableBox(list, "inventory.items")).toHaveAttribute("aria-checked", "false");
    // Narrow to the tables matching "post": uncheck all (some → all → none), search, pick them.
    const all = within(dialog).getByRole("checkbox", { name: "Select all tables" });
    expect(all).toHaveAttribute("aria-checked", "mixed");
    await user.click(all);
    expect(all).toHaveAttribute("aria-checked", "true");
    await user.click(all);
    expect(all).toHaveAttribute("aria-checked", "false");
    await user.type(within(dialog).getByRole("textbox", { name: "Search tables" }), "post");
    const posts = within(list).getAllByRole("checkbox");
    expect(posts.length).toBeGreaterThan(1);
    for (const b of posts) await user.click(b);
    await user.click(within(dialog).getByRole("radio", { name: "SQL" }));
    expect(within(dialog).getByRole("radio", { name: "One file" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    await user.click(within(dialog).getByRole("checkbox", { name: "Include table structure" }));
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Downloads/public.sql");
    const run = vi.spyOn(ipc.exports, "runTables");
    await user.click(exportButton(dialog));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(run).toHaveBeenCalledOnce();
    const req = run.mock.calls[0]![1];
    expect(req.layout).toBe("singleFile");
    expect(req.target).toBe("/Users/me/Downloads/public.sql");
    expect(req.database).toBeNull();
    expect(req.encoding).toBe("utf8");
    expect(req.tables.length).toBe(posts.length);
    expect(req.tables.every((t) => t.name.includes("post"))).toBe(true);
    expect(req.format).toMatchObject({ format: "sqlInsert", structure: true, content: true });
    expect(toast.success.mock.calls[0]![0]).toBe(`Exported ${posts.length} tables`);
  });

  it("writes Excel files per table into a folder when asked", async () => {
    const user = userEvent.setup();
    await setup();
    act(() => openExportTables(info.connectionId, [{ schema: "public", name: "users" }]));
    const dialog = await screen.findByRole("dialog");
    const list = await within(dialog).findByRole("list", { name: "Tables of tabler_dev" });
    await within(list).findByText("public.users");
    // Only the table passed is checked.
    await waitFor(() => expect(within(dialog).getByText(/· 1 of \d+ tables/)).toBeInTheDocument());
    await user.click(within(dialog).getByRole("radio", { name: "Excel" }));
    // A workbook is never transcoded: no encoding choice.
    expect(within(dialog).queryByRole("combobox", { name: "Encoding" })).toBeNull();
    await user.click(within(dialog).getByRole("radio", { name: "One file per table" }));
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Exports/");
    const run = vi.spyOn(ipc.exports, "runTables");
    await user.click(exportButton(dialog));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(window.prompt).toHaveBeenCalledWith("Export the tables into folder", expect.any(String));
    expect(run.mock.calls[0]![1]).toMatchObject({
      layout: "filePerTable",
      target: "/Users/me/Exports",
      tables: [{ schema: "public", name: "users" }],
      format: { format: "xlsx", header: true },
      database: null,
      createFolder: false,
    });
  });

  it("exports tables of several databases, a folder per database", async () => {
    const user = userEvent.setup();
    await setup();
    act(() => openExportTables(info.connectionId, [{ schema: "public", name: "users" }]));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByRole("list", { name: "Tables of tabler_dev" });
    // Checking a closed database opens it and checks all its tables once they are loaded.
    await user.click(
      await within(dialog).findByRole("checkbox", { name: "Select all tables of ecommerce_sales" }),
    );
    const other = await within(dialog).findByRole("list", { name: "Tables of ecommerce_sales" });
    await within(other).findByText("orders");
    await waitFor(() => expect(tableBox(other, "orders")).toHaveAttribute("aria-checked", "true"));
    expect(
      within(dialog).getByRole("checkbox", { name: "Select all tables of ecommerce_sales" }),
    ).toHaveAttribute("aria-checked", "true");
    expect(within(dialog).getByText(/into a folder, a folder per database/)).toBeInTheDocument();
    // One file per table in CSV, in the encoding chosen.
    await user.click(within(dialog).getByRole("radio", { name: "CSV" }));
    await user.click(within(dialog).getByRole("combobox", { name: "Encoding" }));
    await user.click(await screen.findByRole("option", { name: "UTF-8 with BOM" }));
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Exports");
    const run = vi.spyOn(ipc.exports, "runTables");
    await user.click(exportButton(dialog));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(run).toHaveBeenCalledTimes(2);
    expect(run.mock.calls[0]![1]).toMatchObject({
      tables: [{ schema: "public", name: "users" }],
      target: "/Users/me/Exports/tabler_dev",
      database: null,
      createFolder: true,
      encoding: "utf8Bom",
      layout: "filePerTable",
    });
    const second = run.mock.calls[1]![1];
    expect(second).toMatchObject({
      target: "/Users/me/Exports/ecommerce_sales",
      database: "ecommerce_sales",
      createFolder: true,
    });
    expect(second.tables.map((t) => t.name)).toEqual([
      "customers",
      "orders",
      "order_items",
      "daily_revenue",
    ]);
    expect(toast.success.mock.calls[0]![0]).toBe("Exported 5 tables from 2 databases");
  });

  it("Export Single Table… switches to the export of the one table checked", async () => {
    const user = userEvent.setup();
    await setup();
    act(() => openExportTables(info.connectionId, [{ schema: "public", name: "users" }]));
    const dialog = await screen.findByRole("dialog");
    await user.click(await within(dialog).findByRole("button", { name: "Export Single Table…" }));
    await waitFor(() =>
      expect(
        within(screen.getByRole("dialog")).queryByRole("list", { name: "Databases" }),
      ).toBeNull(),
    );
    const single = screen.getByRole("dialog");
    expect(within(single).getAllByText(/users/).length).toBeGreaterThan(0);
    // HTML with an encoding.
    await user.click(within(single).getByRole("radio", { name: "HTML" }));
    await user.click(within(single).getByRole("combobox", { name: "Encoding" }));
    await user.click(await screen.findByRole("option", { name: "Windows-1252" }));
    const spy = spyRun("/Users/me/Downloads/users.html");
    await user.click(exportButton(single));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(requestOf(spy)).toMatchObject({
      source: { kind: "table", table: { schema: "public", name: "users" } },
      format: { format: "html" },
      encoding: "windows1252",
      path: "/Users/me/Downloads/users.html",
    });
  });

  it("shows the open containing folder popup after an export when the preference is on", async () => {
    const user = userEvent.setup();
    resetPrefsForTests(DEFAULT_PREFS);
    await setup();
    render(<NotifyHost />);
    const dialog = await openFromShortcut();
    const spy = spyRun("/Users/me/Downloads/users.csv");
    await user.click(within(dialog).getByRole("radio", { name: "CSV" }));
    await user.click(exportButton(dialog));
    const popup = await screen.findByRole("alertdialog");
    expect(spy).toHaveBeenCalledOnce();
    expect(within(popup).getByText("Export completed")).toBeInTheDocument();
    expect(within(popup).getByText(/^Exported \d+ rows$/)).toBeInTheDocument();
    expect(within(popup).getByText(/users\.csv · /)).toBeInTheDocument();
    expect(within(popup).getByRole("button", { name: "Show in Finder" })).toBeInTheDocument();
    expect(toast.success).not.toHaveBeenCalled();
    // OK has the focus: Return closes it.
    expect(within(popup).getByRole("button", { name: "OK" })).toHaveFocus();
    await user.keyboard("{Enter}");
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  /** The "Encoding" select of the dialog. */
  const encodingSelect = (dialog: HTMLElement) =>
    within(dialog).getByRole("combobox", { name: "Encoding" });

  describe("CSV Space quote and custom decimal separator", () => {
    it("offers Space as a quote and sends it", async () => {
      const user = userEvent.setup();
      await setup();
      const dialog = await openFromShortcut();
      const quotes = within(dialog).getByRole("radiogroup", { name: "Quote" });
      expect(within(quotes).getAllByRole("radio")).toHaveLength(4);
      await user.click(within(quotes).getByRole("radio", { name: "Space" }));
      const run = spyRun("/Users/me/Downloads/space.csv");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
      expect(requestOf(run).format).toMatchObject({ format: "csv", quote: "space" });
    });

    it("takes a custom decimal separator, and refuses one that cannot be written", async () => {
      const user = userEvent.setup();
      await setup();
      const dialog = await openFromShortcut();
      const decimal = within(dialog).getByRole("radiogroup", { name: "Decimal" });
      await user.click(within(decimal).getByRole("radio", { name: "Other…" }));
      // Nothing typed yet: not a separator.
      expect(within(dialog).getByText(/Type the decimal separator/)).toBeVisible();
      expect(exportButton(dialog)).toBeDisabled();
      const field = within(dialog).getByRole("textbox", { name: "Custom decimal separator" });
      await user.type(field, "5");
      expect(within(dialog).getByText(/not a digit or a quote/)).toBeVisible();
      expect(exportButton(dialog)).toBeDisabled();
      await user.clear(field);
      await user.type(field, "#");
      expect(exportButton(dialog)).toBeEnabled();
      const run = spyRun("/Users/me/Downloads/hash.csv");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
      expect(requestOf(run).format).toMatchObject({ format: "csv", decimal: "#" });
      // Remembered with the other options: the field is open the next time.
      expect(JSON.parse(localStorage.getItem(EXPORT_OPTIONS_KEY)!).csv.decimal).toBe("#");
    });
  });

  describe("the columns of the grid", () => {
    it("follows the grid's order when columns were only dragged", async () => {
      const user = userEvent.setup();
      const tabId = await setup();
      // Nothing hidden, `email` dragged to the front.
      useTableTabsStore.getState().update(tabId, {
        columnState: { widths: {}, order: ["email"], hidden: [] },
      });
      const dialog = await openFromShortcut();
      const box = await within(dialog).findByRole("checkbox", { name: "Columns in grid order" });
      expect(box).toBeChecked();
      const run = spyRun("/Users/me/Downloads/ordered.csv");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
      let source = requestOf(run).source;
      expect(source.kind === "table" ? source.columns : null).toEqual(
        expect.arrayContaining(["email", "id"]),
      );
      expect(source.kind === "table" ? source.columns?.[0] : null).toBe("email");
      vi.restoreAllMocks();

      // Unchecked: the table's own order (every column, null).
      const again = await openFromShortcut();
      await user.click(
        await within(again).findByRole("checkbox", { name: "Columns in grid order" }),
      );
      const second = spyRun("/Users/me/Downloads/table-order.csv");
      await user.click(exportButton(again));
      await waitFor(() => expect(toast.success).toHaveBeenCalledTimes(2));
      source = requestOf(second).source;
      expect(source.kind === "table" ? source.columns : "x").toBeNull();
    });

    it("has no column choice while the grid is as the table is", async () => {
      await setup();
      const dialog = await openFromShortcut();
      await within(dialog).findByRole("radio", { name: "Current page" });
      expect(within(dialog).queryByText(/in grid order|Only visible columns/)).toBeNull();
    });
  });

  describe("the encoding of View Using Encoding", () => {
    it("preselects the connection's encoding and writes the file in it", async () => {
      const user = userEvent.setup();
      localStorage.setItem(EXPORT_OPTIONS_KEY, JSON.stringify({ format: "csv" }));
      await setup();
      vi.spyOn(ipc.connections, "getEncoding").mockResolvedValue("latin1");
      const dialog = await openFromShortcut();
      await waitFor(() => expect(encodingSelect(dialog)).toHaveTextContent("Latin-1"));
      expect(within(dialog).getByText("Follows View Using Encoding (latin1)")).toBeVisible();
      const run = spyRun("/Users/me/Downloads/latin.csv");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
      expect(requestOf(run).encoding).toBe("latin1");
      // The remembered choice is the user's, not the connection's.
      expect(JSON.parse(localStorage.getItem(EXPORT_OPTIONS_KEY)!).encoding).toBe("utf8");
    });

    it("keeps the remembered choice for a UTF-8 connection", async () => {
      localStorage.setItem(
        EXPORT_OPTIONS_KEY,
        JSON.stringify({ format: "csv", encoding: "utf8Bom" }),
      );
      await setup();
      const spy = vi.spyOn(ipc.connections, "getEncoding").mockResolvedValue("utf8mb4");
      const dialog = await openFromShortcut();
      await waitFor(() => expect(spy).toHaveBeenCalled());
      expect(encodingSelect(dialog)).toHaveTextContent("UTF-8 with BOM");
      expect(within(dialog).queryByText(/Follows View Using Encoding/)).toBeNull();
    });

    it("lets the user's own choice win", async () => {
      const user = userEvent.setup();
      localStorage.setItem(EXPORT_OPTIONS_KEY, JSON.stringify({ format: "csv" }));
      await setup();
      vi.spyOn(ipc.connections, "getEncoding").mockResolvedValue("WIN1252");
      const dialog = await openFromShortcut();
      await waitFor(() => expect(encodingSelect(dialog)).toHaveTextContent("Windows-1252"));
      await user.click(encodingSelect(dialog));
      await user.click(await screen.findByRole("option", { name: "UTF-16 LE" }));
      expect(encodingSelect(dialog)).toHaveTextContent("UTF-16 LE");
      expect(within(dialog).queryByText(/Follows View Using Encoding/)).toBeNull();
      const run = spyRun("/Users/me/Downloads/utf16.csv");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
      expect(requestOf(run).encoding).toBe("utf16Le");
    });

    // Export Current Page… (the grid's right-click menu) opens the dialog with a pre-set form (a
    // draft written from outside); the connection's encoding applies to it like to any other.
    it("also follows it in the dialog opened by Export Current Page…", async () => {
      const user = userEvent.setup();
      localStorage.setItem(EXPORT_OPTIONS_KEY, JSON.stringify({ format: "csv" }));
      const tabId = await setup();
      vi.spyOn(ipc.connections, "getEncoding").mockResolvedValue("latin1");
      act(() =>
        openExportCurrentPage(info.connectionId, { schema: "public", name: "users" }, tabId),
      );
      const dialog = await screen.findByRole("dialog");
      expect(within(dialog).getByRole("radio", { name: "Current page" })).toHaveAttribute(
        "aria-checked",
        "true",
      );
      await waitFor(() => expect(encodingSelect(dialog)).toHaveTextContent("Latin-1"));
      expect(within(dialog).getByText("Follows View Using Encoding (latin1)")).toBeVisible();
      const run = spyRun("/Users/me/Downloads/page.csv");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
      expect(requestOf(run).encoding).toBe("latin1");
      expect(JSON.parse(localStorage.getItem(EXPORT_OPTIONS_KEY)!).encoding).toBe("utf8");
    });

    it("lets a manual choice win in the Export Current Page… dialog, also when it reopens", async () => {
      const user = userEvent.setup();
      localStorage.setItem(EXPORT_OPTIONS_KEY, JSON.stringify({ format: "csv" }));
      const tabId = await setup();
      vi.spyOn(ipc.connections, "getEncoding").mockResolvedValue("latin1");
      act(() =>
        openExportCurrentPage(info.connectionId, { schema: "public", name: "users" }, tabId),
      );
      let dialog = await screen.findByRole("dialog");
      await waitFor(() => expect(encodingSelect(dialog)).toHaveTextContent("Latin-1"));
      await user.click(encodingSelect(dialog));
      await user.click(await screen.findByRole("option", { name: "UTF-16 LE" }));
      expect(within(dialog).queryByText(/Follows View Using Encoding/)).toBeNull();
      const { spy, ctl } = controlledRun("/Users/me/Downloads/page.csv");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(spy).toHaveBeenCalledOnce());
      expect(requestOf(spy).encoding).toBe("utf16Le");
      // Closed and reopened while it runs: the choice is still the user's.
      act(() => closeExport());
      await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
      dialog = await openFromShortcut();
      expect(encodingSelect(dialog)).toHaveTextContent("UTF-16 LE");
      expect(useExportStore.getState().draft).toMatchObject({ encodingTouched: true });
      act(() =>
        ctl.finish({ path: "/Users/me/Downloads/page.csv", rows: 3, bytes: 90, durationMs: 1 }),
      );
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    });

    it("still follows it when a form that followed it reopens on a running export", async () => {
      const user = userEvent.setup();
      localStorage.setItem(EXPORT_OPTIONS_KEY, JSON.stringify({ format: "csv" }));
      await setup();
      vi.spyOn(ipc.connections, "getEncoding").mockResolvedValue("latin1");
      let dialog = await openFromShortcut();
      await waitFor(() => expect(encodingSelect(dialog)).toHaveTextContent("Latin-1"));
      const { spy, ctl } = controlledRun("/Users/me/Downloads/users.csv");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(spy).toHaveBeenCalledOnce());
      act(() => closeExport());
      await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
      dialog = await openFromShortcut();
      await waitFor(() => expect(encodingSelect(dialog)).toHaveTextContent("Latin-1"));
      expect(within(dialog).getByText("Follows View Using Encoding (latin1)")).toBeVisible();
      // The form remembers the user's own encoding next to the one it follows.
      expect(useExportStore.getState().draft).toMatchObject({
        encodingTouched: false,
        rememberedEncoding: "utf8",
      });
      expect(useExportStore.getState().draft?.options.encoding).toBe("latin1");
      act(() =>
        ctl.finish({ path: "/Users/me/Downloads/users.csv", rows: 3, bytes: 90, durationMs: 1 }),
      );
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    });

    it("also sets the encoding of the multi-table export", async () => {
      const user = userEvent.setup();
      await setup();
      vi.spyOn(ipc.connections, "getEncoding").mockResolvedValue("latin1");
      act(() => openExportTables(info.connectionId, [{ schema: "public", name: "users" }]));
      const dialog = await screen.findByRole("dialog");
      await within(dialog).findByRole("list", { name: "Tables of tabler_dev" });
      await user.click(within(dialog).getByRole("radio", { name: "CSV" }));
      await waitFor(() => expect(encodingSelect(dialog)).toHaveTextContent("Latin-1"));
      vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Exports");
      const run = vi.spyOn(ipc.exports, "runTables");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
      expect(run.mock.calls[0]![1].encoding).toBe("latin1");
    });
  });

  describe("⇧⌘E and File ▸ Export… (the multi-table export)", () => {
    it("opens the tables export with the active table checked, not its single export", async () => {
      await setup();
      const dialog = await openExportKey();
      const source = useExportStore.getState().target?.source;
      expect(source).toMatchObject({
        kind: "tables",
        schema: "public",
        tables: [{ schema: "public", name: "users" }],
      });
      const list = await within(dialog).findByRole("list", { name: "Tables of tabler_dev" });
      await within(list).findByText("public.users");
      await waitFor(() =>
        expect(tableBox(list, "public.users")).toHaveAttribute("aria-checked", "true"),
      );
      // Only the table of the tab: the others are there to be picked.
      expect(tableBox(list, "public.posts")).toHaveAttribute("aria-checked", "false");
    });

    it("keeps the table tab's own Export button on its filtered rows", async () => {
      const tabId = await setup();
      useTableTabsStore.getState().update(tabId, {
        filters: [newFilterRow("id", "gt", "10")],
      });
      await openFromShortcut();
      expect(useExportStore.getState().target?.source).toEqual({
        kind: "table",
        table: { schema: "public", name: "users" },
        tabId,
      });
    });

    it("exports the whole current schema when no table tab is active", async () => {
      await setup();
      act(() => {
        useConnectionsStore.getState().setActive(info.connectionId);
        useConnectionsStore.getState().setCurrentSchema(info.connectionId, "public");
        useTabsStore.getState().newQuery(info.connectionId);
      });
      await openExportKey();
      expect(useExportStore.getState().target?.source).toMatchObject({
        kind: "tables",
        schema: "public",
        all: true,
      });
    });
  });

  describe("all results of a run", () => {
    const col = (name: string): ResultColumn => ({
      name,
      dataType: "int8",
      kind: "int",
      nullable: false,
      table: null,
    });

    /** A query tab whose finished run holds `count` result sets, as the editor leaves it. */
    function runWithResults(count: number): string {
      const tabId = useTabsStore.getState().newQuery(info.connectionId);
      const run: RunState = {
        ...newRun("run-1", "select 1; select 2; select 3", 0),
        status: "done",
        finishedAt: 1,
        totalMs: 1,
        statements: [],
        results: [],
      };
      for (let i = 0; i < count; i++) {
        const id = resultId("run-1", i, 0);
        run.statements.push({
          index: i,
          sql: `select ${i + 1} as n${i}`,
          span: { start: 0, end: 1, line: 1 },
          status: "ok",
          result: null,
          error: null,
        } as RunState["statements"][number]);
        run.results.push({
          id,
          statementIndex: i,
          set: 0,
          columns: [col(`n${i}`)],
          done: true,
          truncated: false,
          durationMs: 1,
        });
        resultData.setColumns(id, [col(`n${i}`)]);
        resultData.append(id, [[{ t: "int", v: i }], [{ t: "int", v: i + 10 }]]);
      }
      updateQueryTab(tabId, { run, pane: run.results[0]!.id });
      return tabId;
    }

    const shown = (tabId: string, index = 0) => {
      const r = runResultsOf(tabId)[index]!;
      return {
        kind: "rows" as const,
        columns: r.columns,
        rows: r.rows,
        label: r.label,
        table: r.table,
        sql: `select ${index + 1}`,
        truncated: false,
        stopped: null,
      };
    };

    beforeEach(() => {
      useQueryTabsStore.setState({ slices: {} });
      resultData.dropWhere(() => true);
    });

    it("offers this result or all of them when the run holds several", async () => {
      const user = userEvent.setup();
      await setup();
      const tabId = runWithResults(2);
      act(() => openExport({ connectionId: info.connectionId, source: shown(tabId, 1) }));
      const dialog = await screen.findByRole("dialog");
      const scope = within(dialog).getByRole("radiogroup", { name: "Results" });
      expect(within(scope).getByRole("radio", { name: "This result" })).toHaveAttribute(
        "aria-checked",
        "true",
      );
      // This result is what it was: the one shown.
      const single = spyRun("/Users/me/Downloads/one.csv");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
      expect(requestOf(single).source).toMatchObject({ kind: "rows" });
      vi.restoreAllMocks();

      act(() => openExport({ connectionId: info.connectionId, source: shown(tabId, 1) }));
      const again = await screen.findByRole("dialog");
      await user.click(within(again).getByRole("radio", { name: "All results (2)" }));
      await user.click(within(again).getByRole("radio", { name: "Excel" }));
      expect(within(again).getByText("One workbook with a sheet per result")).toBeVisible();
      vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Downloads/results.xlsx");
      const all = vi.spyOn(ipc.exports, "runResults");
      const one = vi.spyOn(ipc.exports, "run");
      await user.click(exportButton(again));
      await waitFor(() => expect(toast.success).toHaveBeenCalledTimes(2));
      expect(one).not.toHaveBeenCalled();
      const req = all.mock.calls[0]![1];
      expect(req).toMatchObject({
        layout: "singleFile",
        target: "/Users/me/Downloads/results.xlsx",
        format: { format: "xlsx", header: true },
      });
      expect(req.results.map((r) => r.label)).toEqual(["Result 1", "Result 2"]);
      expect(req.results.map((r) => r.rows.length)).toEqual([2, 2]);
      expect(toast.success.mock.calls[1]![0]).toBe("Exported 2 results");
    });

    it("writes a file per result into a folder for CSV and JSON", async () => {
      const user = userEvent.setup();
      await setup();
      const tabId = runWithResults(3);
      act(() => openExport({ connectionId: info.connectionId, source: shown(tabId) }));
      const dialog = await screen.findByRole("dialog");
      await user.click(within(dialog).getByRole("radio", { name: "All results (3)" }));
      expect(
        within(dialog).getByText(/One \.csv file per result in the folder you choose/),
      ).toBeVisible();
      vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Exports");
      const all = vi.spyOn(ipc.exports, "runResults");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
      expect(window.prompt).toHaveBeenCalledWith(
        "Export the tables into folder",
        expect.any(String),
      );
      expect(all.mock.calls[0]![1]).toMatchObject({
        layout: "filePerTable",
        target: "/Users/me/Exports",
        name: "results",
        format: { format: "csv" },
      });
      expect(all.mock.calls[0]![1].results).toHaveLength(3);
    });

    it("sends a SQL script of every result, each to its own table", async () => {
      const user = userEvent.setup();
      await setup();
      const tabId = runWithResults(2);
      act(() => openExport({ connectionId: info.connectionId, source: shown(tabId) }));
      const dialog = await screen.findByRole("dialog");
      await user.click(within(dialog).getByRole("radio", { name: "All results (2)" }));
      await user.click(within(dialog).getByRole("radio", { name: "SQL" }));
      // Each result names its own table: there is no single name to type.
      expect(within(dialog).queryByLabelText("Table name")).toBeNull();
      vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Downloads/results.sql");
      const all = vi.spyOn(ipc.exports, "runResults");
      await user.click(exportButton(dialog));
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
      expect(all.mock.calls[0]![1]).toMatchObject({
        layout: "singleFile",
        format: { format: "sqlInsert", tableName: "", content: true },
      });
    });

    it("has no such choice for a run holding one result, or for a pinned one", async () => {
      await setup();
      const tabId = runWithResults(1);
      act(() => openExport({ connectionId: info.connectionId, source: shown(tabId) }));
      let dialog = await screen.findByRole("dialog");
      expect(within(dialog).queryByRole("radiogroup", { name: "Results" })).toBeNull();
      act(() => closeExport());
      await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

      // Two results, but the rows exported are not one of them (a result pinned from an earlier run).
      const two = runWithResults(2);
      act(() =>
        openExport({
          connectionId: info.connectionId,
          source: { ...shown(two), label: "Result 9", rows: [[{ t: "int", v: 99 }]] },
        }),
      );
      dialog = await screen.findByRole("dialog");
      expect(within(dialog).queryByRole("radiogroup", { name: "Results" })).toBeNull();
    });

    it("warns about results that are not whole and refuses a cut binary value", async () => {
      const user = userEvent.setup();
      await setup();
      const tabId = runWithResults(2);
      updateQueryTab(tabId, (s) => ({
        run: s.run && {
          ...s.run,
          results: s.run.results.map((r, i) => (i === 0 ? { ...r, truncated: true } : r)),
          statements: s.run.statements.map((st, i) =>
            i === 1 ? { ...st, status: "cancelled" } : st,
          ),
        },
      }));
      act(() => openExport({ connectionId: info.connectionId, source: shown(tabId) }));
      const dialog = await screen.findByRole("dialog");
      await user.click(within(dialog).getByRole("radio", { name: "All results (2)" }));
      expect(within(dialog).getByText(/1 result stopped at the row limit/)).toBeVisible();
      expect(within(dialog).getByText(/1 result cancelled or failed partway/)).toBeVisible();
      expect(exportButton(dialog)).toBeEnabled();
      // A binary value cut to its preview in the second result.
      const second = runResultsOf(tabId)[1]!;
      resultData.replaceRows(resultId("run-1", 1, 0), [
        [{ t: "bytes", v: { base64: "", len: 70_000, truncated: true } }],
      ]);
      expect(second.rows).not.toBe(runResultsOf(tabId)[1]!.rows);
    });
  });
});
