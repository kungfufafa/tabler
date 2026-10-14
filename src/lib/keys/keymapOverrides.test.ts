import { afterEach, describe, expect, it, vi } from "vitest";
import { getPrefs, resetPrefsForTests, setPref } from "@/stores/prefs";
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

describe("menus3: File / Edit keys", () => {
  it("splits ⌘R (Reload Workspace) from ⌥⌘R (Reload current tab)", () => {
    expect(findShortcut("Mod+R", "global")?.id).toBe("workspace.reload");
    expect(findShortcut("Mod+R", "editor")?.id).toBe("workspace.reload");
    expect(findShortcut("Mod+Alt+R", "global")?.id).toBe("view.refresh");
    expect(findShortcut("Mod+Alt+R", "editor")).toBeUndefined();
    expect(shortcutFor("workspace.reload")).toBe("Mod+R");
    expect(shortcutFor("view.refresh")).toBe("Mod+Alt+R");
  });

  it("gives ⇧⌘O to New SQL Viewer; the connection manager has no key", () => {
    expect(findShortcut("Mod+Shift+O", "global")?.id).toBe("sqlviewer.new");
    expect(findShortcut("Mod+Shift+O", "editor")?.id).toBe("sqlviewer.new");
    expect(shortcutFor("ui.launcher")).toBeUndefined();
    // ⇧⌘C is the console log, and in a grid (scope wins) Copy Cell Value.
    expect(findShortcut("Mod+Shift+C", "grid")?.id).toBe("grid.copy-cell");
    expect(findShortcut("Mod+Shift+C", "global")?.id).toBe("ui.toggle-console");
    expect(findShortcut("Mod+Shift+C", "sidebar")?.id).toBe("ui.toggle-console");
    expect(shortcutFor("grid.copy-cell")).toBe("Mod+Shift+C");
    expect(shortcutFor("ui.toggle-console")).toBe("Mod+Shift+C");
  });

  it("opens the SQL editor with ⌘↩ only outside the scoped areas", () => {
    expect(findShortcut("Mod+Enter", "global")?.id).toBe("query.open-editor");
    expect(findShortcut("Mod+Enter", "grid")?.id).toBe("grid.quick-look");
    expect(findShortcut("Mod+Enter", "filter")?.id).toBe("filter.apply");
    expect(findShortcut("Mod+Enter", "editor")?.id).toBe("query.run-current");
    // The sidebar's lists (History, Queries) and cell editors keep ⌘↩ for themselves.
    for (const scope of ["sidebar", "grid-edit", "dialog"] as const)
      expect(findShortcut("Mod+Enter", scope), scope).toBeUndefined();
  });

  it("splits the workspace with ⇧⌘D from anywhere but the SQL editor, cell editors and dialogs", () => {
    expect(findShortcut("Mod+Shift+D", "global")?.id).toBe("workspace.split-right");
    // The grid, the sidebar and the filter bar no longer keep the key for themselves.
    for (const scope of ["grid", "sidebar", "filter"] as const)
      expect(findShortcut("Mod+Shift+D", scope)?.id, scope).toBe("workspace.split-right");
    // The SQL editor splits its pane; Set DEFAULT moved to ⌥⌘D.
    expect(findShortcut("Mod+Shift+D", "editor")?.id).toBe("query.split");
    expect(findShortcut("Mod+Alt+D", "grid")?.id).toBe("grid.set-default");
    expect(shortcutFor("grid.set-default")).toBe("Mod+Alt+D");
    // A cell editor (and the inspector's fields) and dialogs keep it: splitting remounts the tab
    // body under an open edit.
    for (const scope of ["grid-edit", "dialog"] as const)
      expect(findShortcut("Mod+Shift+D", scope), scope).toBeUndefined();
    const split = KEYMAP.find((d) => d.id === "workspace.split-right");
    expect(split).toMatchObject({ notWhileEditing: true });
    expect(split?.unscopedOnly).toBeUndefined();
  });

  it("opens a SQL editor with ⌘E also from the SQL editor; Explain is ⌥⌘E", () => {
    expect(shortcutFor("query.explain")).toBe("Mod+Alt+E");
    expect(findShortcut("Mod+E", "editor")?.id).toBe("tab.new-query");
    expect(KEYMAP.find((d) => d.id === "tab.new-query" && d.keys === "Mod+E")).toMatchObject({
      allowInEditor: true,
    });
    expect(findShortcut("Mod+Alt+E", "editor")?.id).toBe("query.explain");
  });

  it("previews and discards from the SQL editor too", () => {
    for (const id of ["grid.preview", "grid.discard"])
      expect(KEYMAP.find((d) => d.id === id)).toMatchObject({
        allowInEditor: true,
        notWhileEditing: true,
      });
  });

  it("lists the comment toggles as remappable SQL editor keys", () => {
    expect(findShortcut("Mod+/", "editor")).toMatchObject({
      id: "query.comment-line",
      editorCommand: true,
    });
    expect(findShortcut("Mod+Alt+/", "editor")?.id).toBe("query.comment-block");
    setPref("keymapOverrides", { "query.comment-line": ["Mod+Shift+7"] });
    expect(findShortcut("Mod+/", "editor")).toBeUndefined();
    expect(findShortcut("Mod+Shift+7", "editor")?.id).toBe("query.comment-line");
  });

  it("brings a saved ⌥⌘R remap up to date (the old ⌘R slot is gone)", () => {
    setPref("keymapOverrides", { "view.refresh": ["Mod+R", "Mod+Y"] });
    expect(getPrefs().keymapOverrides).toEqual({ "view.refresh": ["Mod+Y"] });
    expect(shortcutFor("view.refresh")).toBe("Mod+Y");
    expect(findShortcut("Mod+R", "global")?.id).toBe("workspace.reload");
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
