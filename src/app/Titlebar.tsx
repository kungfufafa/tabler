import { PanelLeft, Plug, Plus, Search } from "lucide-react";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { actions } from "@/features/palette/actions";
import { TabStrip } from "@/features/tabs/TabStrip";
import { useWindowFullscreen } from "@/hooks/useWindowFullscreen";
import { useConnectionsStore } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";

function IconAction({
  label,
  combo,
  onClick,
  disabled,
  children,
}: {
  label: string;
  combo?: string;
  onClick: () => void;
  disabled?: boolean;
  children: React.ReactNode;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          onClick={onClick}
          aria-label={label}
          disabled={disabled}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent className="flex items-center gap-1.5">
        {label}
        {combo && <Kbd combo={combo} className="border-border/40 bg-transparent text-inherit" />}
      </TooltipContent>
    </Tooltip>
  );
}

/** Unified toolbar under the macOS overlay titlebar; the traffic lights sit in the left inset. */
export function Titlebar() {
  const fullscreen = useWindowFullscreen();
  const hasConnection = useConnectionsStore((s) => s.activeId !== null);
  const view = useUiStore((s) => s.view);
  return (
    <header
      data-tauri-drag-region
      className="border-border bg-surface-2 relative flex h-10 shrink-0 items-stretch border-b"
      style={{ paddingLeft: fullscreen ? 8 : 80 }}
    >
      <div className="flex items-center gap-0.5 pr-1">
        <IconAction
          label="Toggle sidebar"
          combo="Mod+B"
          onClick={() => void actions.run("ui.toggle-sidebar")}
        >
          <PanelLeft />
        </IconAction>
        <IconAction
          label={view === "launcher" ? "Back to workspace" : "Connections"}
          combo="Mod+Shift+O"
          onClick={() => void actions.run("ui.launcher")}
        >
          <Plug />
        </IconAction>
      </div>
      <TabStrip />
      {/* The filler keeps the empty area draggable. */}
      <div data-tauri-drag-region className="min-w-6 flex-1" />
      <div className="flex items-center gap-0.5 pr-2">
        <Button variant="ghost" size="sm" className="text-muted-foreground" disabled>
          <Search />
          <span>Search</span>
          <Kbd combo="Mod+K" className="ml-1" />
        </Button>
        <IconAction
          label="New query"
          combo="Mod+T"
          onClick={() => void actions.run("tab.new-query")}
          disabled={!hasConnection}
        >
          <Plus />
        </IconAction>
      </div>
    </header>
  );
}
