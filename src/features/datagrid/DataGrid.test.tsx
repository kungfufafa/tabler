import { EditorView } from "@codemirror/view";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import type { Value } from "@/lib/ipc";
import { formatCombo, isMac } from "@/lib/keys/combo";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import { useInspectorStore } from "@/stores/inspector";
import { useUiStore } from "@/stores/ui";
import { EMPTY_PLACEHOLDER, MISSING_PLACEHOLDER } from "@/lib/values";
import { DataGrid, type DataGridProps } from "./DataGrid";
import { LacksFieldsContext } from "./lacksFields";
import {
  buildGridColumns,
  HEADER_H,
  MIN_GUTTER_W,
  ROW_H,
  SLIM_GUTTER_W,
  type ColumnState,
  type FkRef,
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
  const { unmount } = render(
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
  return { ...props, grid, cell, click, unmount };
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

  it("Edit ▸ Copy Cell Value (grid.copy-cell) copies the focused cell's value, in an active grid only", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>().mockResolvedValue();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText, readText: vi.fn() },
    });
    const { click, unmount } = setup(state);
    expect(actions.has("grid.copy-cell")).toBe(true);
    // Display column 1 is `name`; its focused cell holds "Bob".
    click(1, 1);
    await act(() => actions.run("grid.copy-cell"));
    expect(writeText).toHaveBeenLastCalledWith("Bob");
    // Another cell, another value (the focus moved, not the selection of rows).
    click(0, 0);
    await act(() => actions.run("grid.copy-cell"));
    expect(writeText).toHaveBeenLastCalledWith("ann@x.io");
    unmount();
    expect(actions.has("grid.copy-cell")).toBe(false);
  });

  it("does not answer grid.copy-cell while another grid is the active one", () => {
    setup(state, { active: false });
    expect(actions.has("grid.copy-cell")).toBe(false);
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

  it("edits a whole blob as hex, and explains that a cut-off preview is not edited inline", () => {
    const blobColumns = [...columns, col("avatar", "bytes")];
    const blobRows: ViewRow[] = rows.map((r, i) => ({
      ...r,
      cells: [
        ...r.cells,
        {
          t: "bytes",
          v: { base64: i === 0 ? "AP8=" : "AAAA", len: i === 0 ? 2 : 100_000, truncated: i === 1 },
        } as Value,
      ],
    }));
    const onEditCommit = vi.fn();
    render(
      <DataGrid
        columns={blobColumns}
        rowCount={blobRows.length}
        getRow={(i) => blobRows[i]}
        editable
        onAction={vi.fn()}
        onEditCommit={onEditCommit}
      />,
    );
    const grid = document.querySelector<HTMLElement>('[data-scope="grid"]')!;
    const blob = (r: number) => document.querySelector(`[data-cell][data-r="${r}"][data-c="3"]`)!;
    fireEvent.pointerDown(blob(0), { button: 0 });
    fireEvent.keyDown(grid, { key: "Enter" });
    const input = screen.getByRole("textbox", { name: "Edit avatar" });
    expect(input).toHaveValue("00ff");
    fireEvent.change(input, { target: { value: "\\x4869" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onEditCommit).toHaveBeenLastCalledWith(0, 3, {
      t: "bytes",
      v: { base64: "SGk=", len: 2, truncated: false },
    });
    fireEvent.pointerDown(blob(1), { button: 0 });
    fireEvent.keyDown(grid, { key: "Enter" });
    expect(toast.message).toHaveBeenCalledWith(
      "Only a preview of this value is loaded",
      expect.anything(),
    );
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

describe("DataGrid paste of a copied column that ends in NULL cells", () => {
  const two = [col("id", "int"), col("a"), col("b")];
  const data: ViewRow[] = ["x", "y", null].map((a, i) => ({
    key: `pk:${i + 1}`,
    cells: [
      { t: "int", v: i + 1 },
      a === null ? { t: "null" } : { t: "text", v: a },
      { t: "null" },
    ],
  }));

  it("pastes every copied row back, the last NULL one included", async () => {
    let clipboard = "";
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: vi.fn((text: string) => {
          clipboard = text;
          return Promise.resolve();
        }),
        readText: vi.fn(() => Promise.resolve(clipboard)),
      },
    });
    const onAction = vi.fn();
    render(
      <DataGrid
        columns={two}
        rowCount={data.length}
        getRow={(i) => data[i]}
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
    const key = (k: string, extra: object = {}) =>
      act(async () => {
        fireEvent.keyDown(grid, {
          key: k,
          code: k.length === 1 ? `Key${k.toUpperCase()}` : k,
          ...extra,
        });
      });
    // Copy column a (x, y, NULL): the text is "x\ny\n".
    click(0, 1);
    // A cell range of one column: ⇧→ starts a range, ⇧← shrinks it back, ⇧↓ grows it.
    await key("ArrowRight", { shiftKey: true });
    await key("ArrowLeft", { shiftKey: true });
    await key("ArrowDown", { shiftKey: true });
    await key("ArrowDown", { shiftKey: true });
    await key("c", mod);
    expect(clipboard).toBe("x\ny\n");
    // Paste it into column b, from its first row.
    click(0, 2);
    await key("v", mod);
    expect(onAction).toHaveBeenLastCalledWith({
      type: "setValues",
      edits: [
        { r: 0, c: 2, value: { t: "text", v: "x" } },
        { r: 1, c: 2, value: { t: "text", v: "y" } },
        { r: 2, c: 2, value: { t: "null" } },
      ],
    });
    expect(toast.success).toHaveBeenLastCalledWith("Pasted 3 cells");
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
    // ⇧→ selects a cell range over those rows (TablePlus's red-border cell selection).
    fireEvent.keyDown(grid, { key: "ArrowRight", shiftKey: true });
    expect(selected()).toEqual([]);
    expect(document.querySelector('[data-selection="cells"]')).not.toBeNull();
    expect(onSelectionChange).toHaveBeenLastCalledWith(3);
    fireEvent.keyDown(grid, { key: "ArrowUp" });
    expect(selected()).toEqual([3]);
    expect(document.querySelector('[data-selection="cells"]')).toBeNull();
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

  it("dragging down a column selects rows; across columns a cell range", () => {
    const { grid, cell, onSelectionChange } = mountMany();
    fireEvent.pointerDown(cell(1, 1), { button: 0 });
    hit = cell(3, 1);
    fireEvent.pointerMove(grid, { clientX: 10, clientY: 10 });
    expect(selected()).toEqual([1, 2, 3]);
    hit = cell(3, 2);
    fireEvent.pointerMove(grid, { clientX: 10, clientY: 10 });
    expect(selected()).toEqual([]);
    const range = document.querySelector<HTMLElement>('[data-selection="cells"]')!;
    expect(range.style.top).toBe(`${HEADER_H + ROW_H}px`);
    expect(range.style.height).toBe(`${3 * ROW_H}px`);
    expect(onSelectionChange).toHaveBeenLastCalledWith(3);
    fireEvent.pointerUp(grid, { button: 0 });
    fireEvent.pointerDown(cell(1, 1), { button: 0 });
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
      onImport: vi.fn(),
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
      "Copy Selected Cells",
      "Copy Selected Cells As",
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
      "Copy Selected Cells",
      "Copy Selected Cells As",
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
      "Copy Selected Cells",
      "Copy Selected Cells As",
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

  it("Refresh, Import ▸ From CSV… / JSON… / SQL Dump… and Export Current Page… call their owner", async () => {
    const onRefresh = vi.fn();
    const onImport = vi.fn();
    const onExportPage = vi.fn();
    const { cell, click } = setup(state, { onRefresh, onImport, onExportPage });
    click(0, 1);
    openMenu(cell(0, 1));
    fireEvent.click(item(/^Refresh/));
    expect(onRefresh).toHaveBeenCalledOnce();
    openMenu(cell(0, 1));
    fireEvent.click(item(/^Export Current Page/));
    await waitFor(() => expect(onExportPage).toHaveBeenCalledOnce());
    for (const [label, kind] of [
      ["From CSV…", "csv"],
      ["From JSON…", "json"],
      ["From SQL Dump…", "sql"],
    ] as const) {
      openMenu(cell(0, 1));
      fireEvent.click(item(/^Import/));
      const labels = (await screen.findAllByRole("menuitem"))
        .map((el) => el.textContent)
        .filter((t) => t?.startsWith("From "));
      expect(labels).toEqual(["From CSV…", "From JSON…", "From SQL Dump…"]);
      fireEvent.click(await screen.findByRole("menuitem", { name: label }));
      await waitFor(() => expect(onImport).toHaveBeenLastCalledWith(kind));
    }
    expect(onImport).toHaveBeenCalledTimes(3);
  });

  it("drops the selection when deselectKey changes (a commit deselects the edited rows)", () => {
    const onSelectionChange = vi.fn();
    const props = {
      columns,
      rowCount: rows.length,
      getRow: (i: number) => rows[i],
      editable: true,
      active: false,
      onSelectionChange,
    };
    const { rerender } = render(<DataGrid {...props} deselectKey={0} />);
    const el = document.querySelector('[data-cell][data-r="1"][data-c="1"]')!;
    fireEvent.pointerDown(el, { button: 0 });
    expect(onSelectionChange).toHaveBeenLastCalledWith(1);
    // The same key again (an unrelated re-render) keeps it.
    rerender(<DataGrid {...props} deselectKey={0} />);
    expect(onSelectionChange).toHaveBeenLastCalledWith(1);
    rerender(<DataGrid {...props} deselectKey={1} />);
    expect(onSelectionChange).toHaveBeenLastCalledWith(0);
  });

  it("Generate JOIN appears in the menu of a foreign key cell only, even for NULL", () => {
    const withFk = [
      col("id", "int"),
      { ...col("owner_id", "int"), fk: { schema: "public", table: "users", column: "id" } },
      col("name"),
    ];
    const data: ViewRow[] = [
      {
        key: "a",
        cells: [
          { t: "int", v: 1 },
          { t: "int", v: 7 },
          { t: "text", v: "x" },
        ],
      },
      { key: "b", cells: [{ t: "int", v: 2 }, { t: "null" }, { t: "text", v: "y" }] },
    ];
    const onGenerateJoin = vi.fn();
    const menuAt = (r: number, c: number) => {
      cleanup();
      render(
        <DataGrid
          columns={withFk}
          rowCount={data.length}
          getRow={(i) => data[i]}
          editable
          active={false}
          onGenerateJoin={onGenerateJoin}
        />,
      );
      const el = document.querySelector(`[data-cell][data-r="${r}"][data-c="${c}"]`)!;
      fireEvent.pointerDown(el, { button: 0 });
      openMenu(el);
    };
    // A plain column has no such item.
    menuAt(0, 2);
    expect(screen.queryByRole("menuitem", { name: /Generate JOIN/ })).not.toBeInTheDocument();
    // The FK column has it, with a value or with NULL (and "Open users" only with a value).
    menuAt(1, 1);
    expect(screen.getByRole("menuitem", { name: /Generate JOIN/ })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /^Open users/ })).not.toBeInTheDocument();
    menuAt(0, 1);
    expect(screen.getByRole("menuitem", { name: /^Open users/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("menuitem", { name: /Generate JOIN/ }));
    expect(onGenerateJoin).toHaveBeenCalledOnce();
    expect(onGenerateJoin.mock.calls[0]![0]).toMatchObject({ id: "owner_id" });
    // Without an owner, no item.
    cleanup();
    render(
      <DataGrid
        columns={withFk}
        rowCount={data.length}
        getRow={(i) => data[i]}
        editable
        active={false}
      />,
    );
    const el = document.querySelector('[data-cell][data-r="0"][data-c="1"]')!;
    fireEvent.pointerDown(el, { button: 0 });
    openMenu(el);
    expect(screen.queryByRole("menuitem", { name: /Generate JOIN/ })).not.toBeInTheDocument();
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
    expect(labels.slice(0, 4)).toEqual(["EMPTY", "NULL", "DEFAULT", "NOW()"]);
    expect(screen.queryByRole("menuitem", { name: /Add a File/ })).not.toBeInTheDocument();
    fireEvent.click(item("NOW()"));
    expect(onAction).toHaveBeenLastCalledWith({ type: "setNow", cells: [{ r: 0, c: 1 }] });
    click(0, 2);
    openMenu(cell(0, 2));
    fireEvent.click(item(/Set Value/));
    expect(screen.queryByRole("menuitem", { name: /^NOW/ })).not.toBeInTheDocument();
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

  it("Copy Rows As offers TablePlus's seven formats and Raw, numbered in the ⌘⌥C popup", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>().mockResolvedValue();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText, readText: vi.fn() },
    });
    const { grid, click } = setup(state, {
      active: false,
      copyContext: { dialect: "postgres", schema: null, table: "people" },
    });
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
      "Raw",
    ]);
    // 3 is HTML.
    await act(async () => {
      fireEvent.keyDown(screen.getByRole("menu"), { key: "3" });
    });
    expect(writeText).toHaveBeenLastCalledWith(expect.stringContaining("<td>Ann</td>"));
    expect(toast.success).toHaveBeenLastCalledWith("Copied 1 row as HTML");
  });

  it("offers no SQL INSERTs without a core dialect to write them in, and numbers the rest", async () => {
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
    expect(names).not.toContain("SQL Insert statement");
    expect(names.at(-1)).toBe("Raw");
    // 7 is Raw now.
    await act(async () => {
      fireEvent.keyDown(screen.getByRole("menu"), { key: "7" });
    });
    expect(toast.success).toHaveBeenLastCalledWith("Copied 1 row as raw values");
  });

  it("Copy Rows As ▸ CSV follows Settings ▸ CSV default options", async () => {
    resetPrefsForTests({ ...DEFAULT_PREFS, csvDelimiter: ";", csvQuote: "double" });
    const writeText = vi.fn<(text: string) => Promise<void>>().mockResolvedValue();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText, readText: vi.fn() },
    });
    const { grid, click } = setup(state, { active: false });
    click(0, 1);
    fireEvent.keyDown(grid, { key: "ç", code: "KeyC", ...mod, altKey: true });
    // 6 is CSV with header.
    await act(async () => {
      fireEvent.keyDown(screen.getByRole("menu"), { key: "6" });
    });
    expect(writeText).toHaveBeenLastCalledWith('"id";"name";"email"\n"1";"Ann";"ann@x.io"');
    resetPrefsForTests();
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

describe("DataGrid grid3 features", () => {
  const state: ColumnState = { widths: {}, order: [], hidden: [] };
  afterEach(() => resetPrefsForTests());

  it("a header click sorts DESC, then ASC, then not at all; Add to Sort appends a key", () => {
    const onSortChange = vi.fn();
    const header = () => document.querySelector<HTMLElement>('[data-header="name"]')!;
    const press = () => {
      fireEvent.pointerDown(header(), { button: 0 });
      fireEvent.pointerUp(header(), { button: 0 });
    };
    setup(state, { onSortChange });
    press();
    expect(onSortChange).toHaveBeenLastCalledWith([{ column: "name", dir: "desc", nulls: null }]);
    cleanup();
    setup(state, { onSortChange, sort: [{ column: "name", dir: "desc", nulls: null }] });
    press();
    expect(onSortChange).toHaveBeenLastCalledWith([{ column: "name", dir: "asc", nulls: null }]);
    cleanup();
    setup(state, { onSortChange, sort: [{ column: "name", dir: "asc", nulls: null }] });
    press();
    expect(onSortChange).toHaveBeenLastCalledWith([]);
    cleanup();
    setup(state, { onSortChange, sort: [{ column: "id", dir: "asc", nulls: null }] });
    openMenu(header());
    fireEvent.pointerDown(item(/Add to Sort/), { button: 0 });
    fireEvent.keyDown(item(/Add to Sort/), { key: "ArrowRight" });
    fireEvent.click(screen.getByRole("menuitem", { name: /^DESC$/ }));
    expect(onSortChange).toHaveBeenLastCalledWith([
      { column: "id", dir: "asc", nulls: null },
      { column: "name", dir: "desc", nulls: null },
    ]);
  });

  it("⌘C then ⌘V keeps NULL apart from EMPTY, and keeps key values", () => {
    const typed = [col("id", "int"), col("name"), col("email")];
    const typedRows: ViewRow[] = [
      { key: "pk:1", cells: [{ t: "int", v: 1 }, { t: "null" }, { t: "text", v: "" }] },
    ];
    const onAction = vi.fn();
    render(
      <DataGrid
        columns={typed}
        rowCount={1}
        getRow={(i) => typedRows[i]}
        editable
        onAction={onAction}
      />,
    );
    const grid = document.querySelector<HTMLElement>('[data-scope="grid"]')!;
    fireEvent.pointerDown(document.querySelector('[data-cell][data-r="0"][data-c="0"]')!, {
      button: 0,
    });
    let copied = "";
    fireEvent.copy(grid, { clipboardData: { setData: (_: string, t: string) => (copied = t) } });
    expect(copied).toBe("1\t\t");
    fireEvent.paste(grid, { clipboardData: { getData: () => copied } });
    expect(onAction).toHaveBeenLastCalledWith({
      type: "insertRows",
      rows: [
        [
          { c: 0, value: { t: "int", v: 1 } },
          { c: 1, value: { t: "null" } },
          { c: 2, value: { t: "text", v: "" } },
        ],
      ],
    });
  });

  it("Copy Selected Cells copies only the cell range", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>().mockResolvedValue();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText, readText: vi.fn() },
    });
    const { grid, click, cell } = setup(state);
    click(0, 1);
    fireEvent.keyDown(grid, { key: "ArrowDown", shiftKey: true });
    fireEvent.keyDown(grid, { key: "ArrowRight", shiftKey: true });
    openMenu(cell(1, 2));
    await act(async () => fireEvent.click(item("Copy Selected Cells")));
    expect(writeText).toHaveBeenLastCalledWith("Ann\tann@x.io\nBob\tbob@x.io");
    expect(toast.success).toHaveBeenLastCalledWith("Copied 4 cells");
    // ⌘C on a range copies the range too.
    await act(() => actions.run("grid.copy"));
    expect(writeText).toHaveBeenLastCalledWith("Ann\tann@x.io\nBob\tbob@x.io");
  });

  it("scrolls back to the top when the page changes", () => {
    const view = render(
      <DataGrid columns={columns} rowCount={2} getRow={(i) => rows[i]} scrollResetKey={0} />,
    );
    const grid = document.querySelector<HTMLElement>('[data-scope="grid"]')!;
    grid.scrollTop = 120;
    view.rerender(
      <DataGrid columns={columns} rowCount={2} getRow={(i) => rows[i]} scrollResetKey={0} />,
    );
    expect(grid.scrollTop).toBe(120);
    view.rerender(
      <DataGrid columns={columns} rowCount={2} getRow={(i) => rows[i]} scrollResetKey={300} />,
    );
    expect(grid.scrollTop).toBe(0);
  });

  it("shows a cut-off value in a tooltip only with Allows expansion Tool Tips", () => {
    const { cell } = setup(state);
    const span = cell(0, 1).firstElementChild as HTMLElement;
    Object.defineProperty(span, "scrollWidth", { configurable: true, value: 500 });
    Object.defineProperty(span, "clientWidth", { configurable: true, value: 100 });
    fireEvent.mouseOver(span);
    expect(cell(0, 1)).not.toHaveAttribute("title");
    act(() => resetPrefsForTests({ ...DEFAULT_PREFS, gridExpansionTooltips: true }));
    fireEvent.mouseOver(span);
    expect(cell(0, 1)).toHaveAttribute("title", "Ann");
  });

  it("shows the loading wallpaper while the first page loads (a preference)", () => {
    const view = render(
      <DataGrid columns={columns} rowCount={0} getRow={() => undefined} loading />,
    );
    expect(screen.getByTestId("loading-wallpaper")).toBeInTheDocument();
    act(() => resetPrefsForTests({ ...DEFAULT_PREFS, gridLoadingWallpaper: false }));
    view.rerender(<DataGrid columns={columns} rowCount={0} getRow={() => undefined} loading />);
    expect(screen.queryByTestId("loading-wallpaper")).not.toBeInTheDocument();
  });

  it("auto-hides the scroll bars with the preference", () => {
    act(() => resetPrefsForTests({ ...DEFAULT_PREFS, gridAutoHideScrollers: true }));
    const { grid } = setup(state);
    expect(grid).toHaveAttribute("data-autohide-scrollers");
    fireEvent.scroll(grid);
    expect(grid).toHaveAttribute("data-scrolling");
  });

  it("shows geometry as WKT and previews it on hover", async () => {
    const geoCols = [col("id", "int"), col("geom", "geometry")];
    const geoRows: ViewRow[] = [
      {
        key: "pk:1",
        cells: [
          { t: "int", v: 1 },
          { t: "other", v: { ty: "geometry", text: "SRID=4326;LINESTRING(0 0,1 1)" } },
        ],
      },
    ];
    render(<DataGrid columns={geoCols} rowCount={1} getRow={(i) => geoRows[i]} />);
    const geo = document.querySelector<HTMLElement>('[data-cell][data-r="0"][data-c="1"]')!;
    expect(geo).toHaveTextContent(/^LINESTRING\(0 0,1 1\)$/);
    fireEvent.mouseOver(geo);
    const tip = await screen.findByTestId("geometry-preview");
    expect(tip).toHaveTextContent("LINESTRING · SRID 4326");
    expect(tip.querySelector('[data-geometry="lines"]')).not.toBeNull();
    fireEvent.mouseLeave(document.querySelector('[data-scope="grid"]')!);
    expect(screen.queryByTestId("geometry-preview")).not.toBeInTheDocument();
  });

  it("a detached Quick Look stays open and follows the focused cell", async () => {
    const { grid, click } = setup(state, { quickLookKey: true });
    click(0, 1);
    await act(() => actions.run("grid.quick-look"));
    fireEvent.click(screen.getByRole("button", { name: /Detach/ }));
    expect(document.querySelector('[data-quick-look="detached"]')).not.toBeNull();
    const text = () =>
      document.querySelector<HTMLElement>('[data-quick-look="detached"] .cm-content')!.textContent;
    expect(text()).toBe("Ann");
    fireEvent.keyDown(grid, { key: "ArrowDown" });
    expect(text()).toBe("Bob");
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    expect(text()).toBe("bob@x.io");
    fireEvent.click(screen.getByRole("button", { name: "Close Quick Look" }));
    expect(document.querySelector('[data-quick-look="detached"]')).toBeNull();
  });

  describe("with an edit that is not saved", () => {
    /** A grid that keeps the edits staged through `onEditCommit`, as the table tab does. */
    function Staged({ onCommit }: { onCommit?: (r: number, c: number, v: Value) => void }) {
      const [data, setData] = useState(rows);
      return (
        <DataGrid
          columns={columns}
          rowCount={data.length}
          getRow={(i) => data[i]}
          columnState={state}
          editable
          active
          quickLookKey
          onAction={vi.fn()}
          onFocusCell={vi.fn()}
          onEditCommit={(r, c, v) => {
            onCommit?.(r, c, v);
            setData((d) =>
              d.map((row, i) =>
                i === r ? { ...row, cells: row.cells.map((x, j) => (j === c ? v : x)) } : row,
              ),
            );
          }}
        />
      );
    }
    const panel = () => document.querySelector<HTMLElement>('[data-quick-look="detached"]');
    const shown = () => panel()!.querySelector<HTMLElement>(".cm-content")!.textContent;
    const type = (text: string) => {
      const view = EditorView.findFromDOM(panel()!.querySelector<HTMLElement>(".cm-editor")!)!;
      act(() => view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } }));
    };
    const cellAt = (r: number, c: number) =>
      document.querySelector<HTMLElement>(`[data-cell][data-r="${r}"][data-c="${c}"]`)!;
    /** Detaches the Quick Look on Ann (row 0, name) and edits it. */
    async function editAnn() {
      const { container } = render(<Staged />);
      const grid = container.querySelector<HTMLElement>('[data-scope="grid"]')!;
      fireEvent.pointerDown(cellAt(0, 1), { button: 0 });
      await act(() => actions.run("grid.quick-look"));
      fireEvent.click(screen.getByRole("button", { name: /Detach/ }));
      expect(shown()).toBe("Ann");
      type("EDITED");
      expect(screen.getByText("Edited — not staged yet")).toBeInTheDocument();
      return grid;
    }

    it("does not move to another focused cell and keeps its text", async () => {
      const grid = await editAnn();
      fireEvent.pointerDown(cellAt(1, 1), { button: 0 });
      expect(shown()).toBe("EDITED");
      expect(panel()).toHaveTextContent("row 1");
      expect(screen.getByText("Edited — not staged yet")).toBeInTheDocument();
      fireEvent.keyDown(grid, { key: "ArrowRight" });
      fireEvent.keyDown(grid, { key: "ArrowUp" });
      expect(shown()).toBe("EDITED");
      expect(panel()).toHaveTextContent("row 1");
    });

    it("follows the focused cell again once the edit is saved", async () => {
      const grid = await editAnn();
      fireEvent.pointerDown(cellAt(1, 1), { button: 0 });
      expect(shown()).toBe("EDITED");
      fireEvent.click(screen.getByRole("button", { name: /Save/ }));
      // Staged on Ann's row, and the panel is on the focused cell (Bob) at once.
      expect(cellAt(0, 1)).toHaveTextContent("EDITED");
      await waitFor(() => expect(shown()).toBe("Bob"));
      expect(panel()).toHaveTextContent("row 2");
      fireEvent.keyDown(grid, { key: "ArrowRight" });
      expect(shown()).toBe("bob@x.io");
    });

    it("follows the focused cell again once the edit is discarded", async () => {
      await editAnn();
      fireEvent.pointerDown(cellAt(1, 1), { button: 0 });
      expect(shown()).toBe("EDITED");
      fireEvent.click(screen.getByRole("button", { name: "Discard" }));
      await waitFor(() => expect(shown()).toBe("Bob"));
      expect(cellAt(0, 1)).toHaveTextContent("Ann");
    });

    it("does not take another cell when Quick Look is asked for it", async () => {
      await editAnn();
      fireEvent.pointerDown(cellAt(1, 1), { button: 0 });
      await act(() => actions.run("grid.quick-look"));
      expect(toast.message).toHaveBeenCalledWith("Save or discard the edit in Quick Look first");
      expect(shown()).toBe("EDITED");
      expect(panel()).toHaveTextContent("row 1");
    });

    it("a closed panel forgets the edit: the next one follows", async () => {
      const grid = await editAnn();
      fireEvent.click(screen.getByRole("button", { name: "Close Quick Look" }));
      expect(panel()).toBeNull();
      fireEvent.pointerDown(cellAt(1, 1), { button: 0 });
      await act(() => actions.run("grid.quick-look"));
      fireEvent.click(screen.getByRole("button", { name: /Detach/ }));
      expect(shown()).toBe("Bob");
      fireEvent.keyDown(grid, { key: "ArrowUp" });
      expect(shown()).toBe("Ann");
    });
  });
});

