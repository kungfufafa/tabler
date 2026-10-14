import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { EditorSelection, EditorState } from "@codemirror/state";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { sortKeysStorageKey } from "@/features/datagrid/quicklook/jsonPrefs";
import type { GridColumn } from "@/features/datagrid/types";
import { actions } from "@/features/palette/actions";
import type { Value } from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import { cmContent, cmDoc, cmType, cmView, polyfillLayout } from "./cmTestUtils";
import { FieldBlock } from "./FieldBlock";

beforeAll(polyfillLayout);
afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.restoreAllMocks();
});

const col = (kind: GridColumn["kind"], extra: Partial<GridColumn> = {}): GridColumn => ({
  id: "doc",
  name: "doc",
  kind,
  dataType: kind === "json" ? "jsonb" : kind,
  nullable: true,
  isPk: false,
  hasDefault: false,
  isAutoInc: false,
  editable: true,
  align: "left",
  ...extra,
});

function mount(column: GridColumn, value: Value, onEdit?: (v: Value) => void) {
  render(
    <KeyboardProvider>
      <FieldBlock
        col={column}
        value={value}
        rowKey="pk:1"
        focused={false}
        hidden={false}
        changed={false}
        onEdit={onEdit}
        onViewCell={vi.fn()}
      />
    </KeyboardProvider>,
  );
  return screen.getByRole("textbox", { name: "doc" });
}

async function openMenu() {
  const trigger = screen.getByRole("button", { name: "doc options" });
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
  return screen.findByRole("menu");
}

