import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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
import { newFilterRow, useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";
import { ExportHost } from "./ExportHost";
import { EXPORT_OPTIONS_KEY } from "./options";
import {
  closeExport,
  openExport,
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

async function openFromShortcut() {
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

beforeEach(() => {
  localStorage.clear();
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
  it("⌘⇧E exports the active table tab with its filter and sort", async () => {
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
    expect(opts.action.label).toBe("Reveal in Finder");
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

    // INSERTs of a query need a target table.
    await user.click(within(dialog).getByRole("radio", { name: "SQL" }));
    const exportButton = within(dialog).getByRole("button", { name: /^Export…/ });
    expect(exportButton).toBeDisabled();
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

  it("does not export hidden columns from a page after the first", async () => {
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
    expect(within(dialog).getByText(/Pages after the first export every column/)).toBeVisible();
    expect(exportButton(dialog)).toBeDisabled();

    // Unchecking makes "every column" an explicit choice.
    await user.click(visibleOnly);
    expect(exportButton(dialog)).toBeEnabled();
    expect(within(dialog).getByText(/^Rows 6–/)).toBeInTheDocument();
    const run = spyRun();
    await user.click(exportButton(dialog));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(requestOf(run).source).toEqual({ kind: "sql", sql: page.sql });
    expect(requestOf(run).maxRows).toBe(5);
  });

  it("waits for a later page to load instead of exporting the previous one", async () => {
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
    expect(within(dialog).getByText("Wait for the page to load to export it.")).toBeVisible();
    expect(within(dialog).queryByText(/^Rows 201/)).not.toBeInTheDocument();
    expect(exportButton(dialog)).toBeDisabled();

    act(() =>
      useTableTabsStore.getState().update(tabId, {
        page: { rows: 200, durationMs: 1, sql: browse(200) },
      }),
    );
    expect(await within(dialog).findByText("Rows 201–400")).toBeInTheDocument();
    expect(exportButton(dialog)).toBeEnabled();
    const run = spyRun();
    await user.click(exportButton(dialog));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(requestOf(run).source).toEqual({ kind: "sql", sql: browse(200) });
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

    it("needs a table name for INSERTs when the result's table is unknown", async () => {
      const user = userEvent.setup();
      await setup();
      act(() =>
        openExport({ connectionId: info.connectionId, source: held({ table: null, sql: null }) }),
      );
      const dialog = await screen.findByRole("dialog");
      await user.click(within(dialog).getByRole("radio", { name: "SQL" }));
      expect(exportButton(dialog)).toBeDisabled();
      expect(within(dialog).getByText(/needs a target table name/)).toBeInTheDocument();
      await user.type(within(dialog).getByLabelText("Table name"), "archive");
      expect(exportButton(dialog)).toBeEnabled();
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

  it("without a table tab, ⌘⇧E exports the current schema's tables", async () => {
    const user = userEvent.setup();
    await setup();
    act(() => {
      useConnectionsStore.getState().setActive(info.connectionId);
      useConnectionsStore.getState().setCurrentSchema(info.connectionId, "public");
      useTabsStore.getState().newQuery(info.connectionId);
    });
    const dialog = await openFromShortcut();
    const list = await within(dialog).findByRole("list", { name: "Tables" });
    await within(list).findByText("users");
    const boxes = within(list).getAllByRole("checkbox");
    expect(boxes.length).toBeGreaterThan(2);
    boxes.forEach((b) => expect(b).toHaveAttribute("aria-checked", "true"));
    // Narrow to two tables: search, then pick them.
    await user.click(within(dialog).getByRole("checkbox", { name: "Select all tables" }));
    await user.type(within(dialog).getByRole("textbox", { name: "Search tables" }), "post");
    const posts = within(list).getAllByRole("checkbox");
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
    const req = run.mock.calls[0]![1];
    expect(req.layout).toBe("singleFile");
    expect(req.target).toBe("/Users/me/Downloads/public.sql");
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
    const list = await within(dialog).findByRole("list", { name: "Tables" });
    await within(list).findByText("users");
    // Only the table passed is checked.
    expect(within(dialog).getByText(/· 1 of \d+ tables/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("radio", { name: "Excel" }));
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
    });
  });
});
