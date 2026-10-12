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
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { useStructureStore } from "./store";
import { StructureTab } from "./StructureTab";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

async function setup(
  profileId = "p-local-pg",
  table: TableRef = { schema: "public", name: "users" },
  { keyboard = false }: { keyboard?: boolean } = {},
) {
  const info = await ipc.connections.connect(profileId);
  useConnectionsStore.getState().add(info);
  const id = useTabsStore.getState().openTable(info.connectionId, table, "structure");
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
  return { id, ops, connectionId: info.connectionId };
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
  it("lists columns, indexes, foreign keys and triggers", async () => {
    await setup("p-local-pg", { schema: "public", name: "posts" });
    expect(cell("c:user_id:name").value).toBe("user_id");
    expect(screen.getByText("posts_pkey")).toBeInTheDocument();
    expect(screen.getByText("posts_user_id_fkey")).toBeInTheDocument();
    const trigger = screen.getByRole("button", { name: /posts_set_updated_at/ });
    fireEvent.click(trigger);
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText(/set_updated_at\(\)/)).toBeInTheDocument();
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
    await setup("p-sample-sqlite", { schema: "main", name: "tags" });
    expect(screen.queryByRole("columnheader", { name: "Comment" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("checkbox", { name: "name nullable" }));
    await act(() => actions.run("grid.commit"));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      /rebuilt from introspected metadata/,
    );
    expect(screen.getByText(/run one by one/)).toBeInTheDocument();
  });

  it("takes keyboard focus once uncached metadata loads, so ⌘N reaches the grid", async () => {
    const { ops } = await setup(
      "p-local-pg",
      { schema: "public", name: "users" },
      { keyboard: true },
    );
    const root = document.querySelector<HTMLElement>("[data-scope='grid']");
    await waitFor(() => expect(document.activeElement).toBe(root));
    await act(async () => {
      fireEvent.keyDown(document.activeElement!, {
        key: "n",
        code: "KeyN",
        [isMac ? "metaKey" : "ctrlKey"]: true,
      });
    });
    expect(ops().map((o) => o.op)).toEqual(["addColumn"]);
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

  it("PostgreSQL: auto increment on a new column needs an integer type", async () => {
    const { ops } = await setup();
    await act(() => actions.run("grid.new-row"));
    const ai = () => screen.getByRole("checkbox", { name: "column_1 auto increment" });
    const column = () => (ops()[0] as Extract<DdlOp, { op: "addColumn" }>).column;
    fireEvent.click(ai());
    expect(toast.error).toHaveBeenCalledWith(expect.stringMatching(/integer type/));
    expect(column().autoIncrement).toBe(false);
    type(cell("n:0:dataType"), "bigint");
    fireEvent.click(ai());
    expect(column()).toMatchObject({ dataType: "bigint", autoIncrement: true });
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

    // The same from the DDL dialog, which closes on the way.
    fireEvent.click(screen.getByRole("button", { name: "Show DDL" }));
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

  it("toggles the primary key, reviews and applies it", async () => {
    const { ops } = await setup();
    const users = { schema: "public", name: "users" };
    const pk = (name: string) => screen.getByRole("button", { name: `${name} primary key` });
    expect(pk("id")).toHaveAttribute("aria-pressed", "true");
    expect(pk("email")).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(pk("email"));
    expect(ops()).toEqual([
      { op: "dropPrimaryKey", table: users },
      { op: "addPrimaryKey", table: users, columns: ["id", "email"] },
    ]);
    expect(pk("email")).toHaveAttribute("aria-pressed", "true");
    expect(pk("email").closest("td")).toHaveClass("bg-changed");
    // Key columns are NOT NULL: their nullable box is off and locked.
    expect(screen.getByRole("checkbox", { name: "email nullable" })).toBeDisabled();

    // ⌘Z takes the toggle back like any other staged edit.
    await act(() => actions.run("grid.undo"));
    expect(ops()).toEqual([]);
    expect(pk("email")).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(pk("email"));

    await act(() => actions.run("grid.commit"));
    expect(await screen.findByTestId("ddl-statement-1")).toHaveTextContent(
      'ALTER TABLE "public"."users" ADD PRIMARY KEY ("id", "email")',
    );
    fireEvent.click(screen.getByRole("button", { name: /^Apply/ }));
    await waitFor(() => expect(toast.success).toHaveBeenCalled());
    expect(ops()).toEqual([]);
    await waitFor(() => expect(pk("email")).toHaveAttribute("aria-pressed", "true"));
    expect(pk("email").closest("td")).not.toHaveClass("bg-changed");

    // A dropped column's key cell is locked.
    const row = cell("c:bio:name").closest("tr")!;
    fireEvent.click(within(row).getByRole("button", { name: "Drop column" }));
    expect(pk("bio")).toBeDisabled();
  });

  it("views are read-only", async () => {
    await setup("p-local-pg", { schema: "public", name: "active_users" });
    expect(screen.getByText("Views are shown read-only.")).toBeInTheDocument();
    expect(cell("c:email:name")).toHaveAttribute("readonly");
    expect(screen.getByRole("button", { name: "email primary key" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: /Column/ })).not.toBeInTheDocument();
    expect(actions.has("grid.commit")).toBe(false);
  });
});