describe("FieldBlock JSON", () => {
  const json: Value = { t: "json", v: '{"a":1,"b":[true,null]}' };
  const pretty = '{\n  "a": 1,\n  "b": [\n    true,\n    null\n  ]\n}';

  it("is a CodeMirror editor, coloured, showing the JSON indented", () => {
    const box = mount(col("json"), json, vi.fn());
    expect(box.closest(".cm-editor")).not.toBeNull();
    expect(screen.getByTestId("quick-look-code")).toBeInTheDocument();
    expect(cmView("doc").state.doc.toString()).toBe(pretty);
    // Syntax colours (the same highlighter as Quick Look's): spans with the colour classes.
    expect(box.querySelector("span[class]")).not.toBeNull();
    expect(document.querySelector("textarea")).toBeNull();
  });

  it("types and stages like the native boxes: ⌘↩ and blur stage, an error shows and blocks", () => {
    const onEdit = vi.fn();
    const box = mount(col("json"), json, onEdit);
    cmType("doc", '{"a": 2}');
    expect(screen.getByText(/to stage/)).toBeInTheDocument();
    // A plain ↩ is a bare line break (at the caret, no auto-indent), not a stage.
    fireEvent.keyDown(box, { key: "Enter", code: "Enter" });
    expect(onEdit).not.toHaveBeenCalled();
    expect(cmView("doc").state.doc.toString()).toBe('\n{"a": 2}');
    cmType("doc", '{"a": 2}');
    fireEvent.keyDown(box, { key: "Enter", code: "Enter", ctrlKey: true });
    expect(onEdit).toHaveBeenLastCalledWith({ t: "json", v: '{"a": 2}' });

    cmType("doc", '{"a": ');
    fireEvent.keyDown(box, { key: "Enter", code: "Enter", ctrlKey: true });
    expect(onEdit).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("alert")).toHaveTextContent(/Invalid JSON/);
    expect(box).toHaveAttribute("aria-invalid", "true");
    // Blur keeps the broken draft and its error instead of staging it.
    fireEvent.blur(box);
    expect(onEdit).toHaveBeenCalledTimes(1);
    expect(cmView("doc").state.doc.toString()).toBe('{"a": ');

    cmType("doc", '{"a": 3}');
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    fireEvent.blur(box);
    expect(onEdit).toHaveBeenLastCalledWith({ t: "json", v: '{"a": 3}' });
  });

  it("Escape reverts an edit, and goes back to the grid when there is none", () => {
    render(
      <>
        <FieldBlock
          col={col("json")}
          value={json}
          rowKey="pk:1"
          focused={false}
          hidden={false}
          changed={false}
          onEdit={vi.fn()}
          onViewCell={vi.fn()}
        />
        <div data-scope="grid" tabIndex={0} data-testid="grid" />
      </>,
    );
    const box = screen.getByRole("textbox", { name: "doc" });
    cmType("doc", '{"a": 9}');
    fireEvent.keyDown(box, { key: "Escape", code: "Escape" });
    expect(cmView("doc").state.doc.toString()).toBe(pretty);
    expect(screen.queryByText(/to stage/)).not.toBeInTheDocument();
    fireEvent.keyDown(box, { key: "Escape", code: "Escape" });
    expect(screen.getByTestId("grid")).toHaveFocus();
  });

  it("does not stage an unchanged value, nor an edit it only echoes", () => {
    const onEdit = vi.fn();
    const box = mount(col("json"), json, onEdit);
    cmType("doc", '{"a": 1}');
    cmType("doc", pretty);
    fireEvent.blur(box);
    expect(onEdit).not.toHaveBeenCalled();
    // Leaving the field with nothing typed stages nothing either.
    fireEvent.blur(box);
    expect(onEdit).not.toHaveBeenCalled();
  });

  it("⌘S stages the field first, then commits the tab once; a broken draft blocks the commit", async () => {
    const order: string[] = [];
    const onEdit = vi.fn(() => order.push("edit"));
    const commit = vi.fn(() => void order.push("commit"));
    const off = actions.register("grid.commit", commit);
    const querySave = vi.fn();
    const offSave = actions.register("query.save", querySave);
    try {
      const box = mount(col("json"), json, onEdit);
      cmType("doc", '{"a": 5}');
      const notPrevented = fireEvent.keyDown(box, { key: "s", code: "KeyS", ctrlKey: true });
      expect(notPrevented).toBe(false);
      await waitFor(() => expect(commit).toHaveBeenCalledTimes(1));
      expect(order).toEqual(["edit", "commit"]);
      expect(onEdit).toHaveBeenCalledWith({ t: "json", v: '{"a": 5}' });
      // The field is not the SQL editor: ⌘S never saves a query file from it.
      expect(querySave).not.toHaveBeenCalled();

      cmType("doc", "{oops");
      fireEvent.keyDown(box, { key: "s", code: "KeyS", ctrlKey: true });
      await new Promise((r) => setTimeout(r, 5));
      expect(commit).toHaveBeenCalledTimes(1);
      expect(screen.getByRole("alert")).toBeInTheDocument();
    } finally {
      off();
      offSave();
    }
  });

  it("⌘S in the Edit JSON… dialog stages the text, then commits the tab; invalid JSON does neither", async () => {
    const onEdit = vi.fn();
    const commit = vi.fn();
    const off = actions.register("grid.commit", commit);
    try {
      mount(col("json"), json, onEdit);
      await openMenu();
      fireEvent.click(screen.getByRole("menuitem", { name: "Edit JSON…" }));
      const dialog = await screen.findByRole("dialog");
      const area = within(dialog).getByRole("textbox");
      fireEvent.change(area, { target: { value: '{"a": ' } });
      fireEvent.keyDown(area, { key: "s", code: "KeyS", ctrlKey: true });
      await new Promise((r) => setTimeout(r, 5));
      expect(onEdit).not.toHaveBeenCalled();
      expect(commit).not.toHaveBeenCalled();

      fireEvent.change(area, { target: { value: '{"a": 4}' } });
      fireEvent.keyDown(area, { key: "s", code: "KeyS", ctrlKey: true });
      expect(onEdit).toHaveBeenCalledWith({ t: "json", v: '{"a": 4}' });
      await waitFor(() => expect(commit).toHaveBeenCalledTimes(1));
      await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    } finally {
      off();
    }
  });

  it("allows multiple carets (⌘-click, ⌃⇧↑ / ⌃⇧↓) and has search and replace (⌘F)", () => {
    const box = mount(col("json"), json, vi.fn());
    const view = cmView("doc");
    expect(view.state.facet(EditorState.allowMultipleSelections)).toBe(true);
    act(() =>
      view.dispatch({
        selection: EditorSelection.create([EditorSelection.cursor(0), EditorSelection.cursor(5)]),
      }),
    );
    expect(view.state.selection.ranges).toHaveLength(2);
    // ⌃⇧↓ adds a caret on the line below.
    act(() => view.dispatch({ selection: EditorSelection.single(0) }));
    fireEvent.keyDown(box, { key: "ArrowDown", code: "ArrowDown", ctrlKey: true, shiftKey: true });
    expect(view.state.selection.ranges.length).toBeGreaterThan(1);
    // ⎋ with several carets collapses them first, and does not revert anything.
    cmType("doc", '{"a": 2}');
    act(() =>
      view.dispatch({
        selection: EditorSelection.create([EditorSelection.cursor(0), EditorSelection.cursor(5)]),
      }),
    );
    fireEvent.keyDown(box, { key: "Escape", code: "Escape" });
    expect(view.state.selection.ranges).toHaveLength(1);
    expect(view.state.doc.toString()).toBe('{"a": 2}');
    // ⌘F opens the search panel in the field; ⎋ closes it before it reverts anything.
    fireEvent.keyDown(box, { key: "f", code: "KeyF", ctrlKey: true });
    expect(document.querySelector(".cm-search")).not.toBeNull();
    fireEvent.keyDown(box, { key: "Escape", code: "Escape" });
    expect(view.state.doc.toString()).toBe('{"a": 2}');
  });

  it("is in the cell editor's keyboard scope while editable (the SQL editor's keys stay away)", () => {
    const runCurrent = vi.fn();
    const off = actions.register("query.run-current", runCurrent);
    try {
      const box = mount(col("json"), json, vi.fn());
      expect(screen.getByTestId("quick-look-code")).toHaveAttribute("data-cm-scope", "grid-edit");
      fireEvent.keyDown(box, { key: "Enter", code: "Enter", ctrlKey: true });
      expect(runCurrent).not.toHaveBeenCalled();
    } finally {
      off();
    }
  });

  it("shows NULL as a placeholder and takes typing", () => {
    const onEdit = vi.fn();
    const box = mount(col("json"), { t: "null" }, onEdit);
    expect(document.querySelector(".cm-placeholder")).toHaveTextContent("NULL");
    cmType("doc", "[1]");
    expect(document.querySelector(".cm-placeholder")).toBeNull();
    fireEvent.keyDown(box, { key: "Enter", code: "Enter", ctrlKey: true });
    expect(onEdit).toHaveBeenCalledWith({ t: "json", v: "[1]" });
  });

  it("colours XML in multi-line text", () => {
    mount(col("text", { name: "doc", id: "doc" }), { t: "text", v: "<a>\n  <b/>\n</a>" }, vi.fn());
    expect(cmContent("doc").querySelector("span[class]")).not.toBeNull();
  });

  it("is read-only (and selectable) for a field that cannot be edited", () => {
    const box = mount(col("json", { editable: false }), json);
    expect(box).toHaveAttribute("aria-readonly", "true");
    expect(screen.getByTestId("quick-look-code")).toHaveAttribute("data-cm-scope", "global");
    expect(cmView("doc").state.readOnly).toBe(true);
  });

  it("Pretty JSON shows the value indented or as stored; Format / Minify stage it", async () => {
    const onEdit = vi.fn();
    mount(col("json"), json, onEdit);
    expect(cmView("doc").state.doc.toString()).toBe(pretty);
    fireEvent.click((await openMenu()).querySelector('[role="menuitemcheckbox"]') as HTMLElement);
    expect(cmView("doc").state.doc.toString()).toBe('{"a":1,"b":[true,null]}');
    await openMenu();
    fireEvent.click(screen.getByRole("menuitem", { name: "Format JSON" }));
    expect(onEdit).toHaveBeenLastCalledWith({ t: "json", v: pretty });
  });

  it("read-only fields have a Pretty JSON toggle", () => {
    mount(col("json", { editable: false }), json);
    fireEvent.click(screen.getByRole("button", { name: "Pretty JSON in doc" }));
    expect(cmView("doc").state.doc.toString()).toBe('{"a":1,"b":[true,null]}');
  });
});

