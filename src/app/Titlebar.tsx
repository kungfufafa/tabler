import { PanelLeft, Plus, Search } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useWindowFullscreen } from "@/hooks/useWindowFullscreen";
import { useUiStore } from "@/stores/ui";

/** Unified toolbar under the macOS overlay titlebar; the traffic lights sit in the left inset. */
export function Titlebar() {
  const fullscreen = useWindowFullscreen();
  const toggleSidebar = useUiStore((s) => s.toggleSidebar);
  return (
    <header
      data-tauri-drag-region
      className="border-border bg-surface-2 relative flex h-10 shrink-0 items-center gap-1 border-b pr-2"
      style={{ paddingLeft: fullscreen ? 8 : 80 }}
    >
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={toggleSidebar}
            aria-label="Toggle sidebar"
          >
            <PanelLeft />
          </Button>
        </TooltipTrigger>
        <TooltipContent>Toggle sidebar ⌘B</TooltipContent>
      </Tooltip>
      {/* Tab strip goes here (M1). The filler keeps the empty area draggable. */}
      <div data-tauri-drag-region className="flex h-full min-w-0 flex-1 items-end" />
      <Button variant="ghost" size="sm" className="text-muted-foreground" disabled>
        <Search />
        <span>Search</span>
        <kbd className="border-border bg-surface text-muted-foreground ml-1 rounded border px-1 font-mono text-[10px]">
          ⌘K
        </kbd>
      </Button>
      <Button variant="ghost" size="icon-sm" aria-label="New query" disabled>
        <Plus />
      </Button>
    </header>
  );
}
