import { redoDepth, undo, undoDepth } from "@codemirror/commands";
import { openSearchPanel, searchPanelOpen } from "@codemirror/search";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { resolveScope } from "@/lib/keys/useShortcuts";
import { CodeView } from "./CodeView";

// jsdom has no layout: CodeMirror measures through these.
beforeAll(() => {
  const rects = () => Object.assign([], { item: () => null }) as unknown as DOMRectList;
  Range.prototype.getClientRects ??= rects;
  Range.prototype.getBoundingClientRect ??= () => new DOMRect();
  Element.prototype.getClientRects ??= rects;
});
afterEach(cleanup);

const view = (): EditorView => {
  const dom = document.querySelector<HTMLElement>(".cm-editor")!;
  return EditorView.findFromDOM(dom)!;
};
const type = (text: string) =>
  act(() => {
    const v = view();
    v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: text } });
  });

describe("CodeView (compact: a field of the Details sidebar)", () => {
  it("allows multiple selections and runs search, like Quick Look's", () => {
    render(<CodeView compact wrap label="Field" text={"one two\none two"} />);
    const v = view();
    expect(v.state.facet(EditorState.allowMultipleSelections)).toBe(true);
    act(() =>
      v.dispatch({
        selection: EditorSelection.create([EditorSelection.cursor(0), EditorSelection.cursor(8)]),
      }),
    );
    expect(v.state.selection.ranges).toHaveLength(2);
    // ⌘F is bound (search & replace), and the panel opens inside the field.
    expect(searchPanelOpen(v.state)).toBe(false);
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Field" }), {
      key: "f",
      code: "KeyF",
      ctrlKey: true,
    });
    expect(searchPanelOpen(view().state)).toBe(true);
    expect(document.querySelector(".cm-search")).not.toBeNull();
    // The same programmatically: openSearchPanel is a no-op while it is open.
    act(() => void openSearchPanel(view()));
    expect(document.querySelectorAll(".cm-search")).toHaveLength(1);
  });

  it("has no line numbers; the full view keeps its gutter", () => {
    const { rerender } = render(<CodeView compact wrap label="Field" text="a" />);
    expect(document.querySelector(".cm-lineNumbers")).toBeNull();
    rerender(<CodeView compact wrap label="Field" text="a" />);
    cleanup();
    render(<CodeView wrap label="Field" text="a" />);
    expect(document.querySelector(".cm-lineNumbers")).not.toBeNull();
  });

  it("adds a caret below with ⌃⇧↓ and above with ⌃⇧↑", () => {
    render(<CodeView wrap label="Field" text={"aaa\nbbb\nccc"} />);
    const content = screen.getByRole("textbox", { name: "Field" });
    act(() => view().dispatch({ selection: EditorSelection.single(5) }));
    fireEvent.keyDown(content, {
      key: "ArrowDown",
      code: "ArrowDown",
      ctrlKey: true,
      shiftKey: true,
    });
    expect(view().state.selection.ranges.length).toBe(2);
    fireEvent.keyDown(content, { key: "ArrowUp", code: "ArrowUp", ctrlKey: true, shiftKey: true });
    expect(view().state.selection.ranges.length).toBe(3);
  });

  it("gives a plain ↩ a bare line break and leaves ⇥ to the browser", () => {
    render(<CodeView compact wrap label="Field" text="  indented" />);
    const content = screen.getByRole("textbox", { name: "Field" });
    act(() => view().dispatch({ selection: EditorSelection.single(10) }));
    fireEvent.keyDown(content, { key: "Enter", code: "Enter" });
    // No copied indentation: a field's text is exactly what was typed.
    expect(view().state.doc.toString()).toBe("  indented\n");
    const tab = new KeyboardEvent("keydown", {
      key: "Tab",
      code: "Tab",
      bubbles: true,
      cancelable: true,
    });
    act(() => void content.dispatchEvent(tab));
    expect(tab.defaultPrevented).toBe(false);
    expect(view().state.doc.toString()).toBe("  indented\n");
  });

  it("calls onChange for what is typed, not for the text prop catching up", () => {
    const onChange = vi.fn();
    const { rerender } = render(
      <CodeView compact wrap label="Field" text="one" onChange={onChange} />,
    );
    rerender(<CodeView compact wrap label="Field" text="two" onChange={onChange} />);
    expect(view().state.doc.toString()).toBe("two");
    expect(onChange).not.toHaveBeenCalled();
    type("three");
    expect(onChange).toHaveBeenCalledExactlyOnceWith("three");
  });

  describe("a new text is a new document, not an edit", () => {
    it("can not be undone: ⌘Z after the text changed keeps the document and never calls onChange", () => {
      // The Details field survives a row change: it must never undo into the previous row's value.
      const onChange = vi.fn();
      const { rerender } = render(
        <CodeView compact wrap label="Field" text='{"row": 1}' onChange={onChange} />,
      );
      rerender(<CodeView compact wrap label="Field" text='{"row": 2}' onChange={onChange} />);
      expect(view().state.doc.toString()).toBe('{"row": 2}');
      expect(undoDepth(view().state)).toBe(0);
      expect(undo(view())).toBe(false);
      fireEvent.keyDown(screen.getByRole("textbox", { name: "Field" }), {
        key: "z",
        code: "KeyZ",
        ctrlKey: true,
      });
      expect(view().state.doc.toString()).toBe('{"row": 2}');
      expect(onChange).not.toHaveBeenCalled();
    });

    it("forgets what was typed before it: undo does not reach back across the replacement", () => {
      const onChange = vi.fn();
      const { rerender } = render(
        <CodeView compact wrap label="Field" text="first" onChange={onChange} />,
      );
      type("typed in row 1");
      expect(undoDepth(view().state)).toBe(1);
      onChange.mockClear();
      // The next row (a new `text`), then ⌘Z.
      rerender(<CodeView compact wrap label="Field" text="second" onChange={onChange} />);
      expect(view().state.doc.toString()).toBe("second");
      expect(undoDepth(view().state)).toBe(0);
      expect(redoDepth(view().state)).toBe(0);
      act(() => void undo(view()));
      expect(view().state.doc.toString()).toBe("second");
      expect(onChange).not.toHaveBeenCalled();
    });

    it("still undoes what is typed after it, back to that text and no further", () => {
      const onChange = vi.fn();
      const { rerender } = render(
        <CodeView compact wrap label="Field" text="first" onChange={onChange} />,
      );
      rerender(<CodeView compact wrap label="Field" text="second" onChange={onChange} />);
      type("second!");
      expect(onChange).toHaveBeenLastCalledWith("second!");
      act(() => void undo(view()));
      expect(view().state.doc.toString()).toBe("second");
      expect(onChange).toHaveBeenLastCalledWith("second");
      act(() => void undo(view()));
      expect(view().state.doc.toString()).toBe("second");
    });

    it("keeps the editor itself: the selection mode and the focus follow the new text", () => {
      const { rerender } = render(<CodeView compact wrap label="Field" text="a" />);
      const dom = document.querySelector(".cm-editor");
      rerender(<CodeView compact wrap label="Field" text="b" />);
      expect(document.querySelector(".cm-editor")).toBe(dom);
      expect(view().state.facet(EditorState.allowMultipleSelections)).toBe(true);
    });
  });

  it("offers every key to onKeyDown first; true keeps it from CodeMirror's own keymap", () => {
    const seen: string[] = [];
    render(
      <CodeView
        compact
        wrap
        label="Field"
        text="a"
        onKeyDown={(e) => {
          seen.push(e.key);
          return e.key === "Enter";
        }}
      />,
    );
    const content = screen.getByRole("textbox", { name: "Field" });
    act(() => view().dispatch({ selection: EditorSelection.single(1) }));
    fireEvent.keyDown(content, { key: "Enter", code: "Enter" });
    expect(view().state.doc.toString()).toBe("a");
    fireEvent.keyDown(content, { key: "x", code: "KeyX" });
    expect(seen).toEqual(["Enter", "x"]);
  });

  it("tells when it loses the focus, but not to its own search panel", () => {
    const onBlur = vi.fn();
    render(<CodeView compact wrap label="Field" text="a" onBlur={onBlur} />);
    const content = screen.getByRole("textbox", { name: "Field" });
    fireEvent.blur(content);
    expect(onBlur).toHaveBeenCalledTimes(1);
    act(() => void openSearchPanel(view()));
    const field = document.querySelector<HTMLElement>(".cm-search input")!;
    fireEvent.blur(content, { relatedTarget: field });
    expect(onBlur).toHaveBeenCalledTimes(1);
  });

  it("shows a placeholder while empty and flags an invalid text", () => {
    const { rerender } = render(<CodeView compact wrap label="Field" text="" placeholder="NULL" />);
    expect(document.querySelector(".cm-placeholder")).toHaveTextContent("NULL");
    rerender(<CodeView compact wrap label="Field" text="" placeholder="EMPTY" invalid />);
    expect(document.querySelector(".cm-placeholder")).toHaveTextContent("EMPTY");
    expect(screen.getByRole("textbox", { name: "Field" })).toHaveAttribute("aria-invalid", "true");
    rerender(<CodeView compact wrap label="Field" text="x" />);
    expect(document.querySelector(".cm-placeholder")).toBeNull();
    expect(screen.getByRole("textbox", { name: "Field" })).not.toHaveAttribute("aria-invalid");
  });

  it("names its own keyboard scope instead of being a SQL editor", () => {
    render(<CodeView compact wrap label="Field" text="a" shortcutScope="grid-edit" />);
    const content = screen.getByRole("textbox", { name: "Field" });
    expect(resolveScope(content)).toBe("grid-edit");
    cleanup();
    render(<CodeView wrap label="Field" text="a" />);
    expect(resolveScope(screen.getByRole("textbox", { name: "Field" }))).toBe("editor");
  });

  it("is read-only on request", () => {
    const { rerender } = render(<CodeView compact wrap label="Field" text="a" readOnly />);
    expect(view().state.readOnly).toBe(true);
    rerender(<CodeView compact wrap label="Field" text="a" />);
    expect(view().state.readOnly).toBe(false);
  });
});
