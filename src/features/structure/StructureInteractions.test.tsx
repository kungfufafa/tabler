import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { resetCommitStore } from "@/features/commit/store";
import { CommitTestHost } from "@/features/commit/testHost";
import { actions } from "@/features/palette/actions";
import { ipc, type TableRef } from "@/lib/ipc";
import { resetMockDdl } from "@/lib/ipc/mock/ddl";
import { isMac } from "@/lib/keys/combo";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import { useConnectionsStore } from "@/stores/connections";
import { resetPrefsForTests } from "@/stores/prefs";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { useColumnSelectionStore } from "./columnSelection";
import { OrderedPicker, type PrefixLengths } from "./parts";
import { useStructureStore } from "./store";
import { StructureTab } from "./StructureTab";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));
const guard = vi.hoisted(() => ({ guardExecution: vi.fn(async () => true) }));
vi.mock("@/features/safety/guard", () => guard);

// CodeMirror measures text ranges, which jsdom does not lay out.
if (!Range.prototype.getClientRects) {
  const empty = () => ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] });
  Range.prototype.getClientRects = empty as unknown as Range["getClientRects"];
  Range.prototype.getBoundingClientRect = () => new DOMRect();
}

const USERS: TableRef = { schema: "public", name: "users" };
const MY_USERS: TableRef = { schema: "tabler_dev", name: "users" };

async function setup(
  profileId = "p-local-pg",
  table: TableRef = USERS,
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
        <StructureTab tab={tab} />
        <CommitTestHost />
      </TooltipProvider>
    </QueryClientProvider>
  );
  render(keyboard ? <KeyboardProvider>{tree}</KeyboardProvider> : tree);
  await screen.findByRole("region", { name: "Columns" });
  const ops = () => useStructureStore.getState().byTab[id]?.ops ?? [];
  return { id, ops, tab };
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

const blankRow = (grid: "columns" | "indexes" | "triggers") =>
  document.querySelector<HTMLElement>(`tr[data-blank-row="${grid}"]`);

/** The `#` button of a column row: what a click on a column hits. */
const handle = (name: string) => screen.getByRole("button", { name: `Select column ${name}` });

/** Names of the columns the staged ops drop, sorted (the order follows the grid's). */
const droppedNames = (ops: readonly { op: string; name?: string }[]) =>
  ops
    .filter((o) => o.op === "dropColumn")
    .map((o) => o.name)
    .sort();

/** A click as a user makes it: mouse down (focus), then click. */
function press(el: HTMLElement, init: MouseEventInit = {}) {
  fireEvent.mouseDown(el, init);
  fireEvent.mouseUp(el, init);
  fireEvent.click(el, init);
}

beforeEach(() => {
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useStructureStore.setState({ byTab: {} });
  useColumnSelectionStore.setState({ selection: null });
  resetPrefsForTests();
  resetCommitStore();
});

afterEach(() => {
  resetMockDdl();
  Object.values(toast).forEach((fn) => fn.mockReset());
  vi.restoreAllMocks();
});

