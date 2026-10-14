import { EditorView } from "@codemirror/view";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createContext, useContext, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { resetCommitStore } from "@/features/commit/store";
import { CommitTestHost } from "@/features/commit/testHost";
import { actions } from "@/features/palette/actions";
import { ipc, type TableRef } from "@/lib/ipc";
import { resetMockDdl } from "@/lib/ipc/mock/ddl";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { useStructureStore } from "./store";
import { StructureTab } from "./StructureTab";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));
const guard = vi.hoisted(() => ({ guardExecution: vi.fn(async () => true) }));
vi.mock("@/features/safety/guard", () => guard);

// Radix Select needs pointer / layout APIs jsdom lacks: stand in a native <select>.
vi.mock("@/components/ui/select", () => {
  interface Ctx {
    value: string;
    onValueChange: (v: string) => void;
    label?: string;
  }
  const SelectCtx = createContext<Ctx>({ value: "", onValueChange: () => {} });
  return {
    Select: ({
      value,
      onValueChange,
      children,
    }: {
      value: string;
      onValueChange: (v: string) => void;
      children: ReactNode;
    }) => <SelectCtx.Provider value={{ value, onValueChange }}>{children}</SelectCtx.Provider>,
    SelectTrigger: (props: { "aria-label"?: string }) => {
      useContext(SelectCtx).label = props["aria-label"];
      return null;
    },
    SelectValue: () => null,
    SelectContent: ({ children }: { children: ReactNode }) => {
      const ctx = useContext(SelectCtx);
      return (
        <select
          aria-label={ctx.label}
          value={ctx.value}
          onChange={(e) => ctx.onValueChange(e.target.value)}
        >
          <option value="" />
          {children}
        </select>
      );
    },
    SelectItem: ({ value, children }: { value: string; children: ReactNode }) => (
      <option value={value}>{children}</option>
    ),
  };
});

// CodeMirror measures text ranges, which jsdom does not lay out.
if (!Range.prototype.getClientRects) {
  const empty = () => ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] });
  Range.prototype.getClientRects = empty as unknown as Range["getClientRects"];
  Range.prototype.getBoundingClientRect = () => new DOMRect();
}

const USERS = { schema: "public", name: "users" };
const MY_USERS = { schema: "tabler_dev", name: "users" };

async function setup(profileId = "p-local-pg", table: TableRef = USERS) {
  const info = await ipc.connections.connect(profileId);
  useConnectionsStore.getState().add(info);
  const id = useTabsStore.getState().openTable(info.connectionId, table, "structure");
  const tab = () =>
    useTabsStore.getState().tabs.find((t) => t.id === id) as Extract<Tab, { kind: "structure" }>;
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const Host = () => {
    const current = useTabsStore((s) => s.tabs.find((t) => t.id === id)) as Extract<
      Tab,
      { kind: "structure" }
    >;
    return <StructureTab tab={current} />;
  };
  render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <Host />
        <CommitTestHost />
      </TooltipProvider>
    </QueryClientProvider>,
  );
  await screen.findByRole("region", { name: "Columns" });
  const ops = () => useStructureStore.getState().byTab[id]?.ops ?? [];
  return { id, ops, tab, connectionId: info.connectionId };
}

const cell = (key: string) => {
  for (const el of document.querySelectorAll<HTMLInputElement>("[data-cell-key]"))
    if (el.dataset.cellKey === key) return el;
  throw new Error(`no cell ${key}`);
};

function type(input: HTMLInputElement, value: string) {
  fireEvent.focus(input);
  fireEvent.change(input, { target: { value } });
  fireEvent.keyDown(input, { key: "Enter" });
}

async function pick(trigger: HTMLElement, item: RegExp | string) {
  fireEvent.keyDown(trigger, { key: "Enter" });
  const menu = await screen.findByRole("menu");
  const role = [
    ...within(menu).queryAllByRole("menuitem"),
    ...within(menu).queryAllByRole("menuitemradio"),
    ...within(menu).queryAllByRole("menuitemcheckbox"),
  ];
  const match = role.find((r) =>
    typeof item === "string" ? r.textContent === item : item.test(r.textContent ?? ""),
  );
  if (!match) throw new Error(`no menu item ${String(item)}`);
  fireEvent.click(match);
}

