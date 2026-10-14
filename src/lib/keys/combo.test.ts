import { describe, expect, it } from "vitest";
import { comboFromEvent, formatCombo } from "./combo";
import { findShortcut } from "./keymap";

const ev = (
  over: Partial<KeyboardEvent>,
): Pick<KeyboardEvent, "code" | "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey"> => ({
  code: "",
  key: "",
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  ...over,
});

describe("comboFromEvent", () => {
  it("uses layout-stable codes and Mod for ⌘ on mac", () => {
    expect(comboFromEvent(ev({ code: "KeyW", key: "w", metaKey: true }), true)).toBe("Mod+W");
    expect(comboFromEvent(ev({ code: "KeyF", key: "ƒ", altKey: true, shiftKey: true }), true)).toBe(
      "Alt+Shift+F",
    );
    expect(
      comboFromEvent(ev({ code: "Enter", key: "Enter", metaKey: true, shiftKey: true }), true),
    ).toBe("Mod+Shift+Enter");
    expect(
      comboFromEvent(ev({ code: "BracketRight", key: "}", metaKey: true, shiftKey: true }), true),
    ).toBe("Mod+Shift+]");
    expect(comboFromEvent(ev({ code: "Tab", key: "Tab", ctrlKey: true }), true)).toBe("Ctrl+Tab");
    expect(comboFromEvent(ev({ code: "Digit3", key: "3", metaKey: true }), true)).toBe("Mod+3");
  });
  it("maps Ctrl to Mod on other platforms", () => {
    expect(comboFromEvent(ev({ code: "KeyS", key: "s", ctrlKey: true }), false)).toBe("Mod+S");
  });
});