describe("double-click a blank row (columns, indexes, triggers)", () => {
  it("adds a column from the blank row, the empty area below the grid, and the filter's empty row", async () => {
    const { ops } = await setup();
    const row = blankRow("columns")!;
    expect(row).toHaveTextContent("double-click to add a column");
    // It ends the columns grid.
    expect(row.parentElement!.lastElementChild).toBe(row);

    fireEvent.doubleClick(row);
    expect(ops().map((o) => o.op)).toEqual(["addColumn"]);

    // The empty area of the scroll container (below the last grid).
    const scroller = screen.getByRole("region", { name: "Columns" }).closest(".overflow-auto")!;
    fireEvent.doubleClick(scroller);
    expect(ops().map((o) => o.op)).toEqual(["addColumn", "addColumn"]);

    // "No column matches" is blank space too: the search is cleared for the new column.
    const search = screen.getByRole("textbox", { name: "Search columns" });
    fireEvent.change(search, { target: { value: "zzz" } });
    const none = screen.getByText(/No column matches/);
    fireEvent.doubleClick(none);
    expect(ops()).toHaveLength(3);
    expect(search).toHaveValue("");
  });

  it("adds an index from the blank row of the Indexes grid and from its empty space", async () => {
    const { ops } = await setup();
    const row = blankRow("indexes")!;
    expect(row).toHaveTextContent("double-click to add an index");
    fireEvent.doubleClick(row);
    expect(ops().map((o) => o.op)).toEqual(["addIndex"]);
    // the section element (its bottom padding, the gap beside the table)
    fireEvent.doubleClick(screen.getByRole("region", { name: "Indexes" }));
    expect(ops().map((o) => o.op)).toEqual(["addIndex", "addIndex"]);
  });

  it("opens the new-trigger editor from the blank row of the Triggers grid", async () => {
    const { ops } = await setup("p-local-pg", { schema: "public", name: "posts" });
    expect(blankRow("triggers")).toBeNull(); // not while the columns are shown
    fireEvent.click(screen.getByRole("button", { name: "Triggers" }));
    const row = blankRow("triggers")!;
    expect(row).toHaveTextContent("double-click to add a trigger");
    fireEvent.doubleClick(row);
    const dialog = await screen.findByRole("dialog", { name: "New trigger" });
    fireEvent.click(within(dialog).getByRole("button", { name: /Add trigger/ }));
    await waitFor(() => expect(ops().map((o) => o.op)).toEqual(["createTrigger"]));
    // The empty area below the grid adds one as well.
    const scroller = screen.getByRole("region", { name: "Triggers" }).closest(".overflow-auto")!;
    fireEvent.doubleClick(scroller);
    expect(await screen.findByRole("dialog", { name: "New trigger" })).toBeInTheDocument();
  });

  it("ignores a double-click on a real row, in a field, on a button or in a section bar", async () => {
    const { ops } = await setup();
    const row = cell("c:bio:name").closest("tr")!;
    fireEvent.doubleClick(row);
    fireEvent.doubleClick(row.querySelector("td")!);
    fireEvent.doubleClick(cell("c:bio:name"));
    fireEvent.doubleClick(screen.getByRole("button", { name: "bio nullable" }));
    fireEvent.doubleClick(screen.getByRole("heading", { name: "Indexes" }));
    fireEvent.doubleClick(cell("i:users_pkey:name").closest("tr")!);
    expect(ops()).toEqual([]);
    // Checks and foreign keys have no blank row to add to.
    fireEvent.doubleClick(screen.getByRole("region", { name: "Checks" }));
    fireEvent.doubleClick(screen.getByRole("region", { name: "Foreign keys" }));
    expect(ops()).toEqual([]);
  });

  it("shows no blank row for a view or a read-only connection", async () => {
    await setup("p-local-pg", { schema: "public", name: "active_users" });
    expect(document.querySelector("[data-blank-row]")).toBeNull();
    const scroller = screen.getByRole("region", { name: "Columns" });
    fireEvent.doubleClick(scroller);
    expect(useStructureStore.getState().byTab).toEqual({});
  });
});