/**
 * The workspace's Preview SQL sheet (⌘S with staged structure): its structure section, once the
 * statements are listed.
 */
async function structureSection() {
  const sheet = await screen.findByRole("dialog", { name: "Preview changes" });
  const section = await within(sheet).findByRole("region", { name: /^Structure / });
  await waitFor(() => expect(section).toHaveTextContent(/[A-Z]{3,}/));
  return { sheet, section };
}

/** The Preview SQL sheet (⇧⌘P), then its Commit. */
async function apply() {
  await act(() => actions.run("grid.preview"));
  const { sheet } = await structureSection();
  fireEvent.click(within(sheet).getByRole("button", { name: /^Commit/ }));
}

beforeEach(() => {
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useStructureStore.setState({ byTab: {} });
  resetCommitStore();
  guard.guardExecution.mockClear();
});

afterEach(() => {
  resetMockDdl();
  Object.values(toast).forEach((fn) => fn.mockReset());
});

describe("structure header", () => {
  it("renames the table from the Name field; applying moves the tab to the new name", async () => {
    const { ops, tab, connectionId } = await setup();
    // A column edit staged after the rename still runs first.
    type(cell("table:name"), "members");
    type(cell("c:bio:name"), "about");
    expect(ops().map((o) => o.op)).toEqual(["renameColumn", "renameTable"]);
    await apply();
    await waitFor(() => expect(toast.success).toHaveBeenCalled());
    expect(guard.guardExecution).toHaveBeenCalledWith(connectionId, {
      kind: "commit",
      statements: [
        'ALTER TABLE "public"."users" RENAME COLUMN "bio" TO "about"',
        'ALTER TABLE "public"."users" RENAME TO "members"',
      ],
    });
    await waitFor(() => expect(tab().table).toEqual({ schema: "public", name: "members" }));
    expect(tab().title).toBe("members");
    await waitFor(() => expect(cell("table:name").value).toBe("members"));
    expect(ops()).toEqual([]);
  });

  it("safe mode can stop the apply: nothing runs, the changes stay staged", async () => {
    const { ops } = await setup();
    guard.guardExecution.mockResolvedValueOnce(false);
    type(cell("table:comment"), "people");
    expect(ops()).toEqual([{ op: "setTableComment", table: USERS, comment: "people" }]);
    await apply();
    await waitFor(() => expect(guard.guardExecution).toHaveBeenCalled());
    expect(toast.success).not.toHaveBeenCalled();
    expect(ops()).toHaveLength(1);
    expect(screen.getByText("Preview changes")).toBeInTheDocument();
  });

  it("MySQL: table options stage one ALTER TABLE", async () => {
    const { ops } = await setup("p-shop-mysql", MY_USERS);
    fireEvent.click(screen.getByRole("button", { name: "Table options" }));
    const engine = await screen.findByRole("button", { name: "Engine" });
    expect(engine).toHaveTextContent("InnoDB");
    await pick(engine, "MyISAM");
    await pick(screen.getByRole("button", { name: "Collation" }), "utf8mb4_bin");
    expect(ops()).toEqual([
      {
        op: "alterTableOptions",
        table: MY_USERS,
        options: {
          engine: "MyISAM",
          charset: null,
          collation: "utf8mb4_bin",
          autoIncrement: null,
          rowFormat: null,
        },
      },
    ]);
    await act(() => actions.run("grid.preview"));
    expect((await structureSection()).section).toHaveTextContent(
      "ALTER TABLE `tabler_dev`.`users` ENGINE = MyISAM, CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_bin",
    );
  });
});

