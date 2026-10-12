import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ObjectsHost } from "@/features/objects/ObjectsHost";
import { closeItemOverview, closeObjectDialog, useObjectsStore } from "@/features/objects/store";
import { actions } from "@/features/palette/actions";
import { guardExecution } from "@/features/safety/guard";
import {
  ipc,
  type ConnectionInfo,
  type DdlPlan,
  type Driver,
  type ObjectOp,
  type RoutineSummary,
  type TableSummary,
} from "@/lib/ipc";
import { isMac } from "@/lib/keys/combo";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { SchemaTree } from "./SchemaTree";
import { reloadSidebarPrefs } from "./sidebarPrefs";
import { buildTreeRows } from "./useSchemaTree";

vi.mock("@/features/safety/guard", () => ({ guardExecution: vi.fn(async () => true) }));

function table(name: string, kind: TableSummary["kind"] = "table"): TableSummary {
  return { schema: "public", name, kind, rowEstimate: null, sizeBytes: null, comment: null };
}

const TABLES = [table("users"), table("posts"), table("tags"), table("user_stats", "view")];

function routine(name: string, kind: RoutineSummary["kind"] = "function"): RoutineSummary {
  return {
    schema: "public",
    name,
    kind,
    arguments: "value text",
    identityArguments: "value text",
    returnType: kind === "procedure" ? null : "text",
    language: "sql",
    comment: null,
  };
}

const MOD = isMac ? { metaKey: true } : { ctrlKey: true };

function connect(driver: Driver = "postgres") {
  const info: ConnectionInfo = {
    connectionId: "c1",
    profileId: "p1",
    name: "Local",
    color: null,
    driver,
    serverVersion: "17.6",
    currentDatabase: "tabler_dev",
    currentUser: "tabler",
    tunnel: null,
    readOnly: false,
  };
  useConnectionsStore.setState({
    byId: { c1: { info, status: "connected", message: null, currentSchema: "public" } },
    order: ["c1"],
    activeId: "c1",
  });
}

function renderTree() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <SchemaTree
        connectionId="c1"
        rows={buildTreeRows(TABLES, "")}
        loading={false}
        error={null}
        search=""
        pinScope="p1|tabler_dev"
      />
      <ObjectsHost />
    </QueryClientProvider>,
  );
}

const row = (name: string) => screen.getAllByRole("button", { name }).at(-1)!;
const sqlPreview = (dialog: HTMLElement) => within(dialog).getByLabelText("SQL preview");
const selected = () =>
  [...document.querySelectorAll<HTMLElement>("[data-selected]")].map((el) => el.textContent);

function planOf(op: ObjectOp): DdlPlan {
  const statements =
    op.op === "dropTables"
      ? op.tables.map((t) => `DROP TABLE "public"."${t.table.name}"${op.cascade ? " CASCADE" : ""}`)
      : op.op === "truncateTables"
        ? op.tables.map((t) => `TRUNCATE TABLE "public"."${t.name}"`)
        : [`-- ${op.op}`];
  return { statements, warnings: [], transactional: true };
}

