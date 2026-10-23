import { describe, expect, it } from "vitest";
import { findShortcut, KEYMAP, type ShortcutDef } from "@/lib/keys/keymap";
import { groupShortcuts, REFERENCE_SHORTCUTS } from "./shortcuts";

const defs: ShortcutDef[] = [
  { id: "query.run", keys: "Mod+Enter", scope: "editor", label: "Run query" },
  { id: "tab.next", keys: "Ctrl+Tab", scope: "global", label: "Next tab" },
  { id: "grid.commit", keys: "Mod+S", scope: "grid", label: "Commit changes" },
  { id: "tab.next", keys: "Mod+Shift+]", scope: "global", label: "Next tab" },
  { id: "palette.open", keys: "Mod+K", scope: "global", label: "Command palette" },
  { id: "palette.open", keys: "Mod+P", scope: "global", label: "Quick open" },
  // Exact duplicate binding (same id + keys): dropped even with another label.
  { id: "palette.open", keys: "Mod+K", scope: "global", label: "Palette (again)" },
  { id: "tab.next", keys: "Ctrl+Tab", scope: "global", label: "Next tab" },
  { id: "query.save", keys: "Mod+S", scope: "editor", label: "Save query" },
];

describe("groupShortcuts", () => {
  it("groups by scope in a fixed order and merges alternative combos", () => {
    const groups = groupShortcuts(defs, "", true, []);
    expect(groups.map((g) => g.title)).toEqual(["Global", "Table grid", "SQL editor"]);
    expect(groups[0]?.rows).toEqual([
      { id: "tab.next", label: "Next tab", keys: ["Ctrl+Tab", "Mod+Shift+]"] },
      { id: "palette.open", label: "Command palette", keys: ["Mod+K"] },
      { id: "palette.open", label: "Quick open", keys: ["Mod+P"] },
    ]);
    expect(groups[2]?.rows.map((r) => r.id)).toEqual(["query.run", "query.save"]);
  });

  it("filters by label, id, scope and key spelling; every term must match", () => {
    const labels = (q: string) =>
      groupShortcuts(defs, q, true, []).flatMap((g) => g.rows.map((r) => r.label));
    expect(labels("commit")).toEqual(["Commit changes"]);
    expect(labels("QUICK")).toEqual(["Quick open"]);
    expect(labels("query.save")).toEqual(["Save query"]);
    expect(labels("⌘s")).toEqual(["Commit changes", "Save query"]);
    expect(labels("cmd+s editor")).toEqual(["Save query"]);
    expect(labels("mod+shift+]")).toEqual(["Next tab"]);
    expect(labels("table grid")).toEqual(["Commit changes"]);
    expect(labels("  ")).toHaveLength(6);
    expect(labels("nothing-like-this")).toEqual([]);
    expect(groupShortcuts(defs, "nothing-like-this", true, [])).toEqual([]);
  });

  it("uses Ctrl spellings off macOS", () => {
    const labels = (q: string) =>
      groupShortcuts(defs, q, false, []).flatMap((g) => g.rows.map((r) => r.label));
    expect(labels("ctrl+k")).toEqual(["Command palette"]);
    expect(labels("⌘k")).toEqual([]);
  });

  it("covers the real keymap without duplicate bindings", () => {
    const groups = groupShortcuts(KEYMAP, "", true, []);
    expect(groups.map((g) => g.title)).toEqual([
      "Global",
      "Table grid",
      "Filter bar",
      "SQL editor",
    ]);
    const bindings = groups.flatMap((g) =>
      g.rows.flatMap((r) => r.keys.map((k) => `${r.id} ${k}`)),
    );
    expect(new Set(bindings).size).toBe(bindings.length);
    const unique = new Set(KEYMAP.map((d) => `${d.id} ${d.keys}`));
    expect(bindings).toHaveLength(unique.size);
    const settings = groups[0]?.rows.find((r) => r.id === "settings.open");
    expect(settings?.keys).toEqual(["Mod+,"]);
  });

  it("lists the TablePlus editor keys, primary combo first", () => {
    const groups = groupShortcuts(KEYMAP, "", true, []);
    const row = (title: string, label: string) =>
      groups.find((g) => g.title === title)?.rows.find((r) => r.label === label);
    expect(row("SQL editor", "Run current statement")).toEqual({
      id: "query.run-current",
      label: "Run current statement",
      keys: ["Mod+Enter"],
    });
    expect(row("SQL editor", "Run all")?.keys).toEqual(["Mod+Shift+Enter"]);
    expect(row("SQL editor", "Beautify SQL")?.keys).toEqual(["Mod+I", "Alt+Shift+F"]);
    expect(row("SQL editor", "Uglify SQL")?.keys).toEqual(["Mod+Shift+I"]);
    // ⌘E is "Open SQL editor" in the SQL editor too (TablePlus has no key for Explain).
    expect(row("SQL editor", "Explain")?.keys).toEqual(["Mod+Alt+E"]);
    // ⌃` off macOS, where the tests run, is Ctrl+` (`nonMacKeys`).
    expect(row("Global", "Toggle result pane")?.keys).toEqual(["Mod+`"]);
  });
});

