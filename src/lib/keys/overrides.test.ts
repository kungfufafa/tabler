import { describe, expect, it } from "vitest";
import type { ShortcutDef } from "./keymap";
import {
  captureResult,
  comboProblem,
  comboToAccelerator,
  conflictsWith,
  EMPTY_OVERRIDES,
  isKeymapOverrides,
  isValidCombo,
  menuAccelerators,
  migrateKeymapOverrides,
  resetBindings,
  resolveKeymap,
  setBinding,
  slotKeys,
} from "./overrides";

const defaults: ShortcutDef[] = [
  {
    id: "tab.new-query",
    keys: "Mod+T",
    scope: "global",
    label: "New query tab",
    allowInEditor: true,
  },
  { id: "tab.new-query", keys: "Mod+E", scope: "global", label: "Open SQL editor" },
  { id: "tab.next", keys: "Mod+]", scope: "global", label: "Next tab" },
  { id: "palette.databases", keys: "Mod+K", scope: "global", label: "Switch database" },
  { id: "view.refresh", keys: "Mod+R", scope: "global", label: "Refresh" },
  { id: "view.refresh", keys: "Mod+Alt+R", scope: "global", label: "Reload current tab" },
  { id: "ui.toggle-sidebar", keys: "Mod+0", scope: "global", label: "Toggle sidebar" },
  { id: "grid.toggle-detail", keys: "Space", scope: "grid", label: "Toggle row detail" },
  { id: "query.explain", keys: "Mod+E", scope: "editor", label: "Explain" },
];

describe("isValidCombo / isKeymapOverrides", () => {
  it("accepts KEYMAP spellings only", () => {
    for (const ok of [
      "Mod+T",
      "Mod+Shift+\\",
      "Mod+Ctrl+Right",
      "Ctrl+-",
      "Alt+Shift+F",
      "F5",
      "Space",
    ])
      expect(isValidCombo(ok), ok).toBe(true);
    for (const bad of ["", "Mod+", "Shift+Mod+T", "Mod+Meta", "Cmd+T", "Mod+Dead", "mod+t", 3])
      expect(isValidCombo(bad), String(bad)).toBe(false);
  });

  it("validates the stored preference shape", () => {
    expect(isKeymapOverrides({})).toBe(true);
    expect(
      isKeymapOverrides({ "tab.next": ["Mod+Alt+Right"], "tab.new-query": ["", "Mod+E"] }),
    ).toBe(true);
    expect(isKeymapOverrides(null)).toBe(false);
    expect(isKeymapOverrides([])).toBe(false);
    expect(isKeymapOverrides({ "tab.next": "Mod+]" })).toBe(false);
    expect(isKeymapOverrides({ "tab.next": ["Cmd+]"] })).toBe(false);
  });
});

describe("resolveKeymap", () => {
  it("returns the defaults untouched without overrides", () => {
    expect(resolveKeymap(defaults, EMPTY_OVERRIDES)).toEqual(defaults);
  });

  it("rebinds per slot, keeps labels and flags, and drops removed bindings", () => {
    const keymap = resolveKeymap(defaults, { "tab.new-query": ["Mod+Y", ""] });
    const own = keymap.filter((d) => d.id === "tab.new-query");
    expect(own).toEqual([
      {
        id: "tab.new-query",
        keys: "Mod+Y",
        scope: "global",
        label: "New query tab",
        allowInEditor: true,
      },
    ]);
    // Order (and thus precedence) is the keymap's.
    expect(keymap.map((d) => d.id).indexOf("tab.new-query")).toBe(0);
  });

  it("keeps fixed bindings (⌘R, ⌘0) and ignores invalid or extra slots", () => {
    const keymap = resolveKeymap(defaults, {
      "view.refresh": ["Mod+Y", "Mod+Shift+Y"],
      "ui.toggle-sidebar": [""],
      "tab.next": ["not a combo", "Mod+Z"],
    });
    expect(keymap.filter((d) => d.id === "view.refresh").map((d) => d.keys)).toEqual([
      "Mod+R",
      "Mod+Shift+Y",
    ]);
    expect(keymap.find((d) => d.id === "ui.toggle-sidebar")?.keys).toBe("Mod+0");
    expect(keymap.filter((d) => d.id === "tab.next").map((d) => d.keys)).toEqual(["Mod+]"]);
  });
});

