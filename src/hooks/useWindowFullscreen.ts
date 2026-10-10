import { useEffect, useState } from "react";
import { isTauri } from "@/lib/ipc/transport";

/** True while the native window is fullscreen (macOS hides the traffic lights, so the titlebar inset goes away). */
export function useWindowFullscreen(): boolean {
  const [fullscreen, setFullscreen] = useState(false);
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    (async () => {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const win = getCurrentWindow();
      const update = async () => {
        const fs = await win.isFullscreen();
        if (!cancelled) setFullscreen(fs);
      };
      await update();
      unlisten = await win.onResized(() => void update());
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
  return fullscreen;
}
