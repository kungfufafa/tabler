import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import type { Value } from "@/lib/ipc";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import type { GridColumn } from "../types";
import { sortKeysStorageKey } from "./jsonPrefs";
import { QuickLook, type QuickLookProps } from "./QuickLook";

const col = (
  name: string,
  kind: GridColumn["kind"],
  extra: Partial<GridColumn> = {},
): GridColumn => ({
  id: name,
  name,
  kind,
  dataType: kind,
  nullable: true,
  isPk: false,
  hasDefault: false,
  isAutoInc: false,
  editable: true,
  align: "left",
  ...extra,
});

const PNG = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
const bytes = (data: number[], truncated = false): Value => ({
  t: "bytes",
  v: { base64: btoa(String.fromCharCode(...data)), len: data.length, truncated },
});
const utf8 = (text: string) => Array.from(new TextEncoder().encode(text));

function mount(props: Partial<QuickLookProps> & Pick<QuickLookProps, "column" | "value">) {
  const handlers = { onClose: vi.fn(), onSave: vi.fn() };
  render(
    <KeyboardProvider>
      <QuickLook {...handlers} {...props} />
    </KeyboardProvider>,
  );
  return handlers;
}

/** The CodeMirror view inside the dialog. */
function editor(): EditorView {
  const dom = document.querySelector<HTMLElement>(".cm-editor");
  if (!dom) throw new Error("no editor");
  const view = EditorView.findFromDOM(dom);
  if (!view) throw new Error("no view");
  return view;
}

function typeText(text: string) {
  const view = editor();
  act(() => view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } }));
}

afterEach(() => {
  vi.restoreAllMocks();
  localStorage.clear();
});

