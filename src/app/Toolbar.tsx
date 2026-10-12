import { Database, Lock, LockOpen, PanelLeft, RefreshCw, Search } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Popover,
  PopoverContent,
  PopoverDescription,
  PopoverHeader,
  PopoverTitle,
  PopoverTrigger,
} from "@/components/ui/popover";
import { openConnectionDialog } from "@/features/connections/store";
import { useProfiles } from "@/features/connections/useConnections";
import { actions } from "@/features/palette/actions";
import { useWindowFullscreen } from "@/hooks/useWindowFullscreen";
import { cn } from "@/lib/utils";
import { useActiveConnection } from "@/stores/connections";
import { ActionControl } from "./ActionControl";
import { PanelToggles } from "./PanelToggles";
import { StatusPill } from "./StatusPill";
import { ToolbarButton, ToolbarTip } from "./ToolbarButton";

/**
 * The lock: the active connection's mode (read-only or normal). Click → a popover that says what
 * the mode means and opens the connection form to change it (safe-mode levels come later).
 */
function SafeModeLock() {
  const active = useActiveConnection();
  const profiles = useProfiles();
  const [open, setOpen] = useState(false);
  const readOnly = active?.info.readOnly ?? false;
  const profile = profiles.data?.find((p) => p.id === active?.info.profileId) ?? null;
  const Icon = readOnly ? Lock : LockOpen;
  const editConnection = () => {
    if (!profile) return;
    setOpen(false);
    openConnectionDialog(profile);
  };
  // While open the trigger is styled from `aria-expanded`: its `data-state` is the tooltip's.
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <ToolbarTip label={readOnly ? "Read-only connection" : "Normal mode"}>
        <PopoverTrigger asChild>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={readOnly ? "Safe mode: read-only" : "Safe mode: off"}
            data-read-only={readOnly}
            disabled={!active}
            className="text-muted-foreground hover:text-foreground data-[read-only=true]:text-brand aria-expanded:bg-muted aria-expanded:text-foreground size-6.5"
          >
            <Icon className="size-3.5" />
          </Button>
        </PopoverTrigger>
      </ToolbarTip>
      <PopoverContent
        align="start"
        className="w-64 gap-2 p-3 text-xs"
        aria-label={readOnly ? "Read-only connection" : "Normal mode"}
      >
        <PopoverHeader className="gap-1 text-xs">
          <PopoverTitle className="flex items-center gap-1.5 text-[13px]">
            <Icon className={cn("size-3.5 shrink-0", readOnly && "text-brand")} aria-hidden />
            {readOnly ? "Read-only connection" : "Normal mode"}
          </PopoverTitle>
          <PopoverDescription className="text-xs leading-snug">
            {readOnly
              ? "Edits and DDL are disabled for this connection."
              : "Queries run without confirmation. Safe-mode levels are coming."}
          </PopoverDescription>
        </PopoverHeader>
        <Button
          variant="outline"
          size="xs"
          disabled={!profile}
          onClick={editConnection}
          className="self-start"
        >
          Edit Connection…
        </Button>
      </PopoverContent>
    </Popover>
  );
}

/**
 * TablePlus's single toolbar row in the macOS title bar: sidebar toggle, the staged-changes action
 * control, the read-only lock, Databases / SQL, the centred status pill, then Refresh, Open
 * Anything and the panel toggles. The empty space drags the window. The traffic lights sit in the
 * left inset (gone in fullscreen). The launcher shows only the title.
 */
export function Toolbar({ launcher }: { launcher: boolean }) {
  const fullscreen = useWindowFullscreen();
  const hasConnection = useActiveConnection() !== null;
  const inset = fullscreen ? 8 : 80;

  if (launcher) {
    return (
      <header
        data-tauri-drag-region
        data-slot="toolbar"
        className="border-border bg-surface-2 flex h-10 shrink-0 items-center justify-center border-b"
        // Mirrored inset: the title stays centred in the window.
        style={{ paddingLeft: inset, paddingRight: inset }}
      >
        <span
          data-tauri-drag-region
          className="text-muted-foreground cursor-default text-[13px] font-semibold select-none"
        >
          Tabler
        </span>
      </header>
    );
  }

  return (
    <header
      data-tauri-drag-region
      data-slot="toolbar"
      className="border-border bg-surface-2 flex h-10 shrink-0 items-center gap-2 overflow-hidden border-b pr-2"
      style={{ paddingLeft: inset }}
    >
      {/* Equal-growth sides keep the pill centred while there is room. */}
      <div data-tauri-drag-region className="flex min-w-fit flex-1 basis-0 items-center gap-1.5">
        <ToolbarButton label="Toggle sidebar" action="ui.toggle-sidebar">
          <PanelLeft className="size-4" />
        </ToolbarButton>
        <ActionControl />
        <SafeModeLock />
        <div className="flex items-center gap-0.5">
          <ToolbarButton label="Switch database" action="palette.databases">
            <Database className="size-3.5" />
          </ToolbarButton>
          <ToolbarTip label="New query" action="tab.new-query">
            <Button
              variant="ghost"
              size="xs"
              aria-label="New query"
              disabled={!hasConnection}
              onClick={() => void actions.run("tab.new-query")}
              className="text-muted-foreground hover:text-foreground h-6.5 px-1.5 text-[11px] font-semibold tracking-wide"
            >
              SQL
            </Button>
          </ToolbarTip>
        </div>
      </div>
      <StatusPill />
      <div
        data-tauri-drag-region
        className="flex min-w-fit flex-1 basis-0 items-center justify-end gap-1"
      >
        <ToolbarButton label="Refresh" action="view.refresh">
          <RefreshCw className="size-3.5" />
        </ToolbarButton>
        <ToolbarButton label="Open anything" action="palette.open">
          <Search className="size-3.5" />
        </ToolbarButton>
        <PanelToggles />
      </div>
    </header>
  );
}
