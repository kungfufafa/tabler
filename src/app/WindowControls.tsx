import { Copy, EllipsisVertical, Minus, Square, X } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { ipc, isTauri } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { ToolbarTip } from "./ToolbarButton";

async function currentWindow() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}

/**
 * The toolbar's ⋮ button (Windows, no menu bar): pops the app menu (File, Edit, View…) up under
 * itself, TablePlus for Windows's main menu.
 */
export function AppMenuButton() {
  return (
    <ToolbarTip label="Menu">
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label="Menu"
        data-slot="app-menu"
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          void ipc.menu.popup(r.left, r.bottom + 2);
        }}
        className="text-muted-foreground hover:text-foreground size-6.5"
      >
        <EllipsisVertical className="size-4" />
      </Button>
    </ToolbarTip>
  );
}

/** Whether the window is maximized, kept current (false outside the app). */
function useMaximized(): boolean {
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      const win = await currentWindow();
      const update = async () => {
        const m = await win.isMaximized();
        if (!cancelled) setMaximized(m);
      };
      await update();
      const off = await win.onResized(() => void update());
      if (cancelled) off();
      else unlisten = off;
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
  return maximized;
}

const CAPTION =
  "text-foreground/80 hover:bg-muted hover:text-foreground flex h-full w-11 items-center justify-center transition-colors outline-none focus-visible:bg-muted";

/**
 * Minimize, maximize / restore and close at the right end of the toolbar (Windows, no native
 * title bar). Full-height and square-cornered like the system's caption buttons; close turns red.
 */
export function WindowControls({ className }: { className?: string }) {
  const maximized = useMaximized();
  const run = (f: (w: Awaited<ReturnType<typeof currentWindow>>) => Promise<void>) => {
    if (isTauri()) void currentWindow().then(f);
  };
  return (
    <div data-slot="window-controls" className={cn("flex h-full shrink-0 self-stretch", className)}>
      <button
        type="button"
        aria-label="Minimize"
        className={CAPTION}
        onClick={() => run((w) => w.minimize())}
      >
        <Minus className="size-4" strokeWidth={1.25} />
      </button>
      <button
        type="button"
        aria-label={maximized ? "Restore" : "Maximize"}
        className={CAPTION}
        onClick={() => run((w) => w.toggleMaximize())}
      >
        {maximized ? (
          <Copy className="size-3.5 -scale-x-100" strokeWidth={1.25} />
        ) : (
          <Square className="size-3.5" strokeWidth={1.25} />
        )}
      </button>
      <button
        type="button"
        aria-label="Close"
        className={cn(CAPTION, "hover:bg-[#c42b1c] hover:text-white focus-visible:bg-[#c42b1c]")}
        onClick={() => run((w) => w.close())}
      >
        <X className="size-4" strokeWidth={1.25} />
      </button>
    </div>
  );
}