describe("SchemaTree objects", () => {
  beforeEach(() => {
    vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(600);
    vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(240);
    localStorage.clear();
    reloadSidebarPrefs();
    useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
    closeObjectDialog();
    closeItemOverview();
    connect();
    vi.spyOn(ipc.schema, "schemas").mockResolvedValue([{ name: "public", isDefault: true }]);
    // In the server's (name) order.
    vi.spyOn(ipc.objects, "routines").mockResolvedValue([
      routine("archive_posts", "procedure"),
      routine("slugify"),
    ]);
    vi.spyOn(ipc.objects, "sequences").mockResolvedValue([]);
    vi.spyOn(ipc.objects, "preview").mockImplementation(async (_c, op) => planOf(op));
    vi.mocked(guardExecution).mockResolvedValue(true);
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists functions and procedures before the tables, and opens one in a SQL tab", async () => {
    vi.spyOn(ipc.objects, "routineDefinition").mockResolvedValue(
      "CREATE OR REPLACE FUNCTION public.slugify(value text)\n RETURNS text\nAS $$ SELECT value $$;\n",
    );
    renderTree();
    const header = await screen.findByRole("button", { name: /^Functions/ });
    expect(header).toHaveTextContent("Functions2");
    const labels = screen
      .getAllByRole("button")
      .filter((b) => b.dataset.rowIndex !== undefined)
      .map((b) => b.textContent);
    expect(labels.slice(0, 4)).toEqual(["Functions2", "archive_posts", "slugify", "Tables3"]);

    fireEvent.click(row("slugify"), { detail: 1 });
    await waitFor(() =>
      expect(useTabsStore.getState().tabs).toEqual([
        expect.objectContaining({ kind: "query", title: "slugify" }),
      ]),
    );
    expect(ipc.objects.routineDefinition).toHaveBeenCalledWith("c1", {
      schema: "public",
      name: "slugify",
      kind: "function",
      identityArguments: "value text",
    });
    // The second click of a double-click opens nothing more.
    fireEvent.click(row("slugify"), { detail: 2 });
    expect(ipc.objects.routineDefinition).toHaveBeenCalledOnce();
  });

  it("selects with ⌘-click, ⇧-click and ⌘A", async () => {
    renderTree();
    await screen.findByRole("button", { name: /^Functions/ });
    fireEvent.click(row("users"));
    expect(selected()).toEqual(["users"]);
    fireEvent.click(row("tags"), MOD);
    expect(selected()).toEqual(["users", "tags"]);
    // The open table is the one filled row; the other selected rows get a lighter fill.
    expect(row("users")).toHaveClass("bg-sidebar-accent");
    expect(row("tags")).not.toHaveClass("bg-sidebar-accent");
    // ⇧-click: the range from the last ⌘-clicked row.
    fireEvent.click(row("posts"), { shiftKey: true });
    expect(row("users")).not.toHaveAttribute("data-selected");
    expect(row("posts")).toHaveAttribute("data-selected");
    expect(row("tags")).toHaveAttribute("data-selected");
    // ⌘-click toggles one off; only the clicked table opened.
    fireEvent.click(row("posts"), MOD);
    expect(row("posts")).not.toHaveAttribute("data-selected");
    expect(useTabsStore.getState().tabs.map((t) => t.title)).toEqual(["users"]);

    row("tags").focus();
    fireEvent.keyDown(row("tags"), { key: "a", ...MOD });
    for (const name of ["slugify", "archive_posts", "users", "posts", "tags", "user_stats"])
      expect(row(name)).toHaveAttribute("data-selected");

    // The menu of a selected row acts on the whole selection (tables and views here).
    fireEvent.contextMenu(row("tags"));
    expect(screen.getByRole("menuitem", { name: "Copy 4 Names" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /^Delete 4 Items…/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /^Truncate 3 Items…/ })).toBeInTheDocument();
  });

  it("⌫ confirms dropping every selected table, one statement each, after the safe mode", async () => {
    const apply = vi.spyOn(ipc.objects, "apply").mockResolvedValue({
      executed: [],
      failedIndex: null,
      error: null,
      durationMs: 4,
    });
    renderTree();
    await screen.findByRole("button", { name: /^Functions/ });
    fireEvent.click(row("posts"));
    fireEvent.click(row("tags"), MOD);
    fireEvent.keyDown(row("tags"), { key: "Backspace" });
    const dialog = await screen.findByRole("dialog", { name: "Delete 2 items?" });
    expect(within(dialog).getByRole("list", { name: "Items" })).toHaveTextContent("poststags");
    await waitFor(() =>
      expect(sqlPreview(dialog)).toHaveTextContent('DROP TABLE "public"."posts"; DROP TABLE'),
    );
    expect(within(dialog).getByRole("checkbox")).not.toBeChecked();
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(apply).toHaveBeenCalledOnce());
    expect(guardExecution).toHaveBeenCalledWith("c1", {
      kind: "commit",
      statements: ['DROP TABLE "public"."posts"', 'DROP TABLE "public"."tags"'],
    });
    expect(apply.mock.calls[0]![1]).toEqual({
      op: "dropTables",
      tables: [
        { table: { schema: "public", name: "posts" }, kind: "table" },
        { table: { schema: "public", name: "tags" }, kind: "table" },
      ],
      cascade: false,
      ignoreForeignKeys: false,
    });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("⇧⌫ presets CASCADE, ⌥⌫ truncates, and a declined safe mode sends nothing", async () => {
    const apply = vi.spyOn(ipc.objects, "apply");
    vi.mocked(guardExecution).mockResolvedValue(false);
    renderTree();
    await screen.findByRole("button", { name: /^Functions/ });
    fireEvent.keyDown(row("users"), { key: "Backspace", shiftKey: true });
    let dialog = await screen.findByRole("dialog", { name: "Delete table users?" });
    expect(within(dialog).getByRole("checkbox", { name: /CASCADE/ })).toBeChecked();
    await waitFor(() => expect(sqlPreview(dialog)).toHaveTextContent("CASCADE;"));
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(guardExecution).toHaveBeenCalled());
    expect(apply).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    // Views cannot be truncated: ⌥⌫ on one does nothing.
    fireEvent.keyDown(row("user_stats"), { key: "Backspace", altKey: true });
    expect(useObjectsStore.getState().dialog).toBeNull();
    fireEvent.keyDown(row("tags"), { key: "Backspace", altKey: true });
    dialog = await screen.findByRole("dialog", { name: "Truncate tags?" });
    await waitFor(() =>
      expect(sqlPreview(dialog)).toHaveTextContent('TRUNCATE TABLE "public"."tags";'),
    );
  });

  it("opens the Item Overview from the Tables header", async () => {
    vi.spyOn(ipc.objects, "overview").mockResolvedValue([
      {
        name: "users",
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
        comment: "Registered accounts",
      },
    ]);
    renderTree();
    fireEvent.contextMenu(await screen.findByRole("button", { name: /^Tables/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Item Overview" }));
    const dialog = await screen.findByRole("dialog", { name: "Item Overview" });
    await within(dialog).findByText("Registered accounts");
    expect(within(dialog).getByText("64 KB")).toBeInTheDocument();
    expect(within(dialog).getByText("tabler")).toBeInTheDocument();
    expect(ipc.objects.overview).toHaveBeenCalledWith("c1", "public");
  });

  it("refetches the routines (and tables) whenever view.refresh is dispatched", async () => {
    renderTree();
    await screen.findByRole("button", { name: /^Functions/ });
    expect(ipc.objects.routines).toHaveBeenCalledTimes(1);
    // ⌘R, the toolbar's Refresh and View ▸ Refresh all dispatch the action; nothing needs to
    // handle it for the sidebar to reload.
    await actions.run("view.refresh");
    await waitFor(() => expect(ipc.objects.routines).toHaveBeenCalledTimes(2));
    await actions.run("tab.new-query");
    expect(ipc.objects.routines).toHaveBeenCalledTimes(2);
  });

  it("hides the Functions section when it is turned off, and on SQLite", async () => {
    localStorage.setItem("tabler.sidebar.options.v2", JSON.stringify({ showFunctions: false }));
    reloadSidebarPrefs();
    const { unmount } = renderTree();
    await screen.findByRole("button", { name: /^Tables/ });
    expect(screen.queryByRole("button", { name: /^Functions/ })).toBeNull();
    unmount();
    localStorage.clear();
    reloadSidebarPrefs();
    connect("sqlite");
    renderTree();
    await screen.findByRole("button", { name: /^Tables/ });
    expect(screen.queryByRole("button", { name: /^Functions/ })).toBeNull();
    expect(ipc.objects.routines).not.toHaveBeenCalled();
  });
});
