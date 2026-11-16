import { Copy, EllipsisVertical, Minus, Square, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { useAction } from "@/features/palette/actions";
import { ipc, isTauri } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { ToolbarTip } from "./ToolbarButton";

async function currentWindow() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}

/** Pops the app menu up under `el`. */
function popupUnder(el: HTMLElement) {
  const r = el.getBoundingClientRect();
  void ipc.menu.popup(r.left, r.bottom + 2);
}

/**
 * The toolbar's ⋮ button (Windows, no menu bar): pops the app menu (File, Edit, View…) up under
 * itself, TablePlus for Windows's main menu. F10 (`ui.app-menu`, Windows only) pops it too, so the
 * menu is reachable from the keyboard as in any Windows app.
 */
export function AppMenuButton() {
  const ref = useRef<HTMLButtonElement>(null);
  const popup = useCallback(() => {
    if (ref.current) popupUnder(ref.current);
  }, []);
  useAction("ui.app-menu", popup);
  return (
    <ToolbarTip label="Menu" action="ui.app-menu">
      <Button
        ref={ref}
        variant="ghost"
        size="icon-sm"
        aria-label="Menu"
        data-slot="app-menu"
        onClick={(e) => popupUnder(e.currentTarget)}
        className="text-muted-foreground hover:bg-accent hover:text-foreground dark:hover:bg-accent aria-expanded:bg-accent transition-colors"
      >
        <EllipsisVertical className="size-3.5" />
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
  "text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:ring-ring/50 flex h-full w-11 items-center justify-center transition-colors outline-none focus-visible:ring-2 focus-visible:ring-inset";

/**
 * Minimize, maximize / restore and close at the right end of the toolbar (Windows, no native
 * title bar). Full-height and square-cornered like the system's caption buttons; close turns red.
 * `maximizable` false (the fixed-size Welcome window): maximize is shown greyed out, as Windows
 * does for a window that cannot be resized.
 */
export function WindowControls({
  className,
  maximizable = true,
}: {
  className?: string;
  maximizable?: boolean;
}) {
  const maximized = useMaximized();
  const run = (f: (w: Awaited<ReturnType<typeof currentWindow>>) => Promise<void>) => {
    if (isTauri()) void currentWindow().then(f);
  };
  // Named like the system's own caption buttons, in their tooltips too ("Restore Down").
  const resize = maximized ? "Restore Down" : "Maximize";
  return (
    <div data-slot="window-controls" className={cn("flex h-full shrink-0 self-stretch", className)}>
      <ToolbarTip label="Minimize">
        <button
          type="button"
          aria-label="Minimize"
          className={CAPTION}
          onClick={() => run((w) => w.minimize())}
        >
          <Minus className="size-4" strokeWidth={1.25} />
        </button>
      </ToolbarTip>
      {/* Wrapped: the greyed-out maximize of a fixed-size window still shows its tooltip. */}
      <ToolbarTip label={resize} wrap wrapClassName="flex h-full">
        <button
          type="button"
          aria-label={resize}
          disabled={!maximizable}
          className={cn(
            CAPTION,
            "disabled:pointer-events-none disabled:opacity-50 disabled:hover:bg-transparent",
          )}
          onClick={() => run((w) => w.toggleMaximize())}
        >
          {maximized ? (
            <Copy className="size-3.5 -scale-x-100" strokeWidth={1.25} />
          ) : (
            <Square className="size-3.5" strokeWidth={1.25} />
          )}
        </button>
      </ToolbarTip>
      <ToolbarTip label="Close">
        <button
          type="button"
          aria-label="Close"
          className={cn(CAPTION, "hover:bg-[#c42b1c] hover:text-white")}
          onClick={() => run((w) => w.close())}
        >
          <X className="size-4" strokeWidth={1.25} />
        </button>
      </ToolbarTip>
    </div>
  );
}
