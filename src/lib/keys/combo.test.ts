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
    expect(findShortcut("Mod+R", "editor")).toBeUndefined();
    expect(findShortcut("Mod+W", "dialog")?.id).toBe("tab.close");
    expect(findShortcut("Mod+T", "dialog")).toBeUndefined();
    expect(findShortcut("Mod+Shift+Enter", "grid")).toBeUndefined();
  });
});

describe("formatCombo", () => {
  it("renders mac glyphs", () => {
    expect(formatCombo("Mod+Shift+Enter", true)).toBe("⌘⇧↩");
    expect(formatCombo("Ctrl+Tab", true)).toBe("⌃⇥");
    expect(formatCombo("Mod+K", false)).toBe("Ctrl+K");
  });
});
