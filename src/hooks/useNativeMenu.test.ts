import { renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { actions, type ActionId } from "@/features/palette/actions";
import {
  dispatchMenuAction,
  isMenuAction,
  MENU_ACTIONS,
  menuActionAllowed,
  useNativeMenu,
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

describe("menuActionAllowed", () => {
  it("mirrors findShortcut's scope rules for every menu action", () => {
    for (const id of MENU_ACTIONS) {
      expect(menuActionAllowed(id, "global")).toBe(true);
      expect(menuActionAllowed(id, "grid")).toBe(true);
      expect(menuActionAllowed(id, "dialog")).toBe(id === "tab.close");
    }
    expect(menuActionAllowed("view.refresh", "editor")).toBe(false);
    expect(menuActionAllowed("query.open", "editor")).toBe(true);
    expect(menuActionAllowed("settings.open", "editor")).toBe(true);
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