describe("findShortcut", () => {
  it("prefers the focused scope and respects editor/dialog allowlists", () => {
    expect(findShortcut("Mod+S", "grid")?.id).toBe("grid.commit");
    expect(findShortcut("Mod+S", "editor")?.id).toBe("query.save");
    expect(findShortcut("Mod+W", "editor")?.id).toBe("tab.close");
    // ⌘R reloads the workspace from the editor too; ⌥⌘R (the current tab) stays out of it.
    expect(findShortcut("Mod+R", "editor")?.id).toBe("workspace.reload");
    expect(findShortcut("Mod+Alt+R", "editor")).toBeUndefined();
    expect(findShortcut("Mod+W", "dialog")?.id).toBe("tab.close");
    expect(findShortcut("Mod+T", "dialog")).toBeUndefined();
    expect(findShortcut("Mod+Shift+Enter", "grid")).toBeUndefined();
  });

  it("follows TablePlus's defaults", () => {
    expect(findShortcut("Mod+P", "global")?.id).toBe("palette.open");
    expect(findShortcut("Mod+K", "grid")?.id).toBe("palette.databases");
    expect(findShortcut("Mod+Shift+K", "editor")?.id).toBe("palette.connections");
    expect(findShortcut("Mod+Enter", "editor")?.id).toBe("query.run-current");
    expect(findShortcut("Mod+Shift+Enter", "editor")?.id).toBe("query.run");
    expect(findShortcut("Mod+Ctrl+]", "grid")?.id).toBe("view.structure");
    expect(findShortcut("Mod+Shift+Z", "grid")?.id).toBe("grid.redo");
    expect(findShortcut("Mod+Shift+Backspace", "grid")?.id).toBe("grid.discard");
    expect(findShortcut("Space", "grid")?.id).toBe("grid.toggle-detail");
    expect(findShortcut("Space", "global")).toBeUndefined();
  });

  it("resolves ⌘E and ⌘I by scope", () => {
    // ⌘E opens a SQL editor everywhere, the SQL editor included; Explain is ⌥⌘E there.
    expect(findShortcut("Mod+E", "grid")?.id).toBe("tab.new-query");
    expect(findShortcut("Mod+E", "global")?.id).toBe("tab.new-query");
    expect(findShortcut("Mod+E", "editor")?.id).toBe("tab.new-query");
    expect(findShortcut("Mod+Alt+E", "editor")?.id).toBe("query.explain");
    expect(findShortcut("Mod+Alt+E", "grid")).toBeUndefined();
    expect(findShortcut("Mod+I", "grid")?.id).toBe("grid.new-row");
    expect(findShortcut("Mod+I", "editor")?.id).toBe("query.format");
    expect(findShortcut("Mod+Shift+I", "editor")?.id).toBe("query.minify");
    expect(findShortcut("Mod+Shift+I", "grid")?.id).toBe("ui.toggle-inspector");
  });

  it("keeps staging shortcuts away from an open cell editor", () => {
    expect(findShortcut("Mod+S", "global")?.id).toBe("grid.commit");
    expect(findShortcut("Mod+S", "grid-edit")).toBeUndefined();
    expect(findShortcut("Mod+Shift+P", "grid-edit")).toBeUndefined();
    expect(findShortcut("Mod+W", "grid-edit")?.id).toBe("tab.close");
    // Switching the view would unmount the editor before its value is staged.
    expect(findShortcut("Mod+Ctrl+]", "grid-edit")).toBeUndefined();
    expect(findShortcut("Mod+Ctrl+[", "grid-edit")).toBeUndefined();
  });

  it("answers ⇧⌘P and ⇧⌘⌫ from the SQL editor too (workspace-wide), but not from a cell editor", () => {
    expect(findShortcut("Mod+Shift+P", "editor")?.id).toBe("grid.preview");
    expect(findShortcut("Mod+Shift+Backspace", "editor")?.id).toBe("grid.discard");
    expect(findShortcut("Mod+Shift+P", "global")?.id).toBe("grid.preview");
    expect(findShortcut("Mod+Shift+P", "grid-edit")).toBeUndefined();
    expect(findShortcut("Mod+Shift+Backspace", "grid-edit")).toBeUndefined();
    // Not inside a dialog; ⌘S commits only where the editor does not keep the key.
    expect(findShortcut("Mod+Shift+P", "dialog")).toBeUndefined();
    expect(findShortcut("Mod+S", "editor")?.id).toBe("query.save");
  });

  it("splits the workspace with ⇧⌘D everywhere but the SQL editor, cell editors and dialogs", () => {
    for (const scope of ["global", "grid", "sidebar", "filter"] as const)
      expect(findShortcut("Mod+Shift+D", scope)?.id, scope).toBe("workspace.split-right");
    expect(findShortcut("Mod+Shift+D", "editor")?.id).toBe("query.split");
    expect(findShortcut("Mod+Shift+D", "grid-edit")).toBeUndefined();
    expect(findShortcut("Mod+Shift+D", "dialog")).toBeUndefined();
    // Set DEFAULT moved to ⌥⌘D (⌥⇧⌘D too: macOS keeps ⌥⌘D for the Dock).
    expect(findShortcut("Mod+Alt+D", "grid")?.id).toBe("grid.set-default");
    expect(findShortcut("Mod+Alt+Shift+D", "grid")?.id).toBe("grid.set-default");
    expect(findShortcut("Mod+Alt+D", "global")).toBeUndefined();
  });

  it("copies the cell value with ⇧⌘C in a grid and toggles the console log elsewhere", () => {
    expect(findShortcut("Mod+Shift+C", "grid")?.id).toBe("grid.copy-cell");
    for (const scope of ["global", "sidebar", "filter", "editor"] as const)
      expect(findShortcut("Mod+Shift+C", scope)?.id, scope).toBe("ui.toggle-console");
  });
});

describe("formatCombo", () => {
  it("renders mac glyphs", () => {
    expect(formatCombo("Mod+Shift+Enter", true)).toBe("⌘⇧↩");
    expect(formatCombo("Ctrl+Tab", true)).toBe("⌃⇥");
    expect(formatCombo("Mod+K", false)).toBe("Ctrl+K");
  });
});
