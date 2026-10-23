import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import type { Value, ValueKind } from "@/lib/ipc";
import { CellEditor } from "./CellEditor";
import type { GridColumn } from "./types";

const col = (kind: ValueKind, extra: Partial<GridColumn> = {}): GridColumn => ({
  id: "c",
  name: "c",
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

function renderEditor(column: GridColumn, previous: Value, initialText: string, typed = false) {
  const onCommit = vi.fn();
  const onCancel = vi.fn();
  render(
    <CellEditor
      col={column}
      previous={previous}
      initialText={initialText}
      typed={typed}
      left={0}
      top={0}
      width={120}
      height={28}
      onCommit={onCommit}
      onCancel={onCancel}
    />,
  );
  return { onCommit, onCancel };
}

describe("CellEditor", () => {
  it("edits multi-line text in a textarea and keeps the line break on ↩", () => {
    const { onCommit } = renderEditor(col("text"), { t: "text", v: "a\nb" }, "a\nb");
    const area = screen.getByRole("textbox", { name: "Edit c" });
    expect(area.tagName).toBe("TEXTAREA");
    expect((area as HTMLTextAreaElement).value).toBe("a\nb");
    expect(screen.getByText("Enter save · Alt+Enter new line · esc cancel")).toBeInTheDocument();
    fireEvent.keyDown(area, { key: "Enter" });
    expect(onCommit).toHaveBeenCalledWith({ t: "text", v: "a\nb" }, "enter");
  });

  it("switches the single-line input to multi-line with ⌥↩", () => {
    const { onCommit } = renderEditor(col("text"), { t: "text", v: "ab" }, "ab");
    const input = screen.getByRole("textbox", { name: "Edit c" }) as HTMLInputElement;
    expect(input.tagName).toBe("INPUT");
    input.setSelectionRange(1, 1);
    fireEvent.keyDown(input, { key: "Enter", altKey: true });
    const area = screen.getByRole("textbox", { name: "Edit c" }) as HTMLTextAreaElement;
    expect(area.tagName).toBe("TEXTAREA");
    expect(area.value).toBe("a\nb");
    fireEvent.keyDown(area, { key: "Escape" });
    expect(onCommit).not.toHaveBeenCalled();
  });

  it("does not commit after ⎋ even when the input then blurs", () => {
    const { onCommit, onCancel } = renderEditor(col("text"), { t: "text", v: "x" }, "y", true);
    const input = screen.getByRole("textbox", { name: "Edit c" });
    fireEvent.keyDown(input, { key: "Escape" });
    fireEvent.blur(input);
    expect(onCancel).toHaveBeenCalledOnce();
    expect(onCommit).not.toHaveBeenCalled();
  });

  it("opens JSON in a dialog that disables Save while the text is invalid", () => {
    const { onCommit } = renderEditor(
      col("json", { dataType: "jsonb" }),
      { t: "json", v: '{"a":1}' },
      '{"a":1}',
    );
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText("Edit JSON — c (jsonb)")).toBeInTheDocument();
    const area = within(dialog).getByRole("textbox") as HTMLTextAreaElement;
    expect(area.value).toBe('{\n  "a": 1\n}');
    const save = within(dialog).getByRole("button", { name: /Save/ });
    expect(save).toBeEnabled();
    fireEvent.change(area, { target: { value: '{"a":' } });
    expect(save).toBeDisabled();
    expect(within(dialog).getByRole("alert")).toHaveTextContent(/JSON/);
    fireEvent.change(area, { target: { value: '{"a": 2}' } });
    expect(save).toBeEnabled();
    fireEvent.click(save);
    expect(onCommit).toHaveBeenCalledWith({ t: "json", v: '{"a": 2}' }, "enter");
  });

  it("saving the JSON dialog unchanged keeps the stored text", () => {
    const { onCommit } = renderEditor(col("json"), { t: "json", v: '{"a":1}' }, '{"a":1}');
    fireEvent.click(screen.getByRole("button", { name: /Save/ }));
    expect(onCommit).toHaveBeenCalledWith({ t: "json", v: '{"a":1}' }, "enter");
  });

  it("offers NULL for a nullable bool and commits it", () => {
    const { onCommit } = renderEditor(col("bool"), { t: "bool", v: true }, "true");
    const select = screen.getByRole("combobox", { name: "Edit c" }) as HTMLSelectElement;
    const options = within(select)
      .getAllByRole("option")
      .map((o) => o.textContent);
    expect(options).toEqual(["true", "false", "NULL"]);
    fireEvent.change(select, { target: { value: select.options[2]!.value } });
    expect(onCommit).toHaveBeenCalledWith({ t: "null" }, "enter");
  });

  it("has no NULL option for a NOT NULL enum", () => {
    renderEditor(
      col("enum", { nullable: false, enumValues: ["admin", "viewer"] }),
      { t: "enum", v: "viewer" },
      "viewer",
    );
    const select = screen.getByRole("combobox", { name: "Edit c" });
    expect(
      within(select)
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual(["admin", "viewer"]);
  });
});

describe("CellEditor ▸ Auto convert empty Numeric, UUID, JSON or Array to NULL", () => {
  afterEach(() => resetPrefsForTests());

  it("commits NULL for an emptied number in a nullable column", () => {
    const { onCommit } = renderEditor(col("int"), { t: "int", v: 3 }, "");
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Edit c" }), { key: "Enter" });
    expect(onCommit).toHaveBeenCalledWith({ t: "null" }, "enter");
  });

  it("leaves NOT NULL columns and a disabled preference to the type's own check", () => {
    const strict = renderEditor(col("uuid", { nullable: false }), { t: "null" }, "");
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Edit c" }), { key: "Enter" });
    expect(strict.onCommit).not.toHaveBeenCalled();
    expect(screen.getByText("Enter a UUID")).toBeInTheDocument();
  });

  it("is off when the preference is", () => {
    resetPrefsForTests({ ...DEFAULT_PREFS, gridAutoNullEmpty: false });
    const { onCommit } = renderEditor(col("int"), { t: "int", v: 3 }, "");
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Edit c" }), { key: "Enter" });
    expect(onCommit).not.toHaveBeenCalled();
    expect(screen.getByText("Enter a whole number")).toBeInTheDocument();
  });

  it("stores the text NULL in a text column as text", () => {
    const { onCommit } = renderEditor(col("text"), { t: "null" }, "NULL");
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Edit c" }), { key: "Enter" });
    expect(onCommit).toHaveBeenCalledWith({ t: "text", v: "NULL" }, "enter");
  });
});

describe("CellEditor ▸ ⌘S stages the edit and commits the tab", () => {
  const offs: Array<() => void> = [];
  afterEach(() => {
    offs.splice(0).forEach((off) => off());
    resetPrefsForTests();
  });
  const ctrlS = { key: "s", code: "KeyS", ctrlKey: true };

  /** Registers `grid.commit` and records the order of the editor's commit and the tab's. */
  function watch(onCommit: ReturnType<typeof vi.fn>) {
    const order: string[] = [];
    onCommit.mockImplementation(() => void order.push("stage"));
    const commit = vi.fn(() => void order.push("commit"));
    offs.push(actions.register("grid.commit", commit));
    return { commit, order };
  }

  it("inline input: stages the typed value where the cell is, then runs grid.commit once", async () => {
    const { onCommit, onCancel } = renderEditor(col("int"), { t: "int", v: 3 }, "7");
    const { commit, order } = watch(onCommit);
    const input = screen.getByRole("textbox", { name: "Edit c" });
    expect(fireEvent.keyDown(input, ctrlS)).toBe(false); // preventDefault: no browser "save page"
    expect(onCommit).toHaveBeenCalledTimes(1);
    expect(onCommit).toHaveBeenCalledWith({ t: "int", v: 7 }, "blur");
    // Not in the same tick: the tab has to render the staged edit first.
    expect(commit).not.toHaveBeenCalled();
    await waitFor(() => expect(commit).toHaveBeenCalledTimes(1));
    expect(order).toEqual(["stage", "commit"]);
    // The blur that follows the editor closing stages nothing twice.
    fireEvent.blur(input);
    expect(onCommit).toHaveBeenCalledTimes(1);
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("inline input: a value that does not parse is neither staged nor committed, and stays open", async () => {
    const { onCommit, onCancel } = renderEditor(col("int"), { t: "int", v: 3 }, "abc");
    const { commit } = watch(onCommit);
    const input = screen.getByRole("textbox", { name: "Edit c" });
    fireEvent.keyDown(input, ctrlS);
    await new Promise((r) => setTimeout(r, 10));
    expect(onCommit).not.toHaveBeenCalled();
    expect(onCancel).not.toHaveBeenCalled();
    expect(commit).not.toHaveBeenCalled();
    expect(screen.getByText("Enter a whole number")).toBeInTheDocument();
    expect(input).toBeInTheDocument();
    // Fixing it and pressing ⌘S again works.
    fireEvent.change(input, { target: { value: "8" } });
    fireEvent.keyDown(input, ctrlS);
    expect(onCommit).toHaveBeenCalledWith({ t: "int", v: 8 }, "blur");
    await waitFor(() => expect(commit).toHaveBeenCalledTimes(1));
  });

  it("inline input: an emptied number follows the auto-NULL preference", async () => {
    const { onCommit } = renderEditor(col("int"), { t: "int", v: 3 }, "");
    const { commit } = watch(onCommit);
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Edit c" }), ctrlS);
    expect(onCommit).toHaveBeenCalledWith({ t: "null" }, "blur");
    await waitFor(() => expect(commit).toHaveBeenCalledTimes(1));
  });

  it("multi-line panel: the same", async () => {
    const { onCommit, onCancel } = renderEditor(col("text"), { t: "text", v: "a\nb" }, "a\nb");
    const { commit, order } = watch(onCommit);
    const area = screen.getByRole("textbox", { name: "Edit c" });
    expect(area.tagName).toBe("TEXTAREA");
    fireEvent.change(area, { target: { value: "a\nb\nc" } });
    expect(fireEvent.keyDown(area, ctrlS)).toBe(false);
    expect(onCommit).toHaveBeenCalledWith({ t: "text", v: "a\nb\nc" }, "blur");
    await waitFor(() => expect(commit).toHaveBeenCalledTimes(1));
    expect(order).toEqual(["stage", "commit"]);
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("multi-line panel: an invalid value stays in the editor and nothing is committed", async () => {
    // Text with a line break opens the panel whatever the column's kind; a number cannot hold it.
    const { onCommit, onCancel } = renderEditor(col("int"), { t: "int", v: 1 }, "1\n2");
    const { commit } = watch(onCommit);
    const area = screen.getByRole("textbox", { name: "Edit c" });
    expect(area.tagName).toBe("TEXTAREA");
    fireEvent.keyDown(area, ctrlS);
    await new Promise((r) => setTimeout(r, 10));
    expect(onCommit).not.toHaveBeenCalled();
    expect(onCancel).not.toHaveBeenCalled();
    expect(commit).not.toHaveBeenCalled();
    expect(screen.getByText("Enter a whole number")).toBeInTheDocument();
    fireEvent.change(area, { target: { value: "12" } });
    fireEvent.keyDown(area, ctrlS);
    expect(onCommit).toHaveBeenCalledWith({ t: "int", v: 12 }, "blur");
    await waitFor(() => expect(commit).toHaveBeenCalledTimes(1));
  });

  it("JSON dialog: stages the valid text and commits; invalid JSON does neither", async () => {
    const { onCommit } = renderEditor(
      col("json", { dataType: "jsonb" }),
      { t: "json", v: '{"a":1}' },
      '{"a":1}',
    );
    const { commit, order } = watch(onCommit);
    const dialog = screen.getByRole("dialog");
    const area = within(dialog).getByRole("textbox");
    fireEvent.change(area, { target: { value: '{"a":' } });
    fireEvent.keyDown(area, ctrlS);
    await new Promise((r) => setTimeout(r, 10));
    expect(onCommit).not.toHaveBeenCalled();
    expect(commit).not.toHaveBeenCalled();
    expect(within(dialog).getByRole("alert")).toHaveTextContent(/JSON/);

    fireEvent.change(area, { target: { value: '{"a": 2}' } });
    expect(fireEvent.keyDown(area, ctrlS)).toBe(false);
    expect(onCommit).toHaveBeenCalledTimes(1);
    expect(onCommit).toHaveBeenCalledWith({ t: "json", v: '{"a": 2}' }, "blur");
    await waitFor(() => expect(commit).toHaveBeenCalledTimes(1));
    expect(order).toEqual(["stage", "commit"]);
  });

  it("JSON dialog: ⌘S on the unchanged text keeps the stored value and commits the tab", async () => {
    const { onCommit } = renderEditor(col("json"), { t: "json", v: '{"a":1}' }, '{"a":1}');
    const { commit } = watch(onCommit);
    fireEvent.keyDown(within(screen.getByRole("dialog")).getByRole("textbox"), ctrlS);
    expect(onCommit).toHaveBeenCalledWith({ t: "json", v: '{"a":1}' }, "blur");
    await waitFor(() => expect(commit).toHaveBeenCalledTimes(1));
  });

  it("leaves Enter, Tab and Escape as they were (they never commit the tab)", async () => {
    const commitTab = vi.fn();
    offs.push(actions.register("grid.commit", commitTab));
    const enter = renderEditor(col("text"), { t: "text", v: "a" }, "b");
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Edit c" }), { key: "Enter" });
    expect(enter.onCommit).toHaveBeenCalledWith({ t: "text", v: "b" }, "enter");
    document.body.innerHTML = "";
    const tab = renderEditor(col("text"), { t: "text", v: "a" }, "b");
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Edit c" }), { key: "Tab" });
    expect(tab.onCommit).toHaveBeenCalledWith({ t: "text", v: "b" }, "tab");
    document.body.innerHTML = "";
    const esc = renderEditor(col("text"), { t: "text", v: "a" }, "b");
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Edit c" }), { key: "Escape" });
    expect(esc.onCancel).toHaveBeenCalledOnce();
    expect(esc.onCommit).not.toHaveBeenCalled();
    // Plain "s" and ⌘⇧S are not the commit key.
    document.body.innerHTML = "";
    const other = renderEditor(col("text"), { t: "text", v: "a" }, "b");
    const input = screen.getByRole("textbox", { name: "Edit c" });
    fireEvent.keyDown(input, { key: "s", code: "KeyS" });
    fireEvent.keyDown(input, { key: "s", code: "KeyS", ctrlKey: true, shiftKey: true });
    await new Promise((r) => setTimeout(r, 10));
    expect(other.onCommit).not.toHaveBeenCalled();
    expect(commitTab).not.toHaveBeenCalled();
  });
});