describe("migrateKeymapOverrides", () => {
  it("drops the old fixed ⌘R slot of view.refresh (⌘R is Reload Workspace now)", () => {
    expect(migrateKeymapOverrides({ "view.refresh": ["Mod+R", "Mod+Y"] })).toEqual({
      "view.refresh": ["Mod+Y"],
    });
    // Back on its default (⌥⌘R) once the old slot is gone: nothing left to keep.
    expect(
      migrateKeymapOverrides({ "view.refresh": ["Mod+R", "Mod+Alt+R"], "tab.next": ["Mod+J"] }),
    ).toEqual({ "tab.next": ["Mod+J"] });
    expect(migrateKeymapOverrides({ "view.refresh": ["Mod+R", ""] })).toEqual({
      "view.refresh": [""],
    });
  });

  it("returns current overrides as they are", () => {
    const current = { "view.refresh": ["Mod+Y"], "tab.next": ["Mod+J"] };
    expect(migrateKeymapOverrides(current)).toBe(current);
    expect(migrateKeymapOverrides(EMPTY_OVERRIDES)).toBe(EMPTY_OVERRIDES);
  });

  it("keeps ⌘R fixed on Reload Workspace", () => {
    const reload: ShortcutDef[] = [
      { id: "workspace.reload", keys: "Mod+R", scope: "global", label: "Reload workspace" },
      { id: "view.refresh", keys: "Mod+Alt+R", scope: "global", label: "Reload current tab" },
    ];
    expect(setBinding(reload, EMPTY_OVERRIDES, "workspace.reload", 0, "Mod+Y")).toBe(
      EMPTY_OVERRIDES,
    );
    // ⌥⌘R moves, but never onto ⌘R.
    expect(comboProblem("Mod+R", reload[1]!)).toBe("reserved");
    expect(setBinding(reload, EMPTY_OVERRIDES, "view.refresh", 0, "Mod+Y")).toEqual({
      "view.refresh": ["Mod+Y"],
    });
  });
});

describe("setBinding / resetBindings", () => {
  it("stores one combo per slot and forgets an action back on its defaults", () => {
    let o = setBinding(defaults, EMPTY_OVERRIDES, "tab.new-query", 1, "Mod+Shift+Y");
    expect(o).toEqual({ "tab.new-query": ["Mod+T", "Mod+Shift+Y"] });
    o = setBinding(defaults, o, "tab.new-query", 0, "");
    expect(slotKeys(defaults, o, "tab.new-query")).toEqual(["", "Mod+Shift+Y"]);
    o = setBinding(defaults, o, "tab.new-query", 0, "Mod+T");
    o = setBinding(defaults, o, "tab.new-query", 1, "Mod+E");
    expect(o).toEqual({});
  });

  it("refuses fixed bindings, unknown slots and invalid combos", () => {
    expect(setBinding(defaults, EMPTY_OVERRIDES, "view.refresh", 0, "Mod+Y")).toBe(EMPTY_OVERRIDES);
    expect(setBinding(defaults, EMPTY_OVERRIDES, "tab.next", 3, "Mod+Y")).toBe(EMPTY_OVERRIDES);
    expect(setBinding(defaults, EMPTY_OVERRIDES, "tab.next", 0, "Cmd+Y")).toBe(EMPTY_OVERRIDES);
    // The fixed slot stays, the other one of the same action can move.
    expect(setBinding(defaults, EMPTY_OVERRIDES, "view.refresh", 1, "Mod+Y")).toEqual({
      "view.refresh": ["Mod+R", "Mod+Y"],
    });
  });

  it("resets some slots or the whole action", () => {
    const o = { "tab.new-query": ["Mod+Y", "Mod+U"], "tab.next": ["Mod+Alt+Right"] };
    expect(resetBindings(defaults, o, "tab.new-query", [1])).toEqual({
      "tab.new-query": ["Mod+Y", "Mod+E"],
      "tab.next": ["Mod+Alt+Right"],
    });
    expect(resetBindings(defaults, o, "tab.new-query")).toEqual({ "tab.next": ["Mod+Alt+Right"] });
    expect(resetBindings(defaults, o, "palette.databases")).toBe(o);
  });
});

