import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import type { Value } from "@/lib/ipc";
import { formatCombo, isMac } from "@/lib/keys/combo";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import { useInspectorStore } from "@/stores/inspector";
import { useUiStore } from "@/stores/ui";
import { DataGrid, type DataGridProps } from "./DataGrid";
import {
  buildGridColumns,
  HEADER_H,
  MIN_GUTTER_W,
  ROW_H,
  SLIM_GUTTER_W,
  type ColumnState,
  type GridColumn,
  type ViewRow,
} from "./types";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

const NULL_TEXT: Value = { t: "null" };

const col = (id: string, kind: GridColumn["kind"] = "text"): GridColumn => ({
  id,
  name: id,
  kind,
  dataType: kind,
  nullable: true,
  isPk: id === "id",
  hasDefault: false,
  isAutoInc: false,
  editable: true,
  align: kind === "int" ? "right" : "left",
});
const columns = [col("id", "int"), col("name"), col("email")];
const rows: ViewRow[] = [
  {
    key: "pk:1",
    cells: [
      { t: "int", v: 1 },
      { t: "text", v: "Ann" },
      { t: "text", v: "ann@x.io" },
    ],
  },
  {
    key: "pk:2",
    cells: [
      { t: "int", v: 2 },
      { t: "text", v: "Bob" },
      { t: "text", v: "bob@x.io" },
    ],
  },
];

// jsdom has no layout: give the scroller a size so the virtualizers render rows and columns.
const sizes: Array<[string, number]> = [
  ["offsetWidth", 1200],
  ["offsetHeight", 600],
  ["clientWidth", 1200],
  ["clientHeight", 600],
];
const saved = new Map<string, PropertyDescriptor | undefined>();
beforeAll(() => {
  for (const [prop, value] of sizes) {
    saved.set(prop, Object.getOwnPropertyDescriptor(HTMLElement.prototype, prop));
    Object.defineProperty(HTMLElement.prototype, prop, { configurable: true, get: () => value });
  }
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
});
afterAll(() => {
  for (const [prop, desc] of saved) {
    if (desc) Object.defineProperty(HTMLElement.prototype, prop, desc);
    else delete (HTMLElement.prototype as unknown as Record<string, unknown>)[prop];
  }
});
afterEach(() => {
  Object.values(toast).forEach((fn) => fn.mockReset());
  useUiStore.setState({ inspectorOpen: false });
  useInspectorStore.setState({ mode: "cell" });
});

const mod = { [isMac ? "metaKey" : "ctrlKey"]: true };
const openMenu = (el: Element) => {
  fireEvent.pointerDown(el, { button: 2 });
  fireEvent.contextMenu(el);
};
const item = (name: RegExp | string) => screen.getByRole("menuitem", { name });

function setup(columnState: ColumnState, extra: Partial<DataGridProps> = {}) {
  const props = {
    onEditCommit: vi.fn(),
    onAction: vi.fn(),
    onFocusCell: vi.fn(),
  };
  render(
    <DataGrid
      columns={columns}
      rowCount={rows.length}
      getRow={(i) => rows[i]}
      columnState={columnState}
      editable
      active
      {...props}
      {...extra}
    />,
  );
  const grid = document.querySelector<HTMLElement>('[data-scope="grid"]')!;
  const cell = (r: number, c: number) =>
    document.querySelector<HTMLElement>(`[data-cell][data-r="${r}"][data-c="${c}"]`)!;
  const click = (r: number, c: number) => fireEvent.pointerDown(cell(r, c), { button: 0 });
  return { ...props, grid, cell, click };
}

describe("DataGrid with hidden / reordered columns", () => {
  // Visual order: email, name (id hidden). Source indexes: email = 2, name = 1.
  const state: ColumnState = { widths: {}, order: ["email", "name", "id"], hidden: ["id"] };

  it("renders each visible column with its own values", () => {
    const { cell } = setup(state);
    const headers = [...document.querySelectorAll("[data-header]")]
      .map((h) => h.getAttribute("data-header"))
      .filter((id) => id !== "__corner");
    expect(headers).toEqual(["email", "name"]);
    expect(cell(0, 0)).toHaveTextContent("ann@x.io");
    expect(cell(0, 1)).toHaveTextContent("Ann");
    expect(cell(1, 1)).toHaveTextContent("Bob");
  });

  it("reports the focused cell's source column", () => {
    const { click, onFocusCell } = setup(state);
    click(1, 0);
    const last = onFocusCell.mock.calls.at(-1)![0];
    expect(last.pos).toEqual({ r: 1, c: 0 });
    expect(last.sourceIndex).toBe(2);
    expect(last.column.id).toBe("email");
    expect(last.value).toEqual({ t: "text", v: "bob@x.io" });
  });

  it("edits and Set NULL target source columns", async () => {
    const { click, grid, onEditCommit, onAction } = setup(state);
    click(0, 1);
    fireEvent.keyDown(grid, { key: "Enter" });
    const input = screen.getByRole("textbox", { name: "Edit name" });
    expect((input as HTMLInputElement).value).toBe("Ann");
    fireEvent.change(input, { target: { value: "Z" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onEditCommit).toHaveBeenCalledWith(0, 1, { t: "text", v: "Z" });

    // Rows 0–1 selected, the focus on `email` (visual 0, source 2): Set NULL sets that column of
    // every selected row.
    click(0, 1);
    fireEvent.pointerDown(document.querySelector('[data-cell][data-r="1"][data-c="0"]')!, {
      button: 0,
      shiftKey: true,
    });
    await act(() => actions.run("grid.set-null"));
    expect(onAction).toHaveBeenLastCalledWith({
      type: "setNull",
      cells: [
        { r: 0, c: 2 },
        { r: 1, c: 2 },
      ],
    });
  });

  it("copies the visible columns in display order", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>().mockResolvedValue();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText, readText: vi.fn() },
    });
    const { click, grid } = setup(state, { active: false });
    click(0, 0);
    fireEvent.pointerDown(document.querySelector('[data-cell][data-r="1"][data-c="1"]')!, {
      button: 0,
      shiftKey: true,
    });
    await act(async () => {
      fireEvent.keyDown(grid, { key: "c", code: "KeyC", [isMac ? "metaKey" : "ctrlKey"]: true });
    });
    expect(writeText).toHaveBeenCalledWith("ann@x.io\tAnn\nbob@x.io\tBob");
  });

  it("pastes into source columns as one batch, skipping values that do not fit", () => {
    const { click, grid, onAction } = setup({ ...state, hidden: [] });
    // Visual order now: email, name, id.
    click(0, 1);
    const data = { getData: () => "Cy\tnot-a-number\nDee\t7\nEve\t8\n" };
    fireEvent.paste(grid, { clipboardData: data });
    expect(onAction).toHaveBeenCalledWith({
      type: "setValues",
      edits: [
        { r: 0, c: 1, value: { t: "text", v: "Cy" } },
        { r: 1, c: 1, value: { t: "text", v: "Dee" } },
        { r: 1, c: 0, value: { t: "int", v: 7 } },
      ],
    });
    expect(toast.success).toHaveBeenCalledWith("Pasted 3 cells · 1 skipped · 1 row didn't fit");
  });

  it("tells read-only tables that paste is not possible", () => {
    const { click, grid, onAction } = setup(state, {
      editable: false,
      readOnlyNotice: "This table is read-only",
    });
    click(0, 0);
    fireEvent.paste(grid, { clipboardData: { getData: () => "x" } });
    expect(onAction).not.toHaveBeenCalled();
    expect(toast.message).toHaveBeenCalledWith("This table is read-only");
  });

  it("explains that blobs are not edited inline", () => {
    const blobColumns = [...columns, col("avatar", "bytes")];
    const blobRows: ViewRow[] = rows.map((r) => ({
      ...r,
      cells: [...r.cells, { t: "bytes", v: { base64: "AA==", len: 1, truncated: false } } as Value],
    }));
    render(
      <DataGrid
        columns={blobColumns}
        rowCount={blobRows.length}
        getRow={(i) => blobRows[i]}
        editable
        onAction={vi.fn()}
      />,
    );
    const grid = document.querySelector<HTMLElement>('[data-scope="grid"]')!;
    fireEvent.pointerDown(document.querySelector('[data-cell][data-r="0"][data-c="3"]')!, {
      button: 0,
    });
    fireEvent.keyDown(grid, { key: "Enter" });
    expect(toast.message).toHaveBeenCalledWith("Binary values can't be edited inline");
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });
});