describe("DataGrid foreign keys: one jumps, several ask which", () => {
  const ref = (table: string, column: string, extra: Partial<FkRef> = {}): FkRef => ({
    schema: "public",
    table,
    column,
    ...extra,
  });
  const users = ref("users", "id");
  const accounts = ref("accounts", "uid", { name: "owner_account_fkey" });
  const pair = ref("orders", "b", {
    name: "owner_pair_fkey",
    localColumns: ["owner_id", "name"],
    refColumns: ["a", "b"],
  });
  const data: ViewRow[] = [
    {
      key: "a",
      cells: [
        { t: "int", v: 1 },
        { t: "int", v: 7 },
        { t: "text", v: "x" },
      ],
    },
    { key: "b", cells: [{ t: "int", v: 2 }, { t: "null" }, { t: "text", v: "y" }] },
  ];
  const owner = (fks: FkRef[], fk?: FkRef): GridColumn[] => [
    col("id", "int"),
    { ...col("owner_id", "int"), fk, fks },
    col("name"),
  ];
  function mountFk(cols: GridColumn[], extra: Partial<DataGridProps> = {}) {
    const onOpenFk = vi.fn();
    const view = render(
      <DataGrid
        columns={cols}
        rowCount={data.length}
        getRow={(i) => data[i]}
        editable
        active
        onOpenFk={onOpenFk}
        {...extra}
      />,
    );
    const cell = (r: number, c: number) =>
      document.querySelector<HTMLElement>(`[data-cell][data-r="${r}"][data-c="${c}"]`)!;
    const arrow = (r: number, c: number) => cell(r, c).querySelector<HTMLElement>("[data-fk]")!;
    /** A press and release on an arrow, as the pointer does it. */
    const press = (el: Element) => {
      fireEvent.pointerDown(el, { button: 0 });
      fireEvent.pointerUp(el, { button: 0 });
    };
    return { ...view, onOpenFk, cell, arrow, press };
  }
  const menuItems = (menu: HTMLElement) =>
    within(menu)
      .getAllByRole("menuitem")
      .map((i) => i.textContent);

  it("one foreign key: the arrow opens it at once, with the column and value only", () => {
    const { onOpenFk, arrow, press } = mountFk(owner([users], users));
    press(arrow(0, 1));
    expect(onOpenFk).toHaveBeenCalledOnce();
    expect(onOpenFk.mock.calls[0]).toHaveLength(2);
    expect(onOpenFk).toHaveBeenCalledWith(expect.objectContaining({ id: "owner_id" }), {
      t: "int",
      v: 7,
    });
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    // NULL has no arrow.
    expect(arrow(1, 1)).toBeNull();
  });

  it("several foreign keys: the arrow opens a menu of 'table (column)', and the pick opens that one", async () => {
    const { onOpenFk, arrow, cell } = mountFk(owner([users, accounts], users));
    expect(arrow(0, 1)).toHaveAttribute("aria-label", "Open a referenced table");
    fireEvent.pointerDown(arrow(0, 1), { button: 0 });
    // The menu opens once the press is released (the press moves the focus to the arrow).
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    fireEvent.pointerUp(arrow(0, 1), { button: 0 });
    const menu = await screen.findByRole("menu");
    expect(menuItems(menu)).toEqual(["users (id)", "accounts (uid)"]);
    // Anchored under the cell that was pressed.
    const anchor = document.querySelector<HTMLElement>('[data-slot="dropdown-menu-trigger"]')!;
    expect(anchor.style.left).toBe(cell(0, 1).style.left);
    expect(anchor.style.top).toBe(`${HEADER_H + ROW_H}px`);
    expect(onOpenFk).not.toHaveBeenCalled();
    fireEvent.click(within(menu).getByRole("menuitem", { name: "accounts (uid)" }));
    expect(onOpenFk).toHaveBeenCalledOnce();
    expect(onOpenFk).toHaveBeenCalledWith(
      expect.objectContaining({ id: "owner_id" }),
      { t: "int", v: 7 },
      accounts,
    );
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
  });

  it("the menu is keyboard operable: ↓ and ↩ pick, ⎋ closes it without opening anything", async () => {
    const { onOpenFk, arrow, press } = mountFk(owner([users, accounts], users));
    press(arrow(0, 1));
    const menu = await screen.findByRole("menu");
    const [first, second] = within(menu).getAllByRole("menuitem");
    await waitFor(() => expect(first).toHaveFocus());
    fireEvent.keyDown(first!, { key: "ArrowDown" });
    await waitFor(() => expect(second).toHaveFocus());
    fireEvent.keyDown(second!, { key: "Enter" });
    expect(onOpenFk).toHaveBeenCalledWith(expect.anything(), { t: "int", v: 7 }, accounts);

    onOpenFk.mockClear();
    press(arrow(0, 1));
    const again = await screen.findByRole("menu");
    fireEvent.keyDown(within(again).getAllByRole("menuitem")[0]!, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
    expect(onOpenFk).not.toHaveBeenCalled();
  });

  it("the arrow stays out of the tab order", () => {
    const { arrow } = mountFk(owner([users, accounts], users));
    expect(arrow(0, 1)).toHaveAttribute("tabindex", "-1");
  });

  it("⌘J opens the focused cell's key at once, or the menu with several; NULL does nothing", async () => {
    const one = mountFk(owner([users], users));
    fireEvent.pointerDown(one.cell(0, 1), { button: 0 });
    await act(() => actions.run("grid.open-fk"));
    expect(one.onOpenFk).toHaveBeenCalledWith(expect.objectContaining({ id: "owner_id" }), {
      t: "int",
      v: 7,
    });
    expect(one.onOpenFk.mock.calls[0]).toHaveLength(2);
    one.onOpenFk.mockClear();
    fireEvent.pointerDown(one.cell(1, 1), { button: 0 });
    await act(() => actions.run("grid.open-fk"));
    expect(one.onOpenFk).not.toHaveBeenCalled();
    cleanup();

    const several = mountFk(owner([users, accounts], users));
    fireEvent.pointerDown(several.cell(0, 1), { button: 0 });
    await act(() => actions.run("grid.open-fk"));
    const menu = await screen.findByRole("menu");
    expect(menuItems(menu)).toEqual(["users (id)", "accounts (uid)"]);
    expect(several.onOpenFk).not.toHaveBeenCalled();
    fireEvent.click(within(menu).getByRole("menuitem", { name: "users (id)" }));
    expect(several.onOpenFk).toHaveBeenCalledWith(expect.anything(), { t: "int", v: 7 }, users);
  });

  it("⌘J is registered only by an active grid that can open keys", () => {
    mountFk(owner([users], users), { onOpenFk: undefined });
    expect(actions.has("grid.open-fk")).toBe(false);
    cleanup();
    mountFk(owner([users], users), { active: false });
    expect(actions.has("grid.open-fk")).toBe(false);
    cleanup();
    mountFk(owner([users], users));
    expect(actions.has("grid.open-fk")).toBe(true);
  });

  it("the cell menu's Open item: the key's table, or 'Open Referenced Row…' with a menu for several", async () => {
    const one = mountFk(owner([users], users));
    fireEvent.pointerDown(one.cell(0, 1), { button: 0 });
    openMenu(one.cell(0, 1));
    fireEvent.click(item(/^Open users · id/));
    expect(one.onOpenFk).toHaveBeenCalledWith(expect.objectContaining({ id: "owner_id" }), {
      t: "int",
      v: 7,
    });
    expect(one.onOpenFk.mock.calls[0]).toHaveLength(2);
    cleanup();

    const several = mountFk(owner([users, accounts], users));
    fireEvent.pointerDown(several.cell(0, 1), { button: 0 });
    openMenu(several.cell(0, 1));
    fireEvent.click(item(/^Open Referenced Row/));
    // Once the cell menu has closed, the keys' menu opens under the cell.
    const menu = await screen.findByRole("menu");
    expect(menuItems(menu)).toEqual(["users (id)", "accounts (uid)"]);
    fireEvent.click(within(menu).getByRole("menuitem", { name: "accounts (uid)" }));
    expect(several.onOpenFk).toHaveBeenCalledWith(expect.anything(), { t: "int", v: 7 }, accounts);
  });

  it("Generate JOIN uses the same menu: one key at once, several ask which", async () => {
    const onGenerateJoin = vi.fn();
    const one = mountFk(owner([users], users), { onGenerateJoin });
    fireEvent.pointerDown(one.cell(0, 1), { button: 0 });
    openMenu(one.cell(0, 1));
    fireEvent.click(item(/Generate JOIN/));
    expect(onGenerateJoin).toHaveBeenCalledOnce();
    expect(onGenerateJoin.mock.calls[0]).toHaveLength(1);
    cleanup();
    onGenerateJoin.mockClear();

    const several = mountFk(owner([users, accounts], users), { onGenerateJoin });
    fireEvent.pointerDown(several.cell(0, 1), { button: 0 });
    openMenu(several.cell(0, 1));
    fireEvent.click(item(/Generate JOIN/));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByText("Generate JOIN with")).toBeInTheDocument();
    expect(menuItems(menu)).toEqual(["users (id)", "accounts (uid)"]);
    expect(onGenerateJoin).not.toHaveBeenCalled();
    fireEvent.click(within(menu).getByRole("menuitem", { name: "accounts (uid)" }));
    expect(onGenerateJoin).toHaveBeenCalledWith(
      expect.objectContaining({ id: "owner_id" }),
      accounts,
    );
  });

  it("Quick Look's Open button jumps through the column's own key, not whichever sorts first", async () => {
    // CodeMirror (Quick Look) measures text ranges; jsdom has no layout.
    Range.prototype.getClientRects ??= () => [] as unknown as DOMRectList;
    Range.prototype.getBoundingClientRect ??= () => new DOMRect();
    const onLoadReferenced = vi.fn().mockResolvedValue({
      table: "public.users",
      columns: [{ name: "id", dataType: "bigint" }],
      rows: [[{ t: "int", v: 7 }]],
    });
    // A composite key sorts first in `fks`; `fk` (what Quick Look shows) is the single one.
    const { onOpenFk, cell } = mountFk(owner([pair, users], users), { onLoadReferenced });
    fireEvent.pointerDown(cell(0, 1), { button: 1 });
    fireEvent.click(await screen.findByRole("button", { name: /Open public\.users/ }));
    expect(onOpenFk).toHaveBeenCalledOnce();
    expect(onOpenFk).toHaveBeenCalledWith(
      expect.objectContaining({ id: "owner_id" }),
      { t: "int", v: 7 },
      users,
    );
  });

  it("a composite key's member has an arrow; the jump gets the key and the row's cells", () => {
    const { onOpenFk, arrow, press } = mountFk(owner([pair]));
    // No single-column key (`fk`), but the arrow is there.
    expect(arrow(0, 1)).toBeInTheDocument();
    press(arrow(0, 1));
    expect(onOpenFk).toHaveBeenCalledOnce();
    expect(onOpenFk).toHaveBeenCalledWith(
      expect.objectContaining({ id: "owner_id" }),
      { t: "int", v: 7 },
      pair,
      data[0]!.cells,
    );
  });

  it("a composite key and a single one on the same column ask which", async () => {
    const { onOpenFk, arrow, press } = mountFk(owner([users, pair], users));
    press(arrow(0, 1));
    const menu = await screen.findByRole("menu");
    expect(menuItems(menu)).toEqual(["users (id)", "orders (a, b)"]);
    fireEvent.click(within(menu).getByRole("menuitem", { name: "orders (a, b)" }));
    expect(onOpenFk).toHaveBeenCalledWith(
      expect.anything(),
      { t: "int", v: 7 },
      pair,
      data[0]!.cells,
    );
  });

  it("a hand-made column with only `fk` still jumps", () => {
    const { onOpenFk, arrow, press } = mountFk([
      col("id", "int"),
      { ...col("owner_id", "int"), fk: users },
      col("name"),
    ]);
    press(arrow(0, 1));
    expect(onOpenFk).toHaveBeenCalledWith(expect.anything(), { t: "int", v: 7 });
  });
});

