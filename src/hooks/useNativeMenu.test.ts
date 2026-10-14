import { renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { actions, type ActionId } from "@/features/palette/actions";
import { DEFAULT_KEYMAP } from "@/lib/keys/keymap";
import { SYNCED_IDS } from "@/lib/keys/menuSync";
import { comboToAccelerator } from "@/lib/keys/overrides";
import {
  dispatchMenuAction,
  isMenuAction,
  MENU_ACTIONS,
  MENU_UNBOUND_ACTIONS,
  menuActionAllowed,
  useNativeMenu,
  type MenuActionId,
} from "./useNativeMenu";
import { handleQuitRequest } from "./useUnsavedGuard";

type Handler = (payload: unknown) => void;
const events = vi.hoisted(() => ({
  handlers: new Map<string, Handler>(),
  unlisten: vi.fn(),
}));
vi.mock("@/lib/ipc/events", () => ({
  listenEvent: vi.fn(async (name: string, handler: Handler) => {
    events.handlers.set(name, handler);
    return events.unlisten;
  }),
}));

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("useNativeMenu", () => {
  let unregister: Array<() => void> = [];
  const register = (id: ActionId) => {
    const fn = vi.fn();
    unregister.push(actions.register(id, fn));
    return fn;
  };

  beforeEach(() => {
    events.handlers.clear();
    events.unlisten.mockClear();
  });
  afterEach(() => {
    unregister.forEach((u) => u());
    unregister = [];
  });

  it("knows exactly the menu's action ids", () => {
    expect(MENU_ACTIONS).toContain("tab.new-query");
    // TablePlus's newer items: File / Edit / View / Navigate.
    for (const id of [
      "query.save-as",
      "query.save-selection",
      "query.add-to-queries",
      "query.format",
      "query.comment-line",
      "query.split",
      "grid.delete-rows",
      "grid.set-now",
      "grid.copy-as",
      "grid.filter",
      "grid.columns",
      "tab.back",
      "tab.goto-9",
      "ui.next-pane",
      "ui.prev-pane",
    ])
      expect(isMenuAction(id), id).toBe(true);
    expect(new Set(MENU_ACTIONS).size).toBe(MENU_ACTIONS.length);
    expect(isMenuAction("ui.toggle-sidebar")).toBe(true);
    expect(isMenuAction("grid.commit")).toBe(false);
    expect(isMenuAction("app.quit")).toBe(false);
    expect(isMenuAction(42)).toBe(false);
  });

  it("dispatches menu:action events into the action registry", async () => {
    const toggle = register("ui.toggle-sidebar");
    const commit = register("grid.commit");
    const { unmount } = renderHook(() => useNativeMenu());
    await flush();
    const emit = events.handlers.get("menu:action");
    expect(emit).toBeDefined();
    emit!("ui.toggle-sidebar");
    await flush();
    expect(toggle).toHaveBeenCalledTimes(1);
    // Ids the menu never sends are ignored, even when a handler exists.
    emit!("grid.commit");
    emit!("rm -rf");
    await flush();
    expect(commit).not.toHaveBeenCalled();
    unmount();
    expect(events.unlisten).toHaveBeenCalledTimes(1);
  });

  it("opens a query tab first when Open SQL File… has no query tab to handle it", async () => {
    const open = vi.fn();
    register("tab.new-query").mockImplementation(() => {
      // The new tab registers its handlers once it has rendered.
      setTimeout(() => unregister.push(actions.register("query.open", open)), 0);
    });
    await expect(dispatchMenuAction("query.open")).resolves.toBe(true);
    expect(open).toHaveBeenCalledTimes(1);
  });

  it("reports unhandled actions", async () => {
    await expect(dispatchMenuAction("view.refresh")).resolves.toBe(false);
  });

  describe("with focus in a dialog or an editor", () => {
    let host: HTMLElement;
    const focusInside = (html: string) => {
      host.innerHTML = html;
      host.querySelector<HTMLElement>("input, textarea")!.focus();
    };
    beforeEach(() => {
      host = document.createElement("div");
      document.body.appendChild(host);
    });
    afterEach(() => host.remove());

    it("does not run actions behind an open dialog (the keymap suppresses them there)", async () => {
      const newQuery = register("tab.new-query");
      const launcher = register("ui.launcher");
      const refresh = register("view.refresh");
      const open = register("query.open");
      focusInside(`<div role="dialog"><input aria-label="Host" /></div>`);
      for (const id of ["tab.new-query", "ui.launcher", "view.refresh", "query.open"]) {
        await expect(dispatchMenuAction(id)).resolves.toBe(false);
      }
      focusInside(`<div role="alertdialog"><input aria-label="Name" /></div>`);
      await expect(dispatchMenuAction("tab.new-query")).resolves.toBe(false);
      expect(newQuery).not.toHaveBeenCalled();
      expect(launcher).not.toHaveBeenCalled();
      expect(refresh).not.toHaveBeenCalled();
      expect(open).not.toHaveBeenCalled();
    });

    it("still runs the actions the keymap allows in dialogs", async () => {
      const close = register("tab.close");
      focusInside(`<div role="dialog"><input aria-label="Host" /></div>`);
      await expect(dispatchMenuAction("tab.close")).resolves.toBe(true);
      expect(close).toHaveBeenCalledTimes(1);
    });

    it("applies the editor rule (allowInEditor, editor-scoped items)", async () => {
      const newQuery = register("tab.new-query");
      const refresh = register("view.refresh");
      const open = register("query.open");
      focusInside(`<div class="cm-editor"><textarea aria-label="SQL"></textarea></div>`);
      await expect(dispatchMenuAction("tab.new-query")).resolves.toBe(true);
      await expect(dispatchMenuAction("query.open")).resolves.toBe(true);
      await expect(dispatchMenuAction("view.refresh")).resolves.toBe(false);
      expect(newQuery).toHaveBeenCalledTimes(1);
      expect(open).toHaveBeenCalledTimes(1);
      expect(refresh).not.toHaveBeenCalled();
    });

    it("runs everything when focus is outside dialogs and editors", async () => {
      const refresh = register("view.refresh");
      focusInside(`<div data-scope="grid"><input aria-label="cell" /></div>`);
      await expect(dispatchMenuAction("view.refresh")).resolves.toBe(true);
      expect(refresh).toHaveBeenCalledTimes(1);
    });
  });
});

describe("menu accelerators", () => {
  it("syncs every keyed item's keymap combo, never the context-dependent grid ones", () => {
    expect(MENU_UNBOUND_ACTIONS).toEqual([
      "grid.new-row",
      "grid.duplicate-row",
      "grid.set-default",
    ]);
    for (const id of MENU_UNBOUND_ACTIONS) expect(SYNCED_IDS).not.toContain(id);
    // Their keys are the editor items' (⌘I Beautify, ⇧⌘D Split Pane): the page decides by focus.
    expect(SYNCED_IDS).toEqual(expect.arrayContaining(["query.format", "query.split"]));
    const modifiers = new Set(["CmdOrCtrl", "Ctrl", "Alt", "Shift"]);
    for (const id of SYNCED_IDS) {
      const combo = DEFAULT_KEYMAP.find((d) => d.id === id)!.keys;
      const parts = comboToAccelerator(combo).split("+");
      expect(
        parts.slice(0, -1).every((p) => modifiers.has(p)),
        `${id}: ${combo}`,
      ).toBe(true);
    }
  });

  it("gives the pane keys (⇧⌘] / ⇧⌘[) to Select Next / Previous Pane", () => {
    const keys = (id: string) => DEFAULT_KEYMAP.filter((d) => d.id === id).map((d) => d.keys);
    expect(keys("ui.next-pane")).toEqual(["Mod+Shift+]"]);
    expect(keys("ui.prev-pane")).toEqual(["Mod+Shift+["]);
    expect(keys("tab.next")).not.toContain("Mod+Shift+]");
  });
});

/** The Edit menu's SQL editor items. */
const EDITOR_EDIT_ITEMS: readonly string[] = [
  "query.format",
  "query.minify",
  "query.comment-line",
  "query.comment-block",
  "query.split",
];

describe("menuActionAllowed", () => {
  it("mirrors findShortcut's scope rules for every menu action", () => {
    for (const id of MENU_ACTIONS) {
      expect(menuActionAllowed(id, "global")).toBe(true);
      expect(menuActionAllowed(id, "grid"), id).toBe(!EDITOR_EDIT_ITEMS.includes(id));
      expect(menuActionAllowed(id, "dialog")).toBe(id === "tab.close");
    }
    expect(menuActionAllowed("view.refresh", "editor")).toBe(false);
    expect(menuActionAllowed("view.structure", "grid-edit")).toBe(false);
    expect(menuActionAllowed("view.refresh", "grid-edit")).toBe(true);
    expect(menuActionAllowed("query.open", "editor")).toBe(true);
    expect(menuActionAllowed("settings.open", "editor")).toBe(true);
  });

  it("keeps the grid's items away from a cell editor, the filter bar, the sidebar and the editor", () => {
    for (const scope of ["grid-edit", "filter", "editor", "sidebar"] as const) {
      expect(menuActionAllowed("grid.delete-rows", scope), scope).toBe(false);
      expect(menuActionAllowed("grid.set-null", scope), scope).toBe(false);
    }
    expect(menuActionAllowed("grid.delete-rows", "grid")).toBe(true);
    expect(menuActionAllowed("grid.delete-rows", "global")).toBe(true);
    // So are the grid's menu-only items; its global ones (⌥⌘F Columns) still work there.
    expect(menuActionAllowed("grid.set-now", "filter")).toBe(false);
    expect(menuActionAllowed("grid.set-now", "grid")).toBe(true);
    expect(menuActionAllowed("grid.columns", "filter")).toBe(true);
    expect(menuActionAllowed("query.comment-line", "editor")).toBe(true);
    expect(menuActionAllowed("ui.next-pane", "editor")).toBe(true);
    expect(menuActionAllowed("query.format", "editor")).toBe(true);
  });

  it("keeps the editor's Edit items (Beautify ⌘I, Split Pane ⇧⌘D…) away from grids and panels", () => {
    for (const id of EDITOR_EDIT_ITEMS as MenuActionId[]) {
      for (const scope of ["grid", "grid-edit", "filter", "sidebar", "dialog"] as const)
        expect(menuActionAllowed(id, scope), `${id} in ${scope}`).toBe(false);
      expect(menuActionAllowed(id, "editor"), id).toBe(true);
      expect(menuActionAllowed(id, "global"), id).toBe(true);
    }
    // The File menu's editor items act on the tab: Open SQL File… works from anywhere but dialogs.
    expect(menuActionAllowed("query.open", "sidebar")).toBe(true);
    expect(menuActionAllowed("query.open", "grid")).toBe(true);
    expect(menuActionAllowed("query.save-as", "grid")).toBe(true);
  });

  it("does not beautify or split the editor when ⌘I / ⇧⌘D reach the menu from a grid", async () => {
    const format = vi.fn();
    const split = vi.fn();
    const off = [actions.register("query.format", format), actions.register("query.split", split)];
    const host = document.createElement("div");
    host.innerHTML = `<div data-scope="grid" tabindex="0"></div>`;
    document.body.appendChild(host);
    host.querySelector<HTMLElement>("[data-scope]")!.focus();
    await expect(dispatchMenuAction("query.format")).resolves.toBe(false);
    await expect(dispatchMenuAction("query.split")).resolves.toBe(false);
    expect(format).not.toHaveBeenCalled();
    expect(split).not.toHaveBeenCalled();
    host.remove();
    await expect(dispatchMenuAction("query.format")).resolves.toBe(true);
    expect(format).toHaveBeenCalledTimes(1);
    off.forEach((u) => u());
  });
});

describe("Edit ▸ Toggle Line / Block Comment", () => {
  it("is handled while the menu is mounted (on the active query tab's editor)", async () => {
    const controller = await import("@/features/editor/controller");
    const toggle = vi.spyOn(controller, "toggleCommentInActiveTab").mockReturnValue(true);
    const { unmount } = renderHook(() => useNativeMenu());
    await expect(dispatchMenuAction("query.comment-line")).resolves.toBe(true);
    await expect(dispatchMenuAction("query.comment-block")).resolves.toBe(true);
    expect(toggle.mock.calls).toEqual([["line"], ["block"]]);
    unmount();
    expect(actions.has("query.comment-line")).toBe(false);
    toggle.mockRestore();
  });
});

describe("handleQuitRequest", () => {
  it("quits right away when nothing is unsaved", () => {
    const quit = vi.fn(async () => {});
    handleQuitRequest(() => true, quit);
    expect(quit).toHaveBeenCalledTimes(1);
  });

  it("leaves the decision to the unsaved-changes dialog otherwise", () => {
    const quit = vi.fn(async () => {});
    const request = vi.fn(() => false);
    handleQuitRequest(request, quit);
    expect(request).toHaveBeenCalledTimes(1);
    expect(quit).not.toHaveBeenCalled();
  });
});
