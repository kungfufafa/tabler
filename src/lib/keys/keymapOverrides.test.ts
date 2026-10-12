import { afterEach, describe, expect, it, vi } from "vitest";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import { comboFromEvent } from "./combo";
import {
  applyKeymapOverrides,
  DEFAULT_KEYMAP,
  findShortcut,
  getKeymapVersion,
  KEYMAP,
  shortcutFor,
} from "./keymap";
import { installMenuAcceleratorSync } from "./menuSync";
import { EMPTY_OVERRIDES } from "./overrides";

afterEach(() => {
  resetPrefsForTests();
  applyKeymapOverrides(EMPTY_OVERRIDES);
});

describe("keymap overrides", () => {
  it("rewrites KEYMAP in place from the preference", () => {
    expect(shortcutFor("tab.new-query")).toBe("Mod+T");
    const before = getKeymapVersion();
    setPref("keymapOverrides", { "tab.new-query": ["Mod+Y", "Mod+E"] });
    expect(getKeymapVersion()).toBe(before + 1);
    expect(shortcutFor("tab.new-query")).toBe("Mod+Y");
    expect(findShortcut("Mod+Y", "editor")?.id).toBe("tab.new-query");
    expect(findShortcut("Mod+T", "global")).toBeUndefined();
    // The defaults stay available for Settings ▸ Shortcuts.
    expect(DEFAULT_KEYMAP.find((d) => d.id === "tab.new-query")?.keys).toBe("Mod+T");
    setPref("keymapOverrides", EMPTY_OVERRIDES);
    expect(KEYMAP).toEqual(DEFAULT_KEYMAP);
  });

  it("dispatches a remapped key through the shortcut lookup", () => {
    setPref("keymapOverrides", { "workspace.next": ["Mod+Alt+Right"] });
    const combo = comboFromEvent(
      {
        code: "ArrowRight",
        key: "ArrowRight",
        metaKey: true,
        ctrlKey: false,
        altKey: true,
        shiftKey: false,
      },
      true,
    );
    expect(findShortcut(combo, "grid")?.id).toBe("workspace.next");
  });

  it("keeps the TablePlus tab and workspace keys", () => {
    const id = (combo: string) => findShortcut(combo, "global")?.id;
    expect(id("Mod+T")).toBe("tab.new-query");
    expect(id("Mod+W")).toBe("tab.close");
    expect(id("Mod+1")).toBe("tab.goto-1");
    expect(id("Mod+9")).toBe("tab.goto-9");
    expect(id("Mod+[")).toBe("tab.prev");
    expect(id("Mod+]")).toBe("tab.next");
    expect(id("Mod+Shift+T")).toBe("tab.reopen");
    expect(id("Mod+Shift+\\")).toBe("workspace.show");
    expect(id("Mod+Ctrl+Right")).toBe("workspace.next");
    expect(id("Mod+Ctrl+Left")).toBe("workspace.prev");
    expect(id("Ctrl+-")).toBe("tab.back");
    expect(id("Ctrl+Shift+-")).toBe("tab.forward");
    // They work while typing SQL too.
    for (const c of ["Mod+Shift+\\", "Mod+Ctrl+Right", "Ctrl+-", "Mod+]"])
      expect(findShortcut(c, "editor")?.scope).toBe("global");
  });
});

describe("installMenuAcceleratorSync", () => {
  it("sends nothing until a binding differs, then the menu's accelerators", async () => {
    const send = vi.fn(async () => {});
    const stop = installMenuAcceleratorSync(send);
    expect(send).not.toHaveBeenCalled();
    setPref("keymapOverrides", { "tab.close-all": ["Mod+Alt+W"] });
    expect(send).toHaveBeenCalledTimes(1);
    const sent = (send.mock.calls[0] as unknown as [Record<string, string | null>])[0];
    expect(sent["tab.close-all"]).toBe("CmdOrCtrl+Alt+W");
    expect(sent["tab.new-query"]).toBe("CmdOrCtrl+T");
    setPref("keymapOverrides", EMPTY_OVERRIDES);
    expect(send).toHaveBeenCalledTimes(2);
    const reset = (send.mock.calls[1] as unknown as [Record<string, string | null>])[0];
    expect(reset["tab.close-all"]).toBe("CmdOrCtrl+Shift+W");
    stop();
  });
});