describe("DataGrid Export Result…", () => {
  const state: ColumnState = { widths: {}, order: [], hidden: [] };
  const exportItems = () =>
    screen.queryAllByRole("menuitem", { name: /^Export / }).map((i) => i.textContent?.trim());

  it("a result grid's cell menu has Export Result…, which calls its owner", async () => {
    const onExportResult = vi.fn();
    const { cell, click } = setup(state, { onExportResult });
    click(0, 1);
    openMenu(cell(0, 1));
    expect(exportItems()).toEqual(["Export Result…"]);
    fireEvent.click(item(/^Export Result/));
    await waitFor(() => expect(onExportResult).toHaveBeenCalledOnce());
  });

  it("is shown only when the owner passes it, and never beside Export Current Page…", () => {
    const { cell, click } = setup(state);
    click(0, 1);
    openMenu(cell(0, 1));
    expect(exportItems()).toEqual([]);
    cleanup();

    const page = setup(state, { onExportPage: vi.fn() });
    page.click(0, 1);
    openMenu(page.cell(0, 1));
    expect(exportItems()).toEqual(["Export Current Page…"]);
    cleanup();

    const both = setup(state, { onExportPage: vi.fn(), onExportResult: vi.fn() });
    both.click(0, 1);
    openMenu(both.cell(0, 1));
    expect(exportItems()).toEqual(["Export Result…"]);
  });
});