describe("DataGrid paste of a copied NULL / empty cell", () => {
  const typed = [col("id", "int"), col("score", "int"), col("name")];
  const typedRows: ViewRow[] = [
    { key: "pk:1", cells: [{ t: "int", v: 1 }, { t: "null" }, { t: "text", v: "" }] },
    {
      key: "pk:2",
      cells: [
        { t: "int", v: 2 },
        { t: "int", v: 5 },
        { t: "text", v: "Bob" },
      ],
    },
  ];

  function mount() {
    const writeText = vi.fn<(text: string) => Promise<void>>().mockResolvedValue();
    let clipboard = "";
    writeText.mockImplementation((text) => {
      clipboard = text;
      return Promise.resolve();
    });
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText, readText: vi.fn(() => Promise.resolve(clipboard)) },
    });
    const onAction = vi.fn();
    render(
      <DataGrid
        columns={typed}
        rowCount={typedRows.length}
        getRow={(i) => typedRows[i]}
        editable
        active={false}
        onAction={onAction}
      />,
    );
    const grid = document.querySelector<HTMLElement>('[data-scope="grid"]')!;
    const click = (r: number, c: number) =>
      fireEvent.pointerDown(document.querySelector(`[data-cell][data-r="${r}"][data-c="${c}"]`)!, {
        button: 0,
      });
    const mod = { [isMac ? "metaKey" : "ctrlKey"]: true };
    const copy = () =>
      act(async () => {
        fireEvent.keyDown(grid, { key: "c", code: "KeyC", ...mod });
      });
    const paste = () =>
      act(async () => {
        fireEvent.keyDown(grid, { key: "v", code: "KeyV", ...mod });
      });
    const copyCell = (r: number, c: number) => {
      openMenu(document.querySelector(`[data-cell][data-r="${r}"][data-c="${c}"]`)!);
      return act(async () => {
        fireEvent.click(item("Copy Cell Value"));
      });
    };
    return { grid, click, copy, copyCell, paste, onAction, writeText };
  }

  it("round-trips Copy Cell Value / ⌘V of a NULL into NULL and of an empty string into ''", async () => {
    const { click, copyCell, paste, onAction, writeText } = mount();
    click(0, 1);
    await copyCell(0, 1);
    expect(writeText).toHaveBeenLastCalledWith("");
    click(1, 1);
    await paste();
    expect(onAction).toHaveBeenLastCalledWith({
      type: "setValues",
      edits: [{ r: 1, c: 1, value: { t: "null" } }],
    });

    click(0, 2);
    await copyCell(0, 2);
    // The cell shows the EMPTY placeholder, but copies the empty string itself.
    expect(writeText).toHaveBeenLastCalledWith("");
    click(1, 2);
    await paste();
    expect(onAction).toHaveBeenLastCalledWith({
      type: "setValues",
      edits: [{ r: 1, c: 2, value: { t: "text", v: "" } }],
    });
    expect(toast.success).toHaveBeenLastCalledWith("Pasted 1 cell");
  });

  it("treats an empty Edit → Paste text item as one empty field, but ignores a textless clipboard", () => {
    const { grid, click, onAction } = mount();
    click(1, 1);
    fireEvent.paste(grid, { clipboardData: { types: ["text/plain"], getData: () => "" } });
    expect(onAction).toHaveBeenLastCalledWith({
      type: "setValues",
      edits: [{ r: 1, c: 1, value: { t: "null" } }],
    });
    onAction.mockClear();
    fireEvent.paste(grid, { clipboardData: { types: ["image/png"], getData: () => "" } });
    fireEvent.paste(grid, { clipboardData: { types: [], getData: () => "" } });
    expect(onAction).not.toHaveBeenCalled();
  });
});

