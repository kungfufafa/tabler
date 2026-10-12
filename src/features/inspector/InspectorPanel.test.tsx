import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GridColumn } from "@/features/datagrid/types";
import { actions } from "@/features/palette/actions";
import { ipc, type Value } from "@/lib/ipc";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import { useInspectorStore, type InspectedCell, type InspectedRow } from "@/stores/inspector";
import { useUiStore } from "@/stores/ui";
import { InspectorPanel } from "./InspectorPanel";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), message: vi.fn() } }));

const TYPES: Partial<Record<GridColumn["kind"], string>> = {
  int: "int8",
  text: "text",
  bytes: "bytea",
  json: "jsonb",
  bool: "bool",
};

const col = (
  id: string,
  kind: GridColumn["kind"],
  extra: Partial<GridColumn> = {},
): GridColumn => ({
  id,
  name: id,
  kind,
  dataType: TYPES[kind] ?? kind,
  nullable: true,
  isPk: id === "id",
  hasDefault: false,
  isAutoInc: false,
  editable: true,
  align: "left",
  ...extra,
});
const columns = [col("id", "int"), col("bio", "text"), col("avatar", "bytes"), col("note", "text")];
const preview: Value = { t: "bytes", v: { base64: btoa("Hel"), len: 70_000, truncated: true } };
const cells: Value[] = [
  { t: "int", v: 1 },
  { t: "text", v: "line 1\nline 2" },
  preview,
  { t: "null" },
];

function show(cell: Partial<InspectedCell> & { focused: number }) {
  const { focused, ...rest } = cell;
  act(() =>
    useInspectorStore.setState({
      cell: {
        column: columns[focused]!,
        value: cells[focused]!,
        rowIndex: 0,
        source: "users",
        row: { columns, cells, focused, hidden: ["note"] },
        fetch: {
          connectionId: "c1",
          table: { schema: "public", name: "users" },
          key: { kind: "pk", value: { id: { t: "int", v: 1 } } },
          column: columns[focused]!.name,
        },
        ...rest,
      },
    }),
  );
}

const cellMode = () => act(() => useInspectorStore.setState({ mode: "cell" }));

beforeEach(() => {
  useInspectorStore.setState({ cell: null, mode: "row" });
});

afterEach(() => {
  vi.restoreAllMocks();
  useInspectorStore.setState({ cell: null, mode: "row" });
});

