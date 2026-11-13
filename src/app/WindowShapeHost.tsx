import { useEffect } from "react";
import { useSqlViewerStore } from "@/features/sqlviewer/store";
import { showListInWelcomeWindow } from "@/features/tabs/windowHandoff";
import { useWindowFullscreen } from "@/hooks/useWindowFullscreen";
import { isTauri } from "@/lib/ipc/transport";
import { applyShape, windowDriver } from "@/lib/windowDriver";
import { useConnectionsStore } from "@/stores/connections";
import { setLauncherElsewhere } from "@/stores/ui";
import {
  desiredShape,
  rememberMaximized,
  shapedWindows,
  useWindowShape,
  type WindowShape,
} from "@/stores/windowShape";

/** One reshape at a time, in the order they were asked for. */
let queue: Promise<void> = Promise.resolve();
/** Reshapes asked for and not done yet (their own resizes are not the user's). */
let pending = 0;

function requestShape(to: WindowShape): void {
  const setShape = useWindowShape.getState().setShape;
  pending++;
  queue = queue
    .then(async () => {
      const driver = await windowDriver();
      // The browser mock has no window to resize: only the layout follows.
      setShape(driver ? await applyShape(to, driver, setShape) : to);
    })
    .catch((e: unknown) => {
      // Never stuck on the Welcome layout with a workspace open: fall back to the in-window list.
      console.warn("[window] could not take the shape", to, e);
      setShape(to === "workspace" ? "workspace" : null);
    })
    .finally(() => {
      pending--;
    });
}

/**
 * Keeps whether the workspace window is maximized (zoomed on macOS; `rememberMaximized`), so it
 * opens maximized again, like TablePlus, after being the Welcome window or a restart.
 */
function useRememberMaximized(): void {
  useEffect(() => {
    if (!shapedWindows || !isTauri()) return;
    let off: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const win = getCurrentWindow();
      const unlisten = await win.onResized(() => {
        if (pending > 0 || useWindowShape.getState().shape !== "workspace") return;
        void (async () => {
          const [visible, minimized, maximized] = await Promise.all([
            win.isVisible(),
            win.isMinimized(),
            win.isMaximized(),
          ]);
          if (visible && !minimized && pending === 0) rememberMaximized(win.label, maximized);
        })().catch(() => undefined);
      });
      if (cancelled) unlisten();
      else off = unlisten;
    })().catch((e: unknown) => console.warn("[window] could not follow maximize", e));
    return () => {
      cancelled = true;
      off?.();
    };
  }, []);
}

/**
 * Keeps the window in the shape its content calls for (macOS and Windows): the Welcome window while
 * nothing is open, the workspace window otherwise. Mounted once by the app shell.
 */
export function WindowShapeHost() {
  const connections = useConnectionsStore((s) => s.order.length);
  useEffect(() => {
    if (!shapedWindows) return;
    setLauncherElsewhere(showListInWelcomeWindow);
    return () => setLauncherElsewhere(null);
  }, []);
  useRememberMaximized();
  const opening = useWindowShape((s) => s.opening || s.linking > 0);
  const decided = useWindowShape((s) => s.decided);
  // The standalone SQL Viewer (no connection) needs a workspace window's room.
  const roomy = useSqlViewerStore((s) => s.standaloneOpen);
  // Leaving full screen: a window that could not become the Welcome window then, can now.
  const fullscreen = useWindowFullscreen();
  const want = shapedWindows ? desiredShape({ connections, opening, decided, roomy }) : null;

  useEffect(() => {
    if (want) requestShape(want);
  }, [want, fullscreen]);
  return null;
}