describe("DataGrid ⌘↑ up to the filter bar", () => {
  const state: ColumnState = { widths: {}, order: [], hidden: [] };
  const many: ViewRow[] = ["Ann", "Bob", "Cy", "Dee"].map((name, i) => ({
    key: `pk:${i + 1}`,
    cells: [
      { t: "int", v: i + 1 },
      { t: "text", v: name },
      { t: "text", v: `${name.toLowerCase()}@x.io` },
    ],
  }));
  const mountMany = (extra: Partial<DataGridProps> = {}) =>
    setup(state, { rowCount: many.length, getRow: (i) => many[i], ...extra });
  const focusOf = (onFocusCell: ReturnType<typeof vi.fn>) => onFocusCell.mock.calls.at(-1)![0].pos;

  it("⌘↑ on the first row hands the focus to the bar; elsewhere it jumps to the first row", () => {
    const onFocusFilters = vi.fn();
    const { grid, click, onFocusCell } = mountMany({ onFocusFilters });
    // Rows, but no focused cell yet: not "the first row".
    fireEvent.keyDown(grid, { key: "ArrowUp", ...mod });
    expect(onFocusFilters).not.toHaveBeenCalled();
    click(2, 1);
    fireEvent.keyDown(grid, { key: "ArrowUp", ...mod });
    expect(onFocusFilters).not.toHaveBeenCalled();
    expect(focusOf(onFocusCell)).toEqual({ r: 0, c: 1 });
    // Now on the first row: the next ⌘↑ leaves the grid.
    fireEvent.keyDown(grid, { key: "ArrowUp", ...mod });
    expect(onFocusFilters).toHaveBeenCalledOnce();
    // A plain ↑ and ⌘↓ never do.
    fireEvent.keyDown(grid, { key: "ArrowUp" });
    fireEvent.keyDown(grid, { key: "ArrowDown", ...mod });
    expect(onFocusFilters).toHaveBeenCalledOnce();
    expect(focusOf(onFocusCell)).toEqual({ r: 3, c: 1 });
  });

  it("without the bar (no prop), ⌘↑ always jumps to the first row", () => {
    const { grid, click, onFocusCell } = mountMany();
    click(0, 1);
    fireEvent.keyDown(grid, { key: "ArrowUp", ...mod });
    expect(focusOf(onFocusCell)).toEqual({ r: 0, c: 1 });
    click(3, 1);
    fireEvent.keyDown(grid, { key: "ArrowUp", ...mod });
    expect(focusOf(onFocusCell)).toEqual({ r: 0, c: 1 });
  });

  it("⇧⌘↑ extends the selection to the first row instead", () => {
    const onFocusFilters = vi.fn();
    const { grid, click } = mountMany({ onFocusFilters });
    click(0, 1);
    fireEvent.keyDown(grid, { key: "ArrowUp", ...mod, shiftKey: true });
    expect(onFocusFilters).not.toHaveBeenCalled();
  });

  it("with no rows ⌘↑ goes to the bar too", () => {
    const onFocusFilters = vi.fn();
    const { grid } = setup(state, { rowCount: 0, getRow: () => undefined, onFocusFilters });
    fireEvent.keyDown(grid, { key: "ArrowUp", ...mod });
    expect(onFocusFilters).toHaveBeenCalledOnce();
  });
});