describe("columns grid", () => {
  it("MySQL: TablePlus columns, extra flags and the default presets", async () => {
    const { ops } = await setup("p-shop-mysql", MY_USERS);
    const heads = within(screen.getByRole("region", { name: "Columns" }))
      .getAllByRole("columnheader")
      .map((h) => h.textContent);
    expect(heads.slice(1, -1)).toEqual([
      "column_name",
      "data_type",
      "is_nullable",
      "column_default",
      "extra",
      "foreign_key",
      "comment",
    ]);
    expect(cell("c:id:extra")).toHaveTextContent("auto_increment");
    // extra: on update CURRENT_TIMESTAMP
    await pick(cell("c:last_login:extra"), /on update/);
    expect(ops()).toEqual([
      {
        op: "alterColumnOnUpdate",
        table: MY_USERS,
        name: "last_login",
        onUpdate: "CURRENT_TIMESTAMP",
      },
    ]);
    // column_default: EMPTY
    await pick(screen.getByRole("button", { name: "nickname default presets" }), /^EMPTY/);
    expect(ops()[1]).toEqual({
      op: "alterColumnDefault",
      table: MY_USERS,
      name: "nickname",
      defaultExpr: "''",
    });
    // AUTO_INCREMENT needs a key (MySQL error 1075)
    await pick(screen.getByRole("button", { name: "big_id default presets" }), /^AUTO_INCREMENT/);
    expect(toast.error).toHaveBeenCalledWith(expect.stringMatching(/needs a key/));
    expect(ops()).toHaveLength(2);
  });

  it("MySQL: right-click a column row to insert a column after it", async () => {
    const { ops } = await setup("p-shop-mysql", MY_USERS);
    const row = screen.getByRole("button", { name: "Select column email" }).closest("tr")!;
    fireEvent.contextMenu(row);
    fireEvent.click(screen.getByRole("menuitem", { name: "Insert Column After" }));
    expect(ops()).toEqual([
      expect.objectContaining({ op: "addColumn", table: MY_USERS, after: "email" }),
    ]);
    const names = within(screen.getByRole("region", { name: "Columns" }))
      .getAllByRole("button", { name: /^Select column / })
      .map((b) => b.getAttribute("aria-label"));
    const at = names.indexOf("Select column email");
    expect(names[at + 1]).toBe("Select column column_1");
    // Delete from the same menu
    fireEvent.contextMenu(screen.getByRole("button", { name: "Select column bio" }).closest("tr")!);
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete Column" }));
    expect(ops().at(-1)).toEqual({ op: "dropColumn", table: MY_USERS, name: "bio" });
  });

  it("PostgreSQL: the row menu has no column position", async () => {
    await setup();
    fireEvent.contextMenu(
      screen.getByRole("button", { name: "Select column email" }).closest("tr")!,
    );
    expect(screen.queryByRole("menuitem", { name: "Insert Column After" })).toBeNull();
    expect(screen.getByRole("menuitem", { name: "Add Column" })).toBeInTheDocument();
  });

  it("PostgreSQL: a Constant default is quoted; the check cell adds, replaces and drops", async () => {
    const { ops } = await setup();
    await pick(screen.getByRole("button", { name: "nickname default presets" }), /^Constant/);
    const input = await screen.findByLabelText("Constant default of nickname");
    fireEvent.change(input, { target: { value: "it's me" } });
    fireEvent.click(screen.getByRole("button", { name: "Set default" }));
    expect(ops()).toEqual([
      { op: "alterColumnDefault", table: USERS, name: "nickname", defaultExpr: "'it''s me'" },
    ]);
    act(() => useStructureStore.getState().discard(useTabsStore.getState().tabs[0]!.id));

    expect(cell("c:balance:check").value).toBe("balance >= 0");
    type(cell("c:balance:check"), "balance > 0");
    expect(ops()).toEqual([
      { op: "dropCheck", table: USERS, name: "users_balance_check" },
      {
        op: "addCheck",
        table: USERS,
        check: { name: "users_balance_check", expression: "balance > 0" },
      },
    ]);
    type(cell("c:score:check"), "score <= 100");
    expect(ops()[2]).toEqual({
      op: "addCheck",
      table: USERS,
      check: { name: "users_score_check", expression: "score <= 100" },
    });
    // A column check names its column only.
    type(cell("c:bio:check"), "score > 1");
    expect(toast.error).toHaveBeenCalledWith(expect.stringMatching(/uses “bio” and no other/));
    expect(ops()).toHaveLength(3);
    await act(() => actions.run("grid.preview"));
    // the three staged checks are clauses of one ALTER TABLE
    expect((await structureSection()).section).toHaveTextContent(
      'ALTER TABLE "public"."users" DROP CONSTRAINT "users_balance_check", ADD CONSTRAINT "users_balance_check" CHECK (balance > 0), ADD CONSTRAINT "users_score_check" CHECK (score <= 100)',
    );
  });

  it("creates a foreign key from the foreign_key cell", async () => {
    const { ops } = await setup("p-local-pg", { schema: "public", name: "posts" });
    fireEvent.click(cell("c:body:foreignKey"));
    const form = await screen.findByRole("form", { name: "Foreign key" });
    expect(
      within(form.parentElement!).getByText(/Create a foreign key on column/),
    ).toBeInTheDocument();
    const select = within(form).getByLabelText("Referenced table");
    await within(select).findByRole("option", { name: "tags" });
    fireEvent.change(select, { target: { value: "tags" } });
    const refs = await within(form).findByRole("group", { name: "Referenced columns" });
    await waitFor(() =>
      expect(within(refs).getByRole("button", { name: /id/ })).toHaveAttribute(
        "aria-pressed",
        "true",
      ),
    );
    fireEvent.change(within(form).getByLabelText("On delete"), { target: { value: "CASCADE" } });
    fireEvent.click(within(form).getByRole("button", { name: "Ok" }));
    expect(ops()).toEqual([
      {
        op: "addForeignKey",
        table: { schema: "public", name: "posts" },
        fk: {
          name: null,
          columns: ["body"],
          refTable: { schema: "public", name: "tags" },
          refColumns: ["id"],
          onUpdate: null,
          onDelete: "CASCADE",
        },
      },
    ]);
    expect(cell("c:body:foreignKey")).toHaveTextContent("tags(id)");
  });

  it("⌘F searches the columns; Delete drops the selected row", async () => {
    const { ops } = await setup();
    await act(() => actions.run("grid.filter"));
    const search = screen.getByRole("textbox", { name: "Search columns" });
    expect(document.activeElement).toBe(search);
    fireEvent.change(search, { target: { value: "last" } });
    const rows = within(screen.getByRole("region", { name: "Columns" })).getAllByRole("row");
    expect(rows).toHaveLength(3); // header + last_login + the blank row that ends the grid
    expect(rows[2]).toHaveAttribute("data-blank-row", "columns");
    fireEvent.keyDown(search, { key: "Escape" });
    expect(search).toHaveValue("");

    const handle = screen.getByRole("button", { name: "Select column bio" });
    handle.focus();
    fireEvent.keyDown(handle, { key: "Delete" });
    expect(ops()).toEqual([{ op: "dropColumn", table: USERS, name: "bio" }]);
    // Delete again restores it.
    fireEvent.keyDown(handle, { key: "Backspace" });
    expect(ops()).toEqual([]);
  });
});