describe("FieldBlock text-like kinds are CodeMirror fields", () => {
  const text = (extra: Partial<GridColumn> = {}) => col("text", { ...extra });
  const undoKey = (el: HTMLElement) =>
    fireEvent.keyDown(el, { key: "z", code: "KeyZ", ctrlKey: true });

  it("text, enum, set, array, spatial and other values: one line or many", () => {
    const kinds: Array<[GridColumn["kind"], Value]> = [
      ["text", { t: "text", v: "one line" }],
      ["text", { t: "text", v: "long ".repeat(80) }],
      ["text", { t: "text", v: "line 1\nline 2" }],
      ["enum", { t: "text", v: "draft" }],
      ["set", { t: "text", v: "a,b" }],
      ["array", { t: "text", v: "{1,2,3}" }],
      ["geometry", { t: "text", v: "POINT(1 2)" }],
      ["other", { t: "text", v: "(1,2)" }],
    ];
    for (const [kind, value] of kinds) {
      const box = mount(col(kind), value, vi.fn());
      expect(box.closest(".cm-editor"), `${kind}: ${value.t}`).not.toBeNull();
      expect(document.querySelector("textarea")).toBeNull();
      // The same editor as Quick Look's: several carets and search.
      expect(cmView("doc").state.facet(EditorState.allowMultipleSelections)).toBe(true);
      cleanup();
    }
  });

  it("numbers, dates and UUIDs stay plain one-line inputs", () => {
    const inputs: Array<[GridColumn["kind"], Value]> = [
      ["int", { t: "int", v: 5 }],
      ["float", { t: "float", v: 1.5 }],
      ["uuid", { t: "text", v: "00000000-0000-0000-0000-000000000000" }],
      ["date", { t: "text", v: "2024-01-02" }],
    ];
    for (const [kind, value] of inputs) {
      const box = mount(col(kind), value, vi.fn());
      expect(box.tagName, kind).toBe("INPUT");
      expect(document.querySelector(".cm-editor")).toBeNull();
      cleanup();
    }
  });

  it("a one-line value stages on ↩ and on blur, reverts on ⎋, like a native box", () => {
    const onEdit = vi.fn();
    const box = mount(text(), { t: "text", v: "Eko" }, onEdit);
    cmType("doc", "Budi");
    expect(screen.getByText(/to stage/)).toHaveTextContent(`${formatCombo("Enter")} to stage`);
    fireEvent.keyDown(box, { key: "Enter", code: "Enter" });
    expect(onEdit).toHaveBeenLastCalledWith({ t: "text", v: "Budi" });
    cmType("doc", "Siti");
    fireEvent.blur(box);
    expect(onEdit).toHaveBeenLastCalledWith({ t: "text", v: "Siti" });
    expect(onEdit).toHaveBeenCalledTimes(2);
    cmType("doc", "gone");
    fireEvent.keyDown(box, { key: "Escape", code: "Escape" });
    expect(cmDoc("doc")).toBe("Eko");
    expect(onEdit).toHaveBeenCalledTimes(2);
  });

  it("⌥↩ adds a line to a one-line value; then ↩ is a line break and ⌘↩ stages", () => {
    const onEdit = vi.fn();
    const box = mount(text(), { t: "text", v: "Eko" }, onEdit);
    act(() => cmView("doc").dispatch({ selection: { anchor: 3 } }));
    fireEvent.keyDown(box, { key: "Enter", code: "Enter", altKey: true });
    expect(cmDoc("doc")).toBe("Eko\n");
    expect(screen.getByText(/to stage/)).toHaveTextContent(`${formatCombo("Mod+Enter")} to stage`);
    fireEvent.keyDown(box, { key: "Enter", code: "Enter" });
    expect(onEdit).not.toHaveBeenCalled();
    expect(cmDoc("doc")).toBe("Eko\n\n");
    fireEvent.keyDown(box, { key: "Enter", code: "Enter", ctrlKey: true });
    expect(onEdit).toHaveBeenCalledWith({ t: "text", v: "Eko\n\n" });
  });

  it("a long single line wraps inside the box and keeps ⌘F search", () => {
    const long = "word ".repeat(200).trim();
    const box = mount(text(), { t: "text", v: long }, vi.fn());
    expect(cmView("doc").state.doc.toString()).toBe(long);
    expect(cmView("doc").contentDOM.classList.contains("cm-lineWrapping")).toBe(true);
    fireEvent.keyDown(box, { key: "f", code: "KeyF", ctrlKey: true });
    expect(document.querySelector(".cm-search")).not.toBeNull();
  });

  it("colours a compact one-line XML or JSON string in a text column, not plain text", () => {
    mount(text(), { t: "text", v: '<a id="1"><b/></a>' }, vi.fn());
    expect(cmContent("doc").querySelector("span[class]")).not.toBeNull();
    cleanup();
    mount(text(), { t: "text", v: '{"a":[1,true]}' }, vi.fn());
    expect(cmContent("doc").querySelector("span[class]")).not.toBeNull();
    cleanup();
    mount(text(), { t: "text", v: "just some words" }, vi.fn());
    expect(cmContent("doc").querySelector("span[class]")).toBeNull();
  });

  it("⌘S stages a one-line text first, then commits the tab", async () => {
    const order: string[] = [];
    const onEdit = vi.fn(() => order.push("edit"));
    const commit = vi.fn(() => void order.push("commit"));
    const off = actions.register("grid.commit", commit);
    try {
      const box = mount(text(), { t: "text", v: "Eko" }, onEdit);
      cmType("doc", "Budi");
      expect(fireEvent.keyDown(box, { key: "s", code: "KeyS", ctrlKey: true })).toBe(false);
      await waitFor(() => expect(commit).toHaveBeenCalledTimes(1));
      expect(order).toEqual(["edit", "commit"]);
      expect(onEdit).toHaveBeenCalledWith({ t: "text", v: "Budi" });
    } finally {
      off();
    }
  });

  it("is a read-only box for a field that cannot be edited, and a typed change is no draft", () => {
    const onEdit = vi.fn();
    const box = mount(text({ editable: false }), { t: "text", v: "fixed" });
    expect(box).toHaveAttribute("aria-readonly", "true");
    cmType("doc", "changed");
    expect(screen.queryByText(/to stage/)).not.toBeInTheDocument();
    fireEvent.blur(box);
    expect(onEdit).not.toHaveBeenCalled();
  });

  describe("⌘Z never reaches into another row's (or an earlier) value", () => {
    function mountRow(column: GridColumn, value: Value, rowKey: string, onEdit: () => void) {
      const view = (v: Value, key: string) => (
        <KeyboardProvider>
          <FieldBlock
            col={column}
            value={v}
            rowKey={key}
            focused={false}
            hidden={false}
            changed={false}
            onEdit={onEdit}
            onViewCell={vi.fn()}
          />
        </KeyboardProvider>
      );
      const r = render(view(value, rowKey));
      return (v: Value, key: string) => r.rerender(view(v, key));
    }

    it("a JSON field that follows the selected row: row 1, row 2, ⌘Z, leave", () => {
      const onEdit = vi.fn();
      const show = mountRow(col("json"), { t: "json", v: '{"row":1}' }, "pk:1", onEdit);
      const box = screen.getByRole("textbox", { name: "doc" });
      show({ t: "json", v: '{"row":2}' }, "pk:2");
      expect(cmDoc("doc")).toBe('{\n  "row": 2\n}');
      box.focus();
      undoKey(box);
      expect(cmDoc("doc")).toBe('{\n  "row": 2\n}');
      expect(screen.queryByText(/to stage/)).not.toBeInTheDocument();
      fireEvent.blur(box);
      fireEvent.keyDown(box, { key: "s", code: "KeyS", ctrlKey: true });
      expect(onEdit).not.toHaveBeenCalled();
    });

    it("a text field: an edit in row 1 is not undone into row 2", () => {
      const onEdit = vi.fn();
      const show = mountRow(text(), { t: "text", v: "first" }, "pk:1", onEdit);
      const box = screen.getByRole("textbox", { name: "doc" });
      cmType("doc", "typed in row 1");
      // The click on row 2 stages row 1's draft; then the panel shows row 2.
      fireEvent.blur(box);
      expect(onEdit).toHaveBeenCalledTimes(1);
      show({ t: "text", v: "second" }, "pk:2");
      expect(cmDoc("doc")).toBe("second");
      undoKey(box);
      expect(cmDoc("doc")).toBe("second");
      fireEvent.blur(box);
      expect(onEdit).toHaveBeenCalledTimes(1);
    });

    it("after ⎋ reverts a draft, ⌘Z does not bring the discarded draft back", () => {
      const onEdit = vi.fn();
      const box = mount(text(), { t: "text", v: "base" }, onEdit);
      cmType("doc", "discarded");
      fireEvent.keyDown(box, { key: "Escape", code: "Escape" });
      expect(cmDoc("doc")).toBe("base");
      undoKey(box);
      expect(cmDoc("doc")).toBe("base");
      expect(screen.queryByText(/to stage/)).not.toBeInTheDocument();
      fireEvent.blur(box);
      expect(onEdit).not.toHaveBeenCalled();
    });

    it("after a staged edit comes back as the value, ⌘Z leaves it as it is", () => {
      const onEdit = vi.fn();
      const show = mountRow(text(), { t: "text", v: "Eko" }, "pk:1", onEdit);
      const box = screen.getByRole("textbox", { name: "doc" });
      cmType("doc", "Budi");
      fireEvent.keyDown(box, { key: "Enter", code: "Enter" });
      expect(onEdit).toHaveBeenCalledTimes(1);
      show({ t: "text", v: "Budi!" }, "pk:1");
      expect(cmDoc("doc")).toBe("Budi!");
      undoKey(box);
      expect(cmDoc("doc")).toBe("Budi!");
      fireEvent.blur(box);
      expect(onEdit).toHaveBeenCalledTimes(1);
    });

    it("still undoes what is typed in the field", () => {
      const box = mount(text(), { t: "text", v: "Eko" }, vi.fn());
      cmType("doc", "Budi");
      expect(cmDoc("doc")).toBe("Budi");
      undoKey(box);
      expect(cmDoc("doc")).toBe("Eko");
    });
  });
});