describe("reference shortcuts", () => {
  const rowsOf = (title: string, q = "") =>
    groupShortcuts(KEYMAP, q, true).find((g) => g.title === title)?.rows ?? [];

  it("lists the built-in grid, cell editor and SQL editor keys in their groups", () => {
    const groups = groupShortcuts(KEYMAP, "", true);
    expect(groups.map((g) => g.title)).toEqual([
      "Global",
      "Sidebar",
      "Table grid",
      "Cell editor",
      "Filter bar",
      "SQL editor",
    ]);
    const grid = rowsOf("Table grid");
    // Keymap rows first, then the reference rows, alternatives merged into one row.
    const isRef = (r: { id: string }) => r.id.startsWith("ref:");
    const firstRef = grid.findIndex(isRef);
    expect(firstRef).toBeGreaterThan(0);
    expect(grid.slice(0, firstRef).some(isRef)).toBe(false);
    expect(grid.slice(firstRef).every(isRef)).toBe(true);
    // The grid selects rows (TablePlus): typing never edits, ⌫ / ⌦ delete the selected rows.
    expect(grid.find((r) => r.label.startsWith("Edit cell"))).toEqual({
      id: "ref:Enter",
      label: "Edit cell (typing does not edit)",
      keys: ["Enter", "F2"],
    });
    expect(grid.find((r) => r.label === "Select the row above / below")?.keys).toEqual([
      "Up",
      "Down",
    ]);
    expect(grid.find((r) => r.label === "Extend the row selection")?.keys).toEqual([
      "Shift+Up",
      "Shift+Down",
      "Shift+Click",
    ]);
    expect(grid.find((r) => r.label === "Previous / next cell in the row")?.keys).toEqual([
      "Left",
      "Right",
      "Tab",
      "Shift+Tab",
    ]);
    expect(grid.find((r) => r.id.startsWith("ref:") && r.label === "Delete selected rows")).toEqual(
      { id: "ref:Backspace", label: "Delete selected rows", keys: ["Backspace", "Delete"] },
    );
    expect(grid.find((r) => r.id === "grid.copy")?.label).toBe("Copy selected rows");
    expect(grid.map((r) => r.label)).toEqual(
      expect.arrayContaining([
        "Add / remove a row",
        "First / last column",
        "First / last row",
        "Page up / down",
        "Select only the focused row",
        "Select all rows",
        "Select a column (on its header)",
      ]),
    );
    expect(grid.map((r) => r.label)).not.toContain("Clear and edit cell");
    expect(rowsOf("Cell editor").map((r) => r.label)).toEqual([
      "Save and move down",
      "Save and move to the next / previous cell",
      "Cancel the edit",
      "New line (opens the multi-line editor)",
      "Save (JSON / multi-line editor)",
      "Stage the edit and commit changes",
    ]);
    const editor = rowsOf("SQL editor");
    expect(editor.find((r) => r.label === "Find next")?.keys).toEqual(["F3", "Mod+G"]);
    expect(editor.find((r) => r.label === "Go to line")?.keys).toEqual(["Mod+Alt+G"]);
    expect(editor.find((r) => r.label === "Show completions")?.keys).toEqual(["Ctrl+Space"]);
  });

  it("is searchable like the keymap (but not by its synthetic ids)", () => {
    const labels = (q: string) =>
      groupShortcuts(KEYMAP, q, true).flatMap((g) => g.rows.map((r) => r.label));
    expect(labels("comment")).toEqual(["Toggle line comment", "Toggle block comment"]);
    expect(labels("⌘/")).toEqual(["Toggle line comment"]);
    expect(labels("cell editor escape")).toEqual(["Cancel the edit"]);
    expect(labels("f2")).toEqual(["Edit cell (typing does not edit)"]);
    expect(labels("ref:")).toEqual([]);
  });

  it("has no duplicate rows or bindings, and never repeats a keymap binding", () => {
    const groups = groupShortcuts(KEYMAP, "", true);
    for (const g of groups) {
      const ids = g.rows.map((r) => `${r.id}\u0000${r.label}`);
      expect(new Set(ids).size, g.title).toBe(ids.length);
      const keys = g.rows.flatMap((r) => r.keys);
      expect(new Set(keys).size, g.title).toBe(keys.length);
    }
    // Every reference binding is listed once; one the keymap binds too is dropped.
    const total = groups.reduce((n, g) => n + g.rows.reduce((m, r) => m + r.keys.length, 0), 0);
    const unique = new Set(KEYMAP.map((d) => `${d.id} ${d.keys}`));
    expect(total).toBe(unique.size + REFERENCE_SHORTCUTS.length);
    const shadowed = groupShortcuts(
      [{ id: "grid.copy", keys: "Mod+A", scope: "grid", label: "Copy" }],
      "",
      true,
      [{ scope: "grid", keys: "Mod+A", label: "Select all" }],
    );
    expect(shadowed.flatMap((g) => g.rows.map((r) => r.label))).toEqual(["Copy"]);
  });

  it("drops a built-in key that a global shortcut takes in its scope", () => {
    const labels = (defs: ShortcutDef[]) =>
      groupShortcuts(defs, "", true, [
        { scope: "editor", keys: "Mod+D", label: "Select next occurrence" },
      ]).flatMap((g) => g.rows.map((r) => r.label));
    // Global keys reach the editor only when allowed there.
    expect(labels([{ id: "tab.next", keys: "Mod+D", scope: "global", label: "Next" }])).toEqual([
      "Next",
      "Select next occurrence",
    ]);
    expect(
      labels([
        { id: "tab.next", keys: "Mod+D", scope: "global", label: "Next", allowInEditor: true },
      ]),
    ).toEqual(["Next"]);
  });

  it("only lists built-in keys the keymap leaves alone", () => {
    for (const r of REFERENCE_SHORTCUTS)
      expect(findShortcut(r.keys, r.scope), `${r.scope} ${r.keys}`).toBeUndefined();
    // ⌘I is Beautify in the editor now, not CodeMirror's "select parent syntax"; ⌘D stays there.
    expect(findShortcut("Mod+I", "editor")?.id).toBe("query.format");
    expect(REFERENCE_SHORTCUTS.some((r) => r.scope === "editor" && r.keys === "Mod+I")).toBe(false);
    expect(REFERENCE_SHORTCUTS).toContainEqual({
      scope: "editor",
      keys: "Mod+D",
      label: "Select next occurrence",
    });
  });
});