describe("DataGrid double-click", () => {
  let hit: Element | null = null;
  beforeAll(() => {
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: () => hit,
    });
  });
  afterAll(() => {
    delete (document as unknown as Record<string, unknown>).elementFromPoint;
  });

  // Pointer capture retargets dblclick to the scroller; the grid hit-tests the point instead.
  const doubleClick = (grid: HTMLElement, target: Element, at: Element | null = target) => {
    hit = target;
    fireEvent.pointerDown(target, { button: 0 });
    fireEvent.pointerUp(target, { button: 0 });
    fireEvent.pointerDown(target, { button: 0 });
    fireEvent.pointerUp(target, { button: 0 });
    hit = at;
    fireEvent.doubleClick(grid);
  };
  const state: ColumnState = { widths: {}, order: [], hidden: [] };

  it("edits the double-clicked cell", () => {
    const { grid, cell } = setup(state);
    doubleClick(grid, cell(1, 1));
    expect(screen.getByRole("textbox", { name: "Edit name" })).toHaveValue("Bob");
  });

  it("does not edit when a row number is double-clicked", () => {
    const { grid } = setup(state);
    doubleClick(grid, document.querySelector('[data-gutter="1"]')!);
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("does not edit the focused cell when blank space is double-clicked", () => {
    const { grid, click } = setup(state);
    click(0, 1);
    doubleClick(grid, grid);
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("falls back to the pressed cell when the point no longer resolves to one", () => {
    const { grid, cell } = setup(state);
    // e.g. the row re-rendered under the pointer between the presses and the dblclick
    doubleClick(grid, cell(0, 2), null);
    expect(screen.getByRole("textbox", { name: "Edit email" })).toHaveValue("ann@x.io");
  });
});

describe("DataGrid context menus", () => {
  /** Labels line up when every item either starts with an icon or is inset by the icon's width. */
  const misaligned = () =>
    [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')]
      .filter((item) => !item.closest('[role="menu"] [role="menu"]'))
      .filter((item) => item.firstElementChild?.tagName.toLowerCase() !== "svg")
      .filter((item) => !item.hasAttribute("data-inset"))
      .map((item) => item.textContent);

  it("aligns the header menu labels", () => {
    setup(
      { widths: {}, order: ["name", "id", "email"], hidden: [] },
      {
        sort: [{ column: "name", dir: "asc", nulls: null }],
        onSortChange: vi.fn(),
      },
    );
    const header = document.querySelector<HTMLElement>('[data-header="name"]')!;
    fireEvent.pointerDown(header, { button: 2 });
    fireEvent.contextMenu(header);
    expect(screen.getByRole("menuitem", { name: /Select Column/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /Remove Sort/ })).toBeInTheDocument();
    expect(
      screen.getByRole("menuitem", { name: /Reset Column Positions and Widths/ }),
    ).toBeInTheDocument();
    expect(misaligned()).toEqual([]);
  });

  it("aligns the cell menu labels", () => {
    const { cell, click } = setup({ widths: {}, order: [], hidden: [] });
    click(0, 1);
    fireEvent.pointerDown(cell(0, 1), { button: 2 });
    fireEvent.contextMenu(cell(0, 1));
    expect(screen.getByRole("menuitem", { name: /Copy Rows As/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /Set Value/ })).toBeInTheDocument();
    expect(misaligned()).toEqual([]);
  });

  it("aligns the cell menu labels with Quick Filter", () => {
    const { cell, click } = setup(
      { widths: {}, order: [], hidden: [] },
      { onQuickFilter: vi.fn() },
    );
    click(0, 1);
    fireEvent.pointerDown(cell(0, 1), { button: 2 });
    fireEvent.contextMenu(cell(0, 1));
    expect(screen.getByRole("menuitem", { name: /Quick Filter/ })).toBeInTheDocument();
    expect(misaligned()).toEqual([]);
  });
});

describe("DataGrid Set Value", () => {
  const state: ColumnState = { widths: {}, order: [], hidden: [] };

  it("groups NULL / EMPTY / DEFAULT under Set Value, enabled by what the selection accepts", () => {
    const withDefault = [
      ...columns.slice(0, 2),
      { ...col("email"), nullable: false, hasDefault: true },
    ];
    const { cell, click, onAction } = setup(state, { columns: withDefault });
    click(0, 0);
    openMenu(cell(0, 0));
    fireEvent.click(item(/Set Value/));
    // `id` is an int column: no empty string, no default.
    expect(item(/NULL/)).not.toHaveAttribute("data-disabled");
    expect(item(/EMPTY/)).toHaveAttribute("data-disabled");
    expect(item(/DEFAULT/)).toHaveAttribute("data-disabled");
    fireEvent.click(item(/NULL/));
    expect(onAction).toHaveBeenLastCalledWith({ type: "setNull", cells: [{ r: 0, c: 0 }] });

    click(1, 2);
    openMenu(cell(1, 2));
    fireEvent.click(item(/Set Value/));
    expect(item(/NULL/)).toHaveAttribute("data-disabled");
    expect(item(/EMPTY/)).not.toHaveAttribute("data-disabled");
    fireEvent.click(item(/DEFAULT/));
    expect(onAction).toHaveBeenLastCalledWith({ type: "setDefault", cells: [{ r: 1, c: 2 }] });
  });

  it("'grid.set-empty' stages EMPTY on the selected source cells", async () => {
    const { click, onAction } = setup({ widths: {}, order: ["email"], hidden: [] });
    click(1, 0);
    await act(() => actions.run("grid.set-empty"));
    expect(onAction).toHaveBeenLastCalledWith({ type: "setEmpty", cells: [{ r: 1, c: 2 }] });
  });

  it("⌥-click opens the Set Value menu on the clicked cell", () => {
    const { cell, onAction } = setup(state);
    fireEvent.pointerDown(cell(1, 1), { button: 0, altKey: true });
    fireEvent.pointerUp(cell(1, 1), { button: 0 });
    expect(screen.getByText("Set Value")).toBeInTheDocument();
    fireEvent.click(item(/EMPTY/));
    expect(onAction).toHaveBeenLastCalledWith({ type: "setEmpty", cells: [{ r: 1, c: 1 }] });
  });

  it("⌥-click inside the selected rows keeps them and sets the clicked column", () => {
    const { cell, click, onAction } = setup(state);
    click(0, 1);
    fireEvent.pointerDown(cell(1, 2), { button: 0, shiftKey: true });
    fireEvent.pointerDown(cell(1, 1), { button: 0, altKey: true });
    fireEvent.pointerUp(cell(1, 1), { button: 0 });
    fireEvent.click(item(/NULL/));
    expect(onAction).toHaveBeenLastCalledWith({
      type: "setNull",
      cells: [
        { r: 0, c: 1 },
        { r: 1, c: 1 },
      ],
    });
  });

  it("⌥-click does not open the menu in a read-only grid", () => {
    const { cell } = setup(state, { editable: false });
    fireEvent.pointerDown(cell(1, 1), { button: 0, altKey: true });
    fireEvent.pointerUp(cell(1, 1), { button: 0 });
    expect(screen.queryByText("Set Value")).not.toBeInTheDocument();
  });
});

describe("DataGrid quick filter", () => {
  const state: ColumnState = { widths: {}, order: [], hidden: [] };

  it("filters by the focused value", () => {
    const onQuickFilter = vi.fn();
    const { cell, click } = setup(state, { onQuickFilter });
    click(0, 1);
    openMenu(cell(0, 1));
    fireEvent.click(item(/Quick Filter/));
    expect(screen.queryByRole("menuitem", { name: /IN/ })).not.toBeInTheDocument();
    fireEvent.click(item("name ≠ Ann"));
    expect(onQuickFilter).toHaveBeenCalledWith({
      column: columns[1],
      op: "ne",
      values: [{ t: "text", v: "Ann" }],
    });
  });

  it("offers IN / NOT IN over the distinct selected values of the focused column", () => {
    const onQuickFilter = vi.fn();
    const { cell, click } = setup(state, { onQuickFilter });
    click(0, 1);
    fireEvent.pointerDown(cell(1, 2), { button: 0, shiftKey: true });
    openMenu(cell(1, 2));
    fireEvent.click(item(/Quick Filter/));
    // The focus is on email (where the shift-click landed).
    fireEvent.click(item("email NOT IN (2 selected values)"));
    expect(onQuickFilter).toHaveBeenCalledWith({
      column: columns[2],
      op: "notIn",
      values: [
        { t: "text", v: "ann@x.io" },
        { t: "text", v: "bob@x.io" },
      ],
    });
  });

  it("offers IS NULL / IS NOT NULL on a NULL cell", () => {
    const onQuickFilter = vi.fn();
    const nullRows: ViewRow[] = [
      { key: "pk:1", cells: [{ t: "int", v: 1 }, { t: "null" }, NULL_TEXT] },
    ];
    const { cell, click } = setup(state, {
      onQuickFilter,
      rowCount: 1,
      getRow: (i) => nullRows[i],
    });
    click(0, 1);
    openMenu(cell(0, 1));
    fireEvent.click(item(/Quick Filter/));
    expect(screen.queryByRole("menuitem", { name: /=/ })).not.toBeInTheDocument();
    fireEvent.click(item("name IS NULL"));
    expect(onQuickFilter).toHaveBeenCalledWith({ column: columns[1], op: "isNull", values: [] });
  });

  it("header menu: Filter with Column hands the column over once the menu has closed", async () => {
    const onFilterColumn = vi.fn();
    setup(state, { onFilterColumn });
    openMenu(document.querySelector('[data-header="email"]')!);
    fireEvent.click(item(/Filter with Column/));
    await waitFor(() => expect(onFilterColumn).toHaveBeenCalledWith(columns[2]));
  });
});

describe("DataGrid row detail (Space)", () => {
  const state: ColumnState = { widths: {}, order: [], hidden: [] };

  it("'grid.toggle-detail' opens the detail panel on the Row view, then closes it", async () => {
    setup(state);
    await act(() => actions.run("grid.toggle-detail"));
    expect(useUiStore.getState().inspectorOpen).toBe(true);
    expect(useInspectorStore.getState().mode).toBe("row");
    act(() => useInspectorStore.getState().setMode("cell"));
    await act(() => actions.run("grid.toggle-detail"));
    expect(useUiStore.getState().inspectorOpen).toBe(false);
    expect(useInspectorStore.getState().mode).toBe("cell");
  });

  it("Space toggles it in a grid that does not own the actions, and never starts an edit", () => {
    const { grid, click } = setup(state, { active: false });
    click(0, 1);
    fireEvent.keyDown(grid, { key: " ", code: "Space" });
    expect(useUiStore.getState().inspectorOpen).toBe(true);
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });
});

describe("DataGrid keys", () => {
  it("opens Copy as… with its keymap shortcut", () => {
    const { grid, click } = setup({ widths: {}, order: [], hidden: [] }, { active: false });
    click(0, 1);
    fireEvent.keyDown(grid, { key: "c", code: "KeyC", ...mod, shiftKey: true });
    expect(screen.queryByText("Copy 1 row as")).not.toBeInTheDocument();
    fireEvent.keyDown(grid, { key: "ç", code: "KeyC", ...mod, altKey: true });
    expect(screen.getByText("Copy 1 row as")).toBeInTheDocument();
  });
});

describe("DataGrid row selection (TablePlus)", () => {
  const state: ColumnState = { widths: {}, order: [], hidden: [] };
  const people = ["Ann", "Bob", "Cy", "Dee", "Eve"];
  const many: ViewRow[] = people.map((name, i) => ({
    key: `pk:${i + 1}`,
    cells: [
      { t: "int", v: i + 1 },
      { t: "text", v: name },
      { t: "text", v: `${name.toLowerCase()}@x.io` },
    ],
  }));
  const mountMany = (extra: Partial<DataGridProps> = {}) => {
    const onSelectionChange = vi.fn();
    const view = setup(state, {
      rowCount: many.length,
      getRow: (i) => many[i],
      onSelectionChange,
      ...extra,
    });
    return { ...view, onSelectionChange };
  };
  const selected = () =>
    [...document.querySelectorAll<HTMLElement>("[data-row][data-selected]")].map((el) =>
      Number(el.dataset.row),
    );
  const focusOf = (onFocusCell: ReturnType<typeof vi.fn>) => onFocusCell.mock.calls.at(-1)![0].pos;

  let hit: Element | null = null;
  beforeAll(() => {
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: () => hit,
    });
  });
  afterAll(() => {
    delete (document as unknown as Record<string, unknown>).elementFromPoint;
  });

  it("↑↓ move the row selection, ⇧↓ extends it, ←→ and ⇥ move the focused cell in the row", () => {
    const { grid, click, onFocusCell, onSelectionChange } = mountMany();
    click(1, 1);
    expect(selected()).toEqual([1]);
    expect(onSelectionChange).toHaveBeenLastCalledWith(1);
    fireEvent.keyDown(grid, { key: "ArrowDown" });
    expect(selected()).toEqual([2]);
    expect(focusOf(onFocusCell)).toEqual({ r: 2, c: 1 });
    fireEvent.keyDown(grid, { key: "ArrowDown", shiftKey: true });
    fireEvent.keyDown(grid, { key: "ArrowDown", shiftKey: true });
    expect(selected()).toEqual([2, 3, 4]);
    expect(onSelectionChange).toHaveBeenLastCalledWith(3);
    // → / ⇥ / ← move the focused cell; the selected rows stay.
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    expect(focusOf(onFocusCell)).toEqual({ r: 4, c: 2 });
    fireEvent.keyDown(grid, { key: "Tab", shiftKey: true });
    fireEvent.keyDown(grid, { key: "ArrowLeft" });
    expect(focusOf(onFocusCell)).toEqual({ r: 4, c: 0 });
    expect(selected()).toEqual([2, 3, 4]);
    // ⇧→ does not select cells.
    fireEvent.keyDown(grid, { key: "ArrowRight", shiftKey: true });
    expect(selected()).toEqual([2, 3, 4]);
    fireEvent.keyDown(grid, { key: "ArrowUp" });
    expect(selected()).toEqual([3]);
  });

  it("⇧-click extends, ⌘-click toggles rows, a row number selects its row", () => {
    const { cell, click, onSelectionChange } = mountMany();
    click(0, 2);
    fireEvent.pointerDown(cell(2, 0), { button: 0, shiftKey: true });
    expect(selected()).toEqual([0, 1, 2]);
    fireEvent.pointerDown(cell(1, 1), { button: 0, ...mod });
    expect(selected()).toEqual([0, 2]);
    fireEvent.pointerDown(cell(4, 1), { button: 0, ...mod });
    expect(selected()).toEqual([0, 2, 4]);
    expect(onSelectionChange).toHaveBeenLastCalledWith(3);
    fireEvent.pointerDown(document.querySelector('[data-gutter="3"]')!, { button: 0 });
    expect(selected()).toEqual([3]);
    fireEvent.pointerDown(document.querySelector('[data-gutter="1"]')!, {
      button: 0,
      shiftKey: true,
    });
    expect(selected()).toEqual([1, 2, 3]);
  });

  it("dragging across rows selects a range of rows", () => {
    const { grid, cell } = mountMany();
    fireEvent.pointerDown(cell(1, 1), { button: 0 });
    hit = cell(3, 2);
    fireEvent.pointerMove(grid, { clientX: 10, clientY: 10 });
    expect(selected()).toEqual([1, 2, 3]);
    hit = document.querySelector('[data-gutter="0"]');
    fireEvent.pointerMove(grid, { clientX: 10, clientY: 10 });
    expect(selected()).toEqual([0, 1]);
    fireEvent.pointerUp(grid, { button: 0 });
    hit = cell(4, 0);
    fireEvent.pointerMove(grid, { clientX: 10, clientY: 10 });
    expect(selected()).toEqual([0, 1]);
  });

  it("Delete and ⌫ stage the selected rows for deletion and never edit", async () => {
    const { grid, cell, click, onAction, onEditCommit } = mountMany();
    click(1, 1);
    fireEvent.pointerDown(cell(3, 1), { button: 0, shiftKey: true });
    fireEvent.keyDown(grid, { key: "Backspace" });
    expect(onAction).toHaveBeenLastCalledWith({ type: "deleteRows", rows: [1, 2, 3] });
    click(0, 1);
    fireEvent.keyDown(grid, { key: "Delete" });
    expect(onAction).toHaveBeenLastCalledWith({ type: "deleteRows", rows: [0] });
    await act(() => actions.run("grid.delete-rows"));
    expect(onAction).toHaveBeenCalledTimes(3);
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(onEditCommit).not.toHaveBeenCalled();
    // ⇧⌘⌫ is Discard all changes, never a delete.
    fireEvent.keyDown(grid, { key: "Backspace", ...mod, shiftKey: true });
    expect(onAction).toHaveBeenCalledTimes(3);
  });

  it("⌘⌫ deletes rows in a grid that does not own the actions", () => {
    const { grid, click, onAction } = mountMany({ active: false });
    click(2, 0);
    fireEvent.keyDown(grid, { key: "Backspace", ...mod });
    expect(onAction).toHaveBeenLastCalledWith({ type: "deleteRows", rows: [2] });
  });

  it("tells a read-only table that rows cannot be deleted", () => {
    const { grid, click, onAction } = mountMany({
      editable: false,
      readOnlyNotice: "This table is read-only",
    });
    click(2, 0);
    fireEvent.keyDown(grid, { key: "Delete" });
    expect(onAction).not.toHaveBeenCalled();
    expect(toast.message).toHaveBeenCalledWith("This table is read-only");
  });

  it("typing does nothing; ↩ and F2 edit the focused cell", () => {
    const { grid, click, onAction, onEditCommit } = mountMany();
    click(1, 1);
    for (const key of ["x", "Z", "1", "=", "é"]) fireEvent.keyDown(grid, { key });
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(onAction).not.toHaveBeenCalled();
    expect(onEditCommit).not.toHaveBeenCalled();
    fireEvent.keyDown(grid, { key: "F2" });
    expect(screen.getByRole("textbox", { name: "Edit name" })).toHaveValue("Bob");
  });

  it("⌘C copies the selected rows (visible columns, display order, no header), even read-only", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>().mockResolvedValue();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText, readText: vi.fn() },
    });
    const { grid, cell, click } = mountMany({
      editable: false,
      active: false,
      columnState: { widths: {}, order: ["name"], hidden: ["email"] },
    });
    click(3, 0);
    fireEvent.pointerDown(cell(0, 1), { button: 0, ...mod });
    await act(async () => {
      fireEvent.keyDown(grid, { key: "c", code: "KeyC", ...mod });
    });
    expect(writeText).toHaveBeenLastCalledWith("Ann\t1\nDee\t4");
    // ⌥⌘C offers the formats over the same rows.
    fireEvent.keyDown(grid, { key: "ç", code: "KeyC", ...mod, altKey: true });
    expect(screen.getByText("Copy 2 rows as")).toBeInTheDocument();
  });

  it("⌘V of whole rows inserts them as new rows, then selects them", () => {
    const onAction = vi.fn();
    const onSelectionChange = vi.fn();
    const props = {
      columns,
      getRow: (i: number) => rows[i],
      editable: true,
      onAction,
      onSelectionChange,
    };
    const view = render(<DataGrid {...props} rowCount={rows.length} />);
    const grid = document.querySelector<HTMLElement>('[data-scope="grid"]')!;
    fireEvent.paste(grid, {
      clipboardData: { getData: () => "3\tCy\tcy@x.io\n4\tDee\t\n" },
    });
    expect(onAction).toHaveBeenCalledWith({
      type: "insertRows",
      rows: [
        [
          { c: 0, value: { t: "int", v: 3 } },
          { c: 1, value: { t: "text", v: "Cy" } },
          { c: 2, value: { t: "text", v: "cy@x.io" } },
        ],
        [
          { c: 0, value: { t: "int", v: 4 } },
          { c: 1, value: { t: "text", v: "Dee" } },
          { c: 2, value: { t: "text", v: "" } },
        ],
      ],
    });
    expect(toast.success).toHaveBeenCalledWith("Pasted 2 rows");

    const added: ViewRow[] = [3, 4].map((id) => ({
      key: `new:${id}`,
      cells: [{ t: "int", v: id }, NULL_TEXT, NULL_TEXT],
      status: "inserted",
    }));
    const more = [...rows, ...added];
    view.rerender(<DataGrid {...props} rowCount={more.length} getRow={(i) => more[i]} />);
    expect(selected()).toEqual([2, 3]);
    expect(onSelectionChange).toHaveBeenLastCalledWith(2);
  });

  it("⌘V of anything else pastes into the focused cell", () => {
    const { grid, click, onAction } = mountMany();
    click(2, 1);
    fireEvent.keyDown(grid, { key: "ArrowDown", shiftKey: true });
    fireEvent.paste(grid, { clipboardData: { getData: () => "Zed" } });
    expect(onAction).toHaveBeenLastCalledWith({
      type: "setValues",
      edits: [{ r: 3, c: 1, value: { t: "text", v: "Zed" } }],
    });
    // A block starts in the focused column, on the first selected row.
    fireEvent.paste(grid, { clipboardData: { getData: () => "P\tp@x\nQ\tq@x" } });
    expect(onAction).toHaveBeenLastCalledWith({
      type: "setValues",
      edits: [
        { r: 2, c: 1, value: { t: "text", v: "P" } },
        { r: 2, c: 2, value: { t: "text", v: "p@x" } },
        { r: 3, c: 1, value: { t: "text", v: "Q" } },
        { r: 3, c: 2, value: { t: "text", v: "q@x" } },
      ],
    });
  });
});