describe("FieldBlock JSON ▸ Sort keys", () => {
  const raw = '{"b":9007199254740993,"a":{"d":1.10,"c":2},"b":3}';

  it("lists the keys in order from the field menu, display only and remembered", async () => {
    const onEdit = vi.fn();
    const box = mount(col("json"), { t: "json", v: raw }, onEdit);
    expect(cmView("doc").state.doc.toString()).toBe(
      '{\n  "b": 9007199254740993,\n  "a": {\n    "d": 1.10,\n    "c": 2\n  },\n  "b": 3\n}',
    );
    const menu = await openMenu();
    fireEvent.click(within(menu).getByRole("menuitemcheckbox", { name: "Sort keys" }));
    expect(cmView("doc").state.doc.toString()).toBe(
      '{\n  "a": {\n    "c": 2,\n    "d": 1.10\n  },\n  "b": 9007199254740993,\n  "b": 3\n}',
    );
    // Nothing is staged by looking at it that way; saving the sorted text would stage it.
    expect(onEdit).not.toHaveBeenCalled();
    fireEvent.blur(box);
    expect(onEdit).not.toHaveBeenCalled();
    expect(localStorage.getItem(sortKeysStorageKey("details"))).toBe("1");
    // A later field opens sorted.
    cleanup();
    mount(col("json"), { t: "json", v: raw }, onEdit);
    expect(cmView("doc").state.doc.toString().indexOf('"a"')).toBeLessThan(
      cmView("doc").state.doc.toString().indexOf('"b"'),
    );
  });

  it("read-only fields have a Sort keys button", () => {
    mount(col("json", { editable: false }), { t: "json", v: '{"b":1,"a":2}' });
    const sort = screen.getByRole("button", { name: "Sort keys in doc" });
    expect(sort).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(sort);
    expect(sort).toHaveAttribute("aria-pressed", "true");
    expect(cmView("doc").state.doc.toString()).toBe('{\n  "a": 2,\n  "b": 1\n}');
  });

  it("works when localStorage throws", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    mount(col("json", { editable: false }), { t: "json", v: '{"b":1,"a":2}' });
    fireEvent.click(screen.getByRole("button", { name: "Sort keys in doc" }));
    expect(cmView("doc").state.doc.toString()).toBe('{\n  "a": 2,\n  "b": 1\n}');
  });
});