describe("QuickLook", () => {
  it("edits text in an editor and stages it with Save", () => {
    const { onSave, onClose } = mount({
      column: col("bio", "text"),
      value: { t: "text", v: "hi" },
    });
    expect(screen.getByRole("dialog")).toHaveTextContent("bio");
    expect(editor().state.doc.toString()).toBe("hi");
    const save = screen.getByRole("button", { name: /Save/ });
    expect(save).toBeDisabled();
    typeText("hello\nworld");
    expect(screen.getByText("Edited — not staged yet")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Save/ }));
    expect(onSave).toHaveBeenCalledWith({ t: "text", v: "hello\nworld" });
    expect(onClose).not.toHaveBeenCalled();
  });

  it("toggles wrapping", () => {
    mount({ column: col("bio", "text"), value: { t: "text", v: "x" } });
    const wrap = screen.getByRole("button", { name: "Wrap" });
    expect(wrap).toHaveAttribute("aria-pressed", "true");
    expect(document.querySelector(".cm-lineWrapping")).not.toBeNull();
    fireEvent.click(wrap);
    expect(wrap).toHaveAttribute("aria-pressed", "false");
    expect(document.querySelector(".cm-lineWrapping")).toBeNull();
  });

  it("validates with the column's type; an emptied number saves NULL", () => {
    const { onSave } = mount({ column: col("age", "int"), value: { t: "int", v: 3 } });
    typeText("abc");
    fireEvent.click(screen.getByRole("button", { name: /Save/ }));
    expect(screen.getByRole("alert")).toHaveTextContent("Enter a whole number");
    expect(onSave).not.toHaveBeenCalled();
    typeText("");
    fireEvent.click(screen.getByRole("button", { name: /Save/ }));
    expect(onSave).toHaveBeenCalledWith({ t: "null" });
  });

  it("⌘↩ in the editor saves instead of reaching the SQL editor's Run Current", () => {
    const run = vi.fn();
    const off = actions.register("query.run-current", run);
    const { onSave } = mount({ column: col("bio", "text"), value: { t: "text", v: "a" } });
    typeText("b");
    const content = document.querySelector<HTMLElement>(".cm-content")!;
    act(() => content.focus());
    fireEvent.keyDown(content, { key: "Enter", code: "Enter", metaKey: true, ctrlKey: true });
    expect(onSave).toHaveBeenCalledWith({ t: "text", v: "b" });
    expect(run).not.toHaveBeenCalled();
    off();
  });

  it("is read-only without onSave", () => {
    render(
      <QuickLook column={col("bio", "text")} value={{ t: "text", v: "a" }} onClose={vi.fn()} />,
    );
    expect(screen.queryByRole("button", { name: /Save/ })).not.toBeInTheDocument();
    // The footer says Close (next to the dialog's own ✕), not Cancel.
    expect(screen.getAllByRole("button", { name: "Close" })).toHaveLength(2);
    expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();
    expect(editor().state.readOnly).toBe(true);
  });

  it("JSON: Format / Raw and a tree view that keeps numbers exact", () => {
    const raw = '{"id":9007199254740993,"tags":["a","b"]}';
    const { onSave } = mount({ column: col("doc", "json"), value: { t: "json", v: raw } });
    expect(editor().state.doc.toString()).toBe(
      '{\n  "id": 9007199254740993,\n  "tags": [\n    "a",\n    "b"\n  ]\n}',
    );
    fireEvent.click(screen.getByRole("radio", { name: "Raw" }));
    expect(editor().state.doc.toString()).toBe(raw);
    fireEvent.click(screen.getByRole("radio", { name: "Format" }));
    // Unchanged: nothing to stage.
    expect(screen.getByRole("button", { name: /Save/ })).toBeDisabled();
    fireEvent.click(screen.getByRole("radio", { name: "Tree" }));
    const tree = screen.getByRole("tree", { name: "JSON tree" });
    expect(tree).toHaveTextContent("id: 9007199254740993");
    expect(tree).toHaveTextContent("tags: [2 items]");
    fireEvent.click(screen.getByRole("radio", { name: "Json" }));
    typeText('{"a": ');
    fireEvent.click(screen.getByRole("button", { name: /Save/ }));
    expect(screen.getByRole("alert")).toHaveTextContent("Invalid JSON");
    expect(onSave).not.toHaveBeenCalled();
  });

  it("bytes: hex and base64 views, and a preview of images (read-only)", () => {
    mount({ column: col("avatar", "bytes"), value: bytes(PNG), onSave: undefined });
    expect(screen.getByTestId("quick-look-hex")).toHaveTextContent("89 50 4e 47");
    expect(screen.getByRole("img")).toHaveAttribute(
      "src",
      expect.stringMatching(/^data:image\/png;base64,/),
    );
    fireEvent.click(screen.getByRole("radio", { name: "Base64" }));
    expect(editor().state.doc.toString()).toBe(btoa(String.fromCharCode(...PNG)));
    expect(screen.queryByRole("button", { name: /Save/ })).not.toBeInTheDocument();
  });

  it("bytes: a blob holding text has a Text view, the default; binary ones have none", () => {
    const { onSave } = mount({
      column: col("data", "bytes"),
      value: bytes(utf8("Héllo wörld\nline 2")),
    });
    const text = screen.getByRole("radio", { name: "Text" });
    expect(text).toHaveAttribute("aria-checked", "true");
    expect(screen.getAllByRole("radio").map((r) => r.textContent)).toEqual([
      "Hex",
      "Base64",
      "Text",
    ]);
    expect(editor().state.doc.toString()).toBe("Héllo wörld\nline 2");
    expect(editor().state.readOnly).toBe(true);
    // Editing stays hex: one click away, and saves bytes.
    fireEvent.click(screen.getByRole("radio", { name: "Hex" }));
    expect(editor().state.doc.toString()).toBe(
      "48 c3 a9 6c 6c 6f 20 77 c3 b6 72 6c 64 0a 6c 69\n6e 65 20 32",
    );
    expect(editor().state.readOnly).toBe(false);
    act(() => editor().dispatch({ changes: { from: 0, to: 2, insert: "68" } }));
    fireEvent.click(screen.getByRole("button", { name: /Save/ }));
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ t: "bytes" }));
  });

  it("bytes: binary data offers Hex and Base64 only, and opens on Hex", () => {
    mount({ column: col("id", "bytes"), value: bytes([0x1f, 0x8b, 0x08, 0x00, 0xfe, 0x80]) });
    expect(screen.queryByRole("radio", { name: "Text" })).not.toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Hex" })).toHaveAttribute("aria-checked", "true");
    expect(editor().state.doc.toString()).toBe("1f 8b 08 00 fe 80");
  });

  it("bytes: a text preview reads as text and says it is only a preview", () => {
    // The preview ends in the middle of a 3-byte character.
    const cut = utf8("ab日").slice(0, 4);
    mount({ column: col("body", "bytes"), value: bytes(cut, true) });
    expect(screen.getByRole("radio", { name: "Text" })).toHaveAttribute("aria-checked", "true");
    expect(editor().state.doc.toString()).toBe("ab");
    expect(screen.getByText(/Only a preview is loaded/)).toBeInTheDocument();
    cleanup();
    // A preview that is not text has no Text view.
    mount({ column: col("body", "bytes"), value: bytes([0x61, 0xff, 0x62], true) });
    expect(screen.queryByRole("radio", { name: "Text" })).not.toBeInTheDocument();
  });

  it("bytes: edits a whole blob as hex", () => {
    const { onSave } = mount({ column: col("avatar", "bytes"), value: bytes([1, 2, 255]) });
    expect(editor().state.doc.toString()).toBe("01 02 ff");
    act(() => editor().dispatch({ changes: { from: 0, to: 8, insert: "48 69" } }));
    fireEvent.click(screen.getByRole("button", { name: /Save/ }));
    expect(onSave).toHaveBeenCalledWith({
      t: "bytes",
      v: { base64: btoa("Hi"), len: 2, truncated: false },
    });
  });

  it("bytes: a cut-off preview can load in full, then edits as hex", async () => {
    const loadFull = vi.fn().mockResolvedValue(bytes([1, 2, 3, 4]));
    mount({ column: col("data", "bytes"), value: bytes([1, 2], true), loadFull });
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Save/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Load full value/ }));
    await waitFor(() => expect(editor().state.doc.toString()).toBe("01 02 03 04"));
    expect(screen.getByRole("button", { name: /Save/ })).toBeDisabled();
  });

  it("JSON: Sort keys reorders the Tree and the Json text; nothing is staged until saved", () => {
    const raw = '{"b":1,"a":{"d":2,"c":3}}';
    const { onSave } = mount({ column: col("doc", "json"), value: { t: "json", v: raw } });
    const unsorted = '{\n  "b": 1,\n  "a": {\n    "d": 2,\n    "c": 3\n  }\n}';
    const sorted = '{\n  "a": {\n    "c": 3,\n    "d": 2\n  },\n  "b": 1\n}';
    expect(editor().state.doc.toString()).toBe(unsorted);
    const sort = screen.getByRole("button", { name: "Sort keys" });
    expect(sort).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(sort);
    expect(sort).toHaveAttribute("aria-pressed", "true");
    expect(editor().state.doc.toString()).toBe(sorted);
    // Display only: the cell is not staged, and there is nothing to save.
    expect(screen.getByRole("button", { name: /Save/ })).toBeDisabled();
    expect(screen.queryByText("Edited — not staged yet")).not.toBeInTheDocument();
    expect(onSave).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("radio", { name: "Tree" }));
    const tree = screen.getByRole("tree", { name: "JSON tree" }).textContent ?? "";
    expect(tree.indexOf("a: {2 keys}")).toBeGreaterThanOrEqual(0);
    expect(tree.indexOf("a: {2 keys}")).toBeLessThan(tree.indexOf("b: 1"));
    expect(tree.indexOf("c: 3")).toBeLessThan(tree.indexOf("d: 2"));
    fireEvent.click(sort);
    const plain = screen.getByRole("tree", { name: "JSON tree" }).textContent ?? "";
    expect(plain.indexOf("b: 1")).toBeLessThan(plain.indexOf("a: {2 keys}"));
    expect(plain.indexOf("d: 2")).toBeLessThan(plain.indexOf("c: 3"));

    // Saving from the sorted text stages that text (as Format does).
    fireEvent.click(sort);
    fireEvent.click(screen.getByRole("radio", { name: "Json" }));
    typeText(sorted.replace('"b": 1', '"b": 7'));
    fireEvent.click(screen.getByRole("button", { name: /Save/ }));
    expect(onSave).toHaveBeenCalledWith({ t: "json", v: sorted.replace('"b": 1', '"b": 7') });
  });

  it("JSON: Sort keys keeps big numbers, duplicate keys and escapes, and is remembered", () => {
    const raw = '{"b":9007199254740993,"a":1.10,"b":2,"u":"\\u00e9"}';
    mount({ column: col("doc", "json"), value: { t: "json", v: raw } });
    fireEvent.click(screen.getByRole("button", { name: "Sort keys" }));
    expect(editor().state.doc.toString()).toBe(
      '{\n  "a": 1.10,\n  "b": 9007199254740993,\n  "b": 2,\n  "u": "\\u00e9"\n}',
    );
    expect(localStorage.getItem(sortKeysStorageKey("quicklook"))).toBe("1");
    // A later Quick Look opens sorted.
    cleanup();
    mount({ column: col("doc", "json"), value: { t: "json", v: raw } });
    expect(screen.getByRole("button", { name: "Sort keys" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(editor().state.doc.toString().indexOf('"a"')).toBeLessThan(
      editor().state.doc.toString().indexOf('"b"'),
    );
    // Show raw is the stored text: not sorted, and no toggle.
    fireEvent.click(screen.getByRole("button", { name: "Show raw" }));
    expect(editor().state.doc.toString()).toBe(raw);
    expect(screen.queryByRole("button", { name: "Sort keys" })).not.toBeInTheDocument();
  });

  it("JSON: Sort keys works without localStorage", () => {
    const get = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    mount({ column: col("doc", "json"), value: { t: "json", v: '{"b":1,"a":2}' } });
    expect(get).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Sort keys" }));
    expect(editor().state.doc.toString()).toBe('{\n  "a": 2,\n  "b": 1\n}');
  });

  it("colours JSON, shows the raw value, and draws geometry on a map", () => {
    mount({ column: col("doc", "text"), value: { t: "text", v: '{"a":1}' } });
    expect(document.querySelector(".cm-content span[class]")).not.toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Show raw" }));
    expect(document.querySelector(".cm-content span[class]")).toBeNull();
  });

  it("geometry: a Map view with the WKT one step away", () => {
    mount({
      column: col("geom", "geometry"),
      value: { t: "other", v: { ty: "geometry", text: "SRID=4326;POLYGON((0 0,1 0,0 1,0 0))" } },
    });
    expect(screen.getByTestId("quick-look-map")).toHaveTextContent("lon ↔ · lat ↕");
    expect(document.querySelector('[data-geometry="polygons"]')).not.toBeNull();
    fireEvent.click(screen.getByRole("radio", { name: "WKT" }));
    expect(editor().state.doc.toString()).toBe("SRID=4326;POLYGON((0 0,1 0,0 1,0 0))");
  });

  it("detaches into a floating panel that follows the value it is given", () => {
    const onDetach = vi.fn();
    const { rerender } = render(
      <QuickLook
        column={col("bio", "text")}
        value={{ t: "text", v: "one" }}
        onClose={vi.fn()}
        onSave={vi.fn()}
        onDetach={onDetach}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /Detach/ }));
    expect(onDetach).toHaveBeenCalled();
    rerender(
      <QuickLook
        column={col("bio", "text")}
        value={{ t: "text", v: "two" }}
        onClose={vi.fn()}
        onSave={vi.fn()}
        detached
      />,
    );
    expect(document.querySelector('[data-quick-look="detached"]')).not.toBeNull();
    expect(screen.queryByRole("button", { name: /Detach/ })).not.toBeInTheDocument();
    expect(editor().state.doc.toString()).toBe("two");
  });

  it("tells whether it holds an edit that is not saved, and that it is gone on unmount", () => {
    const onDirtyChange = vi.fn();
    const view = render(
      <QuickLook
        column={col("bio", "text")}
        value={{ t: "text", v: "one" }}
        onClose={vi.fn()}
        onSave={vi.fn()}
        onDirtyChange={onDirtyChange}
        detached
      />,
    );
    const last = () => onDirtyChange.mock.calls.at(-1)![0];
    expect(last()).toBe(false);
    typeText("two");
    expect(last()).toBe(true);
    typeText("one");
    expect(last()).toBe(false);
    typeText("three");
    expect(last()).toBe(true);
    view.unmount();
    expect(last()).toBe(false);
  });

  it("a detached panel stops being dirty once its edit is saved, and can discard it", () => {
    const onDirtyChange = vi.fn();
    const onSave = vi.fn();
    render(
      <QuickLook
        column={col("bio", "text")}
        value={{ t: "text", v: "one" }}
        onClose={vi.fn()}
        onSave={onSave}
        onDirtyChange={onDirtyChange}
        detached
      />,
    );
    const last = () => onDirtyChange.mock.calls.at(-1)![0];
    expect(screen.queryByRole("button", { name: "Discard" })).not.toBeInTheDocument();
    typeText("two");
    expect(screen.getByText("Edited — not staged yet")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Save/ }));
    expect(onSave).toHaveBeenCalledWith({ t: "text", v: "two" });
    // The edit is staged: nothing is pending, the panel keeps showing it.
    expect(last()).toBe(false);
    expect(screen.queryByText("Edited — not staged yet")).not.toBeInTheDocument();
    expect(editor().state.doc.toString()).toBe("two");
    // Discard drops what was typed.
    typeText("three");
    expect(last()).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(last()).toBe(false);
    expect(editor().state.doc.toString()).toBe("two");
    expect(screen.queryByRole("button", { name: "Discard" })).not.toBeInTheDocument();
  });

  it("FK cells show the referenced row first", async () => {
    const loadReferenced = vi.fn().mockResolvedValue({
      table: "public.users",
      columns: [
        { name: "id", dataType: "bigint" },
        { name: "email", dataType: "text" },
      ],
      rows: [
        [
          { t: "int", v: 7 },
          { t: "text", v: "ann@x.io" },
        ],
      ],
    });
    const onOpenReferenced = vi.fn();
    mount({
      column: col("user_id", "int", { fk: { schema: "public", table: "users", column: "id" } }),
      value: { t: "int", v: 7 },
      loadReferenced,
      onOpenReferenced,
    });
    const row = await screen.findByRole("list", { name: "Referenced row" });
    expect(row).toHaveTextContent("email");
    expect(row).toHaveTextContent("ann@x.io");
    fireEvent.click(screen.getByRole("button", { name: /Open public.users/ }));
    expect(onOpenReferenced).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByRole("radio", { name: "Value" }));
    expect(editor().state.doc.toString()).toBe("7");
  });

  it("⎋ closes", () => {
    const { onClose } = mount({ column: col("bio", "text"), value: { t: "text", v: "a" } });
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });
});
