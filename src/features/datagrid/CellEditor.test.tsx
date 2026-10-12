import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
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
    expect(screen.getByText("↩ save · ⌥↩ new line · esc cancel")).toBeInTheDocument();
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
