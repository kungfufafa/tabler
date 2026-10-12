import { useQueryClient } from "@tanstack/react-query";
import {
  Database,
  LayoutGrid,
  Lock,
  LockKeyhole,
  LockOpen,
  PanelLeft,
  RefreshCw,
  Search,
  UserCog,
  type LucideIcon,
} from "lucide-react";
import { useRef, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { openConnectionDialog } from "@/features/connections/store";
import { profileKeys, useProfiles } from "@/features/connections/useConnections";
import { actions } from "@/features/palette/actions";
import {
  effectiveLevel,
  isPasswordLevel,
  SAFE_MODE_LABEL,
  SAFE_MODE_OPTIONS,
} from "@/features/safety/levels";
import { changeSafeModeLevel } from "@/features/safety/setLevel";
import { useWindowFullscreen } from "@/hooks/useWindowFullscreen";
import { toIpcError, type ConnectionProfile, type SafeModeLevel } from "@/lib/ipc";
import { useActiveConnection } from "@/stores/connections";
import { ActionControl } from "./ActionControl";
import { PanelToggles } from "./PanelToggles";
import { StatusPill } from "./StatusPill";
import { ToolbarButton, ToolbarTip } from "./ToolbarButton";

/** The lock's icon: open = Silent, closed = Alert Mode 1 / 2, keyhole (accent) = Safe Mode 1 / 2. */
function levelIcon(level: SafeModeLevel): LucideIcon {
  if (level === "silent") return LockOpen;
  return isPasswordLevel(level) ? LockKeyhole : Lock;
}

/**
 * The lock: the active connection's safe-mode level. Click → TablePlus's menu of the five levels;
 * picking one saves the connection's profile and applies to the next query. Leaving a Safe level
 * for one that asks less needs the password first. Below: the read-only note and Edit Connection….
 */
function SafeModeLock() {
  const active = useActiveConnection();
  const profiles = useProfiles();
  const qc = useQueryClient();
  const [busy, setBusy] = useState(false);
  /** Set when a menu item opened a dialog: the closing menu leaves focus to it. */
  const openedDialog = useRef(false);
  const readOnly = active?.info.readOnly ?? false;
  const profile = profiles.data?.find((p) => p.id === active?.info.profileId) ?? null;
  const level = effectiveLevel(profile?.options.safeMode);
  const label = SAFE_MODE_LABEL[level];
  const Icon = levelIcon(level);

  const choose = async (next: SafeModeLevel) => {
    if (!active || busy || next === level) return;
    setBusy(true);
    try {
      const saved = await changeSafeModeLevel(active.info, next);
      if (saved) {
        qc.setQueryData<ConnectionProfile[]>(profileKeys.all, (list) =>
          list?.map((p) => (p.id === saved.id ? saved : p)),
        );
        void qc.invalidateQueries({ queryKey: profileKeys.all });
      }
    } catch (err) {
      toast.error("Could not change the safe mode", { description: toIpcError(err).message });
    } finally {
      setBusy(false);
    }
  };

  const editConnection = () => {
    if (!profile) return;
    openedDialog.current = true;
    openConnectionDialog(profile);
  };
  // While open the trigger is styled from `aria-expanded`: its `data-state` is the tooltip's.
  return (
    <DropdownMenu>
      <ToolbarTip label={readOnly ? `${label} · Read-only connection` : label}>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={`Safe mode: ${label}${readOnly ? " (read-only)" : ""}`}
            data-level={level}
            data-read-only={readOnly}
            disabled={!active}
            className="text-muted-foreground hover:text-foreground data-[level=alert]:text-foreground data-[level=alertNonSelect]:text-foreground data-[level=safe]:text-brand data-[level=safeNonSelect]:text-brand aria-expanded:bg-muted aria-expanded:text-foreground relative size-6.5"
          >
            <Icon className="size-3.5" />
            {readOnly && (
              <span
                aria-hidden
                data-slot="read-only-dot"
                className="bg-brand ring-surface-2 absolute right-1 bottom-1 size-1.5 rounded-full ring-1"
              />
            )}
          </Button>
        </DropdownMenuTrigger>
      </ToolbarTip>
      <DropdownMenuContent
        align="start"
        className="w-80"
        onCloseAutoFocus={(e) => {
          if (openedDialog.current) e.preventDefault();
          openedDialog.current = false;
        }}
      >
        <DropdownMenuLabel className="text-muted-foreground truncate text-[11px] font-normal">
          Safe mode{profile ? ` · ${profile.name}` : ""}
        </DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={level}
          onValueChange={(v) => void choose(v as SafeModeLevel)}
        >
          {SAFE_MODE_OPTIONS.map((o) => (
            <DropdownMenuRadioItem
              key={o.level}
              value={o.level}
              textValue={o.label}
              disabled={!profile || busy}
              className="items-start py-1.5"
            >
              <span className="flex min-w-0 flex-col gap-0.5">
                <span className="text-[13px] leading-4">{o.label}</span>
                <span className="text-muted-foreground text-[11px] leading-snug">
                  {o.description}
                </span>
              </span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        {readOnly && (
          <p
            data-slot="read-only-note"
            className="text-muted-foreground flex items-start gap-1.5 px-1.5 py-1 text-[11px] leading-snug"
          >
            <Lock className="text-brand mt-px size-3 shrink-0" aria-hidden />
            Read-only connection: edits and DDL are disabled.
          </p>
        )}
        <DropdownMenuItem disabled={!profile} onSelect={editConnection} className="text-[13px]">
          Edit Connection…
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/**
 * TablePlus's single toolbar row in the macOS title bar: sidebar toggle, the staged-changes action
 * control, the safe-mode lock, Databases / SQL, the centred status pill, then Refresh, Open
 * Anything and the panel toggles. The empty space drags the window. The traffic lights sit in the
 * left inset (gone in fullscreen). The launcher shows only the title.
 */
export function Toolbar({ launcher }: { launcher: boolean }) {
  const fullscreen = useWindowFullscreen();
  const active = useActiveConnection();
  const hasConnection = active !== null;
  const canManageUsers = active !== null && active.info.driver !== "sqlite";
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
        {/* TablePlus's toolbar order: Open Anything, Show Workspaces, User Management, panels. */}
        <ToolbarButton label="Show workspaces" action="workspace.show">
          <LayoutGrid className="size-3.5" />
        </ToolbarButton>
        <ToolbarButton label="User management" action="admin.users" disabled={!canManageUsers}>
          <UserCog className="size-3.5" />
        </ToolbarButton>
        <PanelToggles />
      </div>
    </header>
  );
}
