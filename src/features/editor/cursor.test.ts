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
  it("gives the 1-based line, column and location of the cursor", () => {
    expect(cursorLabel(null)).toBe("Line 1, Column 1, Location 1");
    const doc = "select 1;\nselect *\n  from users;";
    expect(cursorLabel(state(doc, EditorSelection.single(0)))).toBe("Line 1, Column 1, Location 1");
    expect(cursorLabel(state(doc, EditorSelection.single(9)))).toBe(
      "Line 1, Column 10, Location 10",
    );
    const users = doc.indexOf("users");
    expect(cursorLabel(state(doc, EditorSelection.single(users)))).toBe(
      `Line 3, Column 8, Location ${users + 1}`,
    );
  });

  it("counts the location from the start of the document, not the line", () => {
    // "select 1;\n" is 10 characters, so offset 12 is the 3rd column of line 2, location 13.
    const doc = "select 1;\nabcdef";
    expect(cursorLabel(state(doc, EditorSelection.single(12)))).toBe(
      "Line 2, Column 3, Location 13",
    );
    expect(cursorLabel(state("a\nb\ncd", EditorSelection.single(5)))).toBe(
      "Line 3, Column 2, Location 6",
    );
  });

  it("keeps line, column and location while text is selected and adds the selected count", () => {
    const doc = "x".repeat(2000);
    expect(cursorLabel(state(doc, EditorSelection.single(4, 5)))).toBe(
      "Line 1, Column 6, Location 6 (1 character selected)",
    );
    // The head is where the selection was extended to.
    expect(cursorLabel(state(doc, EditorSelection.single(1500, 0)))).toBe(
      "Line 1, Column 1, Location 1 (1,500 characters selected)",
    );
    const multi = EditorSelection.create(
      [EditorSelection.range(0, 3), EditorSelection.cursor(10), EditorSelection.range(20, 24)],
      2,
    );
    expect(cursorLabel(state(doc, multi))).toBe(
      "Line 1, Column 25, Location 25 (7 characters selected)",
    );
  });
});