describe("InspectorPanel", () => {
  it("says so when nothing is selected", () => {
    render(<InspectorPanel />);
    expect(screen.getByText("No row selected")).toBeInTheDocument();
  });

  it("opens on Details, switches to Cell and closes", () => {
    act(() => useUiStore.setState({ inspectorOpen: true }));
    render(<InspectorPanel />);
    show({ focused: 1 });
    const details = screen.getByRole("radio", { name: "Details" });
    expect(details).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("list", { name: "Row fields" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("radio", { name: "Cell" }));
    expect(useInspectorStore.getState().mode).toBe("cell");
    expect(screen.queryByRole("list", { name: "Row fields" })).not.toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("radio", { name: "Cell" }), { key: "ArrowLeft" });
    expect(useInspectorStore.getState().mode).toBe("row");

    fireEvent.click(screen.getByRole("button", { name: "Close row details" }));
    expect(useUiStore.getState().inspectorOpen).toBe(false);
  });

  it("shows text stats in the Cell view", () => {
    cellMode();
    render(<InspectorPanel />);
    show({ focused: 1 });
    expect(screen.getByText("· 13 chars · 2 lines")).toBeInTheDocument();
  });

  it("loads the full blob on demand", async () => {
    const full = new Uint8Array(70_000).fill(65);
    let binary = "";
    for (const b of full) binary += String.fromCharCode(b);
    const cell = vi.spyOn(ipc.browse, "cell").mockResolvedValue({
      t: "bytes",
      v: { base64: btoa(binary), len: 70_000, truncated: false },
    });
    cellMode();
    render(<InspectorPanel />);
    show({ focused: 2 });
    expect(screen.getByText(/\(preview\)/)).toBeInTheDocument();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: /Load full value/ })));
    expect(cell).toHaveBeenCalledWith(
      "c1",
      { schema: "public", name: "users" },
      {
        kind: "pk",
        value: { id: { t: "int", v: 1 } },
      },
      "avatar",
    );
    expect(screen.queryByRole("button", { name: /Load full value/ })).not.toBeInTheDocument();
    expect(screen.queryByText(/\(preview\)/)).not.toBeInTheDocument();
    expect(screen.getByText(/Showing the first 4\.0 KB of 68\.4 KB/)).toBeInTheDocument();
  });

  it("never presents a still-truncated result as the full value", async () => {
    vi.spyOn(ipc.browse, "cell").mockResolvedValue(preview);
    cellMode();
    render(<InspectorPanel />);
    show({ focused: 2 });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: /Load full value/ })));
    expect(screen.getByText(/Only a preview of this value could be loaded/)).toBeInTheDocument();
    expect(screen.getByText(/\(preview\)/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Load full value/ })).toBeInTheDocument();
  });

  it("shows and copies JSON exactly as stored (no JSON.parse round trip)", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>().mockResolvedValue();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText, readText: vi.fn() },
    });
    const raw = '{"id": 9007199254740993, "p": 1.10, "a": 1, "a": 2, "u": "\\u00e9"}';
    const json: Value = { t: "json", v: raw };
    const doc = col("doc", "json");
    const pretty =
      '{\n  "id": 9007199254740993,\n  "p": 1.10,\n  "a": 1,\n  "a": 2,\n  "u": "\\u00e9"\n}';
    const isPretty = (_: string, el: Element | null) =>
      el?.tagName === "PRE" && el.textContent === pretty;

    cellMode();
    render(<InspectorPanel />);
    show({
      focused: 0,
      column: doc,
      value: json,
      row: { columns: [doc], cells: [json], focused: 0 },
    });
    expect(screen.getByText(isPretty)).toBeInTheDocument();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Copy value" })));
    expect(writeText).toHaveBeenLastCalledWith(raw);

    fireEvent.click(screen.getByRole("radio", { name: "Details" }));
    const box = screen.getByRole("textbox", { name: "doc" });
    expect(box).toHaveValue(pretty);
    expect(box).toHaveAttribute("readonly");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Copy doc" })));
    expect(writeText).toHaveBeenLastCalledWith(raw);
  });

  it("offers no full load without a fetch target", () => {
    cellMode();
    render(<InspectorPanel />);
    show({ focused: 2, fetch: null });
    expect(screen.queryByRole("button", { name: /Load full value/ })).not.toBeInTheDocument();
  });

  it("lists the whole row in Details: names, types, placeholders, hidden, focus, search", () => {
    const scrolled = vi.fn();
    Element.prototype.scrollIntoView = scrolled;
    render(<InspectorPanel />);
    show({ focused: 1 });
    const list = screen.getByRole("list", { name: "Row fields" });
    const items = within(list).getAllByRole("listitem");
    expect(items).toHaveLength(4);
    expect(items[1]).toHaveClass("bg-brand-soft");
    expect(scrolled.mock.contexts[0]).toBe(items[1]);
    expect(within(items[0]!).getByText("int8")).toBeInTheDocument();
    expect(within(items[0]!).getByRole("img", { name: "Primary key" })).toBeInTheDocument();
    expect(within(items[1]!).queryByRole("img", { name: "Primary key" })).not.toBeInTheDocument();
    expect(within(items[3]!).getByText("hidden")).toBeInTheDocument();
    expect(within(items[3]!).getByRole("textbox", { name: "note" })).toHaveAttribute(
      "placeholder",
      "NULL",
    );
    expect(within(items[1]!).getByRole("textbox", { name: "bio" })).toHaveValue("line 1\nline 2");
    // A blob is summarised; its bytes are in the Cell view.
    expect(within(items[2]!).getByText(/68\.4 KB binary \(preview\)/)).toBeInTheDocument();

    fireEvent.change(screen.getByRole("searchbox", { name: "Search for field" }), {
      target: { value: "BI" },
    });
    const filtered = within(list).getAllByRole("listitem");
    expect(filtered).toHaveLength(1);
    expect(within(filtered[0]!).getByRole("textbox", { name: "bio" })).toBeInTheDocument();
    fireEvent.change(screen.getByRole("searchbox", { name: "Search for field" }), {
      target: { value: "zzz" },
    });
    expect(screen.getByText("No matching fields")).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("searchbox", { name: "Search for field" }), {
      key: "Escape",
    });
    expect(within(list).getAllByRole("listitem")).toHaveLength(4);
  });

  it("shows a blob of another field in the Cell view", () => {
    vi.spyOn(ipc.browse, "cell").mockResolvedValue(preview);
    render(<InspectorPanel />);
    show({ focused: 1 });
    fireEvent.click(screen.getByRole("button", { name: "Show avatar in the Cell view" }));
    expect(useInspectorStore.getState().mode).toBe("cell");
    expect(screen.getByText("bytea")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Load full value/ }));
    expect(ipc.browse.cell).toHaveBeenCalledWith(
      "c1",
      expect.anything(),
      expect.anything(),
      "avatar",
    );
    // Back on Details, the grid's focused field is still the highlighted one.
    fireEvent.click(screen.getByRole("radio", { name: "Details" }));
    const items = within(screen.getByRole("list", { name: "Row fields" })).getAllByRole("listitem");
    expect(items[1]).toHaveClass("bg-brand-soft");
  });
});

