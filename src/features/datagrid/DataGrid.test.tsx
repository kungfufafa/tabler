import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import type { Value } from "@/lib/ipc";
import { isMac } from "@/lib/keys/combo";
import { DataGrid, type DataGridProps } from "./DataGrid";
import type { ColumnState, GridColumn, ViewRow } from "./types";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

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
});

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
    fireEvent.keyDown(grid, { key: "Z" });
    const input = screen.getByRole("textbox", { name: "Edit name" });
    expect((input as HTMLInputElement).value).toBe("Z");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onEditCommit).toHaveBeenCalledWith(0, 1, { t: "text", v: "Z" });

    click(0, 0);
    fireEvent.pointerDown(document.querySelector('[data-cell][data-r="1"][data-c="1"]')!, {
      button: 0,
      shiftKey: true,
    });
    await act(() => actions.run("grid.set-null"));
    expect(onAction).toHaveBeenLastCalledWith({
      type: "setNull",
      cells: [
        { r: 0, c: 2 },
        { r: 0, c: 1 },
        { r: 1, c: 2 },
        { r: 1, c: 1 },
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
    return { grid, click, copy, paste, onAction, writeText };
  }

  it("round-trips ⌘C / ⌘V of a NULL into NULL and of an empty string into ''", async () => {
    const { click, copy, paste, onAction, writeText } = mount();
    click(0, 1);
    await copy();
    expect(writeText).toHaveBeenLastCalledWith("");
    click(1, 1);
    await paste();
    expect(onAction).toHaveBeenLastCalledWith({
      type: "setValues",
      edits: [{ r: 1, c: 1, value: { t: "null" } }],
    });

    click(0, 2);
    await copy();
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
    expect(screen.getByRole("menuitem", { name: /Remove from Sort/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /Reset Column Order/ })).toBeInTheDocument();
    expect(misaligned()).toEqual([]);
  });

  it("aligns the cell menu labels", () => {
    const { cell, click } = setup({ widths: {}, order: [], hidden: [] });
    click(0, 1);
    fireEvent.pointerDown(cell(0, 1), { button: 2 });
    fireEvent.contextMenu(cell(0, 1));
    expect(screen.getByRole("menuitem", { name: /Copy as/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /Set NULL/ })).toBeInTheDocument();
    expect(misaligned()).toEqual([]);
  });
});
