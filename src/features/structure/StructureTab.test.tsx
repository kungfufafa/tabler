import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { actions } from "@/features/palette/actions";
import { ipc, type DdlOp, type TableRef } from "@/lib/ipc";
import { resetMockDdl } from "@/lib/ipc/mock/ddl";
import { isMac } from "@/lib/keys/combo";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import { getEditorState } from "@/features/editor/editorStates";
import { EditorView } from "@codemirror/view";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { useStructureStore } from "./store";
import { StructureTab } from "./StructureTab";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));

// CodeMirror measures text ranges, which jsdom does not lay out.
if (!Range.prototype.getClientRects) {
  const empty = () => ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] });
  Range.prototype.getClientRects = empty as unknown as Range["getClientRects"];
  Range.prototype.getBoundingClientRect = () => new DOMRect();
}
vi.mock("sonner", () => ({ toast }));

async function setup(
  profileId = "p-local-pg",
  table: TableRef = { schema: "public", name: "users" },
  { keyboard = false, preview = false }: { keyboard?: boolean; preview?: boolean } = {},
) {
  const info = await ipc.connections.connect(profileId);
  useConnectionsStore.getState().add(info);
  const id = useTabsStore.getState().openTable(info.connectionId, table, "structure", { preview });
  const tab = useTabsStore.getState().tabs.find((t) => t.id === id) as Extract<
    Tab,
    { kind: "structure" }
  >;
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const tree = (
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        {/* Where focus sits while the metadata loads (e.g. the sidebar row that opened it). */}
        <button type="button" data-scope="sidebar">
          sidebar row
        </button>
        <StructureTab tab={tab} />
      </TooltipProvider>
    </QueryClientProvider>
  );
  render(keyboard ? <KeyboardProvider>{tree}</KeyboardProvider> : tree);
  screen.getByRole("button", { name: "sidebar row" }).focus();
  await screen.findByRole("region", { name: "Columns" });
  const ops = () => useStructureStore.getState().byTab[id]?.ops ?? [];
  const current = () => useTabsStore.getState().tabs.find((t) => t.id === id)!;
  return { id, ops, current, connectionId: info.connectionId };
}

const cell = (key: string) => {
  const el = document.querySelector<HTMLInputElement>(`[data-cell-key="${key}"]`);
  if (!el) throw new Error(`no cell ${key}`);
  return el;
};

function type(input: HTMLInputElement, value: string) {
  fireEvent.focus(input);
  fireEvent.change(input, { target: { value } });
  fireEvent.keyDown(input, { key: "Enter" });
}

beforeEach(() => {
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useStructureStore.setState({ byTab: {} });
});

afterEach(() => {
  resetMockDdl();
  Object.values(toast).forEach((fn) => fn.mockReset());
});