describe("DataGrid cell menu (TablePlus order)", () => {
  const state: ColumnState = { widths: {}, order: [], hidden: [] };
  /** The open menu's top-level item labels (shortcuts left out) and separators ("—"), in order. */
  const entries = () =>
    [
      ...document.querySelectorAll<HTMLElement>(
        '[role="menu"] > [role="menuitem"], [role="menu"] > [role="separator"]',
      ),
    ].map((el) => {
      if (el.getAttribute("role") === "separator") return "—";
      const label = el.cloneNode(true) as HTMLElement;
      label.querySelectorAll('[data-slot="context-menu-shortcut"]').forEach((k) => k.remove());
      return label.textContent!.trim();
    });
  /** Top-level items whose label neither starts with an icon nor is inset (see context menus). */
  const misalignedItems = () =>
    [...document.querySelectorAll<HTMLElement>('[role="menu"] > [role="menuitem"]')]
      .filter((el) => el.firstElementChild?.tagName.toLowerCase() !== "svg")
      .filter((el) => !el.hasAttribute("data-inset"))
      .map((el) => el.textContent);

  it("lists the items in TablePlus's 2021 order", () => {
    const { cell, click } = setup(state, {
      quickLookKey: true,
      onQuickFilter: vi.fn(),
      onRefresh: vi.fn(),
      onImportCsv: vi.fn(),
      onExportPage: vi.fn(),
    });
    click(0, 1);
    openMenu(cell(0, 1));
    expect(entries()).toEqual([
      "Quick Look Editor",
      "Open Row Details",
      "Refresh",
      "—",
      "Paste",
      "Add Row",
      "Duplicate",
      "Set Value",
      "—",
      "Copy",
      "Copy Cell Value",
      "Copy All Column Values",
      "Copy Rows As",
      "—",
      "Quick Filter",
      "—",
      "Import",
      "Export Current Page…",
      "—",
      "Delete",
    ]);
    // ⌫ is Delete's shortcut, ⌘↩ Quick Look's, ⌥⌘R Refresh's, ⌥↩ Set Value's.
    expect(item(/^Delete/)).toHaveTextContent(formatCombo("Backspace"));
    expect(item(/^Quick Look Editor/)).toHaveTextContent(formatCombo("Mod+Enter"));
    expect(item(/^Refresh/)).toHaveTextContent(formatCombo("Mod+Alt+R"));
    expect(item(/^Set Value/)).toHaveTextContent(formatCombo("Alt+Enter"));
    expect(misalignedItems()).toEqual([]);
  });

  it("leaves out what the grid's owner does not offer", () => {
    const { cell, click } = setup(state, { onQuickFilter: vi.fn() });
    click(0, 1);
    openMenu(cell(0, 1));
    expect(entries()).toEqual([
      "Quick Look Editor",
      "Open Row Details",
      "—",
      "Paste",
      "Add Row",
      "Duplicate",
      "Set Value",
      "—",
      "Copy",
      "Copy Cell Value",
      "Copy All Column Values",
      "Copy Rows As",
      "—",
      "Quick Filter",
      "—",
      "Delete",
    ]);
  });

  it("keeps a read-only grid to Quick Look and the copy items", () => {
    const { cell, click } = setup(state, { editable: false });
    click(1, 1);
    openMenu(cell(1, 1));
    expect(entries()).toEqual([
      "Quick Look Editor",
      "Open Row Details",
      "—",
      "Copy",
      "Copy Cell Value",
      "Copy All Column Values",
      "Copy Rows As",
    ]);
  });

  it("copies the right-clicked cell's value, and every value of its column", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>().mockResolvedValue();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText, readText: vi.fn() },
    });
    const { cell, click } = setup(state);
    click(0, 1);
    fireEvent.pointerDown(cell(1, 1), { button: 0, shiftKey: true });
    // Inside the selection: the rows stay selected, the clicked cell is focused.
    openMenu(cell(1, 2));
    expect(document.querySelectorAll("[data-row][data-selected]")).toHaveLength(2);
    await act(async () => fireEvent.click(item("Copy Cell Value")));
    expect(writeText).toHaveBeenLastCalledWith("bob@x.io");
    openMenu(cell(0, 1));
    await act(async () => fireEvent.click(item("Copy All Column Values")));
    expect(writeText).toHaveBeenLastCalledWith("'Ann', 'Bob'");
    expect(toast.success).toHaveBeenLastCalledWith("Copied 2 values");
    openMenu(cell(0, 0));
    await act(async () => fireEvent.click(item("Copy All Column Values")));
    expect(writeText).toHaveBeenLastCalledWith("1, 2");
  });

  it("Delete, Duplicate and Add Row act on the selected rows", () => {
    const { cell, click, onAction } = setup(state);
    click(0, 1);
    fireEvent.pointerDown(cell(1, 1), { button: 0, shiftKey: true });
    openMenu(cell(1, 1));
    fireEvent.click(item(/^Duplicate/));
    expect(onAction).toHaveBeenLastCalledWith({ type: "duplicateRows", rows: [0, 1] });
    openMenu(cell(1, 1));
    fireEvent.click(item(/^Delete/));
    expect(onAction).toHaveBeenLastCalledWith({ type: "deleteRows", rows: [0, 1] });
    openMenu(cell(1, 1));
    fireEvent.click(item(/^Add Row/));
    expect(onAction).toHaveBeenLastCalledWith({ type: "newRow" });
  });

  it("Refresh, Import ▸ From CSV… and Export Current Page… call their owner", async () => {
    const onRefresh = vi.fn();
    const onImportCsv = vi.fn();
    const onExportPage = vi.fn();
    const { cell, click } = setup(state, { onRefresh, onImportCsv, onExportPage });
    click(0, 1);
    openMenu(cell(0, 1));
    fireEvent.click(item(/^Refresh/));
    expect(onRefresh).toHaveBeenCalledOnce();
    openMenu(cell(0, 1));
    fireEvent.click(item(/^Export Current Page/));
    await waitFor(() => expect(onExportPage).toHaveBeenCalledOnce());
    openMenu(cell(0, 1));
    fireEvent.click(item(/^Import/));
    fireEvent.click(await screen.findByRole("menuitem", { name: "From CSV…" }));
    await waitFor(() => expect(onImportCsv).toHaveBeenCalledOnce());
  });

  it("offers Discard Changes only on rows with staged changes", () => {
    const staged: ViewRow[] = [rows[0]!, { ...rows[1]!, status: "updated", changed: new Set([1]) }];
    const { cell, click, onAction } = setup(state, { getRow: (i) => staged[i] });
    click(0, 1);
    openMenu(cell(0, 1));
    expect(screen.queryByRole("menuitem", { name: /Discard Changes/ })).not.toBeInTheDocument();
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    click(1, 1);
    openMenu(cell(1, 1));
    fireEvent.click(item(/Discard Changes/));
    expect(onAction).toHaveBeenLastCalledWith({ type: "revertRows", rows: [1] });
  });

  it("Open Row Details opens the details sidebar on the Row view, and keeps it open", () => {
    const { cell, click } = setup(state);
    click(0, 1);
    openMenu(cell(0, 1));
    fireEvent.click(item(/Open Row Details/));
    expect(useUiStore.getState().inspectorOpen).toBe(true);
    expect(useInspectorStore.getState().mode).toBe("row");
    act(() => useInspectorStore.getState().setMode("cell"));
    openMenu(cell(0, 1));
    fireEvent.click(item(/Open Row Details/));
    expect(useUiStore.getState().inspectorOpen).toBe(true);
    expect(useInspectorStore.getState().mode).toBe("row");
  });
});

