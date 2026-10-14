/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useStructureStore } from "@/features/structure/store";
import { listenEvent } from "@/lib/ipc/events";
import { hasUnsaved, useTabsStore } from "@/stores/tabs";
import {
  attachBeforeUnload,
  attachQuitShortcut,
  createUnsavedReporter,
  createWindowCloseGuard,
  handleQuitRequest,
  requestWindowClose,
  subscribeUnsaved,
  type GuardedWindow,
} from "./useUnsavedGuard";

type CloseHandler = Parameters<GuardedWindow["onCloseRequested"]>[0];

/** A stand-in for Tauri's window: tracks the close listener like the real event registry. */
function fakeWindow() {
  let handler: CloseHandler | null = null;
  const closed = vi.fn();
  const win: GuardedWindow = {
    onCloseRequested: vi.fn(async (h: CloseHandler) => {
      handler = h;
      return async () => {
        if (handler === h) handler = null;
      };
    }),
    close: vi.fn(async () => {
      // Like Tauri: with a JS listener the close is intercepted, otherwise the window closes.
      if (!handler) return closed();
      handler({ preventDefault: () => {} });
    }),
  };
  return {
    win,
    closed,
    listening: () => handler !== null,
    /** The user clicked the window's close button. Returns whether the close was prevented. */
    userCloses(): boolean {
      if (!handler) {
        closed();
        return false;
      }
      let prevented = false;
      handler({ preventDefault: () => (prevented = true) });
      return prevented;
    },
  };
}