describe("Delete on selected columns", () => {
  it("clicking a column selects and focuses its row: Delete drops it with no focus call", async () => {
    const { ops } = await setup();
    const bio = handle("bio");
    press(bio);
    const row = bio.closest("tr")!;
    expect(row).toHaveAttribute("aria-selected", "true");
    expect(document.activeElement).toBe(row);
    fireEvent.keyDown(document.activeElement!, { key: "Delete" });
    expect(ops()).toEqual([{ op: "dropColumn", table: USERS, name: "bio" }]);
    // Delete again restores it.
    fireEvent.keyDown(document.activeElement!, { key: "Backspace" });
    expect(ops()).toEqual([]);
  });

  it("a click on the row's own padding selects it, a click in a field does not", async () => {
    const { ops } = await setup();
    const row = cell("c:bio:name").closest("tr")!;
    const td = cell("c:bio:name").closest("td")!;
    press(td);
    expect(row).toHaveAttribute("aria-selected", "true");
    expect(document.activeElement).toBe(row);
    fireEvent.keyDown(row, { key: "Delete" });
    expect(ops()).toEqual([{ op: "dropColumn", table: USERS, name: "bio" }]);

    // another row: pressing a text field leaves the selection (and the focus) to the field
    const other = cell("c:role:name");
    press(other);
    expect(other.closest("tr")).toHaveAttribute("aria-selected", "false");
    other.focus();
    fireEvent.keyDown(other, { key: "Delete" });
    expect(ops()).toHaveLength(1);
  });

  it("⌘-clicking three columns and pressing Delete drops all three in one undo step", async () => {
    const { ops } = await setup();
    press(handle("bio"));
    press(handle("role"), { metaKey: true });
    press(handle("nickname"), { metaKey: true });
    for (const name of ["bio", "role", "nickname"])
      expect(handle(name).closest("tr")).toHaveAttribute("aria-selected", "true");
    expect(document.activeElement).toBe(handle("nickname").closest("tr"));

    fireEvent.keyDown(document.activeElement!, { key: "Delete" });
    expect(ops()).toHaveLength(3);
    expect(droppedNames(ops())).toEqual(["bio", "nickname", "role"]);
    for (const name of ["bio", "role", "nickname"])
      expect(handle(name).closest("tr")).toHaveAttribute("data-status", "dropped");

    // one step back brings all three back
    await act(() => actions.run("grid.undo"));
    expect(ops()).toEqual([]);
  });

  it("⌘⌫ (Delete selected rows) takes the whole selection too, and Delete on a dropped one restores them", async () => {
    const { ops } = await setup();
    press(handle("bio"));
    press(handle("role"), { metaKey: true });
    await act(() => actions.run("grid.delete-rows"));
    expect(droppedNames(ops())).toEqual(["bio", "role"]);
    expect(ops()).toHaveLength(2);

    // the rows are dropped and still selected: Delete restores both
    const row = handle("role").closest("tr")!;
    row.focus();
    fireEvent.keyDown(row, { key: "Delete" });
    expect(ops()).toEqual([]);
  });

  it("the context menu of a selected column says 'Delete 3 columns'; on another row it drops that row", async () => {
    const { ops } = await setup();
    press(handle("bio"));
    press(handle("role"), { metaKey: true });
    press(handle("nickname"), { metaKey: true });

    fireEvent.contextMenu(handle("role").closest("tr")!);
    const item = await screen.findByRole("menuitem", { name: "Delete 3 columns" });
    fireEvent.click(item);
    expect(ops()).toHaveLength(3);
    expect(droppedNames(ops())).toEqual(["bio", "nickname", "role"]);

    // Restore says so for the dropped selection, and restores all of them.
    fireEvent.contextMenu(handle("bio").closest("tr")!);
    fireEvent.click(await screen.findByRole("menuitem", { name: "Restore 3 columns" }));
    expect(ops()).toEqual([]);

    // A row outside the selection: one column, as before.
    fireEvent.contextMenu(handle("email").closest("tr")!);
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete Column" }));
    expect(ops()).toEqual([{ op: "dropColumn", table: USERS, name: "email" }]);
  });

  it("index and check rows take the focus when pressed: Delete drops them", async () => {
    const { ops } = await setup();
    // a check: pressing its column_name cell (not a field) focuses the row
    const check = document.querySelector<HTMLElement>("tr[data-row-kind='check']")!;
    expect(check).toHaveAttribute("tabindex", "-1");
    press(check.querySelector("td:nth-child(3)")!);
    expect(document.activeElement).toBe(check);
    fireEvent.keyDown(check, { key: "Delete" });
    expect(ops().map((o) => o.op)).toEqual(["dropCheck"]);

    // a new index
    fireEvent.click(screen.getAllByRole("button", { name: "Index" })[0]!);
    await waitFor(() => expect(ops().map((o) => o.op)).toEqual(["dropCheck", "addIndex"]));
    const index = cell("ni:0:name").closest("tr")!;
    expect(index).toHaveAttribute("tabindex", "-1");
    press(index.querySelector("td:last-child")!);
    expect(document.activeElement).toBe(index);
    fireEvent.keyDown(index, { key: "Delete" });
    expect(ops().map((o) => o.op)).toEqual(["dropCheck"]);

    // a field inside the row keeps the key for its text
    const field = cell("c:email:name");
    field.focus();
    press(field);
    fireEvent.keyDown(field, { key: "Delete" });
    expect(ops().map((o) => o.op)).toEqual(["dropCheck"]);
  });

  it("new columns in a selection go too, whatever their position", async () => {
    const { ops } = await setup();
    await act(() => actions.run("grid.new-row"));
    await act(() => actions.run("grid.new-row"));
    await act(() => actions.run("grid.new-row"));
    expect(ops().map((o) => o.op)).toEqual(["addColumn", "addColumn", "addColumn"]);
    press(handle("column_1"));
    press(handle("column_3"), { metaKey: true });
    const row = handle("column_3").closest("tr")!;
    fireEvent.keyDown(row, { key: "Delete" });
    // column_2 is left (and is the only one now)
    expect(ops().map((o) => (o.op === "addColumn" ? o.column.name : o.op))).toEqual(["column_2"]);
  });
});