describe("StructureTab", () => {
  it("lists columns, indexes, foreign keys and (Triggers button) triggers", async () => {
    await setup("p-local-pg", { schema: "public", name: "posts" });
    expect(cell("c:user_id:name").value).toBe("user_id");
    expect(cell("i:posts_pkey:name").value).toBe("posts_pkey");
    expect(screen.getByText("posts_user_id_fkey")).toBeInTheDocument();
    // The foreign_key cell names the referenced table.
    expect(cell("c:user_id:foreignKey")).toHaveTextContent("users(id)");
    // TablePlus's PostgreSQL grid columns.
    const heads = within(screen.getByRole("region", { name: "Columns" }))
      .getAllByRole("columnheader")
      .map((h) => h.textContent);
    expect(heads.slice(1, -1)).toEqual([
      "column_name",
      "data_type",
      "is_nullable",
      "check",
      "column_default",
      "foreign_key",
      "comment",
    ]);
    expect(screen.queryByRole("region", { name: "Triggers" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Triggers" }));
    const triggers = screen.getByRole("region", { name: "Triggers" });
    expect(within(triggers).getByText("posts_set_updated_at")).toBeInTheDocument();
    expect(within(triggers).getByText(/EXECUTE FUNCTION set_updated_at\(\)/)).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Columns" })).toBeNull();
  });

  it("stages a rename, reviews it with ⌘S and applies it", async () => {
    const { ops } = await setup();
    type(cell("c:nickname:name"), "nick");
    expect(ops()).toEqual([
      {
        op: "renameColumn",
        table: { schema: "public", name: "users" },
        name: "nickname",
        newName: "nick",
      },
    ]);
    expect(cell("c:nickname:name").value).toBe("nick");
    expect(cell("c:nickname:name").closest("td")).toHaveClass("bg-changed");
    expect(screen.getByText("1 change")).toBeInTheDocument();

    await act(() => actions.run("grid.commit"));
    const statement = await screen.findByTestId("ddl-statement-0");
    expect(statement).toHaveTextContent(
      'ALTER TABLE "public"."users" RENAME COLUMN "nickname" TO "nick"',
    );
    expect(screen.getByText(/run in a single transaction/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /^Apply/ }));
    await waitFor(() => expect(toast.success).toHaveBeenCalled());
    expect(ops()).toEqual([]);
    await waitFor(() => expect(cell("c:nick:name").value).toBe("nick"));
    expect(cell("c:nick:name").closest("td")).not.toHaveClass("bg-changed");
    expect(screen.queryByText("Review structure changes")).not.toBeInTheDocument();
  });

  it("shows the failed statement and keeps the staged ops when apply fails", async () => {
    const { ops } = await setup();
    type(cell("c:bio:dataType"), "nosuchtype");
    await act(() => actions.run("grid.commit"));
    await screen.findByTestId("ddl-statement-0");
    fireEvent.click(screen.getByRole("button", { name: /^Apply/ }));
    const failed = await screen.findByText('type "nosuchtype" does not exist');
    expect(failed.closest("li")).toHaveAttribute("data-failed", "true");
    expect(ops()).toHaveLength(1);
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("⌘S inside a cell commits the edit and opens the review", async () => {
    const { ops } = await setup();
    const input = cell("c:balance:defaultExpr");
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "" } });
    await act(async () => {
      fireEvent.keyDown(input, { key: "s", metaKey: true });
    });
    expect(ops()).toEqual([
      {
        op: "alterColumnDefault",
        table: { schema: "public", name: "users" },
        name: "balance",
        defaultExpr: null,
      },
    ]);
    expect(await screen.findByTestId("ddl-statement-0")).toHaveTextContent(
      'ALTER TABLE "public"."users" ALTER COLUMN "balance" DROP DEFAULT',
    );
  });

  it("adds a column (⌘N), drops another, undoes and discards", async () => {
    const { ops } = await setup();
    await act(() => actions.run("grid.new-row"));
    const name = cell("n:0:name");
    expect(name.value).toBe("column_1");
    expect(name.closest("tr")).toHaveClass("bg-inserted");
    await waitFor(() => expect(document.activeElement).toBe(name));

    const row = cell("c:bio:name").closest("tr")!;
    fireEvent.click(within(row).getByRole("button", { name: "Drop column" }));
    expect(row).toHaveAttribute("data-status", "dropped");
    expect(row).toHaveClass("bg-deleted");
    expect(ops().map((o) => o.op)).toEqual(["addColumn", "dropColumn"]);

    await act(() => actions.run("grid.undo"));
    expect(ops().map((o) => o.op)).toEqual(["addColumn"]);
    await act(() => actions.run("grid.discard"));
    expect(ops()).toEqual([]);
    expect(document.querySelector('[data-cell-key="n:0:name"]')).toBeNull();
  });

  it("SQLite: warns about the table rebuild and hides comments", async () => {
    const { ops } = await setup("p-sample-sqlite", { schema: "main", name: "tags" });
    expect(screen.queryByRole("columnheader", { name: "comment" })).not.toBeInTheDocument();
    // is_nullable is keyboard-first: Y / N set it.
    const nullable = screen.getByRole("button", { name: "name nullable" });
    expect(nullable).toHaveTextContent("NO");
    fireEvent.keyDown(nullable, { key: "y" });
    expect(ops()).toEqual([
      {
        op: "alterColumnNullable",
        table: { schema: "main", name: "tags" },
        name: "name",
        nullable: true,
      },
    ]);
    await act(() => actions.run("grid.commit"));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      /rebuilt from introspected metadata/,
    );
    expect(screen.getByText(/run one by one/)).toBeInTheDocument();
  });

  it("takes keyboard focus once uncached metadata loads, so ⌘I reaches the grid", async () => {
    const { ops } = await setup(
      "p-local-pg",
      { schema: "public", name: "users" },
      { keyboard: true },
    );
    const root = document.querySelector<HTMLElement>("[data-scope='grid']");
    await waitFor(() => expect(document.activeElement).toBe(root));
    // ⌘I twice (⌘N is "New connection", as in TablePlus).
    for (const key of ["i", "i"]) {
      // The added column's name input takes focus (grid-edit scope: no new column from there).
      root!.focus();
      await act(async () => {
        fireEvent.keyDown(document.activeElement!, {
          key,
          code: `Key${key.toUpperCase()}`,
          [isMac ? "metaKey" : "ctrlKey"]: true,
        });
      });
    }
    expect(ops().map((o) => o.op)).toEqual(["addColumn", "addColumn"]);
  });

  it("switches the tab to its data view (bottom switch, ⌃⌘[)", async () => {
    const { current } = await setup();
    const bar = screen.getByRole("toolbar", { name: "Table structure" });
    const switcher = within(bar).getByRole("radiogroup", { name: "Table view" });
    expect(within(switcher).getByRole("radio", { name: "Structure" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    fireEvent.click(within(switcher).getByRole("radio", { name: "Data" }));
    expect(current().kind).toBe("table");

    act(() => useTabsStore.getState().setView(current().id, "structure"));
    await act(() => actions.run("view.data"));
    expect(current().kind).toBe("table");
  });

  it("the bottom bar holds + Column, + Index, the search, the summary and the staged changes", async () => {
    const { ops } = await setup("p-local-pg", { schema: "public", name: "posts" });
    const bar = screen.getByRole("toolbar", { name: "Table structure" });
    expect(bar).toHaveTextContent("9 columns · 1 index · 1 foreign key");
    for (const name of ["Column", "Index", "Open in SQL", "Refresh"])
      expect(within(bar).getByRole("button", { name })).toBeInTheDocument();
    expect(within(bar).getByRole("textbox", { name: "Search columns" })).toBeInTheDocument();
    expect(within(bar).queryByRole("button", { name: "Review & apply" })).toBeNull();

    type(cell("c:title:name"), "headline");
    expect(within(bar).getByText("1 change")).toBeInTheDocument();
    expect(within(bar).getByRole("button", { name: "Undo last change" })).toBeEnabled();
    fireEvent.click(within(bar).getByRole("button", { name: "Review & apply" }));
    expect(await screen.findByTestId("ddl-statement-0")).toHaveTextContent(/RENAME COLUMN/);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByTestId("ddl-statement-0")).toBeNull());
    fireEvent.click(within(bar).getByRole("button", { name: "Discard all changes" }));
    expect(ops()).toEqual([]);
    expect(within(bar).queryByText("1 change")).toBeNull();
  });

  it("pins a preview tab once a change is staged", async () => {
    const { current } = await setup(
      "p-local-pg",
      { schema: "public", name: "users" },
      {
        preview: true,
      },
    );
    expect(current()).toMatchObject({ preview: true });
    type(cell("c:nickname:name"), "nick");
    await waitFor(() => expect(current()).not.toHaveProperty("preview"));
  });

  it("↑/↓ commits a cell once (one toast for an invalid name)", async () => {
    const { ops } = await setup();
    const input = cell("c:nickname:name");
    input.focus();
    fireEvent.change(input, { target: { value: "email" } });
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(document.activeElement).toBe(cell("c:role:name"));
    expect(toast.error).toHaveBeenCalledTimes(1);
    expect(toast.error).toHaveBeenCalledWith("A column named “email” already exists");
    expect(ops()).toEqual([]);
  });

  it("MySQL: a new column offers no auto increment box (ADD COLUMN cannot key it)", async () => {
    await setup("p-shop-mysql", { schema: "tabler_dev", name: "users" });
    await act(() => actions.run("grid.new-row"));
    expect(cell("n:0:name").value).toBe("column_1");
    expect(screen.queryByRole("checkbox", { name: "column_1 auto increment" })).toBeNull();
  });

  it("PostgreSQL: the Sequence default of a new column needs an integer type", async () => {
    const { ops } = await setup();
    await act(() => actions.run("grid.new-row"));
    const column = () => (ops()[0] as Extract<DdlOp, { op: "addColumn" }>).column;
    const sequence = async () => {
      fireEvent.keyDown(screen.getByRole("button", { name: "column_1 default presets" }), {
        key: "Enter",
      });
      fireEvent.click(await screen.findByRole("menuitem", { name: /^Sequence/ }));
    };
    await sequence();
    expect(toast.error).toHaveBeenCalledWith(expect.stringMatching(/integer type/));
    expect(column().autoIncrement).toBe(false);
    type(cell("n:0:dataType"), "bigint");
    await sequence();
    expect(column()).toMatchObject({ dataType: "bigint", autoIncrement: true });
    expect(cell("n:0:defaultExpr")).toHaveAttribute("placeholder", "identity");
    expect(toast.error).toHaveBeenCalledTimes(1);
  });

  it("MySQL: a partly applied plan stays flagged after the sheet closes", async () => {
    const { ops } = await setup("p-shop-mysql", { schema: "tabler_dev", name: "users" });
    type(cell("c:nickname:name"), "nick");
    type(cell("c:bio:dataType"), "nosuchtype");
    await act(() => actions.run("grid.commit"));
    await screen.findByTestId("ddl-statement-1");
    fireEvent.click(screen.getByRole("button", { name: /^Apply/ }));
    await screen.findByText(/already ran and stay applied/);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() =>
      expect(screen.queryByText("Review structure changes")).not.toBeInTheDocument(),
    );

    const banner = screen.getByText(/The last apply stopped partway/).closest("[role='alert']")!;
    expect(banner).toHaveTextContent("1 statement already ran");
    expect(ops()).toHaveLength(2);
    fireEvent.click(within(banner as HTMLElement).getByRole("button", { name: /Discard/ }));
    await waitFor(() => expect(ops()).toEqual([]));
    await waitFor(() => expect(cell("c:nick:name").value).toBe("nick"));
    expect(screen.queryByText(/stopped partway/)).not.toBeInTheDocument();
  });

  it("keeps the staged ops of recently closed tabs (⌘⇧T restores them)", async () => {
    const closed = {
      id: "closed-1",
      kind: "structure" as const,
      connectionId: "c",
      table: { schema: "public", name: "tags" },
      title: "tags",
    };
    useTabsStore.setState({ closed: [closed] });
    useStructureStore.getState().stage("closed-1", () => [{ op: "rawSql", sql: "a" }]);
    useStructureStore.getState().stage("gone", () => [{ op: "rawSql", sql: "b" }]);
    await setup();
    expect(useStructureStore.getState().byTab["closed-1"]?.ops).toHaveLength(1);
    expect(useStructureStore.getState().byTab.gone).toBeUndefined();
  });

  it("opens the CREATE statement in a new SQL editor tab", async () => {
    const { connectionId } = await setup();
    const users = { schema: "public", name: "users" };
    const ddl = await ipc.ddl.tableDdl(connectionId, users);
    const activeTab = () => {
      const { tabs, activeId } = useTabsStore.getState();
      return tabs.find((t) => t.id === activeId);
    };

    fireEvent.click(screen.getByRole("button", { name: "Open in SQL" }));
    await waitFor(() => expect(activeTab()?.kind).toBe("query"));
    const first = activeTab()!;
    expect(first).toMatchObject({ kind: "query", connectionId, title: "users.sql" });
    expect(getEditorState(first.id)?.doc.toString()).toBe(ddl);

    // The same from the Info dialog (the CREATE statement), which closes on the way.
    fireEvent.click(screen.getByRole("button", { name: "Info" }));
    const dialog = await screen.findByRole("dialog");
    const open = within(dialog).getByRole("button", { name: "Open in SQL Editor" });
    await waitFor(() => expect(open).toBeEnabled());
    fireEvent.click(open);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    const second = activeTab()!;
    expect(second.id).not.toBe(first.id);
    expect(second).toMatchObject({ kind: "query", title: "users.sql" });
    expect(getEditorState(second.id)?.doc.toString()).toBe(ddl);
  });

  it("says so when the CREATE statement cannot be loaded", async () => {
    await setup();
    vi.spyOn(ipc.ddl, "tableDdl").mockRejectedValueOnce({
      code: "DB_ERROR",
      message: "permission denied for table users",
    });
    fireEvent.click(screen.getByRole("button", { name: "Open in SQL" }));
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith("Could not load the CREATE statement", {
        description: "permission denied for table users",
      }),
    );
    expect(useTabsStore.getState().tabs.some((t) => t.kind === "query")).toBe(false);
    vi.restoreAllMocks();
  });

  it("changes the primary key in the Primary token field, reviews and applies it", async () => {
    const { ops } = await setup();
    const users = { schema: "public", name: "users" };
    const keyField = () => screen.getByRole("list", { name: "Primary key" });
    const tokens = () =>
      within(keyField())
        .getAllByRole("listitem")
        .map((t) => t.textContent);
    expect(tokens()).toEqual(["id"]);

    fireEvent.keyDown(screen.getByRole("button", { name: "Add a column to the primary key" }), {
      key: "Enter",
    });
    fireEvent.click(await screen.findByRole("menuitem", { name: "email" }));
    expect(ops()).toEqual([
      { op: "dropPrimaryKey", table: users },
      { op: "addPrimaryKey", table: users, columns: ["id", "email"] },
    ]);
    expect(tokens()).toEqual(["id", "email"]);
    // Key columns are NOT NULL: their is_nullable cell says NO and is locked.
    const nullable = screen.getByRole("button", { name: "email nullable" });
    expect(nullable).toHaveTextContent("NO");
    expect(nullable).toBeDisabled();

    // ⌘Z takes the change back like any other staged edit; ⇧⌘Z redoes it.
    await act(() => actions.run("grid.undo"));
    expect(ops()).toEqual([]);
    await act(() => actions.run("grid.redo"));
    expect(ops()).toHaveLength(2);

    await act(() => actions.run("grid.commit"));
    expect(await screen.findByTestId("ddl-statement-1")).toHaveTextContent(
      'ALTER TABLE "public"."users" ADD PRIMARY KEY ("id", "email")',
    );
    fireEvent.click(screen.getByRole("button", { name: /^Apply/ }));
    await waitFor(() => expect(toast.success).toHaveBeenCalled());
    expect(ops()).toEqual([]);
    await waitFor(() => expect(tokens()).toEqual(["id", "email"]));

    // Removing a token drops it from the key.
    fireEvent.click(screen.getByRole("button", { name: "Remove email from the primary key" }));
    expect(ops()).toEqual([
      { op: "dropPrimaryKey", table: users },
      { op: "addPrimaryKey", table: users, columns: ["id"] },
    ]);
  });

  it("views: the Structure view edits the definition (CREATE OR REPLACE VIEW)", async () => {
    const { ops } = await setup("p-local-pg", { schema: "public", name: "active_users" });
    const host = await screen.findByTestId("structure-sql-editor");
    await waitFor(() => expect(host.textContent).toMatch(/CREATE OR REPLACE VIEW/));
    // The columns are listed read-only below.
    expect(cell("c:email:name")).toHaveAttribute("readonly");
    expect(screen.queryByRole("button", { name: "Column" })).not.toBeInTheDocument();
    const view = EditorView.findFromDOM(host.querySelector(".cm-editor") as HTMLElement)!;
    const text = view.state.doc.toString();
    act(() =>
      view.dispatch({
        changes: {
          from: 0,
          to: text.length,
          insert: text.replace("WHERE is_active", "WHERE true"),
        },
      }),
    );
    expect(ops()).toEqual([{ op: "rawSql", sql: expect.stringMatching(/WHERE true;$/) }]);
    await act(() => actions.run("grid.commit"));
    expect(await screen.findByTestId("ddl-statement-0")).toHaveTextContent(
      /CREATE OR REPLACE VIEW "public"\."active_users" AS SELECT .* WHERE true;/,
    );
  });
});