describe("FieldBlock binary", () => {
  const bytes = (data: number[], truncated = false, len = data.length): Value => ({
    t: "bytes",
    v: { base64: btoa(String.fromCharCode(...data)), len, truncated },
  });
  const utf8 = (text: string) => Array.from(new TextEncoder().encode(text));
  const mountBytes = (value: Value) =>
    render(
      <FieldBlock
        col={col("bytes", { name: "data", id: "data" })}
        value={value}
        rowKey="pk:1"
        focused={false}
        hidden={false}
        changed={false}
        onEdit={vi.fn()}
        onViewCell={vi.fn()}
      />,
    );

  it("shows the start of a blob that is text, next to its size", () => {
    mountBytes(bytes(utf8("Hello, wörld")));
    expect(screen.getByTestId("field-blob-text")).toHaveTextContent("Hello, wörld");
    expect(screen.getByText(/12 B binary|13 B binary/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show data in the Cell view" })).toBeInTheDocument();
  });

  it("cuts the text at 40 characters", () => {
    const long = "abcdefghij".repeat(6);
    mountBytes(bytes(utf8(long)));
    expect(screen.getByTestId("field-blob-text").textContent).toBe(`${long.slice(0, 40)}…`);
  });

  it("marks a preview, and shows nothing for binary data (the size only)", () => {
    mountBytes(bytes(utf8("Hel"), true, 70_000));
    expect(screen.getByTestId("field-blob-text").textContent).toBe("Hel…");
    expect(screen.getByText(/68\.4 KB binary \(preview\)/)).toBeInTheDocument();
    cleanup();
    mountBytes(bytes([0x1f, 0x8b, 0x08, 0x00, 0xfe, 0x80]));
    expect(screen.queryByTestId("field-blob-text")).not.toBeInTheDocument();
    expect(screen.getByText("6 B binary")).toBeInTheDocument();
  });
});
