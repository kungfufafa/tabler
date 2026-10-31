import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { applyWorkspace, collectWorkspace, previewWorkspace } from "@/features/commit/workspace";
import { ConsolePanel } from "@/features/console/ConsolePanel";
import type { DataGridProps } from "@/features/datagrid/DataGrid";
import { runQuery } from "@/features/editor/controller";
import { seedEditor } from "@/features/editor/editorStates";
import { languageOf } from "@/features/editor/language";
import { InspectorPanel } from "@/features/inspector/InspectorPanel";
import { usePaletteItems } from "@/features/palette/usePaletteItems";
import type { PaletteCommands } from "@/features/palette/commands";
import { editorSql } from "@/features/plugins/editorText";
import { executeSql } from "@/features/plugins/execute";
import { openSavedQuery } from "@/features/queries/open";
import { sessionWrites, type GuardRequest } from "@/features/safety/guard";
import { HistoryPane } from "@/features/sidebar/HistoryPane";
import { laneMenuEntries } from "@/features/sidebar/laneMenu";
import { SchemaTree } from "@/features/sidebar/SchemaTree";
import { buildTreeRows } from "@/features/sidebar/useSchemaTree";
import { pendingCount } from "@/features/staging/model";
import { ShowDdlDialog } from "@/features/structure/ShowDdlDialog";
import { StructureTab } from "@/features/structure/StructureTab";
import { TableTab } from "@/features/table-tab/TableTab";
import { TabBody } from "@/features/tabs/TabContent";
import { ipc, type ConnectionInfo, type Driver, type SqlDialectId } from "@/lib/ipc";
import { isMac } from "@/lib/keys/combo";
import { fakeMockDriver } from "@/lib/ipc/mock/drivers/fake";
import { buildMockDrivers, registerMockDriver, resetMockBuild } from "@/lib/ipc/mock/drivers";
import { mockProfiles } from "@/lib/ipc/mock/handlers";
import { useAppInfoStore } from "@/stores/appInfo";
import { useConnectionsStore } from "@/stores/connections";
import { useInspectorStore } from "@/stores/inspector";
import { getQueryTab } from "@/stores/queryTabs";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { fakeLane, fakeSqlLane } from "./__fixtures__/fakeLane";
import { CORE_KINDS } from "./catalog";
import type { LaneId } from "./lanes";
import { awaitDialectUi, awaitDriverUi, registerLaneOverride } from "./registry";

// F9b2: the app's tabs, grid, editor, sidebar and palette on the two fake sessions, a document
// store (the fake standing in for MongoDB: a console, cursor pages, sequential edits, its own
// table view, inspector and object menu) and a SQL session of an extension dialect (the fake as
// SQL Server's T-SQL: the app's grid and editor, batch edits, Show DDL).

const toast = vi.hoisted(() =>
  Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn(), message: vi.fn() }),
);
vi.mock("sonner", () => ({ toast }));

// Safe Mode answers yes and records what it was asked; `sessionWrites` stays the real one.
const guard = vi.hoisted(() => ({ calls: [] as GuardRequest[] }));
vi.mock("@/features/safety/guard", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/features/safety/guard")>()),
  guardExecution: async (_connectionId: string, request: GuardRequest) => {
    guard.calls.push(request);
    return true;
  },
}));

// The grid is a probe: the tests read the rows it is handed and stage edits through its props.
const grid = vi.hoisted(() => ({ props: null as DataGridProps | null }));
vi.mock("@/features/datagrid/DataGrid", () => ({
  DataGrid: (props: DataGridProps) => {
    grid.props = props;
    return <div data-scope="grid" tabIndex={0} />;
  },
}));
const gridProps = () => grid.props!;

/** The grid itself, for the tests that open its menus. */
const { DataGrid: RealDataGrid } = await vi.importActual<
  typeof import("@/features/datagrid/DataGrid")
>("@/features/datagrid/DataGrid");

const mod = { [isMac ? "metaKey" : "ctrlKey"]: true };

/** jsdom has no layout: gives the grid a size so its virtualizers render rows and columns. */
function sizeTheGrid() {
  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(1200);
  vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(600);
  vi.spyOn(Element.prototype, "clientWidth", "get").mockReturnValue(1200);
  vi.spyOn(Element.prototype, "clientHeight", "get").mockReturnValue(600);
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
}

type Fake = "documents" | "sql";

