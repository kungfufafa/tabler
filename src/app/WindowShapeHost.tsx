import { useEffect } from "react";
import { useSqlViewerStore } from "@/features/sqlviewer/store";
import { showListInWelcomeWindow } from "@/features/tabs/windowHandoff";
import { useWindowFullscreen } from "@/hooks/useWindowFullscreen";
import { applyShape, windowDriver } from "@/lib/windowDriver";
import { useConnectionsStore } from "@/stores/connections";
import { setLauncherElsewhere } from "@/stores/ui";
import {
  desiredShape,
  shapedWindows,
  useWindowShape,
  type WindowShape,
} from "@/stores/windowShape";

/** One reshape at a time, in the order they were asked for. */
let queue: Promise<void> = Promise.resolve();

function requestShape(to: WindowShape): void {
  const setShape = useWindowShape.getState().setShape;
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
    });
}

/**
 * Keeps the window in the shape its content calls for (macOS): the Welcome window while nothing
 * is open, the workspace window otherwise. Mounted once by the app shell.
 */
export function WindowShapeHost() {
  const connections = useConnectionsStore((s) => s.order.length);
  useEffect(() => {
    if (!shapedWindows) return;
    setLauncherElsewhere(showListInWelcomeWindow);
    return () => setLauncherElsewhere(null);
  }, []);
  const opening = useWindowShape((s) => s.opening);
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