describe("InspectorPanel editing (Details)", () => {
  const ecols = [
    col("id", "int", { nullable: false, hasDefault: true, isAutoInc: true }),
    col("name", "text", { nullable: false }),
    col("age", "int"),
    col("active", "bool", { nullable: false }),
    col("bio", "text", { hasDefault: true }),
    col("doc", "json"),
    col("avatar", "bytes"),
    col("slug", "text", { editable: false }),
  ];
  const ecells: Value[] = [
    { t: "int", v: 1 },
    { t: "text", v: "Eko" },
    { t: "null" },
    { t: "bool", v: true },
    { t: "text", v: "line 1\nline 2" },
    { t: "json", v: '{"a":1}' },
    preview,
    { t: "text", v: "" },
  ];
  const idx = (name: string) => ecols.findIndex((c) => c.name === name);

  function showRow(opts: Partial<InspectedRow> & { rowIndex?: number } = {}) {
    const { rowIndex = 0, ...row } = opts;
    const focused = row.focused ?? 1;
    const rowCells = row.cells ?? ecells;
    act(() =>
      useInspectorStore.setState({
        cell: {
          column: ecols[focused]!,
          value: rowCells[focused]!,
          rowIndex,
          source: "users",
          row: { columns: ecols, cells: rowCells, focused, ...row },
          fetch: null,
        },
      }),
    );
  }
  const box = (name: string) => screen.getByRole("textbox", { name });

  it("bulk edit: N rows selected, Multiple values, and an edit for all of them", () => {
    const edit = vi.fn();
    render(<InspectorPanel />);
    showRow({
      edit,
      bulk: { count: 3, mixed: new Set([idx("name"), idx("active")]), id: "a|b|c" },
    });
    expect(screen.getByRole("status")).toHaveTextContent("3 rows selected");
    expect(box("name")).toHaveValue("");
    expect(box("name")).toHaveAttribute("placeholder", "Multiple values");
    // A field the rows agree on shows its value.
    expect(box("bio")).toHaveValue("line 1\nline 2");
    expect(screen.getByRole("combobox", { name: "active" })).toHaveDisplayValue("Multiple values");
    fireEvent.change(box("name"), { target: { value: "Same" } });
    fireEvent.keyDown(box("name"), { key: "Enter", code: "Enter" });
    expect(edit).toHaveBeenCalledWith(idx("name"), { t: "text", v: "Same" });
  });

  it("each field's menu offers NULL / EMPTY / DEFAULT, and NOW() for dates", async () => {
    const edit = vi.fn();
    const cols = [...ecols, col("born", "date")];
    const cells = [...ecells, { t: "null" } as Value];
    render(<InspectorPanel />);
    act(() =>
      useInspectorStore.setState({
        cell: {
          column: cols[1]!,
          value: cells[1]!,
          rowIndex: 0,
          source: "users",
          row: {
            columns: cols,
            cells,
            focused: 1,
            edit,
            bulk: { count: 2, mixed: new Set([cols.length - 1]), id: "x" },
          },
          fetch: null,
        },
      }),
    );
    fireEvent.pointerDown(screen.getByRole("button", { name: "born options" }), {
      button: 0,
      ctrlKey: false,
    });
    const menu = await screen.findByRole("menu");
    // Mixed values: Set NULL applies even though the first row is NULL.
    expect(within(menu).getByRole("menuitem", { name: "Set NULL" })).not.toHaveAttribute(
      "data-disabled",
    );
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Set Now (local time)" }));
    expect(edit).toHaveBeenCalledWith(cols.length - 1, {
      t: "date",
      v: expect.stringMatching(/^\d{4}-\d{2}-\d{2}$/),
    });
  });

  it("stages a single-line field on Enter, as a value of the column's kind", () => {
    const edit = vi.fn();
    render(<InspectorPanel />);
    showRow({ edit });
    fireEvent.change(box("name"), { target: { value: "Budi" } });
    expect(screen.getByText(/to stage/)).toBeInTheDocument();
    fireEvent.keyDown(box("name"), { key: "Enter", code: "Enter" });
    expect(edit).toHaveBeenCalledWith(idx("name"), { t: "text", v: "Budi" });

    fireEvent.change(box("id"), { target: { value: "9007199254740993" } });
    fireEvent.keyDown(box("id"), { key: "Enter", code: "Enter" });
    expect(edit).toHaveBeenLastCalledWith(idx("id"), { t: "bigInt", v: "9007199254740993" });
  });

  it("rejects invalid text with an inline error; Escape reverts", () => {
    const edit = vi.fn();
    render(<InspectorPanel />);
    showRow({ edit });
    expect(box("age")).toHaveAttribute("placeholder", "NULL");
    fireEvent.change(box("age"), { target: { value: "abc" } });
    fireEvent.keyDown(box("age"), { key: "Enter", code: "Enter" });
    expect(edit).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent("Enter a whole number");
    expect(box("age")).toHaveAttribute("aria-invalid", "true");
    // Blur keeps the draft and its error instead of staging it.
    fireEvent.blur(box("age"));
    expect(edit).not.toHaveBeenCalled();
    expect(box("age")).toHaveValue("abc");

    fireEvent.keyDown(box("age"), { key: "Escape", code: "Escape" });
    expect(box("age")).toHaveValue("");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    fireEvent.change(box("age"), { target: { value: " 42 " } });
    fireEvent.blur(box("age"));
    expect(edit).toHaveBeenCalledWith(idx("age"), { t: "int", v: 42 });
  });

  it("a cleared nullable number means NULL; a cleared non-null one is an error", () => {
    const edit = vi.fn();
    const rowCells = [...ecells];
    rowCells[idx("age")] = { t: "int", v: 30 };
    render(<InspectorPanel />);
    showRow({ edit, cells: rowCells });
    fireEvent.change(box("age"), { target: { value: "" } });
    expect(box("age")).toHaveAttribute("placeholder", "NULL");
    fireEvent.keyDown(box("age"), { key: "Enter", code: "Enter" });
    expect(edit).toHaveBeenCalledWith(idx("age"), { t: "null" });

    fireEvent.change(box("id"), { target: { value: "" } });
    fireEvent.keyDown(box("id"), { key: "Enter", code: "Enter" });
    expect(edit).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("alert")).toHaveTextContent("Enter a whole number");
  });

  it("multi-line and JSON fields: ↩ adds a line, ⌘↩ stages; invalid JSON is not staged", () => {
    const edit = vi.fn();
    render(<InspectorPanel />);
    showRow({ edit });
    fireEvent.change(box("bio"), { target: { value: "line 1\nline 2\nline 3" } });
    expect(fireEvent.keyDown(box("bio"), { key: "Enter", code: "Enter" })).toBe(true);
    expect(edit).not.toHaveBeenCalled();
    fireEvent.keyDown(box("bio"), { key: "Enter", code: "Enter", ctrlKey: true });
    expect(edit).toHaveBeenCalledWith(idx("bio"), { t: "text", v: "line 1\nline 2\nline 3" });

    expect(box("doc")).toHaveValue('{\n  "a": 1\n}');
    fireEvent.change(box("doc"), { target: { value: '{"a": ' } });
    fireEvent.keyDown(box("doc"), { key: "Enter", code: "Enter", ctrlKey: true });
    expect(edit).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("alert")).toHaveTextContent(/Invalid JSON/);
    fireEvent.change(box("doc"), { target: { value: '{"a": 2}' } });
    fireEvent.keyDown(box("doc"), { key: "Enter", code: "Enter", ctrlKey: true });
    expect(edit).toHaveBeenLastCalledWith(idx("doc"), { t: "json", v: '{"a": 2}' });
  });

  it("⌥↩ adds a line break to a one-line text field, which then needs ⌘↩", () => {
    const edit = vi.fn();
    render(<InspectorPanel />);
    showRow({ edit });
    const name = box("name") as HTMLTextAreaElement;
    name.setSelectionRange(3, 3);
    fireEvent.keyDown(name, { key: "Enter", code: "Enter", altKey: true });
    expect(name).toHaveValue("Eko\n");
    fireEvent.keyDown(name, { key: "Enter", code: "Enter" });
    expect(edit).not.toHaveBeenCalled();
    fireEvent.keyDown(name, { key: "Enter", code: "Enter", ctrlKey: true });
    expect(edit).toHaveBeenCalledWith(idx("name"), { t: "text", v: "Eko\n" });
  });

  it("does not stage an unchanged field", () => {
    const edit = vi.fn();
    render(<InspectorPanel />);
    showRow({ edit });
    fireEvent.change(box("name"), { target: { value: "Ek" } });
    fireEvent.change(box("name"), { target: { value: "Eko" } });
    fireEvent.keyDown(box("name"), { key: "Enter", code: "Enter" });
    fireEvent.change(box("doc"), { target: { value: '{\n  "a": 1\n}' } });
    fireEvent.blur(box("doc"));
    expect(edit).not.toHaveBeenCalled();
  });

  it("picks bool values from a list", () => {
    const edit = vi.fn();
    render(<InspectorPanel />);
    showRow({ edit });
    const active = screen.getByRole("combobox", { name: "active" });
    expect(
      within(active)
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual(["true", "false"]);
    fireEvent.change(active, { target: { value: "false" } });
    expect(edit).toHaveBeenCalledWith(idx("active"), { t: "bool", v: false });
  });

  it("offers Set NULL / EMPTY / DEFAULT by column in the field menu", async () => {
    const edit = vi.fn();
    render(<InspectorPanel />);
    showRow({ edit });
    const open = async (name: string) => {
      fireEvent.keyDown(screen.getByRole("button", { name: `${name} options` }), {
        key: "Enter",
      });
      return screen.findByRole("menu");
    };
    let menu = await open("bio");
    expect(within(menu).getByRole("menuitem", { name: "Set NULL" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Set DEFAULT" })).toBeInTheDocument();
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Set EMPTY" }));
    expect(edit).toHaveBeenLastCalledWith(idx("bio"), { t: "text", v: "" });
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());

    menu = await open("name");
    expect(within(menu).queryByRole("menuitem", { name: "Set NULL" })).not.toBeInTheDocument();
    expect(within(menu).queryByRole("menuitem", { name: "Set DEFAULT" })).not.toBeInTheDocument();
    fireEvent.keyDown(menu, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());

    menu = await open("age");
    expect(within(menu).queryByRole("menuitem", { name: "Set EMPTY" })).not.toBeInTheDocument();
    // Already NULL.
    expect(within(menu).getByRole("menuitem", { name: "Set NULL" })).toHaveAttribute(
      "data-disabled",
    );
    fireEvent.keyDown(menu, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());

    menu = await open("id");
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Set DEFAULT" }));
    expect(edit).toHaveBeenLastCalledWith(idx("id"), { t: "default" });
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());

    // A blob cannot be typed, but can be set to NULL.
    menu = await open("avatar");
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Set NULL" }));
    expect(edit).toHaveBeenLastCalledWith(idx("avatar"), { t: "null" });
  });

  it("is read-only for generated columns, deleted rows and rows without `edit`", () => {
    const edit = vi.fn();
    render(<InspectorPanel />);
    showRow({ edit });
    expect(box("slug")).toHaveAttribute("readonly");
    expect(box("slug")).toHaveAttribute("placeholder", "EMPTY");
    expect(screen.getByRole("button", { name: "Copy slug" })).toBeInTheDocument();
    expect(box("name")).not.toHaveAttribute("readonly");

    showRow({ edit, status: "deleted" });
    expect(box("name")).toHaveAttribute("readonly");
    expect(screen.queryByRole("combobox", { name: "active" })).not.toBeInTheDocument();
    expect(box("active")).toHaveValue("true");

    showRow({});
    expect(box("name")).toHaveAttribute("readonly");
    expect(screen.queryByRole("button", { name: "name options" })).not.toBeInTheDocument();
    fireEvent.change(box("name"), { target: { value: "x" } });
    fireEvent.blur(box("name"));
    expect(edit).not.toHaveBeenCalled();
  });

  it("highlights staged values and shows the re-published value", () => {
    const edit = vi.fn();
    render(<InspectorPanel />);
    showRow({ edit });
    fireEvent.change(box("name"), { target: { value: "Budi" } });
    fireEvent.keyDown(box("name"), { key: "Enter", code: "Enter" });
    const staged = [...ecells];
    staged[idx("name")] = { t: "text", v: "Budi" };
    showRow({ edit, cells: staged, changed: new Set([idx("name")]) });
    expect(box("name")).toHaveValue("Budi");
    expect(box("name")).toHaveClass("bg-changed");
    expect(box("bio")).not.toHaveClass("bg-changed");
  });

  it("stages a draft before a click elsewhere selects another row", () => {
    const edit = vi.fn();
    render(
      <>
        <InspectorPanel />
        <div data-testid="grid" />
      </>,
    );
    showRow({ edit });
    fireEvent.change(box("name"), { target: { value: "Budi" } });
    const grid = screen.getByTestId("grid");
    grid.addEventListener("pointerdown", () => {
      // The grid re-targets the panel on pointerdown: the draft must already be staged.
      expect(edit).toHaveBeenCalledWith(idx("name"), { t: "text", v: "Budi" });
      const other = [...ecells];
      other[idx("name")] = { t: "text", v: "Eko" };
      useInspectorStore.setState({
        cell: {
          ...useInspectorStore.getState().cell!,
          rowIndex: 1,
          row: { columns: ecols, cells: other, focused: 1, edit },
        },
      });
    });
    act(() => {
      fireEvent.pointerDown(grid);
    });
    expect(edit).toHaveBeenCalledTimes(1);
    // The other row shows its own value; leaving the field stages nothing into it.
    expect(box("name")).toHaveValue("Eko");
    fireEvent.blur(box("name"));
    expect(edit).toHaveBeenCalledTimes(1);
  });

  it("drops a draft when another row is published without a click", () => {
    const edit = vi.fn();
    render(<InspectorPanel />);
    showRow({ edit });
    fireEvent.change(box("age"), { target: { value: "7" } });
    showRow({ edit, rowIndex: 1 });
    expect(box("age")).toHaveValue("");
    fireEvent.blur(box("age"));
    expect(edit).not.toHaveBeenCalled();
  });

  it("⌘S in a field stages it first, then commits the tab once", async () => {
    const order: string[] = [];
    const edit = vi.fn(() => order.push("edit"));
    const commit = vi.fn(() => {
      order.push("commit");
    });
    const off = actions.register("grid.commit", commit);
    try {
      render(
        <KeyboardProvider>
          <InspectorPanel />
        </KeyboardProvider>,
      );
      showRow({ edit });
      fireEvent.change(box("name"), { target: { value: "Budi" } });
      const notPrevented = fireEvent.keyDown(box("name"), {
        key: "s",
        code: "KeyS",
        ctrlKey: true,
      });
      expect(notPrevented).toBe(false);
      await waitFor(() => expect(commit).toHaveBeenCalledTimes(1));
      expect(order).toEqual(["edit", "commit"]);
      expect(edit).toHaveBeenCalledWith(idx("name"), { t: "text", v: "Budi" });

      // An invalid draft blocks the commit.
      fireEvent.change(box("age"), { target: { value: "x" } });
      fireEvent.keyDown(box("age"), { key: "s", code: "KeyS", ctrlKey: true });
      await new Promise((r) => setTimeout(r, 5));
      expect(commit).toHaveBeenCalledTimes(1);
    } finally {
      off();
    }
  });

  it("Escape in an unchanged field goes back to the grid", () => {
    render(
      <>
        <InspectorPanel />
        <div data-scope="grid" tabIndex={0} data-testid="grid" />
      </>,
    );
    showRow({ edit: vi.fn() });
    box("name").focus();
    fireEvent.keyDown(box("name"), { key: "Escape", code: "Escape" });
    expect(screen.getByTestId("grid")).toHaveFocus();
  });
});