describe("prefix lengths of MySQL index columns", () => {
  /** A picker with state, like ColumnsCell holds it. */
  function Harness({
    options,
    lengths,
    initial = [],
    onValue,
  }: {
    options: string[];
    lengths?: PrefixLengths;
    initial?: string[];
    onValue: (v: string[]) => void;
  }) {
    const [value, setValue] = useState(initial);
    return (
      <OrderedPicker
        label="Index columns"
        options={options}
        value={value}
        lengths={lengths}
        onChange={(next) => {
          setValue(next);
          onValue(next);
        }}
      />
    );
  }
  const TYPES: Record<string, string> = {
    name: "varchar(500)",
    body: "longtext",
    qty: "int",
    raw: "blob",
  };
  const LENGTHS: PrefixLengths = {
    eligible: (c) => /char|text|blob/.test(TYPES[c] ?? ""),
    required: (c) => /text|blob/.test(TYPES[c] ?? ""),
  };

  it("typing a length on a varchar chip makes `name(10)`, clearing it makes `name` again", () => {
    const seen = vi.fn();
    render(<Harness options={["name", "qty"]} lengths={LENGTHS} onValue={seen} />);
    expect(screen.queryByLabelText("name prefix length")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "name" }));
    expect(seen).toHaveBeenLastCalledWith(["name"]);
    const input = screen.getByLabelText("name prefix length");
    expect(input).toHaveAttribute("placeholder", "len");
    fireEvent.change(input, { target: { value: "10" } });
    expect(seen).toHaveBeenLastCalledWith(["name(10)"]);
    expect(screen.getByLabelText("name prefix length")).toHaveValue("10");
    // the chip stays pressed and numbered
    expect(screen.getByRole("button", { name: /name/ })).toHaveAttribute("aria-pressed", "true");
    fireEvent.change(screen.getByLabelText("name prefix length"), { target: { value: "" } });
    expect(seen).toHaveBeenLastCalledWith(["name"]);
    // digits only, within 1..3072
    fireEvent.change(screen.getByLabelText("name prefix length"), { target: { value: "9x9" } });
    expect(seen).toHaveBeenLastCalledWith(["name(99)"]);
    fireEvent.change(screen.getByLabelText("name prefix length"), { target: { value: "99999" } });
    expect(seen).toHaveBeenLastCalledWith(["name(3072)"]);
    fireEvent.change(screen.getByLabelText("name prefix length"), { target: { value: "0" } });
    expect(seen).toHaveBeenLastCalledWith(["name"]);
    // removing the chip removes the entry
    fireEvent.click(screen.getByRole("button", { name: /name/ }));
    expect(seen).toHaveBeenLastCalledWith([]);
  });

  it("an int column has no length input; PostgreSQL pickers (no `lengths`) have none at all", () => {
    const { unmount } = render(
      <Harness
        options={["qty", "name"]}
        lengths={LENGTHS}
        initial={["qty", "name"]}
        onValue={vi.fn()}
      />,
    );
    expect(screen.queryByLabelText("qty prefix length")).toBeNull();
    expect(screen.getByLabelText("name prefix length")).toBeInTheDocument();
    unmount();
    render(<Harness options={["qty", "name"]} initial={["qty", "name"]} onValue={vi.fn()} />);
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("entries match their chips by column name and keep the sort direction", () => {
    const seen = vi.fn();
    render(
      <Harness
        options={["name", "body", "qty"]}
        lengths={LENGTHS}
        initial={["body(20)", "name(5) DESC"]}
        onValue={seen}
      />,
    );
    expect(screen.getByLabelText("body prefix length")).toHaveValue("20");
    expect(screen.getByLabelText("name prefix length")).toHaveValue("5");
    fireEvent.change(screen.getByLabelText("name prefix length"), { target: { value: "8" } });
    expect(seen).toHaveBeenLastCalledWith(["body(20)", "name(8) DESC"]);
  });

  it("a TEXT or BLOB column without a length shows a warning that goes away with the length", () => {
    render(<Harness options={["body", "raw", "name"]} lengths={LENGTHS} onValue={vi.fn()} />);
    expect(screen.queryByRole("status")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "body" }));
    expect(screen.getByRole("status")).toHaveTextContent(
      "“body” is a TEXT or BLOB column: MySQL cannot index it in full, type a length.",
    );
    fireEvent.click(screen.getByRole("button", { name: "raw" }));
    expect(screen.getByRole("status")).toHaveTextContent("“body”, “raw” are TEXT or BLOB columns");
    fireEvent.click(screen.getByRole("button", { name: "name" })); // a varchar needs none
    fireEvent.change(screen.getByLabelText("body prefix length"), { target: { value: "30" } });
    fireEvent.change(screen.getByLabelText("raw prefix length"), { target: { value: "30" } });
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("MySQL structure: the index grid offers lengths; a TEXT column without one blocks Apply, with one it applies as name(10)", async () => {
    const { ops } = await setup("p-shop-mysql", MY_USERS);
    // email is TEXT in the mock's users table: + Index starts on it
    fireEvent.click(screen.getAllByRole("button", { name: "Index" })[0]!);
    await waitFor(() => expect(ops().map((o) => o.op)).toEqual(["addIndex"]));
    const columns = cell("ni:0:columns");
    expect(columns).toHaveTextContent("email");
    fireEvent.click(columns);
    expect(await screen.findByRole("status")).toHaveTextContent("TEXT or BLOB");
    expect(screen.getByLabelText("email prefix length")).toHaveValue("");

    // Apply is refused by the planner: the Preview says why and nothing is sent
    // (close the picker first: its draft is staged when it closes)
    fireEvent.keyDown(columns, { key: "Escape" });
    await act(() => actions.run("grid.preview"));
    const sheet = await screen.findByRole("dialog", { name: "Preview changes" });
    expect(await within(sheet).findByText(/MySQL needs a prefix length/)).toBeInTheDocument();
    fireEvent.click(within(sheet).getByRole("button", { name: /^Commit/ }));
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(ops()).toHaveLength(1);
    fireEvent.click(within(sheet).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    // give it a length
    fireEvent.click(cell("ni:0:columns"));
    fireEvent.change(await screen.findByLabelText("email prefix length"), {
      target: { value: "10" },
    });
    expect(screen.queryByRole("status")).toBeNull();
    fireEvent.keyDown(screen.getByLabelText("email prefix length"), { key: "Escape" });
    await waitFor(() =>
      expect(ops()[0]).toMatchObject({ op: "addIndex", index: { columns: ["email(10)"] } }),
    );
    await act(() => actions.run("grid.preview"));
    const preview = await screen.findByRole("dialog", { name: "Preview changes" });
    const section = await within(preview).findByRole("region", { name: /^Structure / });
    await waitFor(() => expect(section).toHaveTextContent("`email`(10)"));
    fireEvent.click(within(preview).getByRole("button", { name: /^Commit/ }));
    await waitFor(() => expect(toast.success).toHaveBeenCalled());
    expect(ops()).toEqual([]);
  });
});

describe("ON UPDATE of a MySQL column", () => {
  it("switching it on stages CURRENT_TIMESTAMP with the column's fractional seconds", async () => {
    const { ops } = await setup("p-shop-mysql", MY_USERS);
    type(cell("c:last_login:dataType"), "datetime(3)");
    fireEvent.keyDown(screen.getByRole("button", { name: "last_login extra" }), { key: "Enter" });
    const menu = await screen.findByRole("menu");
    // the item offers what will be staged
    fireEvent.click(
      within(menu).getByRole("menuitemcheckbox", { name: "on update CURRENT_TIMESTAMP(3)" }),
    );
    expect(ops().at(-1)).toEqual({
      op: "alterColumnOnUpdate",
      table: MY_USERS,
      name: "last_login",
      onUpdate: "CURRENT_TIMESTAMP(3)",
    });
  });

  it("a plain datetime takes the bare form", async () => {
    const { ops } = await setup("p-shop-mysql", MY_USERS);
    type(cell("c:last_login:dataType"), "datetime");
    fireEvent.keyDown(screen.getByRole("button", { name: "last_login extra" }), { key: "Enter" });
    const menu = await screen.findByRole("menu");
    fireEvent.click(
      within(menu).getByRole("menuitemcheckbox", { name: "on update CURRENT_TIMESTAMP" }),
    );
    expect(ops().at(-1)).toMatchObject({
      op: "alterColumnOnUpdate",
      onUpdate: "CURRENT_TIMESTAMP",
    });
  });
});

describe("⌘↩ in a structure tab", () => {
  it("opens the SQL editor (query.open-editor) instead of doing nothing", async () => {
    const open = vi.fn();
    const off = actions.register("query.open-editor", open);
    try {
      await setup("p-local-pg", USERS, { keyboard: true });
      const root = document.querySelector<HTMLElement>("[data-scope='grid']")!;
      root.focus();
      await act(async () => {
        fireEvent.keyDown(root, {
          key: "Enter",
          code: "Enter",
          [isMac ? "metaKey" : "ctrlKey"]: true,
        });
      });
      expect(open).toHaveBeenCalledTimes(1);
      // from a button of a row as well (the structure body is one grid scope)
      const nullable = screen.getByRole("button", { name: "bio nullable" });
      nullable.focus();
      await act(async () => {
        fireEvent.keyDown(nullable, {
          key: "Enter",
          code: "Enter",
          [isMac ? "metaKey" : "ctrlKey"]: true,
        });
      });
      expect(open).toHaveBeenCalledTimes(2);
    } finally {
      off();
    }
  });

  it("registers nothing while the tab is not the active one", async () => {
    const { id } = await setup();
    expect(actions.has("grid.quick-look")).toBe(true);
    act(() => useTabsStore.setState({ activeId: `${id}-elsewhere` }));
    await waitFor(() => expect(actions.has("grid.quick-look")).toBe(false));
  });
});

describe("the last apply failed with nothing applied", () => {
  it("MySQL: the Preview lists one ALTER; when it fails, All changes were reverted", async () => {
    const { ops } = await setup("p-shop-mysql", MY_USERS);
    type(cell("c:bio:dataType"), "nosuchtype");
    fireEvent.keyDown(screen.getByRole("button", { name: "email nullable" }), { key: "y" });
    expect(ops().map((o) => o.op)).toEqual(["alterColumnType", "alterColumnNullable"]);

    await act(() => actions.run("grid.preview"));
    const sheet = await screen.findByRole("dialog", { name: "Preview changes" });
    const section = await within(sheet).findByRole("region", { name: /^Structure / });
    await waitFor(() => expect(section).toHaveTextContent(/ALTER TABLE/));
    // both edits are clauses of ONE statement
    expect(section.textContent!.match(/ALTER TABLE/g)).toHaveLength(1);
    expect(section).toHaveTextContent("MODIFY COLUMN `bio`");
    expect(section).toHaveTextContent(", MODIFY COLUMN `email`");

    fireEvent.click(within(sheet).getByRole("button", { name: /^Commit/ }));
    await screen.findByText(/You have an error in your SQL syntax/);
    fireEvent.click(within(sheet).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    // nothing ran (one statement is all or nothing): no "stopped partway", the table is as it
    // was and the staged changes are still here
    expect(screen.queryByText(/stopped partway/)).toBeNull();
    const note = screen.getByText(/All changes were reverted/).closest("[role='alert']")!;
    expect(note).toHaveTextContent("the staged changes are still here");
    expect(ops()).toHaveLength(2);
    expect(cell("c:email:name").value).toBe("email");

    // the next edit clears the note
    type(cell("c:nickname:name"), "nick");
    expect(screen.queryByText(/All changes were reverted/)).toBeNull();
  });

  it("MySQL: a plan that stopped partway still says so, not that it was reverted", async () => {
    const { ops } = await setup("p-shop-mysql", MY_USERS);
    type(cell("c:nickname:name"), "nick");
    type(cell("c:bio:dataType"), "nosuchtype");
    await act(() => actions.run("grid.preview"));
    const sheet = await screen.findByRole("dialog", { name: "Preview changes" });
    await within(sheet).findByRole("region", { name: /^Structure / });
    await waitFor(() => expect(sheet.textContent).toMatch(/RENAME COLUMN/));
    fireEvent.click(within(sheet).getByRole("button", { name: /^Commit/ }));
    await screen.findByText(/Changes before it stay committed/);
    fireEvent.click(within(sheet).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByText(/The last apply stopped partway/)).toBeInTheDocument();
    expect(screen.queryByText(/All changes were reverted/)).toBeNull();
    expect(ops()).toHaveLength(2);
  });

  it("PostgreSQL: a failed apply is reverted as well, and the note stays after the sheet closes", async () => {
    const { ops } = await setup();
    type(cell("c:bio:dataType"), "nosuchtype");
    await act(() => actions.run("grid.preview"));
    const sheet = await screen.findByRole("dialog", { name: "Preview changes" });
    await within(sheet).findByRole("region", { name: /^Structure / });
    await waitFor(() => expect(sheet.textContent).toMatch(/ALTER TABLE/));
    fireEvent.click(within(sheet).getByRole("button", { name: /^Commit/ }));
    await screen.findByText('type "nosuchtype" does not exist');
    fireEvent.click(within(sheet).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByText(/All changes were reverted/)).toBeInTheDocument();
    expect(ops()).toHaveLength(1);
  });
});
