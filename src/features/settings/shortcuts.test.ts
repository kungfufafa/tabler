import { describe, expect, it } from "vitest";
import { KEYMAP, type ShortcutDef } from "@/lib/keys/keymap";
import { groupShortcuts } from "./shortcuts";

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
    const groups = groupShortcuts(defs, "", true);
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
      groupShortcuts(defs, q, true).flatMap((g) => g.rows.map((r) => r.label));
    expect(labels("commit")).toEqual(["Commit changes"]);
    expect(labels("QUICK")).toEqual(["Quick open"]);
    expect(labels("query.save")).toEqual(["Save query"]);
    expect(labels("⌘s")).toEqual(["Commit changes", "Save query"]);
    expect(labels("cmd+s editor")).toEqual(["Save query"]);
    expect(labels("mod+shift+]")).toEqual(["Next tab"]);
    expect(labels("table grid")).toEqual(["Commit changes"]);
    expect(labels("  ")).toHaveLength(6);
    expect(labels("nothing-like-this")).toEqual([]);
    expect(groupShortcuts(defs, "nothing-like-this", true)).toEqual([]);
  });

  it("uses Ctrl spellings off macOS", () => {
    const labels = (q: string) =>
      groupShortcuts(defs, q, false).flatMap((g) => g.rows.map((r) => r.label));
    expect(labels("ctrl+k")).toEqual(["Command palette"]);
    expect(labels("⌘k")).toEqual([]);
  });

  it("covers the real keymap without duplicate bindings", () => {
    const groups = groupShortcuts(KEYMAP, "", true);
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