const SESSIONS: Record<Fake, { driver: Driver; lane: LaneId; dialect: SqlDialectId | null }> = {
  documents: { driver: "mongodb", lane: "mongodb", dialect: null },
  sql: { driver: "sqlserver", lane: "sqlserver", dialect: "tsql" },
};

/** Connects one of the fake sessions, with its lane and mock registered and built. */
async function connect(fake: Fake): Promise<ConnectionInfo> {
  const { driver, lane, dialect } = SESSIONS[fake];
  const mock = fakeMockDriver(driver, { dialect });
  registerMockDriver(driver, mock);
  buildMockDrivers([driver]);
  useAppInfoStore.setState({
    info: { version: "1", platform: "test", debug: true, kinds: [...CORE_KINDS, driver] },
  });
  registerLaneOverride(
    lane,
    dialect ? fakeSqlLane(driver, driver, dialect) : fakeLane(driver, driver),
  );
  if (dialect) await awaitDialectUi(dialect);
  else await awaitDriverUi(driver);
  const profile = mock.seedProfiles()[0]!;
  mockProfiles.set(profile.id, profile);
  const info = await ipc.connections.connect(profile.id);
  useConnectionsStore.getState().add(info);
  useConnectionsStore.getState().setActive(info.connectionId);
  return info;
}

function wrap(node: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>{node}</TooltipProvider>
    </QueryClientProvider>,
  );
}

function tableTab(info: ConnectionInfo, limit?: number): Extract<Tab, { kind: "table" }> {
  const id = useTabsStore.getState().openTable(info.connectionId, { schema: null, name: "orders" });
  if (limit) useTableTabsStore.getState().ensure(id, { limit });
  return useTabsStore.getState().tabs.find((t) => t.id === id) as Extract<Tab, { kind: "table" }>;
}

/** The ids of the rows the grid shows. */
const shownIds = () =>
  Array.from({ length: gridProps().rowCount }, (_, i) => {
    const v = gridProps().getRow(i)?.cells[0];
    return v?.t === "text" ? v.v : null;
  });

beforeAll(async () => {
  Element.prototype.scrollIntoView ??= () => {};
  await import("@/lib/ipc/mock");
});

beforeEach(() => {
  resetPrefsForTests({ ...DEFAULT_PREFS, confirmCommit: false });
  guard.calls = [];
  grid.props = null;
});