function harness(initialDirty: boolean) {
  let dirty = initialDirty;
  const listeners = new Set<() => void>();
  const request = vi.fn(() => !dirty);
  const w = fakeWindow();
  const guard = createWindowCloseGuard(w.win, {
    isDirty: () => dirty,
    request,
    subscribe: (fn) => {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
  });
  return {
    ...w,
    guard,
    request,
    setDirty(v: boolean) {
      dirty = v;
      listeners.forEach((l) => l());
    },
  };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("createWindowCloseGuard", () => {
  it("leaves a clean window alone so it closes natively", async () => {
    const h = harness(false);
    await flush();
    expect(h.listening()).toBe(false);
    expect(h.userCloses()).toBe(false);
    expect(h.closed).toHaveBeenCalledTimes(1);
    h.guard.dispose();
  });

  it("intercepts the close and asks while something is unsaved", async () => {
    const h = harness(false);
    h.setDirty(true);
    await flush();
    expect(h.listening()).toBe(true);
    expect(h.userCloses()).toBe(true);
    expect(h.request).toHaveBeenCalledTimes(1);
    expect(h.closed).not.toHaveBeenCalled();

    h.setDirty(false);
    await flush();
    expect(h.listening()).toBe(false);
    h.guard.dispose();
  });

  it("closes for real after the user discards", async () => {
    const h = harness(true);
    await flush();
    await h.guard.closeNow();
    expect(h.listening()).toBe(false);
    expect(h.closed).toHaveBeenCalledTimes(1);
    // Store updates after confirming must not re-arm the guard.
    h.setDirty(true);
    await flush();
    expect(h.listening()).toBe(false);
  });

  it("closes right away when the work was saved between the click and the event", async () => {
    const h = harness(true);
    await flush();
    h.request.mockReturnValueOnce(true);
    expect(h.userCloses()).toBe(true);
    await flush();
    expect(h.closed).toHaveBeenCalledTimes(1);
  });

  it("detaches on dispose", async () => {
    const h = harness(true);
    await flush();
    h.guard.dispose();
    await flush();
    expect(h.listening()).toBe(false);
  });
});

describe("attachBeforeUnload", () => {
  const fire = () => {
    const e = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(e);
    return e.defaultPrevented;
  };

  it("prompts only while something is unsaved", () => {
    let dirty = false;
    const detach = attachBeforeUnload(window, () => dirty);
    expect(fire()).toBe(false);
    dirty = true;
    expect(fire()).toBe(true);
    detach();
    expect(fire()).toBe(false);
  });
});

describe("attachQuitShortcut", () => {
  const press = (init: KeyboardEventInit) => {
    const e = new KeyboardEvent("keydown", { cancelable: true, bubbles: true, ...init });
    document.body.dispatchEvent(e);
    return e.defaultPrevented;
  };
  const cmdQ = { code: "KeyQ", key: "q", metaKey: true };

  function setup(dirty: boolean) {
    const opts = {
      isDirty: vi.fn(() => dirty),
      broadcast: vi.fn(async () => {}),
      fallback: vi.fn(),
    };
    return { opts, detach: attachQuitShortcut(window, opts) };
  }

  afterEach(() => {
    useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  });

  it("tells every window to quit, like the menu's Quit item, instead of closing this one", () => {
    const { opts, detach } = setup(true);
    expect(press(cmdQ)).toBe(true);
    expect(opts.broadcast).toHaveBeenCalledTimes(1);
    expect(opts.fallback).not.toHaveBeenCalled();
    detach();
    expect(press(cmdQ)).toBe(false);
    expect(opts.broadcast).toHaveBeenCalledTimes(1);
  });

  it("with unsaved work in the tabs: broadcasts once, asks nothing itself, closes nothing", () => {
    useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
    const id = useTabsStore.getState().newQuery("c1");
    useTabsStore.getState().setDirty(id, true);
    expect(hasUnsaved()).toBe(true);
    const broadcast = vi.fn(async () => {});
    const detach = attachQuitShortcut(window, { isDirty: hasUnsaved, broadcast });
    expect(press(cmdQ)).toBe(true);
    expect(broadcast).toHaveBeenCalledTimes(1);
    // The dialog and the close come from the broadcast's `app:quit-requested` (handleQuitRequest),
    // in every window: not from the key handler.
    expect(useTabsStore.getState().closeRequest).toBeNull();
    expect(useTabsStore.getState().tabs).toHaveLength(1);
    detach();
  });

  it("reaches every window's quit handler through the app:quit-requested event", async () => {
    const heard = vi.fn();
    const unlisten = await listenEvent("app:quit-requested", heard);
    const detach = attachQuitShortcut(window, { isDirty: () => true });
    expect(press(cmdQ)).toBe(true);
    await vi.waitFor(() => expect(heard).toHaveBeenCalledTimes(1));
    detach();
    unlisten();
  });

  it("handles the quit in this window alone when the broadcast fails", async () => {
    const opts = {
      isDirty: () => true,
      broadcast: vi.fn(async () => Promise.reject(new Error("emit refused"))),
      fallback: vi.fn(),
    };
    const detach = attachQuitShortcut(window, opts);
    expect(press(cmdQ)).toBe(true);
    await vi.waitFor(() => expect(opts.fallback).toHaveBeenCalledTimes(1));
    detach();
  });

  it("lets a clean window quit natively", () => {
    const { opts, detach } = setup(false);
    expect(press(cmdQ)).toBe(false);
    expect(opts.broadcast).not.toHaveBeenCalled();
    detach();
  });

  it("ignores other Q combos", () => {
    const { opts, detach } = setup(true);
    expect(press({ code: "KeyQ", key: "q" })).toBe(false);
    expect(press({ ...cmdQ, shiftKey: true })).toBe(false);
    expect(press({ code: "KeyQ", key: "q", ctrlKey: true })).toBe(false);
    expect(opts.broadcast).not.toHaveBeenCalled();
    detach();
  });
});

describe("handleQuitRequest", () => {
  it("acknowledges the request before deciding", () => {
    const order: string[] = [];
    const acknowledge = vi.fn(async () => void order.push("ack"));
    const quit = vi.fn(async () => void order.push("quit"));
    handleQuitRequest(
      () => {
        order.push("request");
        return true;
      },
      quit,
      acknowledge,
    );
    expect(order).toEqual(["ack", "request", "quit"]);
  });

  it("acknowledges even when the dialog has to ask", () => {
    const acknowledge = vi.fn(async () => {});
    const quit = vi.fn(async () => {});
    handleQuitRequest(
      () => false,
      quit,
      acknowledge,
      async () => {},
    );
    expect(acknowledge).toHaveBeenCalledTimes(1);
    expect(quit).not.toHaveBeenCalled();
  });

  it("survives a failing acknowledgement", async () => {
    const quit = vi.fn(async () => {});
    handleQuitRequest(
      () => true,
      quit,
      () => Promise.reject(new Error("gone")),
      async () => {},
    );
    await Promise.resolve();
    expect(quit).toHaveBeenCalledTimes(1);
  });

  it("brings a dirty window forward (its dialog may be behind another window) and does not quit", () => {
    const quit = vi.fn(async () => {});
    const focus = vi.fn(async () => {});
    handleQuitRequest(
      () => false,
      quit,
      async () => {},
      focus,
    );
    expect(focus).toHaveBeenCalledTimes(1);
    expect(quit).not.toHaveBeenCalled();
  });

  it("quits a clean window without touching the focus", () => {
    const quit = vi.fn(async () => {});
    const focus = vi.fn(async () => {});
    handleQuitRequest(
      () => true,
      quit,
      async () => {},
      focus,
    );
    expect(quit).toHaveBeenCalledTimes(1);
    expect(focus).not.toHaveBeenCalled();
  });

  it("survives a failing focus", async () => {
    const focus = vi.fn(() => Promise.reject(new Error("no permission")));
    const quit = vi.fn(async () => {});
    expect(() =>
      handleQuitRequest(
        () => false,
        quit,
        async () => {},
        focus,
      ),
    ).not.toThrow();
    await Promise.resolve();
    expect(quit).not.toHaveBeenCalled();
  });
});

describe("the window permissions the quit flow needs", () => {
  it("are granted to every window by the default capability", () => {
    const cap = JSON.parse(
      readFileSync(
        path.resolve(import.meta.dirname, "../../src-tauri/capabilities/default.json"),
        "utf8",
      ),
    ) as { permissions: string[]; windows: string[] };
    expect(cap.windows).toEqual(expect.arrayContaining(["main", "w-*"]));
    for (const permission of [
      "core:window:allow-set-focus",
      "core:window:allow-unminimize",
      "core:window:allow-show",
      "core:window:allow-close",
    ])
      expect(cap.permissions).toContain(permission);
  });
});

describe("createUnsavedReporter", () => {
  it("reports only changes of the unsaved state", () => {
    let dirty = false;
    const report = vi.fn(async (_unsaved: boolean) => {});
    const sync = createUnsavedReporter(() => dirty, report);
    sync();
    sync();
    dirty = true;
    sync();
    sync();
    dirty = false;
    sync();
    expect(report.mock.calls.map(([u]) => u)).toEqual([false, true, false]);
  });

  it("reports again after a failed report", async () => {
    const report = vi.fn(async (_unsaved: boolean) => {});
    report.mockRejectedValueOnce(new Error("ipc down"));
    const sync = createUnsavedReporter(() => true, report);
    sync();
    await vi.waitFor(() => expect(report).toHaveBeenCalledTimes(1));
    await Promise.resolve();
    sync();
    expect(report).toHaveBeenCalledTimes(2);
  });

  it("follows staged DDL too (subscribeUnsaved covers every store hasUnsaved reads)", () => {
    useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
    useStructureStore.setState({ byTab: {} });
    const id = useTabsStore
      .getState()
      .openTable("c1", { schema: "public", name: "t" }, "structure");
    const report = vi.fn(async (_unsaved: boolean) => {});
    const sync = createUnsavedReporter(hasUnsaved, report);
    const unsubscribe = subscribeUnsaved(sync);
    sync();
    useStructureStore
      .getState()
      .stage(id, (ops) => [
        ...ops,
        { op: "renameTable", table: { schema: "public", name: "t" }, newName: "u" },
      ]);
    useStructureStore.getState().remove([id]);
    unsubscribe();
    expect(report.mock.calls.map(([u]) => u)).toEqual([false, true, false]);
  });
});

describe("requestWindowClose (⌘W with no tab to close)", () => {
  const asTauri = (on: boolean) => {
    if (on) (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    else delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  };
  afterEach(() => asTauri(false));

  it("has no window to close in the browser", async () => {
    const close = vi.fn(async () => {});
    expect(await requestWindowClose(close)).toBe(false);
    expect(close).not.toHaveBeenCalled();
  });

  it("closes a clean window straight away, like its red button", async () => {
    asTauri(true);
    const h = harness(false);
    await flush();
    expect(await requestWindowClose(() => h.win.close())).toBe(true);
    expect(h.closed).toHaveBeenCalledTimes(1);
    h.guard.dispose();
  });

  it("goes through the unsaved-work guard: a window with unsaved work asks first", async () => {
    asTauri(true);
    const h = harness(true);
    await flush();
    expect(await requestWindowClose(() => h.win.close())).toBe(true);
    // The guard intercepted the native close request and opened its dialog (`request`).
    expect(h.request).toHaveBeenCalledTimes(1);
    expect(h.closed).not.toHaveBeenCalled();
    h.guard.dispose();
  });

  it("reports a window that could not be asked", async () => {
    asTauri(true);
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    expect(
      await requestWindowClose(async () => {
        throw new Error("no window");
      }),
    ).toBe(false);
    warn.mockRestore();
  });
});