describe("indexes and checks", () => {
  it("+ Index stages a new index inline; editing an existing one stages drop + create", async () => {
    const { ops } = await setup();
    const handle = screen.getByRole("button", { name: "Select column email" });
    handle.focus();
    fireEvent.click(screen.getAllByRole("button", { name: "Index" })[0]!);
    expect(ops()).toEqual([
      {
        op: "addIndex",
        table: USERS,
        index: {
          name: "users_email_idx",
          columns: ["email"],
          unique: false,
          method: null,
          predicate: null,
          comment: null,
          include: [],
        },
      },
    ]);
    await waitFor(() => expect(document.activeElement).toBe(cell("ni:0:name")));
    // Unique: the name follows (still the suggested one).
    fireEvent.keyDown(cell("ni:0:unique"), { key: "t" });
    expect(ops()[0]).toMatchObject({ index: { name: "users_email_key", unique: true } });
    type(cell("ni:0:comment"), "logins");
    expect(ops()[0]).toMatchObject({ index: { comment: "logins" } });

    fireEvent.keyDown(cell("i:users_pkey:unique"), { key: "f" });
    expect(ops()).toHaveLength(1); // the primary key is changed in the header
  });

  it("PostgreSQL indexes show INCLUDE columns, set on new indexes", async () => {
    const { ops } = await setup();
    const indexes = screen.getByRole("region", { name: "Indexes" });
    const heads = within(indexes)
      .getAllByRole("columnheader")
      .map((h) => h.textContent);
    expect(heads).toContain("include");
    expect(heads.indexOf("include")).toBe(heads.indexOf("column_name") + 1);
    // existing indexes keep theirs
    expect(cell("i:users_pkey:include")).toBeDisabled();
    const handle = screen.getByRole("button", { name: "Select column email" });
    handle.focus();
    fireEvent.click(screen.getAllByRole("button", { name: "Index" })[0]!);
    const include = cell("ni:0:include");
    expect(include).toBeEnabled();
    expect(include).toHaveTextContent("none");
    fireEvent.click(include);
    const group = await screen.findByRole("group", { name: "users_email_idx include" });
    fireEvent.click(within(group).getByRole("button", { name: "name" }));
    fireEvent.keyDown(group, { key: "Escape" });
    await waitFor(() =>
      expect(ops()[0]).toMatchObject({ op: "addIndex", index: { include: ["name"] } }),
    );
    expect(cell("ni:0:include")).toHaveTextContent("name");
    await act(() => actions.run("grid.preview"));
    expect((await structureSection()).section).toHaveTextContent(
      'CREATE INDEX "users_email_idx" ON "public"."users" ("email") INCLUDE ("name")',
    );
  });

  it("MySQL indexes have no include column", async () => {
    await setup("p-shop-mysql", MY_USERS);
    const heads = within(screen.getByRole("region", { name: "Indexes" }))
      .getAllByRole("columnheader")
      .map((h) => h.textContent);
    expect(heads).not.toContain("include");
  });

  it("+ Check adds a check; an empty one goes away again", async () => {
    const { ops } = await setup();
    const checks = screen.getByRole("region", { name: "Checks" });
    expect(within(checks).getAllByRole("row")).toHaveLength(2);
    fireEvent.click(within(checks).getByRole("button", { name: "Check" }));
    const expr = cell("nk:0:expression");
    await waitFor(() => expect(document.activeElement).toBe(expr));
    fireEvent.blur(expr);
    expect(ops()).toEqual([]);
    fireEvent.click(within(checks).getByRole("button", { name: "Check" }));
    type(cell("nk:0:expression"), "length(name) > 0");
    expect(ops()).toEqual([
      {
        op: "addCheck",
        table: USERS,
        check: { name: "users_check", expression: "length(name) > 0" },
      },
    ]);
    expect(within(checks).getByText("name")).toBeInTheDocument();
  });
});

