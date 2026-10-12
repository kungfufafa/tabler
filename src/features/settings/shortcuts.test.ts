import { describe, expect, it } from "vitest";
import { KEYMAP, type ShortcutDef } from "@/lib/keys/keymap";
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
    expect(groups.map((g) => g.title)).toEqual(["Global", "Table grid", "SQL editor"]);
    const bindings = groups.flatMap((g) =>
      g.rows.flatMap((r) => r.keys.map((k) => `${r.id} ${k}`)),
    );
    expect(new Set(bindings).size).toBe(bindings.length);
    const unique = new Set(KEYMAP.map((d) => `${d.id} ${d.keys}`));
    expect(bindings).toHaveLength(unique.size);
    const settings = groups[0]?.rows.find((r) => r.id === "settings.open");
    expect(settings?.keys).toEqual(["Mod+,"]);
  });
});

describe("reference shortcuts", () => {
  const rowsOf = (title: string, q = "") =>
    groupShortcuts(KEYMAP, q, true).find((g) => g.title === title)?.rows ?? [];

  it("lists the built-in grid, cell editor and SQL editor keys in their groups", () => {
    const groups = groupShortcuts(KEYMAP, "", true);
    expect(groups.map((g) => g.title)).toEqual([
      "Global",
      "Table grid",
      "Cell editor",
      "SQL editor",
    ]);
    const grid = rowsOf("Table grid");
    // Keymap rows first, then the reference rows, alternatives merged into one row.
    expect(grid[0]?.id).toBe("grid.commit");
    expect(grid.find((r) => r.label === "Edit cell")).toEqual({
      id: "ref:Enter",
      label: "Edit cell",
      keys: ["Enter", "F2"],
    });
    expect(grid.find((r) => r.label === "Move the selection")?.keys).toEqual([
      "Up",
      "Down",
      "Left",
      "Right",
    ]);
    expect(grid.map((r) => r.label)).toEqual(
      expect.arrayContaining([
        "Extend the selection",
        "Jump to the edge",
        "First / last column",
        "First / last row",
        "Page up / down",
        "Next / previous cell",
        "Clear and edit cell",
        "Collapse the selection",
        "Select all",
        "Select a column (on its header)",
      ]),
    );
    expect(rowsOf("Cell editor").map((r) => r.label)).toEqual([
      "Save and move down",
      "Save and move to the next / previous cell",
      "Cancel the edit",
      "New line (opens the multi-line editor)",
      "Save (JSON / multi-line editor)",
    ]);
    const editor = rowsOf("SQL editor");
    expect(editor.find((r) => r.label === "Find next")?.keys).toEqual(["F3", "Mod+G"]);
    expect(editor.find((r) => r.label === "Go to line")?.keys).toEqual(["Mod+Alt+G"]);
    expect(editor.find((r) => r.label === "Show completions")?.keys).toEqual(["Ctrl+Space"]);
  });

  it("is searchable like the keymap (but not by its synthetic ids)", () => {
    const labels = (q: string) =>
      groupShortcuts(KEYMAP, q, true).flatMap((g) => g.rows.map((r) => r.label));
    expect(labels("comment")).toEqual(["Toggle comment"]);
    expect(labels("⌘/")).toEqual(["Toggle comment"]);
    expect(labels("cell editor escape")).toEqual(["Cancel the edit"]);
    expect(labels("f2")).toEqual(["Edit cell"]);
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
});
