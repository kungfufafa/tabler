import { describe, expect, it, vi } from "vitest";
import { useStructureStore } from "@/features/structure/store";
import { hasUnsaved, useTabsStore } from "@/stores/tabs";
import {
  attachBeforeUnload,
  attachQuitShortcut,
  createUnsavedReporter,
  createWindowCloseGuard,
  handleQuitRequest,
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

  function setup(dirty: boolean, mayClose = false) {
    const opts = {
      isDirty: vi.fn(() => dirty),
      request: vi.fn(() => mayClose),
      closeNow: vi.fn(async () => {}),
    };
    return { opts, detach: attachQuitShortcut(window, opts) };
  }

  it("routes ⌘Q through the unsaved-changes dialog instead of letting the menu quit", () => {
    const { opts, detach } = setup(true);
    expect(press(cmdQ)).toBe(true);
    expect(opts.request).toHaveBeenCalledTimes(1);
    expect(opts.closeNow).not.toHaveBeenCalled();
    detach();
    expect(press(cmdQ)).toBe(false);
  });

  it("lets a clean window quit natively", () => {
    const { opts, detach } = setup(false);
    expect(press(cmdQ)).toBe(false);
    expect(opts.request).not.toHaveBeenCalled();
    detach();
  });

  it("closes right away when the request finds nothing to lose", () => {
    const { opts, detach } = setup(true, true);
    expect(press(cmdQ)).toBe(true);
    expect(opts.closeNow).toHaveBeenCalledTimes(1);
    detach();
  });

  it("ignores other Q combos", () => {
    const { opts, detach } = setup(true);
    expect(press({ code: "KeyQ", key: "q" })).toBe(false);
    expect(press({ ...cmdQ, shiftKey: true })).toBe(false);
    expect(press({ code: "KeyQ", key: "q", ctrlKey: true })).toBe(false);
    expect(opts.request).not.toHaveBeenCalled();
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
    handleQuitRequest(() => false, quit, acknowledge);
    expect(acknowledge).toHaveBeenCalledTimes(1);
    expect(quit).not.toHaveBeenCalled();
  });

  it("survives a failing acknowledgement", async () => {
    const quit = vi.fn(async () => {});
    handleQuitRequest(
      () => true,
      quit,
      () => Promise.reject(new Error("gone")),
    );
    await Promise.resolve();
    expect(quit).toHaveBeenCalledTimes(1);
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