describe("DataGrid new rows", () => {
  let hit: Element | null = null;
  beforeAll(() => {
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: () => hit,
    });
  });
  afterAll(() => {
    delete (document as unknown as Record<string, unknown>).elementFromPoint;
  });
  const below = HEADER_H + rows.length * ROW_H + 10;

  it("double-clicking the blank area below the rows inserts a row, then focuses it", () => {
    const onAction = vi.fn();
    const onFocusCell = vi.fn();
    const props = {
      columns,
      getRow: (i: number) => rows[i],
      editable: true,
      onAction,
      onFocusCell,
    };
    const view = render(<DataGrid {...props} rowCount={rows.length} />);
    const grid = document.querySelector<HTMLElement>('[data-scope="grid"]')!;
    hit = grid;
    fireEvent.pointerDown(grid, { button: 0 });
    fireEvent.doubleClick(grid, { clientY: HEADER_H + ROW_H });
    expect(onAction).not.toHaveBeenCalled();
    fireEvent.doubleClick(grid, { clientY: below });
    expect(onAction).toHaveBeenCalledWith({ type: "newRow" });
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();

    const added: ViewRow = {
      key: "new:1",
      cells: [{ t: "default" }, NULL_TEXT, NULL_TEXT],
      status: "inserted",
    };
    const more = [...rows, added];
    view.rerender(<DataGrid {...props} rowCount={more.length} getRow={(i) => more[i]} />);
    expect(onFocusCell).toHaveBeenLastCalledWith(
      expect.objectContaining({ pos: { r: 2, c: 0 }, row: added }),
    );
  });

  it("does not insert rows into a read-only grid", () => {
    const { grid, onAction } = setup({ widths: {}, order: [], hidden: [] }, { editable: false });
    hit = grid;
    fireEvent.doubleClick(grid, { clientY: below });
    expect(onAction).not.toHaveBeenCalled();
  });
});