describe("DataGrid paste of CSV text", () => {
  const state: ColumnState = { widths: {}, order: [], hidden: [] };
  const paste = (grid: HTMLElement, text: string) =>
    fireEvent.paste(grid, { clipboardData: { getData: () => text } });
  const text = (v: string): Value => ({ t: "text", v });

  it("two comma separated lines of one field per column insert one new row each", () => {
    const { grid, onAction } = setup(state, { columns: columns.slice(1) });
    paste(grid, "a,b\n1,2");
    expect(onAction).toHaveBeenCalledWith({
      type: "insertRows",
      rows: [
        [
          { c: 0, value: text("a") },
          { c: 1, value: text("b") },
        ],
        [
          { c: 0, value: text("1") },
          { c: 1, value: text("2") },
        ],
      ],
    });
    expect(toast.success).toHaveBeenCalledWith("Pasted 2 rows");
  });

  it("reads quotes, semicolons and the preference's delimiter", () => {
    const { grid, onAction } = setup(state, { columns: columns.slice(1) });
    paste(grid, '"Smith, John";x\r\n"say ""hi""";y\r\n');
    expect(onAction).toHaveBeenLastCalledWith({
      type: "insertRows",
      rows: [
        [
          { c: 0, value: text("Smith, John") },
          { c: 1, value: text("x") },
        ],
        [
          { c: 0, value: text('say "hi"') },
          { c: 1, value: text("y") },
        ],
      ],
    });
  });

  it("a block narrower than the grid fills cells from the focused one", () => {
    const { grid, click, onAction } = setup(state);
    click(0, 1);
    paste(grid, "P,p@x\nQ,q@x");
    expect(onAction).toHaveBeenLastCalledWith({
      type: "setValues",
      edits: [
        { r: 0, c: 1, value: text("P") },
        { r: 0, c: 2, value: text("p@x") },
        { r: 1, c: 1, value: text("Q") },
        { r: 1, c: 2, value: text("q@x") },
      ],
    });
  });

  it("one line with one field per column is a row; any other single line is a value", () => {
    const { grid, click, onAction } = setup(state);
    paste(grid, "3,Cy,cy@x.io");
    expect(onAction).toHaveBeenLastCalledWith({
      type: "insertRows",
      rows: [
        [
          { c: 0, value: { t: "int", v: 3 } },
          { c: 1, value: text("Cy") },
          { c: 2, value: text("cy@x.io") },
        ],
      ],
    });
    onAction.mockClear();
    // A lone 'Smith, John' stays one value in the focused cell.
    click(1, 1);
    paste(grid, "Smith, John");
    expect(onAction).toHaveBeenLastCalledWith({
      type: "setValues",
      edits: [{ r: 1, c: 1, value: text("Smith, John") }],
    });
    // Even in a two-column grid, where its two fields would fit.
    cleanup();
    const two = setup(state, { columns: columns.slice(1) });
    two.click(0, 0);
    paste(two.grid, "Smith, John");
    expect(two.onAction).toHaveBeenLastCalledWith({
      type: "setValues",
      edits: [{ r: 0, c: 0, value: text("Smith, John") }],
    });
  });

  it("tab separated text is parsed as before", () => {
    const { grid, click, onAction } = setup(state);
    click(0, 1);
    paste(grid, "P\tp@x\nQ\tq,x");
    expect(onAction).toHaveBeenLastCalledWith({
      type: "setValues",
      edits: [
        { r: 0, c: 1, value: text("P") },
        { r: 0, c: 2, value: text("p@x") },
        { r: 1, c: 1, value: text("Q") },
        { r: 1, c: 2, value: text("q,x") },
      ],
    });
  });

  it("text without a delimiter pastes into the cell as ever", () => {
    const { grid, click, onAction } = setup(state);
    click(0, 1);
    paste(grid, "Zed");
    expect(onAction).toHaveBeenLastCalledWith({
      type: "setValues",
      edits: [{ r: 0, c: 1, value: text("Zed") }],
    });
  });
});

