import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { ImportHost } from "./ImportHost";
import { openImport, useImportStore } from "./store";

const toast = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  message: vi.fn(),
  warning: vi.fn(),
}));
vi.mock("sonner", () => ({ toast }));
const guard = vi.hoisted(() => vi.fn(async () => true));
vi.mock("@/features/safety/guard", () => ({ guardExecution: guard }));

let info: ConnectionInfo;

async function setup() {
  info = await ipc.connections.connect("p-local-pg");
  const store = useConnectionsStore.getState();
  store.add(info);
  store.setActive(info.connectionId);
  store.setCurrentSchema(info.connectionId, "public");
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <ImportHost />
    </QueryClientProvider>,
  );
  await waitFor(() => expect(actions.has("import.csv")).toBe(true));
  return qc;
}

const users = { schema: "public", name: "users" };

beforeEach(() => {
  localStorage.clear();
  useImportStore.setState({ request: null, path: null, session: 0 });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  guard.mockClear();
});

afterEach(() => {
  vi.restoreAllMocks();
  Object.values(toast).forEach((fn) => fn.mockReset());
});

describe("CSV import wizard", { timeout: 20_000 }, () => {
  it("matches the CSV's columns by name and imports into the table", async () => {
    const user = userEvent.setup();
    await setup();
    act(() =>
      openImport(
        { connectionId: info.connectionId, kind: "csv", table: users },
        "/Users/me/Downloads/users.csv",
      ),
    );
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("users.csv")).toBeInTheDocument();
    const status = await within(dialog).findByTestId("csv-status");
    await waitFor(() =>
      expect(status).toHaveTextContent("CSV 7 columns, table 16 columns, matched 7 columns"),
    );
    // The preview shows the first 100 rows.
    expect(within(dialog).getByText("user0@example.com")).toBeInTheDocument();
    expect(within(dialog).getByText(/first 100 rows shown/)).toBeInTheDocument();
    const run = vi.spyOn(ipc.imports, "csv");
    await user.click(within(dialog).getByRole("button", { name: /^Import/ }));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(guard).toHaveBeenCalledWith(info.connectionId, {
      kind: "operation",
      label: "Import CSV into users",
      write: true,
    });
    const req = run.mock.calls[0]![1];
    expect(req.target).toEqual({ kind: "existing", table: users });
    expect(req.mapping).toEqual([
      "id",
      "email",
      "name",
      "role",
      "is_active",
      "balance",
      "created_at",
    ]);
    expect(req.format).toEqual({ delimiter: ",", quote: '"', encoding: "utf8", header: true });
    expect(req).toMatchObject({ emptyAsNull: true, mode: "stopOnError" });
    expect(toast.success.mock.calls[0]![0]).toBe("Imported 240 rows");
    await waitFor(() => expect(useImportStore.getState().request).toBeNull());
  });

  it("creates a new table from the CSV with the detected types", async () => {
    const user = userEvent.setup();
    await setup();
    act(() =>
      openImport(
        { connectionId: info.connectionId, kind: "csv", table: null },
        "/Users/me/Downloads/Product List.csv",
      ),
    );
    const dialog = await screen.findByRole("dialog");
    await user.click(await within(dialog).findByRole("checkbox", { name: "Create new table" }));
    expect(within(dialog).getByRole("textbox", { name: "New table name" })).toHaveValue(
      "product_list",
    );
    const status = within(dialog).getByTestId("csv-status");
    await waitFor(() =>
      expect(status).toHaveTextContent("CSV 5 columns, table 5 columns, matched 5 columns"),
    );
    // Leave the second column out and rename the first.
    await user.click(within(dialog).getByRole("checkbox", { name: "Import column 2" }));
    const name = within(dialog).getByRole("textbox", { name: "Name of column 1" });
    await user.clear(name);
    await user.type(name, "code");
    const run = vi.spyOn(ipc.imports, "csv");
    await user.click(within(dialog).getByRole("button", { name: /^Import/ }));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    const req = run.mock.calls[0]![1];
    expect(req.mapping).toEqual(["code", null, "price", "in_stock", "added"]);
    expect(req.target).toEqual({
      kind: "create",
      table: { schema: "public", name: "product_list" },
      columns: [
        { name: "code", ty: "text", precision: null, scale: null },
        { name: "price", ty: "numeric", precision: 8, scale: 2 },
        { name: "in_stock", ty: "integer", precision: null, scale: null },
        { name: "added", ty: "date", precision: null, scale: null },
      ],
    });
  });

  it("keeps the wizard open on a failure, with the rows it concerns", async () => {
    const user = userEvent.setup();
    await setup();
    act(() =>
      openImport(
        { connectionId: info.connectionId, kind: "csv", table: users },
        "/Users/me/Downloads/users_bad.csv",
      ),
    );
    const dialog = await screen.findByRole("dialog");
    await waitFor(() =>
      expect(within(dialog).getByTestId("csv-status")).toHaveTextContent("matched 7 columns"),
    );
    await user.click(within(dialog).getByRole("button", { name: /^Import/ }));
    // The mock reads and writes in steps that re-render the 100-row preview: slow under load.
    const alert = await within(dialog).findByRole("alert", {}, { timeout: 5000 });
    expect(alert).toHaveTextContent("violates not-null constraint");
    expect(alert).toHaveTextContent("rows 1–240, line 2");
    expect(alert).toHaveTextContent("Nothing was imported.");
    expect(toast.success).not.toHaveBeenCalled();
    expect(useImportStore.getState().request).not.toBeNull();
  });

  it("starts once while the safety guard is still asking", async () => {
    const user = userEvent.setup();
    await setup();
    let answer: (ok: boolean) => void = () => {};
    guard.mockImplementationOnce(() => new Promise<boolean>((resolve) => (answer = resolve)));
    act(() =>
      openImport(
        { connectionId: info.connectionId, kind: "csv", table: users },
        "/Users/me/Downloads/users.csv",
      ),
    );
    const dialog = await screen.findByRole("dialog");
    await waitFor(() =>
      expect(within(dialog).getByTestId("csv-status")).toHaveTextContent("matched 7 columns"),
    );
    const run = vi.spyOn(ipc.imports, "csv");
    await user.click(within(dialog).getByRole("button", { name: /^Import/ }));
    await user.keyboard("{Meta>}{Enter}{/Meta}");
    await user.keyboard("{Meta>}{Enter}{/Meta}");
    expect(guard).toHaveBeenCalledOnce();
    act(() => answer(true));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(run).toHaveBeenCalledOnce();
  });

  it("lists the rows it skipped in the toast", async () => {
    const user = userEvent.setup();
    await setup();
    vi.spyOn(ipc.imports, "csv").mockResolvedValue({
      rowsImported: 238,
      rowsRead: 240,
      singleTransaction: true,
      createdTable: false,
      cancelled: false,
      failure: null,
      rowsSkipped: 2,
      skipped: [
        { row: 7, line: 8, reason: "the row has 3 fields, the header has 7" },
        { row: 9, line: 10, reason: "the row has 8 fields, the header has 7" },
      ],
      notes: [],
      durationMs: 5,
    });
    act(() =>
      openImport(
        { connectionId: info.connectionId, kind: "csv", table: users },
        "/Users/me/Downloads/users.csv",
      ),
    );
    const dialog = await screen.findByRole("dialog");
    await waitFor(() =>
      expect(within(dialog).getByTestId("csv-status")).toHaveTextContent("matched 7 columns"),
    );
    await user.click(within(dialog).getByRole("button", { name: /^Import/ }));
    await waitFor(() => expect(toast.warning).toHaveBeenCalledOnce());
    expect(toast.warning.mock.calls[0]![0]).toBe("Imported 238 rows, skipped 2 rows");
    expect(toast.warning.mock.calls[0]![1]).toMatchObject({
      description:
        "Row 7 (line 8): the row has 3 fields, the header has 7\nRow 9 (line 10): the row has 8 fields, the header has 7",
    });
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("asks nothing when the guard declines", async () => {
    const user = userEvent.setup();
    await setup();
    guard.mockResolvedValueOnce(false);
    act(() =>
      openImport(
        { connectionId: info.connectionId, kind: "csv", table: users },
        "/Users/me/Downloads/users.csv",
      ),
    );
    const dialog = await screen.findByRole("dialog");
    await waitFor(() =>
      expect(within(dialog).getByTestId("csv-status")).toHaveTextContent("matched 7 columns"),
    );
    const run = vi.spyOn(ipc.imports, "csv");
    await user.click(within(dialog).getByRole("button", { name: /^Import/ }));
    await waitFor(() => expect(guard).toHaveBeenCalled());
    expect(run).not.toHaveBeenCalled();
  });
});

describe("File ▸ Import", () => {
  it("From CSV… asks for a file, preselecting the active table tab's table", async () => {
    await setup();
    act(() => {
      useTabsStore.getState().openTable(info.connectionId, users);
    });
    const prompt = vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Downloads/users.csv");
    await act(() => actions.run("import.csv"));
    expect(prompt).toHaveBeenCalledWith("Import CSV file", expect.any(String));
    const dialog = await screen.findByRole("dialog");
    expect(useImportStore.getState().request).toMatchObject({ kind: "csv", table: users });
    await waitFor(() =>
      expect(within(dialog).getByRole("combobox", { name: "Table" })).toHaveTextContent("users"),
    );
  });

  it("closes when the file panel is dismissed", async () => {
    await setup();
    vi.spyOn(window, "prompt").mockReturnValue(null);
    await act(() => actions.run("import.sql"));
    await waitFor(() => expect(useImportStore.getState().request).toBeNull());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("needs a connection", async () => {
    await setup();
    act(() => useConnectionsStore.getState().setActive(null));
    await act(() => actions.run("import.csv"));
    expect(toast.message).toHaveBeenCalledWith("Nothing to import into", expect.anything());
    expect(useImportStore.getState().request).toBeNull();
  });
});

describe("SQL dump import", () => {
  it("previews the dump, then runs it", async () => {
    const user = userEvent.setup();
    const qc = await setup();
    const invalidate = vi.spyOn(qc, "invalidateQueries");
    act(() =>
      openImport(
        { connectionId: info.connectionId, kind: "sql", table: null },
        "/Users/me/Downloads/dump.sql",
      ),
    );
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByTestId("sql-preview")).toHaveTextContent(
      "CREATE TABLE `demo_items`",
    );
    expect(within(dialog).getByText(/The first 20 statements of many/)).toBeInTheDocument();
    expect(within(dialog).getByRole("checkbox", { name: "Stop on error" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    const run = vi.spyOn(ipc.imports, "sql");
    await user.click(within(dialog).getByRole("button", { name: /^Continue/ }));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(guard).toHaveBeenCalledWith(info.connectionId, {
      kind: "operation",
      label: "Import SQL dump",
      write: true,
    });
    expect(run.mock.calls[0]![1]).toMatchObject({
      path: "/Users/me/Downloads/dump.sql",
      stopOnError: true,
    });
    expect(toast.success.mock.calls[0]![1]).toMatchObject({
      description: expect.stringContaining("63 statements"),
    });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["tables", info.connectionId] });
  });

  it("warns about bytes that are not UTF-8 and stays on the report's notes", async () => {
    const user = userEvent.setup();
    await setup();
    act(() =>
      openImport(
        { connectionId: info.connectionId, kind: "sql", table: null },
        "/Users/me/Downloads/latin.sql",
      ),
    );
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByTestId("sql-preview");
    expect(within(dialog).getByRole("note")).toHaveTextContent(
      "3 byte sequences are not UTF-8 (the first on line 12): they will be read as Windows-1252.",
    );
    await user.click(within(dialog).getByRole("button", { name: /^Continue/ }));
    const notes = await within(dialog).findByRole("list", { name: "Notes" });
    expect(notes).toHaveTextContent("decoded as Windows-1252");
    expect(within(dialog).getByText(/63 statements ran\./)).toBeInTheDocument();
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("runs the dump once while the safety guard is still asking", async () => {
    const user = userEvent.setup();
    await setup();
    let answer: (ok: boolean) => void = () => {};
    guard.mockImplementationOnce(() => new Promise<boolean>((resolve) => (answer = resolve)));
    act(() =>
      openImport(
        { connectionId: info.connectionId, kind: "sql", table: null },
        "/Users/me/Downloads/dump.sql",
      ),
    );
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByTestId("sql-preview");
    const run = vi.spyOn(ipc.imports, "sql");
    await user.click(within(dialog).getByRole("button", { name: /^Continue/ }));
    await user.keyboard("{Meta>}{Enter}{/Meta}");
    expect(guard).toHaveBeenCalledOnce();
    act(() => answer(true));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(run).toHaveBeenCalledOnce();
  });

  it("reports the failed statements when it goes on after errors", async () => {
    const user = userEvent.setup();
    await setup();
    act(() =>
      openImport(
        { connectionId: info.connectionId, kind: "sql", table: null },
        "/Users/me/Downloads/bad.sql",
      ),
    );
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByTestId("sql-preview");
    await user.click(within(dialog).getByRole("checkbox", { name: "Stop on error" }));
    await user.click(within(dialog).getByRole("button", { name: /^Continue/ }));
    const errors = await within(dialog).findByRole("list", { name: "Errors" });
    expect(errors).toHaveTextContent("doesn't exist");
    expect(within(dialog).getByText(/63 statements ran/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(useImportStore.getState().request).toBeNull());
  });
});

describe("JSON import", () => {
  it(
    "File ▸ Import ▸ From JSON… maps the keys by name and imports through the wizard",
    {
      timeout: 20_000,
    },
    async () => {
      const user = userEvent.setup();
      await setup();
      await waitFor(() => expect(actions.has("import.json")).toBe(true));
      vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Downloads/users.json");
      act(() => {
        useTabsStore.getState().openTable(info.connectionId, users);
      });
      await act(() => actions.run("import.json"));
      const dialog = await screen.findByRole("dialog");
      expect(within(dialog).getByText("Import JSON")).toBeInTheDocument();
      expect(window.prompt).toHaveBeenCalledWith("Import JSON file", expect.any(String));
      const status = await within(dialog).findByTestId("csv-status");
      await waitFor(() =>
        expect(status).toHaveTextContent("JSON 7 keys, table 16 columns, matched 7 columns"),
      );
      expect(within(dialog).getByText(/array of objects/)).toBeInTheDocument();
      // No CSV reading options: keys are names, null is NULL.
      expect(within(dialog).queryByRole("checkbox", { name: "First line is header" })).toBeNull();
      expect(within(dialog).queryByRole("combobox", { name: "Delimiter" })).toBeNull();
      expect(within(dialog).getByRole("combobox", { name: "Encoding" })).toBeInTheDocument();
      const run = vi.spyOn(ipc.imports, "json");
      await user.click(within(dialog).getByRole("button", { name: /^Import/ }));
      await waitFor(() => expect(toast.success).toHaveBeenCalledOnce(), { timeout: 5000 });
      expect(guard).toHaveBeenCalledWith(info.connectionId, {
        kind: "operation",
        label: "Import JSON into users",
        write: true,
      });
      const req = run.mock.calls[0]![1];
      expect(req).toMatchObject({
        path: "/Users/me/Downloads/users.json",
        encoding: "utf8",
        keys: ["id", "email", "name", "role", "is_active", "balance", "created_at"],
        target: { kind: "existing", table: users },
        mode: "stopOnError",
      });
      expect(req.mapping).toEqual([
        "id",
        "email",
        "name",
        "role",
        "is_active",
        "balance",
        "created_at",
      ]);
      expect(toast.success.mock.calls[0]![0]).toBe("Imported 240 rows");
    },
  );

  it("shows why a file is not JSON objects", async () => {
    await setup();
    act(() =>
      openImport(
        { connectionId: info.connectionId, kind: "json", table: users },
        "/Users/me/Downloads/users_notjson.json",
      ),
    );
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("not an object");
  });
});