describe("DataGrid look (TablePlus)", () => {
  const state: ColumnState = { widths: {}, order: [], hidden: [] };
  const rowEl = (r: number) => document.querySelector<HTMLElement>(`[data-row="${r}"]`)!;
  const gutter = (r: number) => document.querySelector<HTMLElement>(`[data-gutter="${r}"]`)!;

  it("lays compact 22px rows out under the header, from one row height", () => {
    expect(ROW_H).toBe(22);
    setup(state);
    expect(rowEl(0).style.top).toBe(`${HEADER_H}px`);
    expect(rowEl(1).style.top).toBe(`${HEADER_H + ROW_H}px`);
    expect(rowEl(1).style.height).toBe(`${ROW_H}px`);
  });

  it("right-aligns numeric kinds only, in one font with tabular digits", () => {
    const kinds = ["int", "bigInt", "decimal", "float", "text", "bool", "date", "uuid"] as const;
    const meta = kinds.map((kind, ordinal) => ({
      name: kind,
      ordinal,
      dataType: kind,
      kind,
      nullable: true,
      defaultExpr: null,
      isPk: false,
      pkOrdinal: null,
      isAutoIncrement: false,
      isGenerated: false,
      isUnsigned: false,
      comment: null,
      enumValues: null,
      charMaxLen: null,
      numericPrecision: null,
      numericScale: null,
      collation: null,
      arrayElemType: null,
      extra: null,
    }));
    const values: Value[] = [
      { t: "int", v: 1 },
      { t: "bigInt", v: "9007199254740993" },
      { t: "decimal", v: "1.50" },
      { t: "float", v: 2.5 },
      { t: "text", v: "x" },
      { t: "bool", v: true },
      { t: "date", v: "2024-01-02" },
      { t: "uuid", v: "00000000-0000-0000-0000-000000000000" },
    ];
    const { cell } = setup(state, {
      columns: buildGridColumns(meta),
      rowCount: 1,
      getRow: () => ({ key: "k", cells: values }),
    });
    const right = kinds.filter((_, c) => cell(0, c).classList.contains("justify-end"));
    expect(right).toEqual(["int", "bigInt", "decimal", "float"]);
    kinds.forEach((_, c) => {
      expect(cell(0, c)).toHaveClass("tabular-nums", "text-[12px]");
      expect(cell(0, c).querySelector(".font-mono")).toBeNull();
    });
  });

  it("centres the header name and keeps the key icon and the type tooltip", () => {
    setup(state, { sort: [{ column: "name", dir: "desc", nulls: null }], onSortChange: vi.fn() });
    const id = document.querySelector<HTMLElement>('[data-header="id"]')!;
    const name = document.querySelector<HTMLElement>('[data-header="name"]')!;
    expect(id).toHaveClass("justify-center", "font-semibold", "text-[12px]");
    expect(id.querySelector("[data-pk]")).not.toBeNull();
    expect(name.querySelector("[data-pk]")).toBeNull();
    expect(id.title).toBe("id · int");
    expect(name.querySelector('[data-sort="desc"]')).not.toBeNull();
    expect(name.querySelector("[data-resize]")).not.toBeNull();
  });

  it("stripes every other row, except selected ones", () => {
    const { click } = setup(state);
    expect(rowEl(0).className).not.toContain("bg-foreground/");
    expect(rowEl(1).className).toContain("bg-foreground/[2.5%]");
    click(1, 1);
    expect(rowEl(1).className).not.toContain("bg-foreground/");
  });

  it("has a narrow, muted, right-aligned row-number gutter sized to the row count", () => {
    setup(state);
    expect(gutter(0)).toHaveClass("text-muted-foreground", "justify-end", "tabular-nums");
    expect(gutter(0)).toHaveTextContent("1");
    expect(gutter(0).style.width).toBe(`${MIN_GUTTER_W}px`);
  });

  it("highlights whole selected rows, row number included, and outlines the focused cell", () => {
    const { grid, click } = setup(state);
    // A click on a cell selects its row (TablePlus) and rings the clicked cell.
    click(1, 2);
    expect(rowEl(1)).toHaveAttribute("data-selected");
    expect(rowEl(0)).not.toHaveAttribute("data-selected");
    expect(gutter(1)).toHaveClass("text-foreground");
    const band = document.querySelector<HTMLElement>('[data-selection="rows"]')!;
    expect(band).toHaveClass("bg-(--grid-selection)");
    expect(band).not.toHaveClass("ring-1");
    expect(band.style.top).toBe(`${HEADER_H + ROW_H}px`);
    const ring = document.querySelector<HTMLElement>("[data-focus-ring]")!;
    expect(ring).toHaveClass("ring-brand", "ring-1");
    expect(ring.style.top).toBe(`${HEADER_H + ROW_H}px`);

    // A row number selects its row too; ⌘A selects every row.
    fireEvent.pointerDown(gutter(0), { button: 0 });
    expect(rowEl(0)).toHaveAttribute("data-selected");
    expect(rowEl(1)).not.toHaveAttribute("data-selected");
    fireEvent.keyDown(grid, { key: "a", ...mod });
    expect(rowEl(0)).toHaveAttribute("data-selected");
    expect(rowEl(1)).toHaveAttribute("data-selected");
  });

  it("⌥-clicking a header selects the whole column instead", () => {
    setup(state);
    const header = document.querySelector<HTMLElement>('[data-header="name"]')!;
    fireEvent.pointerDown(header, { button: 0, altKey: true });
    fireEvent.pointerUp(header, { button: 0 });
    expect(document.querySelector("[data-selected]")).toBeNull();
    expect(document.querySelector('[data-selection="columns"]')).toHaveClass(
      "bg-(--grid-selection)",
    );
  });

  it("shows NULL and an empty string as muted placeholders", () => {
    const { cell } = setup(state, {
      rowCount: 1,
      getRow: () => ({
        key: "k",
        cells: [{ t: "int", v: 1 }, { t: "null" }, { t: "text", v: "" }],
      }),
    });
    expect(cell(0, 1)).toHaveTextContent("NULL");
    expect(cell(0, 1).querySelector("span")).toHaveClass("text-null", "italic");
    expect(cell(0, 2)).toHaveTextContent("EMPTY");
    expect(cell(0, 2).querySelector("span")).toHaveClass("text-null", "italic");
  });

  it("keeps the placeholder out of the editor", () => {
    const { grid, click } = setup(state, {
      rowCount: 1,
      getRow: () => ({ key: "k", cells: [{ t: "int", v: 1 }, { t: "text", v: "" }, NULL_TEXT] }),
    });
    click(0, 1);
    fireEvent.keyDown(grid, { key: "Enter" });
    expect(screen.getByRole("textbox", { name: "Edit name" })).toHaveValue("");
  });
});

