import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import type { Value } from "@/lib/ipc";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import type { GridColumn } from "../types";
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

afterEach(() => vi.restoreAllMocks());

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

  it("bytes: hex and base64 views, and a preview of images", () => {
    mount({ column: col("avatar", "bytes"), value: bytes(PNG) });
    expect(screen.getByTestId("quick-look-hex")).toHaveTextContent("89 50 4e 47");
    expect(screen.getByRole("img")).toHaveAttribute(
      "src",
      expect.stringMatching(/^data:image\/png;base64,/),
    );
    fireEvent.click(screen.getByRole("radio", { name: "Base64" }));
    expect(editor().state.doc.toString()).toBe(btoa(String.fromCharCode(...PNG)));
    // Binary values are not edited here.
    expect(screen.queryByRole("button", { name: /Save/ })).not.toBeInTheDocument();
  });

  it("bytes: a cut-off preview can load in full", async () => {
    const loadFull = vi.fn().mockResolvedValue(bytes([1, 2, 3, 4]));
    mount({ column: col("data", "bytes"), value: bytes([1, 2], true), loadFull });
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Load full value/ }));
    await waitFor(() =>
      expect(screen.getByTestId("quick-look-hex")).toHaveTextContent("01 02 03 04"),
    );
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
