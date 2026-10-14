import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ColumnsPane } from "./ColumnsPane";
import type { ColumnSelection } from "./columnSelection";
import type { ColumnRow } from "./model";

const row = (name: string, over: Partial<ColumnRow> = {}): ColumnRow =>
  ({
    key: `c:${name}`,
    name,
    dataType: "text",
    nullable: true,
    defaultExpr: null,
    comment: null,
    status: "existing",
    ...over,
  }) as ColumnRow;

function selection(rows: ColumnRow[], over: Partial<ColumnSelection> = {}): ColumnSelection {
  return {
    tabId: "t",
    table: "posts",
    dialect: "postgres",
    rows,
    editable: true,
    onEdit: vi.fn(),
    ...over,
  };
}

const input = (label: string) => screen.getByLabelText(label) as HTMLInputElement;

describe("the right sidebar's column attributes", () => {
  it("one column: its name and attributes, edited on Enter or blur only when changed", () => {
    const sel = selection([row("title", { comment: "headline", defaultExpr: "''" })]);
    render(<ColumnsPane selection={sel} />);
    expect(screen.getByRole("status")).toHaveTextContent("title");
    expect(input("Name").value).toBe("title");
    expect(input("Type").value).toBe("text");
    expect(input("Default").value).toBe("''");
    expect(input("Comment").value).toBe("headline");
    expect((screen.getByLabelText("Nullable") as HTMLSelectElement).value).toBe("YES");

    // Unchanged: blurring stages nothing.
    fireEvent.blur(input("Type"));
    expect(sel.onEdit).not.toHaveBeenCalled();
    fireEvent.change(input("Type"), { target: { value: "varchar(40)" } });
    fireEvent.keyDown(input("Type"), { key: "Enter" });
    expect(sel.onEdit).toHaveBeenLastCalledWith("dataType", "varchar(40)");
    fireEvent.change(input("Comment"), { target: { value: "new" } });
    fireEvent.blur(input("Comment"));
    expect(sel.onEdit).toHaveBeenLastCalledWith("comment", "new");
    // Escape puts the box back.
    fireEvent.change(input("Default"), { target: { value: "x" } });
    fireEvent.keyDown(input("Default"), { key: "Escape" });
    expect(input("Default").value).toBe("''");
    fireEvent.change(screen.getByLabelText("Nullable"), { target: { value: "NO" } });
    expect(sel.onEdit).toHaveBeenLastCalledWith("nullable", false);
  });

  it("several columns: shared values, 'Multiple values' where they differ, no name", () => {
    const sel = selection([
      row("a", { dataType: "text", nullable: true, comment: "same" }),
      row("b", { dataType: "text", nullable: false, comment: "same", defaultExpr: "'x'" }),
    ]);
    render(<ColumnsPane selection={sel} />);
    expect(screen.getByRole("status")).toHaveTextContent("2 columns selected");
    expect(screen.getByRole("status")).toHaveTextContent("edits apply to all of them");
    expect(screen.queryByLabelText("Name")).toBeNull();
    expect(input("Type").value).toBe("text");
    expect(input("Comment").value).toBe("same");
    // Differing attributes read "Multiple values" and start empty.
    expect(input("Default").value).toBe("");
    expect(input("Default")).toHaveAttribute("placeholder", "Multiple values");
    const nullable = screen.getByLabelText("Nullable") as HTMLSelectElement;
    expect(nullable.value).toBe("");
    expect(nullable.options[0]).toHaveTextContent("Multiple values");

    fireEvent.change(input("Default"), { target: { value: "'y'" } });
    fireEvent.keyDown(input("Default"), { key: "Enter" });
    expect(sel.onEdit).toHaveBeenCalledWith("defaultExpr", "'y'");
    fireEvent.change(nullable, { target: { value: "NO" } });
    expect(sel.onEdit).toHaveBeenLastCalledWith("nullable", false);
  });

  it("follows the staged values: a new common value replaces a draft", () => {
    const { rerender } = render(
      <ColumnsPane selection={selection([row("a"), row("b", { comment: "x" })])} />,
    );
    expect(input("Comment").value).toBe("");
    expect(input("Comment")).toHaveAttribute("placeholder", "Multiple values");
    rerender(
      <ColumnsPane
        selection={selection([row("a", { comment: "x" }), row("b", { comment: "x" })])}
      />,
    );
    expect(input("Comment").value).toBe("x");
  });

  it("is read-only on a read-only structure and for dropped columns", () => {
    const { rerender } = render(
      <ColumnsPane selection={selection([row("a")], { editable: false })} />,
    );
    expect(input("Type")).toHaveAttribute("readonly");
    expect(screen.getByLabelText("Nullable")).toBeDisabled();
    expect(screen.getByText("This structure is read-only.")).toBeInTheDocument();

    rerender(<ColumnsPane selection={selection([row("a", { status: "dropped" })])} />);
    expect(input("Type")).toHaveAttribute("readonly");
    expect(screen.getByText("Dropped columns cannot be edited.")).toBeInTheDocument();

    // A dropped column in a mixed selection is left out of the common values and the edits.
    rerender(
      <ColumnsPane
        selection={selection([row("a", { comment: "x" }), row("b", { status: "dropped" })])}
      />,
    );
    expect(input("Comment").value).toBe("x");
    expect(input("Comment")).not.toHaveAttribute("readonly");
  });
});
