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
} from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { newFilterRow, useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";
import { ExportHost } from "./ExportHost";
import { EXPORT_OPTIONS_KEY } from "./options";
import { closeExport, openExport, resetExportStore, useExportStore } from "./store";

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
    expect(req.format).toEqual({
      format: "csv",
      delimiter: "\t",
      header: true,
      quoteAll: true,
      nullAs: "NULL",
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
    expect(requestOf(run).format).toEqual({
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

  it("explains that a query tab exports from the SQL editor", async () => {
    await setup();
    act(() => {
      useTabsStore.getState().newQuery(info.connectionId);
    });
    await act(() => actions.run("export.open"));
    expect(toast.message).toHaveBeenCalledWith("Nothing to export here", expect.anything());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});