describe("DataGrid M8 grid features", () => {
  const state: ColumnState = { widths: {}, order: [], hidden: [] };
  beforeAll(() => {
    // CodeMirror (Quick Look) measures text ranges; jsdom has no layout.
    Range.prototype.getClientRects ??= () => [] as unknown as DOMRectList;
    Range.prototype.getBoundingClientRect ??= () => new DOMRect();
  });
  const rowEl = (r: number) => document.querySelector<HTMLElement>(`[data-row="${r}"]`)!;
  const gutter = (r: number) => document.querySelector<HTMLElement>(`[data-gutter="${r}"]`)!;
  afterEach(() => resetPrefsForTests());

  it("Set Value ▸ NOW() for date and time columns, Add a File… for binary ones", () => {
    const typed = [col("id", "int"), col("born", "date"), col("photo", "bytes")];
    const typedRows: ViewRow[] = [
      { key: "pk:1", cells: [{ t: "int", v: 1 }, { t: "null" }, { t: "null" }] },
    ];
    const { cell, click, onAction } = setup(state, {
      columns: typed,
      rowCount: 1,
      getRow: (i) => typedRows[i],
    });
    click(0, 1);
    openMenu(cell(0, 1));
    fireEvent.click(item(/Set Value/));
    // TablePlus's order: EMPTY, NULL, DEFAULT, then NOW().
    const labels = [
      ...document.querySelectorAll('[role="menu"] [role="menu"] [role="menuitem"]'),
    ].map((el) => {
      const label = el.cloneNode(true) as HTMLElement;
      label.querySelectorAll('[data-slot="context-menu-shortcut"]').forEach((k) => k.remove());
      return label.textContent?.trim();
    });
    expect(labels.slice(0, 4)).toEqual(["EMPTY", "NULL", "DEFAULT", "Now (local time)"]);
    expect(screen.queryByRole("menuitem", { name: /Add a File/ })).not.toBeInTheDocument();
    fireEvent.click(item("Now (local time)"));
    expect(onAction).toHaveBeenLastCalledWith({ type: "setNow", cells: [{ r: 0, c: 1 }] });
    click(0, 2);
    openMenu(cell(0, 2));
    fireEvent.click(item(/Set Value/));
    expect(screen.queryByRole("menuitem", { name: /^Now/ })).not.toBeInTheDocument();
    fireEvent.click(item(/Add a File/));
    expect(onAction).toHaveBeenLastCalledWith({ type: "setFile", cells: [{ r: 0, c: 2 }] });
  });

  it("⌥↩ opens the Set Value menu on the focused cell", async () => {
    const { click, onAction } = setup(state);
    click(1, 1);
    await act(() => actions.run("grid.set-value"));
    expect(screen.getByText("Set Value")).toBeInTheDocument();
    fireEvent.click(item(/EMPTY/));
    expect(onAction).toHaveBeenLastCalledWith({ type: "setEmpty", cells: [{ r: 1, c: 1 }] });
  });

  it("Copy Rows As offers TablePlus's seven formats, numbered in the ⌘⌥C popup", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>().mockResolvedValue();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText, readText: vi.fn() },
    });
    const { grid, click } = setup(state, { active: false });
    click(0, 1);
    fireEvent.keyDown(grid, { key: "ç", code: "KeyC", ...mod, altKey: true });
    const names = screen
      .getAllByRole("menuitem")
      .map((el) => el.textContent?.replace(/\d$/, "").trim());
    expect(names).toEqual([
      "Plain text",
      "JSON",
      "HTML",
      "Markdown",
      "CSV",
      "CSV with header",
      "SQL Insert statement",
    ]);
    // 3 is HTML.
    await act(async () => {
      fireEvent.keyDown(screen.getByRole("menu"), { key: "3" });
    });
    expect(writeText).toHaveBeenLastCalledWith(expect.stringContaining("<td>Ann</td>"));
    expect(toast.success).toHaveBeenLastCalledWith("Copied 1 row as HTML");
  });

  it("orders the header menu like TablePlus and resets positions and widths", () => {
    const onColumnStateChange = vi.fn();
    const onSortChange = vi.fn();
    setup(
      { widths: { name: 300 }, order: ["name", "id", "email"], hidden: [] },
      { onSortChange, onFilterColumn: vi.fn(), onColumnStateChange },
    );
    const header = document.querySelector<HTMLElement>('[data-header="name"]')!;
    openMenu(header);
    const top = [
      ...document.querySelectorAll<HTMLElement>('[role="menu"] > [role="menuitem"]'),
    ].map((el) => el.textContent?.trim());
    expect(top.slice(0, 7)).toEqual([
      "Copy Name",
      "Filter with Column",
      "Hide Column",
      "Reset Column Positions and Widths",
      "Sort ASC",
      "Sort DESC",
      "Remove Sort",
    ]);
    expect(item("Remove Sort")).toHaveAttribute("data-disabled");
    fireEvent.click(item("Reset Column Positions and Widths"));
    expect(onColumnStateChange).toHaveBeenLastCalledWith({ widths: {}, order: [], hidden: [] });
    openMenu(header);
    fireEvent.click(item(/Sort DESC/));
    expect(onSortChange).toHaveBeenLastCalledWith([{ column: "name", dir: "desc", nulls: null }]);
  });

  it("opens Quick Look with ⌘↩ (table grids), a middle-click or the menu, and saves through onEditCommit", async () => {
    const { cell, click, onEditCommit } = setup(state, { quickLookKey: true });
    click(1, 1);
    await act(() => actions.run("grid.quick-look"));
    expect(screen.getByRole("dialog", { name: /^name/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    fireEvent.pointerDown(cell(0, 2), { button: 1 });
    expect(screen.getByRole("dialog", { name: /^email/ })).toBeInTheDocument();
    const view = (await import("@codemirror/view")).EditorView.findFromDOM(
      document.querySelector<HTMLElement>(".cm-editor")!,
    )!;
    act(() =>
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: "new@x.io" } }),
    );
    fireEvent.click(screen.getByRole("button", { name: /Save/ }));
    expect(onEditCommit).toHaveBeenCalledWith(0, 2, { t: "text", v: "new@x.io" });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    openMenu(cell(1, 0));
    fireEvent.click(item(/Quick Look Editor/));
    expect(screen.getByRole("dialog", { name: /^id/ })).toBeInTheDocument();
  });

  it("leaves ⌘↩ alone without quickLookKey (a SQL tab's results run the query)", async () => {
    const { click } = setup(state);
    click(0, 1);
    expect(actions.has("grid.quick-look")).toBe(false);
    openMenu(document.querySelector<HTMLElement>('[data-cell][data-r="0"][data-c="1"]')!);
    expect(item(/Quick Look Editor/)).not.toHaveTextContent(formatCombo("Mod+Enter"));
  });

  it("reports the selected row indexes", () => {
    const onSelectedRowsChange = vi.fn();
    const { cell, click } = setup(state, { onSelectedRowsChange });
    click(0, 1);
    expect(onSelectedRowsChange).toHaveBeenLastCalledWith([0]);
    fireEvent.pointerDown(cell(1, 1), { button: 0, shiftKey: true });
    expect(onSelectedRowsChange).toHaveBeenLastCalledWith([0, 1]);
  });

  it("can drop the row numbers to a slim status strip", () => {
    setup(state, { rowNumbers: false });
    expect(gutter(0).style.width).toBe(`${SLIM_GUTTER_W}px`);
    expect(gutter(0)).toHaveTextContent("");
  });

  it("stripes rows only while 'Alternating row background colors' is on", () => {
    resetPrefsForTests({ ...DEFAULT_PREFS, gridAlternatingRows: false });
    setup(state);
    expect(rowEl(1).className).not.toContain("bg-foreground/[2.5%]");
  });
});
