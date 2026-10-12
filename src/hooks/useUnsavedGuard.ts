import { useEffect } from "react";
import { useResultsStore } from "@/features/results/store";
import { useStructureStore } from "@/features/structure/store";
import { ipc, isTauri } from "@/lib/ipc";
import { listenEvent } from "@/lib/ipc/events";
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
 * macOS ⌘Q while something is unsaved: WKWebView offers key equivalents to the page first, and a
 * prevented keydown keeps the menu from firing, so ⌘Q is swallowed here and routed straight into
 * the unsaved-changes dialog. A clean window lets the key through to the menu.
 *
 * The app menu's "Quit Tabler" is a custom item (not NSApp `terminate:`, which exits without any
 * `CloseRequested`): it emits `app:quit-requested`, handled by `handleQuitRequest` with the same
 * dialog, and the backend turns exit requests made while the window is open into that event too.
 * Quit from the Dock (and logout / shutdown) sends `terminate:`; the backend adds the
 * `applicationShouldTerminate:` that tao 0.37 lacks and, while `createUnsavedReporter` says
 * something is unsaved, cancels it and emits `app:quit-requested` as well (see docs/qa-macos.md).
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

/**
 * Answers the backend's `app:quit-requested`: acknowledges it (so the backend does not take a
 * later request for a hung renderer's), then quits right away when nothing is unsaved, otherwise
 * opens the unsaved-changes dialog (kind "window"), whose Discard closes the window; the backend
 * then lets the app exit because no window is left.
 */
export function handleQuitRequest(
  request: () => boolean = () => useTabsStore.getState().requestWindowClose(),
  quit: () => Promise<void> = ipc.app.quit,
  acknowledge: () => Promise<void> = () => ipc.app.setUnsaved(hasUnsaved()),
): void {
  void acknowledge().catch(() => undefined);
  if (request()) void quit().catch(() => undefined);
}

/**
 * Keeps the backend told whether anything is unsaved (`ipc.app.setUnsaved`), so a Dock / system
 * quit with nothing to lose is answered right away instead of waiting on (and cancelling a logout
 * for) the renderer. Returns the sync function: call it on every store change; it only reports
 * changes, and retries after a failed report.
 */
export function createUnsavedReporter(
  isDirty: () => boolean,
  report: (unsaved: boolean) => Promise<void>,
): () => void {
  let reported: boolean | null = null;
  return () => {
    const unsaved = isDirty();
    if (unsaved === reported) return;
    reported = unsaved;
    void report(unsaved).catch(() => {
      if (reported === unsaved) reported = null;
    });
  };
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

/** Every store `hasUnsaved` reads: tabs (dirty query tabs), staged rows and staged DDL. */
export const subscribeUnsaved = (onChange: () => void) => {
  const a = useTabsStore.subscribe(onChange);
  const b = useTableTabsStore.subscribe(onChange);
  const c = useStructureStore.subscribe(onChange);
  const d = useResultsStore.subscribe(onChange);
  return () => {
    a();
    b();
    c();
    d();
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
    const reportUnsaved = createUnsavedReporter(hasUnsaved, ipc.app.setUnsaved);
    const unsubscribeReport = subscribeUnsaved(reportUnsaved);
    reportUnsaved();
    const detachQuit = isMac
      ? attachQuitShortcut(window, {
          isDirty: hasUnsaved,
          request: () => useTabsStore.getState().requestWindowClose(),
          closeNow: guard.closeNow,
        })
      : undefined;
    let unlistenQuit: (() => void) | undefined;
    let disposed = false;
    void listenEvent("app:quit-requested", () => handleQuitRequest()).then((u) => {
      if (disposed) u();
      else unlistenQuit = u;
    });
    return () => {
      disposed = true;
      unlistenQuit?.();
      unsubscribeReport();
      detachQuit?.();
      guard.dispose();
      if (activeGuard === guard) activeGuard = null;
    };
  }, []);
}
