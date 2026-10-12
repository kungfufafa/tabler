import { EditorSelection, EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";
import { cursorLabel } from "./cursor";

const state = (doc: string, selection: EditorSelection) =>
  EditorState.create({
    doc,
    selection,
    extensions: EditorState.allowMultipleSelections.of(true),
  });

describe("cursorLabel", () => {
  it("gives the 1-based line and column of the cursor", () => {
    expect(cursorLabel(null)).toBe("Line 1, Column 1");
    const doc = "select 1;\nselect *\n  from users;";
    expect(cursorLabel(state(doc, EditorSelection.single(0)))).toBe("Line 1, Column 1");
    expect(cursorLabel(state(doc, EditorSelection.single(9)))).toBe("Line 1, Column 10");
    expect(cursorLabel(state(doc, EditorSelection.single(doc.indexOf("users"))))).toBe(
      "Line 3, Column 8",
    );
  });

  it("counts the selected characters of every range", () => {
    const doc = "x".repeat(2000);
    expect(cursorLabel(state(doc, EditorSelection.single(4, 5)))).toBe("1 character selected");
    expect(cursorLabel(state(doc, EditorSelection.single(1500, 0)))).toBe(
      "1,500 characters selected",
    );
    const multi = EditorSelection.create([
      EditorSelection.range(0, 3),
      EditorSelection.cursor(10),
      EditorSelection.range(20, 24),
    ]);
    expect(cursorLabel(state(doc, multi))).toBe("7 characters selected");
  });
});