afterEach(() => {
  for (const { driver, lane } of Object.values(SESSIONS)) {
    registerMockDriver(driver, null);
    registerLaneOverride(lane, null);
  }
  resetMockBuild();
  useAppInfoStore.setState({ info: null });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTabsStore.setState({ tabs: [], activeId: null });
  useInspectorStore.getState().set(null);
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

describe("a document store's table tab", () => {
  it("shows the lane's table view in place of the grid", async () => {
    const info = await connect("documents");
    wrap(<TabBody tab={tableTab(info)} />);
    expect(await screen.findByTestId("fake-table-view")).toHaveTextContent("Documents of orders");
  });

  it("pages by cursor: Next and Previous, no offset to jump to", async () => {
    const info = await connect("documents");
    wrap(<TableTab tab={tableTab(info, 10)} />);
    await waitFor(() => expect(shownIds()).toHaveLength(10));
    expect(shownIds()[0]).toBe("o1");
    const next = screen.getByRole("button", { name: "Next page" });
    const prev = screen.getByRole("button", { name: "Previous page" });
    expect(prev).toBeDisabled();
    fireEvent.click(next);
    await waitFor(() => expect(shownIds()[0]).toBe("o11"));
    expect(screen.getByTestId("row-summary")).toHaveTextContent("11–20");
    fireEvent.click(next);
    await waitFor(() => expect(shownIds()).toEqual(["o21", "o22", "o23", "o24", "o25"]));
    // The last page has no cursor after it.
    expect(next).toBeDisabled();
    fireEvent.click(prev);
    await waitFor(() => expect(shownIds()[0]).toBe("o11"));
    fireEvent.click(screen.getByRole("button", { name: "Pagination settings" }));
    expect(await screen.findByText("Page 2")).toBeInTheDocument();
    expect(screen.queryByLabelText("Offset")).toBeNull();
  });

  it("offers the session's filter operators and its JSON raw filter, and no sorting", async () => {
    const info = await connect("documents");
    wrap(<TableTab tab={tableTab(info, 10)} />);
    await waitFor(() => expect(shownIds()).toHaveLength(10));
    expect(gridProps().onSortChange).toBeUndefined();
    fireEvent.click(screen.getByRole("button", { name: "Filters" }));
    fireEvent.click(await screen.findByRole("combobox", { name: "Filter column" }));
    expect(await screen.findByRole("option", { name: /Raw query/ })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Any column" })).toBeNull();
  });

  it("previews and applies a staged edit through the session's own apply", async () => {
    const info = await connect("documents");
    const tab = tableTab(info, 10);
    wrap(<TableTab tab={tab} />);
    await waitFor(() => expect(shownIds()).toHaveLength(10));
    const status = gridProps().columns.findIndex((c) => c.name === "status");
    act(() => gridProps().onEditCommit!(0, status, { t: "text", v: "shipped" }));
    const { sources } = collectWorkspace(info.connectionId);
    const plan = await previewWorkspace(info.connectionId, sources);
    expect(plan.groups[0]!.statements).toEqual([
      'db.orders.updateOne({_id: "o1"}, {$set: {status: "shipped"}})',
    ]);
    expect(await applyWorkspace(info.connectionId)).toBe(true);
    expect(guard.calls.at(-1)).toEqual({ kind: "commit", statements: plan.groups[0]!.statements });
    const page = await ipc.browse.rows(info.connectionId, {
      table: { schema: null, name: "orders" },
      columns: null,
      filter: null,
      sort: [],
      limit: 1,
      offset: 0,
    });
    expect(page.rows[0]![status]).toEqual({ t: "text", v: "shipped" });
    expect(pendingCount(useTableTabsStore.getState().slices[tab.id]!.staging)).toBe(0);
  });

  it("keeps only the failed change staged when a sequential commit stops partway", async () => {
    const info = await connect("documents");
    const tab = tableTab(info, 10);
    wrap(<TableTab tab={tab} />);
    await waitFor(() => expect(shownIds()).toHaveLength(10));
    const status = gridProps().columns.findIndex((c) => c.name === "status");
    act(() => gridProps().onEditCommit!(0, status, { t: "text", v: "shipped" }));
    act(() => gridProps().onEditCommit!(1, status, { t: "text", v: "__fail__" }));
    expect(await applyWorkspace(info.connectionId)).toBe(false);
    const staging = useTableTabsStore.getState().slices[tab.id]!.staging;
    expect([...staging.changes.keys()]).toEqual(["pk:o2"]);
    expect(staging.changes.get("pk:o2")!.error).toBe("document failed validation");
    // An applied change cannot come back by undo, to be sent twice.
    expect(staging.undo).toEqual([]);
    expect(toast.error).toHaveBeenCalledWith(
      "The commit stopped partway",
      expect.objectContaining({ description: expect.stringContaining("1 change before it") }),
    );
  });

  it("shows the lane's inspector for the focused row", async () => {
    const info = await connect("documents");
    wrap(
      <>
        <TableTab tab={tableTab(info, 10)} />
        <InspectorPanel />
      </>,
    );
    await waitFor(() => expect(shownIds()).toHaveLength(10));
    const row = gridProps().getRow(4)!;
    act(() =>
      gridProps().onFocusCell!({
        pos: { r: 4, c: 0 },
        sourceIndex: 0,
        column: gridProps().columns[0]!,
        value: row.cells[0]!,
        row,
      }),
    );
    const inspector = await screen.findByTestId("fake-inspector");
    // o5 has no status: the field is missing, not NULL.
    expect(inspector).toHaveTextContent('"_id":{"t":"text","v":"o5"}');
    expect(inspector).toHaveTextContent('"status":{"t":"other","v":{"ty":"missing","text":""}}');
  });

  it("names its tables as the session does", async () => {
    // jsdom has no layout: give the scroller and its rows a size so the virtualizer renders.
    vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(400);
    vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(240);
    const info = await connect("documents");
    const tables = await ipc.schema.tables(info.connectionId, "");
    wrap(
      <SchemaTree
        connectionId={info.connectionId}
        rows={buildTreeRows(tables, "")}
        loading={false}
        error={null}
        search=""
      />,
    );
    expect(screen.getByRole("button", { name: /^Collections/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Tables/ })).toBeNull();
  });

  it("adds the lane's object menu, whose items call the session", async () => {
    const info = await connect("documents");
    const call = vi.spyOn(ipc, "sessionCall");
    const ui = await awaitDriverUi("mongodb");
    const entries = laneMenuEntries(
      ui,
      { table: { schema: null, name: "orders" }, kind: "table", database: "app" },
      info,
      () => {},
    );
    expect(entries.map((e) => (e.kind === "item" ? e.label : e.kind))).toEqual([
      "separator",
      "Collection Stats",
      "Compact…",
    ]);
    const stats = entries[1]!;
    if (stats.kind === "item") stats.onSelect();
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith(info.connectionId, "stats", { collection: "orders" }),
    );
    expect(guard.calls).toContainEqual({ kind: "operation", label: "stats", write: true });
  });

  it("shows the lane's structure view", async () => {
    const info = await connect("documents");
    wrap(
      <StructureTab
        tab={{
          id: "s1",
          kind: "structure",
          connectionId: info.connectionId,
          table: { schema: null, name: "orders" },
          title: "orders",
        }}
      />,
    );
    expect(await screen.findByTestId("fake-structure")).toHaveTextContent("Indexes of orders");
  });
});

describe("the console of a document store", () => {
  it("runs its commands, and Safe Mode reads only the write as one", async () => {
    const info = await connect("documents");
    expect(languageOf(info)).toMatchObject({ fileExtension: "fake", placeholder: "find orders" });
    const tabId = useTabsStore.getState().newQuery(info.connectionId);
    seedEditor(tabId, "find orders\ncount users");
    await runQuery(tabId);
    const run = getQueryTab(tabId).run!;
    expect(run.status).toBe("done");
    expect(run.statements.map((s) => s.sql)).toEqual(["find orders", "count users"]);
    expect(guard.calls).toEqual([{ kind: "query", statements: ["find orders", "count users"] }]);
    // The lane's console classifier reads them: no write, so no prompt.
    expect(await sessionWrites(info, guard.calls[0]!)).toBe(false);
    const write: GuardRequest = { kind: "query", statements: ["find orders", "delete orders o1"] };
    expect(await sessionWrites(info, write)).toBe(true);
  });
});

describe("the SQL session of an extension dialect", () => {
  it("splits its editor's text with the dialect's splitter and runs it", async () => {
    const info = await connect("sql");
    await waitFor(() => expect(languageOf(info)).toMatchObject({ label: "Fake SQL" }));
    const tabId = useTabsStore.getState().newQuery(info.connectionId);
    // T-SQL's batches end at GO (a terminator is optional): split as the backend splits them.
    seedEditor(tabId, "SELECT * FROM orders\nGO\nSELECT * FROM users");
    await runQuery(tabId);
    const run = getQueryTab(tabId).run!;
    expect(run.status).toBe("done");
    expect(run.results).toHaveLength(2);
    expect(guard.calls.at(-1)).toEqual({
      kind: "query",
      statements: ["SELECT * FROM orders", "SELECT * FROM users"],
    });
  });

  it("gives a plugin its editor's statements and runs its SQL, split the same way", async () => {
    const info = await connect("sql");
    await waitFor(() => expect(languageOf(info)).toMatchObject({ label: "Fake SQL" }));
    const tabId = useTabsStore.getState().newQuery(info.connectionId);
    const text = "SELECT * FROM orders\nGO\nSELECT * FROM users";
    seedEditor(tabId, text);
    // The cursor is at the start: the first batch, up to GO (no core scanner reads T-SQL).
    expect(editorSql(tabId)).toEqual({ text, selected: "", statement: "SELECT * FROM orders" });
    const result = await executeSql(info.connectionId, text);
    expect(result.statements).toBe(2);
    expect(guard.calls.at(-1)).toEqual({
      kind: "query",
      statements: ["SELECT * FROM orders", "SELECT * FROM users"],
    });
  });

  it("shows its structure read-only, with Show DDL", async () => {
    const info = await connect("sql");
    wrap(
      <StructureTab
        tab={{
          id: "s2",
          kind: "structure",
          connectionId: info.connectionId,
          table: { schema: null, name: "orders" },
          title: "orders",
        }}
      />,
    );
    expect(await screen.findByText(/Table \(read-only\)/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show DDL" }));
    expect(await screen.findByText(/CREATE TABLE orders/)).toBeInTheDocument();
  });

  it("filters with its own operators and a SQL WHERE, and says how a batch commits", async () => {
    const info = await connect("sql");
    wrap(<TableTab tab={tableTab(info, 10)} />);
    await waitFor(() => expect(shownIds()).toHaveLength(10));
    expect(gridProps().onSortChange).toBeDefined();
    // Another engine's SQL: no core copy formats.
    expect(gridProps().copyContext).toBeUndefined();
    fireEvent.click(screen.getByRole("button", { name: "Filters" }));
    fireEvent.click(await screen.findByRole("combobox", { name: "Filter operator" }));
    const ops = (await screen.findAllByRole("option")).map((o) => o.textContent);
    expect(ops).toEqual(["=", "<>", "IS NULL", "IS NOT NULL", "LIKE"]);
  });
});

describe("panes and menus reachable from either fake session (critique A10)", () => {
  for (const fake of ["documents", "sql"] as const) {
    it(`render without a core dialect: ${fake}`, async () => {
      const info = await connect(fake);
      const connection = useConnectionsStore.getState().byId[info.connectionId]!;
      // The console pane, after a statement of the session was logged.
      const tabId = useTabsStore.getState().newQuery(info.connectionId);
      seedEditor(tabId, fake === "sql" ? "SELECT * FROM users" : "find users");
      if (fake === "sql") await waitFor(() => expect(languageOf(info)).toBeTruthy());
      await runQuery(tabId);
      const panes = wrap(
        <>
          <ConsolePanel />
          <HistoryPane connection={connection} />
          <ShowDdlDialog
            open
            onOpenChange={() => {}}
            connectionId={info.connectionId}
            table={{ schema: null, name: "orders" }}
            dialect={null}
          />
        </>,
      );
      expect(
        (await screen.findAllByText(fake === "sql" ? "SELECT * FROM users" : "find users")).length,
      ).toBeGreaterThan(0);
      panes.unmount();
      // A saved query opens in the session's editor.
      const opened = openSavedQuery(connection, {
        id: "q1",
        name: "Users",
        keyword: null,
        sql: "find users",
        folderId: null,
        profileId: null,
      } as Parameters<typeof openSavedQuery>[1]);
      expect(opened).not.toBeNull();
      // The palette lists the session's items.
      const commands = new Proxy({}, { get: () => vi.fn() }) as PaletteCommands;
      const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
      const { result } = renderHook(() => usePaletteItems("", commands), {
        wrapper: ({ children }) => (
          <QueryClientProvider client={qc}>{children}</QueryClientProvider>
        ),
      });
      await waitFor(() => expect(result.current).toBeTruthy());

      // The grid's Copy As: its formats, and no SQL INSERTs in a dialect the session does not
      // speak (they would be PostgreSQL's). The real grid, with the props the table tab gives it.
      const tableView = wrap(<TableTab tab={tableTab(info, 10)} />);
      await waitFor(() => expect(shownIds()).toHaveLength(10));
      const props = gridProps();
      tableView.unmount();
      expect(props.copyContext).toBeUndefined();
      sizeTheGrid();
      const realGrid = wrap(<RealDataGrid {...props} active={false} />);
      const first = document.querySelector<HTMLElement>('[data-cell][data-r="0"][data-c="0"]')!;
      fireEvent.pointerDown(first, { button: 0 });
      const scope = document.querySelector<HTMLElement>('[data-scope="grid"]')!;
      fireEvent.keyDown(scope, { key: "ç", code: "KeyC", ...mod, altKey: true });
      const offered = screen
        .getAllByRole("menuitem")
        .map((el) => el.textContent?.replace(/\d$/, "").trim());
      expect(offered).toEqual([
        "Plain text",
        "JSON",
        "HTML",
        "Markdown",
        "CSV",
        "CSV with header",
        "Raw",
      ]);
      realGrid.unmount();

      // A view's Structure: never the core definition editor (`ViewDefinitionPane` edits a core
      // dialect's CREATE VIEW). The SQL session shows it read-only with Show DDL, the document
      // store its lane's view. The fakes have no views, so `orders` is described as one.
      const describe = ipc.schema.table.bind(ipc.schema);
      const asView = vi
        .spyOn(ipc.schema, "table")
        .mockImplementation(async (...args) => ({ ...(await describe(...args)), kind: "view" }));
      wrap(
        <StructureTab
          tab={{
            id: `v-${fake}`,
            kind: "structure",
            connectionId: info.connectionId,
            table: { schema: null, name: "orders" },
            title: "orders",
          }}
        />,
      );
      if (fake === "sql") {
        expect(await screen.findByText(/Table \(read-only\)/)).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Show DDL" })).toBeInTheDocument();
      } else {
        expect(await screen.findByTestId("fake-structure")).toBeInTheDocument();
      }
      expect(screen.queryByLabelText("View definition")).toBeNull();
      expect(screen.queryByRole("region", { name: "Definition" })).toBeNull();
      if (fake === "sql") expect(asView).toHaveBeenCalled();
      asView.mockRestore();
    });
  }
});
