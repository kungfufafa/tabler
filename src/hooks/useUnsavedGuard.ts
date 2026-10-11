import { useEffect } from "react";
import { isTauri } from "@/lib/ipc";
import { comboFromEvent, isMac } from "@/lib/keys/combo";
import { hasUnsaved, useTabsStore } from "@/stores/tabs";
import { useTableTabsStore } from "@/stores/tableTabs";

type Unlisten = () => void | Promise<void>;

/** The slice of Tauri's `Window` the guard needs (injectable for tests). */
export interface GuardedWindow {
  onCloseRequested: (handler: (event: { preventDefault: () => void }) => void) => Promise<Unlisten>;
  close: () => Promise<void>;
}

export interface WindowCloseGuard {
  /** Closes the window for real, bypassing the guard (after the user chose to discard). */
  closeNow: () => Promise<void>;
  dispose: () => void;
}

/**
 * Keeps a `closeRequested` listener on the native window only while something is unsaved.
 *
 * Tauri prevents the native close whenever a JS listener exists and expects JS to call
 * `destroy()`, which needs an extra capability. Attaching on demand (and always handling the close
 * ourselves) means a clean window closes natively, and a confirmed close detaches the listener and
 * calls `close()` again (covered by `core:window:allow-close`).
 */
export function createWindowCloseGuard(
  win: GuardedWindow,
  opts: {
    isDirty: () => boolean;
    /** Asks the user; returns true when the window may close right away. */
    request: () => boolean;
    subscribe: (onChange: () => void) => () => void;
  },
): WindowCloseGuard {
  let listener: Promise<Unlisten> | null = null;
  let disposed = false;
  let closing = false;

  const attach = () => {
    if (listener) return;
    listener = win
      .onCloseRequested((event) => {
        event.preventDefault();
        if (closing) return;
        if (opts.request()) void closeNow();
      })
      .catch(() => () => {});
  };
  const detach = async () => {
    const pending = listener;
    listener = null;
    if (pending) await (await pending)();
  };
  const sync = () => {
    if (disposed || closing) return;
    if (opts.isDirty()) attach();
    else void detach();
  };
  const closeNow = async () => {
    closing = true;
    try {
      await detach();
      await win.close();
    } catch {
      // The window stayed open: re-arm the guard.
      closing = false;
      sync();
    }
  };

  const unsubscribe = opts.subscribe(sync);
  sync();
  return {
    closeNow,
    dispose: () => {
      disposed = true;
      unsubscribe();
      void detach();
    },
  };
}

/** Browser / mock mode: let the browser show its own "Leave site?" prompt. */
export function attachBeforeUnload(
  target: Window,
  isDirty: () => boolean = hasUnsaved,
): () => void {
  const onBeforeUnload = (e: BeforeUnloadEvent) => {
    if (!isDirty()) return;
    e.preventDefault();
    e.returnValue = "";
  };
  target.addEventListener("beforeunload", onBeforeUnload);
  return () => target.removeEventListener("beforeunload", onBeforeUnload);
}

/**
 * macOS ⌘Q: the default app menu's Quit sends NSApp `terminate:`, which exits without ever emitting
 * `CloseRequested`, so the window guard above never runs. WKWebView offers key equivalents to the
 * page first, and a prevented keydown keeps the menu from firing; while something is unsaved ⌘Q is
 * swallowed here and routed through the same dialog. A clean window quits natively.
 *
 * Only the keyboard path is covered: Quit from the menu bar or the Dock still needs the backend to
 * intercept `terminate:` (a custom Quit item or `RunEvent::ExitRequested`).
 */
export function attachQuitShortcut(
  target: Pick<Window, "addEventListener" | "removeEventListener">,
  opts: {
    isDirty: () => boolean;
    /** Asks the user; returns true when the window may close right away. */
    request: () => boolean;
    closeNow: () => Promise<void>;
  },
): () => void {
  const onKeyDown = (e: KeyboardEvent) => {
    if (comboFromEvent(e, true) !== "Mod+Q" || !opts.isDirty()) return;
    e.preventDefault();
    e.stopPropagation();
    if (opts.request()) void opts.closeNow();
  };
  target.addEventListener("keydown", onKeyDown, { capture: true });
  return () => target.removeEventListener("keydown", onKeyDown, { capture: true });
}

let activeGuard: WindowCloseGuard | null = null;

/** Called by the unsaved-changes dialog once the user chose to discard and quit. */
export async function closeWindowNow(): Promise<void> {
  await activeGuard?.closeNow();
}

async function tauriWindow() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}

const subscribeUnsaved = (onChange: () => void) => {
  const a = useTabsStore.subscribe(onChange);
  const b = useTableTabsStore.subscribe(onChange);
  return () => {
    a();
    b();
  };
};

/**
 * Guards closing the window while tabs hold unsaved work: in Tauri the close is intercepted and the
 * shared unsaved-changes dialog asks; in the browser a `beforeunload` prompt is used. Mount once.
 */
export function useUnsavedGuard(): void {
  useEffect(() => {
    if (!isTauri()) return attachBeforeUnload(window);
    const guard = createWindowCloseGuard(
      {
        onCloseRequested: async (handler) => (await tauriWindow()).onCloseRequested(handler),
        close: async () => (await tauriWindow()).close(),
      },
      {
        isDirty: hasUnsaved,
        request: () => useTabsStore.getState().requestWindowClose(),
        subscribe: subscribeUnsaved,
      },
    );
    activeGuard = guard;
    const detachQuit = isMac
      ? attachQuitShortcut(window, {
          isDirty: hasUnsaved,
          request: () => useTabsStore.getState().requestWindowClose(),
          closeNow: guard.closeNow,
        })
      : undefined;
    return () => {
      detachQuit?.();
      guard.dispose();
      if (activeGuard === guard) activeGuard = null;
    };
  }, []);
}
