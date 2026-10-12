import { describe, expect, it } from "vitest";
import { DEFAULT_KEYMAP, type ShortcutDef } from "@/lib/keys/keymap";
import { resolveKeymap } from "@/lib/keys/overrides";
import { keymapRows, rowConflicts } from "./keymapRows";

const defaults: ShortcutDef[] = [
  { id: "tab.new-query", keys: "Mod+T", scope: "global", label: "New query tab" },
  { id: "tab.new-query", keys: "Mod+E", scope: "global", label: "Open SQL editor" },
  { id: "tab.next", keys: "Mod+]", scope: "global", label: "Next tab" },
  { id: "tab.next", keys: "Ctrl+Tab", scope: "global", label: "Next tab" },
  { id: "palette.databases", keys: "Mod+K", scope: "global", label: "Switch database" },
  { id: "view.refresh", keys: "Mod+R", scope: "global", label: "Refresh" },
];

describe("keymapRows", () => {
  it("groups an action's bindings by label, with their slot and current keys", () => {
    const rows = keymapRows(defaults, { "tab.next": ["Mod+Alt+Right", ""] });
    expect(rows.map((r) => [r.id, r.label, r.modified])).toEqual([
      ["tab.new-query", "New query tab", false],
      ["tab.new-query", "Open SQL editor", false],
      ["tab.next", "Next tab", true],
      ["palette.databases", "Switch database", false],
      ["view.refresh", "Refresh", false],
    ]);
    expect(rows[1]!.slots).toEqual([
      { slot: 1, keys: "Mod+E", defaultKeys: "Mod+E", fixed: false },
    ]);
    expect(rows[2]!.slots).toEqual([
      { slot: 0, keys: "Mod+Alt+Right", defaultKeys: "Mod+]", fixed: false },
      { slot: 1, keys: "", defaultKeys: "Ctrl+Tab", fixed: false },
    ]);
    expect(rows[4]!.slots[0]?.fixed).toBe(true);
  });

  it("flags combos another action of the scope uses", () => {
    const overrides = { "tab.next": ["Mod+K", "Ctrl+Tab"] };
    const rows = keymapRows(defaults, overrides);
    const keymap = resolveKeymap(defaults, overrides);
    const row = (id: string) => rows.find((r) => r.id === id)!;
    expect(rowConflicts(row("tab.next"), keymap)).toEqual(["Switch database"]);
    expect(rowConflicts(row("palette.databases"), keymap)).toEqual(["Next tab"]);
    expect(rowConflicts(row("view.refresh"), keymap)).toEqual([]);
  });

  it("covers every binding of the real keymap once", () => {
    const rows = keymapRows(DEFAULT_KEYMAP, {});
    const slots = rows.reduce((n, r) => n + r.slots.length, 0);
    expect(slots).toBe(new Set(DEFAULT_KEYMAP.map((d) => `${d.id} ${d.label} ${d.keys}`)).size);
    expect(rows.some((r) => r.modified)).toBe(false);
  });
});