describe("conflictsWith", () => {
  it("finds other actions in the same scope only", () => {
    const keymap = resolveKeymap(defaults, { "tab.next": ["Mod+K"] });
    expect(conflictsWith(keymap, "global", "Mod+K", "tab.next").map((d) => d.id)).toEqual([
      "palette.databases",
    ]);
    // The same action twice is no conflict; ⌘E in the editor is Explain, a different scope.
    expect(conflictsWith(keymap, "global", "Mod+T", "tab.new-query")).toEqual([]);
    expect(conflictsWith(keymap, "global", "Mod+E", "tab.new-query")).toEqual([]);
    expect(conflictsWith(keymap, "editor", "Mod+E", "tab.new-query").map((d) => d.id)).toEqual([
      "query.explain",
    ]);
  });
});

describe("recording a shortcut", () => {
  const ev = (
    key: string,
    mods: Partial<Record<"metaKey" | "ctrlKey" | "altKey" | "shiftKey", boolean>> = {},
  ) => ({
    key,
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    ...mods,
  });

  it("maps ⎋ to cancel, ⌫ / ⌦ to clear and modifiers alone to waiting", () => {
    expect(captureResult(ev("Escape"), "Escape")).toEqual({ type: "cancel" });
    expect(captureResult(ev("Backspace"), "Backspace")).toEqual({ type: "clear" });
    expect(captureResult(ev("Delete"), "Delete")).toEqual({ type: "clear" });
    expect(captureResult(ev("Meta", { metaKey: true }), "Mod+Meta")).toEqual({ type: "pending" });
    expect(captureResult(ev("Shift", { shiftKey: true }), "Shift+Shift")).toEqual({
      type: "pending",
    });
    expect(captureResult(ev("Backspace", { metaKey: true }), "Mod+Backspace")).toEqual({
      type: "combo",
      combo: "Mod+Backspace",
    });
    expect(captureResult(ev("y", { metaKey: true }), "Mod+Y")).toEqual({
      type: "combo",
      combo: "Mod+Y",
    });
  });

  it("refuses reserved combos and plain keys", () => {
    const tab = { id: "tab.next", keys: "Mod+]" } as const;
    expect(comboProblem("Mod+R", tab)).toBe("reserved");
    expect(comboProblem("Mod+0", tab)).toBe("reserved");
    expect(comboProblem("Mod+Q", tab)).toBe("reserved");
    expect(comboProblem("Shift+A", tab)).toBe("needs-modifier");
    expect(comboProblem("A", tab)).toBe("needs-modifier");
    expect(comboProblem("Mod+Meta", tab)).toBe("invalid");
    expect(comboProblem("F6", tab)).toBeNull();
    expect(comboProblem("Ctrl+Alt+]", tab)).toBeNull();
    // A binding built without a modifier may go back to it.
    expect(comboProblem("Space", { id: "grid.toggle-detail", keys: "Space" })).toBeNull();
    // ⌘R keeps being ⌘R for Refresh.
    expect(comboProblem("Mod+R", { id: "view.refresh", keys: "Mod+R" })).toBeNull();
  });
});

describe("native menu accelerators", () => {
  it("spells combos the Tauri way", () => {
    expect(comboToAccelerator("Mod+Shift+T")).toBe("CmdOrCtrl+Shift+T");
    expect(comboToAccelerator("Mod+Ctrl+[")).toBe("CmdOrCtrl+Ctrl+[");
    expect(comboToAccelerator("Ctrl+-")).toBe("Ctrl+-");
  });

  it("uses each action's primary combo, null when it has none", () => {
    const keymap = resolveKeymap(defaults, { "tab.new-query": ["", "Mod+E"], "tab.next": [""] });
    expect(menuAccelerators(keymap, ["tab.new-query", "tab.next", "view.refresh"])).toEqual({
      "tab.new-query": "CmdOrCtrl+E",
      "tab.next": null,
      "view.refresh": "CmdOrCtrl+R",
    });
  });
});