describe("triggers", () => {
  it("PostgreSQL: the enabled cell stages DISABLE / ENABLE TRIGGER; a disabled row is muted", async () => {
    const { ops } = await setup("p-local-pg", { schema: "public", name: "posts" });
    const posts = { schema: "public", name: "posts" };
    fireEvent.click(screen.getByRole("button", { name: "Triggers" }));
    const region = screen.getByRole("region", { name: "Triggers" });
    const row = within(region).getByText("posts_set_updated_at").closest("tr")!;
    expect(row).toHaveAttribute("data-enabled", "true");
    const enabled = cell("t:posts_set_updated_at:enabled");
    expect(enabled).toHaveTextContent("TRUE");
    fireEvent.keyDown(enabled, { key: "f" });
    expect(ops()).toEqual([{ op: "disableTrigger", table: posts, name: "posts_set_updated_at" }]);
    expect(row).toHaveAttribute("data-enabled", "false");
    expect(row).toHaveClass("text-muted-foreground");
    expect(cell("t:posts_set_updated_at:enabled")).toHaveTextContent("FALSE");
    // back on: nothing left to stage
    fireEvent.keyDown(enabled, { key: "t" });
    expect(ops()).toEqual([]);
    expect(row).not.toHaveClass("text-muted-foreground");
    await pick(enabled, "FALSE");
    await act(() => actions.run("grid.preview"));
    expect((await structureSection()).section).toHaveTextContent(
      'ALTER TABLE "public"."posts" DISABLE TRIGGER "posts_set_updated_at"',
    );
  });

  it("MySQL triggers have no enabled column", async () => {
    await setup("p-shop-mysql", { schema: "tabler_dev", name: "posts" });
    fireEvent.click(screen.getByRole("button", { name: "Triggers" }));
    const region = screen.getByRole("region", { name: "Triggers" });
    expect(within(region).getByText("posts_set_updated_at")).toBeInTheDocument();
    expect(within(region).queryByText("enabled")).toBeNull();
    expect(document.querySelector("[data-cell-key='t:posts_set_updated_at:enabled']")).toBeNull();
  });

  it("+ Trigger opens a template; editing stages DROP + CREATE; Delete drops", async () => {
    const { ops } = await setup("p-local-pg", { schema: "public", name: "posts" });
    const posts = { schema: "public", name: "posts" };
    fireEvent.click(screen.getByRole("button", { name: "Triggers" }));
    const region = screen.getByRole("region", { name: "Triggers" });
    fireEvent.click(within(region).getByRole("button", { name: "Trigger" }));
    const dialog = await screen.findByRole("dialog", { name: "New trigger" });
    const host = within(dialog).getByTestId("structure-sql-editor");
    const view = EditorView.findFromDOM(host.querySelector(".cm-editor") as HTMLElement)!;
    expect(view.state.doc.toString()).toMatch(
      /^CREATE TRIGGER "posts_trigger"\nBEFORE INSERT OR UPDATE ON "public"\."posts"/,
    );
    fireEvent.click(within(dialog).getByRole("button", { name: /Add trigger/ }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(ops()).toEqual([
      {
        op: "createTrigger",
        table: posts,
        name: "posts_trigger",
        sql: expect.stringMatching(/EXECUTE FUNCTION trigger_function\(\);$/),
      },
    ]);
    expect(within(region).getByText("posts_trigger").closest("tr")).toHaveAttribute(
      "data-status",
      "added",
    );

    // Edit the existing trigger.
    const row = within(region).getByText("posts_set_updated_at").closest("tr")!;
    fireEvent.doubleClick(row);
    const edit = await screen.findByRole("dialog", { name: "Edit trigger" });
    const editor = EditorView.findFromDOM(
      within(edit).getByTestId("structure-sql-editor").querySelector(".cm-editor") as HTMLElement,
    )!;
    const text = editor.state.doc.toString();
    act(() =>
      editor.dispatch({
        changes: {
          from: 0,
          to: text.length,
          insert: text.replace("BEFORE UPDATE", "AFTER UPDATE"),
        },
      }),
    );
    fireEvent.click(within(edit).getByRole("button", { name: /Stage changes/ }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(ops().slice(1)).toEqual([
      { op: "dropTrigger", table: posts, name: "posts_set_updated_at" },
      {
        op: "createTrigger",
        table: posts,
        name: "posts_set_updated_at",
        sql: expect.stringContaining("AFTER UPDATE ON posts"),
      },
    ]);
    expect(row).toHaveAttribute("data-status", "changed");

    // Delete on the new trigger's row removes it again.
    const added = within(region).getByText("posts_trigger").closest("tr")!;
    added.focus();
    fireEvent.keyDown(added, { key: "Delete" });
    expect(ops().map((o) => o.op)).toEqual(["dropTrigger", "createTrigger"]);

    await act(() => actions.run("grid.preview"));
    expect((await structureSection()).section).toHaveTextContent(
      'DROP TRIGGER "posts_set_updated_at" ON "public"."posts"',
    );
  });
});