describe("DataGrid missing fields", () => {
  // `Other{missing}`: a document without the field, or a PostgreSQL value of a type so named.
  const other: Value = { t: "other", v: { ty: "missing", text: "" } };
  const typed: Value = { t: "other", v: { ty: "missing", text: "(1,2)" } };
  const page: ViewRow[] = [{ key: "pk:1", cells: [{ t: "int", v: 1 }, other, typed] }];
  const state: ColumnState = { widths: {}, order: [], hidden: [] };
  const grid = () => (
    <DataGrid
      columns={[col("id", "int"), col("status", "other"), col("shape", "other")]}
      rowCount={1}
      getRow={(i) => page[i]}
      columnState={state}
    />
  );
  const cell = (c: number) =>
    document.querySelector<HTMLElement>(`[data-cell][data-r="0"][data-c="${c}"]`)!;

  it("shows a missing field apart on a session whose rows can lack one", () => {
    render(<LacksFieldsContext.Provider value>{grid()}</LacksFieldsContext.Provider>);
    expect(cell(1)).toHaveTextContent(MISSING_PLACEHOLDER);
    expect(cell(2)).toHaveTextContent(MISSING_PLACEHOLDER);
  });

  it("shows a value of a type named missing as itself on a core session", () => {
    render(grid());
    expect(cell(1)).toHaveTextContent(EMPTY_PLACEHOLDER);
    expect(cell(2)).toHaveTextContent("(1,2)");
  });
});
