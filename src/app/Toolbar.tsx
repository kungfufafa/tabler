import { useQueryClient } from "@tanstack/react-query";
import {
  Database,
  Gauge,
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
import { useRef, useState, type ReactNode } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuRadioGroup,
  ContextMenuRadioItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
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
import { driverLabel } from "@/features/drivers/catalog";
import { actions } from "@/features/palette/actions";
import {
  effectiveLevel,
  isPasswordLevel,
  SAFE_MODE_LABEL,
  SAFE_MODE_OPTIONS,
  safeModeText,
} from "@/features/safety/levels";
import { changeSafeModeLevel } from "@/features/safety/setLevel";
import { useWindowFullscreen } from "@/hooks/useWindowFullscreen";
import { toIpcError, type ConnectionProfile, type Driver, type SafeModeLevel } from "@/lib/ipc";
import { has } from "@/lib/caps";
import { cn } from "@/lib/utils";
import { useActiveConnection } from "@/stores/connections";
import { setPref, usePrefs } from "@/stores/prefs";
import { ActionControl } from "./ActionControl";
import { CustomizeToolbarDialog } from "./CustomizeToolbar";
import { PanelToggles } from "./PanelToggles";
import { StatusPill } from "./StatusPill";
import { overlayTitleBar, ownTitleBar } from "./titleBar";
import { ToolbarButton, ToolbarGroup, ToolbarTip } from "./ToolbarButton";
import { useActionAvailable } from "./useActionAvailable";
import { AppMenuButton, WindowControls } from "./WindowControls";
import {
  splitAtStatus,
  TOOLBAR_DISPLAY_LABEL,
  TOOLBAR_DISPLAYS,
  TOOLBAR_ITEM_SHORT,
  visibleItems,
  type ToolbarDisplay,
  type ToolbarItemId,
} from "./toolbarLayout";

/** Why a connection's toolbar buttons are greyed out without one. */
const NO_CONNECTION = "No connection";

/** Why User Management / Metrics Board are greyed out for this connection. */
function unsupportedReason(driver: Driver): string {
  return `Not supported by ${driverLabel(driver)}`;
}

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
  const pendingLevel = useRef<SafeModeLevel | null>(null);
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
        // The menu closes and only the small lock icon changes: say what the connection is now
        // set to, a risky setting worth a confirmation.
        toast.success(`Safe mode: ${SAFE_MODE_LABEL[next]}`, { description: saved.name });
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
  // While open the trigger is styled from `aria-expanded`. The tooltip hangs on a span around it,
  // so it still says what the lock is while it is greyed out (no connection).
  return (
    <DropdownMenu>
      <ToolbarTip
        label={readOnly ? `${safeModeText(level)} · Read-only connection` : safeModeText(level)}
        disabledReason={active ? undefined : NO_CONNECTION}
        wrap
      >
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={`Safe mode: ${label}${readOnly ? " (read-only)" : ""}`}
            data-level={level}
            data-read-only={readOnly}
            disabled={!active}
            className="text-muted-foreground hover:bg-accent hover:text-foreground dark:hover:bg-accent data-[level=alert]:text-foreground data-[level=alertNonSelect]:text-foreground data-[level=safe]:text-brand data-[level=safeNonSelect]:text-brand aria-expanded:bg-accent aria-expanded:text-foreground relative transition-colors"
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
          // Release the menu's focus scope before a downgrade can open a modal.
          const next = pendingLevel.current;
          pendingLevel.current = null;
          if (next !== null) void choose(next);
        }}
      >
        <DropdownMenuLabel className="truncate">
          Safe mode{profile ? ` · ${profile.name}` : ""}
        </DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={level}
          onValueChange={(v) => {
            pendingLevel.current = v as SafeModeLevel;
          }}
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
                <span className="leading-4">
                  {o.label} <span className="text-muted-foreground">· {o.summary}</span>
                </span>
                <span className="text-muted-foreground text-xs">{o.description}</span>
              </span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        {readOnly && (
          <p
            data-slot="read-only-note"
            className="text-muted-foreground flex items-start gap-1.5 px-1.5 py-1 text-xs"
          >
            <Lock className="text-brand mt-px size-3 shrink-0" aria-hidden />
            Read-only connection: edits and DDL are disabled.
          </p>
        )}
        <DropdownMenuItem disabled={!profile} onSelect={editConnection}>
          Edit Connection…
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** One toolbar item; `text` adds the captions of "Icon and Text". */
function ToolbarItem({ id, text }: { id: ToolbarItemId; text: boolean }) {
  const active = useActiveConnection();
  const hasConnection = active !== null;
  const caption = text ? TOOLBAR_ITEM_SHORT[id] : undefined;
  // The panel switches leave the left sidebar to the Toggle Sidebar item when the toolbar has it.
  const sidebarItem = usePrefs((p) => p.toolbarItems.includes("sidebar"));
  switch (id) {
    case "sidebar":
      return (
        <ToolbarButton label="Toggle sidebar" action="ui.toggle-sidebar" text={caption}>
          <PanelLeft className="size-3.5" />
        </ToolbarButton>
      );
    case "actions":
      return <ActionControl />;
    case "safeMode":
      return <SafeModeLock />;
    case "databases":
      return (
        // Always in a capsule (`GROUPED`): spaced like its other buttons.
        <div className="flex items-center gap-0.5">
          <ToolbarButton label="Open database" action="palette.databases" text={caption}>
            <Database className="size-3.5" />
          </ToolbarButton>
          <ToolbarTip
            label="New query"
            action="tab.new-query"
            disabledReason={hasConnection ? undefined : NO_CONNECTION}
            wrap
          >
            <Button
              variant="ghost"
              size="sm"
              aria-label="New query"
              disabled={!hasConnection}
              onClick={() => void actions.run("tab.new-query")}
              className="text-muted-foreground hover:bg-accent hover:text-foreground dark:hover:bg-accent px-2 text-xs transition-colors"
            >
              SQL
            </Button>
          </ToolbarTip>
        </div>
      );
    case "status":
      return <StatusPill />;
    case "refresh":
      return (
        <ToolbarButton label="Refresh" action="view.refresh" text={caption}>
          <RefreshCw className="size-3.5" />
        </ToolbarButton>
      );
    case "openAnything":
      // Gives way in a narrow window: Ctrl/⌘P opens it all the same.
      return (
        <span data-slot="toolbar-open-anything" className="flex @max-3xl/toolbar:hidden">
          <ToolbarButton label="Open anything" action="palette.open" text={caption}>
            <Search className="size-3.5" />
          </ToolbarButton>
        </span>
      );
    case "workspaces":
      return (
        <ToolbarButton label="Show workspaces" action="workspace.show" text={caption}>
          <LayoutGrid className="size-3.5" />
        </ToolbarButton>
      );
    case "users":
      return (
        <ToolbarButton
          label="User management"
          action="admin.users"
          text={caption}
          disabled={!active || active.info.driver === "sqlite" || !has(active.info, "users")}
          disabledReason={active ? unsupportedReason(active.info.driver) : NO_CONNECTION}
        >
          <UserCog className="size-3.5" />
        </ToolbarButton>
      );
    case "metrics":
      // Gives way first in a narrow window (also on the View menu and the palette).
      return (
        <span data-slot="toolbar-metrics" className="flex @max-3xl/toolbar:hidden">
          <ToolbarButton
            label="Metrics board"
            action="metrics.open"
            text={caption}
            disabled={!hasConnection || (!!active && !has(active.info, "metrics"))}
            disabledReason={active ? unsupportedReason(active.info.driver) : NO_CONNECTION}
          >
            <Gauge className="size-3.5" />
          </ToolbarButton>
        </span>
      );
    case "panels":
      return <PanelToggles leftSidebar={!sidebarItem} />;
    case "space":
      return <div data-tauri-drag-region data-slot="toolbar-space" className="w-5 shrink-0" />;
    case "flexibleSpace":
      return (
        <div
          data-tauri-drag-region
          data-slot="toolbar-flexible-space"
          className="h-full min-w-5 flex-1"
        />
      );
  }
}

/** What a toolbar side shows: its items, and on Windows the ⋮ app menu among them. */
type Entry = ToolbarItemId | "appMenu";

/**
 * The entries that share a capsule with their neighbours, as TablePlus draws each cluster
 * ([sidebar ⋮], [lock Databases SQL], [Metrics Refresh Open Anything]). The action control and
 * the panel switches are capsules of their own; the pill and the spaces sit outside any.
 */
const GROUPED: ReadonlySet<Entry> = new Set<Entry>([
  "sidebar",
  "appMenu",
  "safeMode",
  "databases",
  "refresh",
  "openAnything",
  "workspaces",
  "users",
  "metrics",
]);

/** The items that give way in a narrow window (`@max-3xl/toolbar:hidden` on their own). */
const NARROW_HIDDEN: ReadonlySet<Entry> = new Set<Entry>(["metrics", "openAnything"]);

/** Occurrence-numbered keys: spaces may repeat. */
function itemKeys(items: readonly Entry[]): string[] {
  const seen = new Map<Entry, number>();
  return items.map((id) => {
    const n = seen.get(id) ?? 0;
    seen.set(id, n + 1);
    return `${id}:${n}`;
  });
}

function EntryItem({ id, text }: { id: Entry; text: boolean }) {
  return id === "appMenu" ? <AppMenuButton /> : <ToolbarItem id={id} text={text} />;
}

/** The entries in order, each run of adjacent groupable ones in one capsule. */
function Items({ items, text }: { items: readonly Entry[]; text: boolean }): ReactNode {
  const keys = itemKeys(items);
  const out: ReactNode[] = [];
  for (let i = 0; i < items.length;) {
    if (!GROUPED.has(items[i]!)) {
      out.push(<EntryItem key={keys[i]} id={items[i]!} text={text} />);
      i++;
      continue;
    }
    let end = i;
    while (end < items.length && GROUPED.has(items[end]!)) end++;
    const run = items.slice(i, end);
    out.push(
      <ToolbarGroup
        key={`group:${keys[i]}`}
        // A capsule of items that all give way in a narrow window goes with them: no empty pill.
        className={cn(run.every((id) => NARROW_HIDDEN.has(id)) && "@max-3xl/toolbar:hidden")}
      >
        {run.map((id, k) => (
          <EntryItem key={keys[i + k]} id={id} text={text} />
        ))}
      </ToolbarGroup>,
    );
    i = end;
  }
  return out;
}

/**
 * The left items with the ⋮ app menu when the toolbar is the title bar (Windows): after the
 * sidebar toggle when that comes first, as in TablePlus for Windows, else at the very start.
 */
function LeftItems({ items, text }: { items: readonly ToolbarItemId[]; text: boolean }) {
  if (!ownTitleBar) return <Items items={items} text={text} />;
  const entries: Entry[] =
    items[0] === "sidebar" ? ["sidebar", "appMenu", ...items.slice(1)] : ["appMenu", ...items];
  return <Items items={entries} text={text} />;
}

/**
 * TablePlus's single toolbar row in the macOS title bar, customisable like Safari's (right-click
 * ▸ Customize Toolbar…, Icon Only / Icon and Text; the `toolbarItems` preference). The default
 * set: sidebar toggle, the staged-changes action control, the safe-mode lock, Databases / SQL,
 * the centred status pill, then Refresh, Open Anything, Show Workspaces, User Management, Metrics
 * Board (once something handles it) and the panel toggles. The empty space drags the window. The
 * traffic lights sit in the left inset (gone in fullscreen). The launcher shows only the title.
 * On Windows the toolbar is the title bar without an inset: the ⋮ app menu on the left, the window
 * buttons on the right, and the launcher shows just those two. Linux looks like Windows.
 */
export function Toolbar({ launcher }: { launcher: boolean }) {
  const fullscreen = useWindowFullscreen();
  const items = usePrefs((p) => p.toolbarItems);
  const display = usePrefs((p) => p.toolbarDisplay);
  const metrics = useActionAvailable("metrics.open");
  const [customizing, setCustomizing] = useState(false);
  const inset = overlayTitleBar && !fullscreen ? 80 : 8;

  if (launcher) {
    if (ownTitleBar)
      return (
        <header
          data-tauri-drag-region
          data-slot="toolbar"
          className="bg-surface-2 dark:bg-background flex h-10 shrink-0 items-center pl-2"
        >
          <AppMenuButton />
          <div data-tauri-drag-region className="h-full flex-1" />
          <WindowControls />
        </header>
      );
    if (!overlayTitleBar) return null;
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
          className="text-muted-foreground cursor-default text-sm font-medium select-none"
        >
          Tabler
        </span>
      </header>
    );
  }

  const text = display === "iconAndText";
  const rows = splitAtStatus(visibleItems(items, { metrics }));
  return (
    <>
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <header
            data-tauri-drag-region
            data-slot="toolbar"
            data-display={display}
            className={cn(
              "border-border bg-surface-2 @container/toolbar flex h-10 shrink-0 items-center overflow-hidden border-b",
              !ownTitleBar && "pr-2",
            )}
            style={{ paddingLeft: inset }}
          >
            {/* The items clip when the window is too narrow for them (a window snapped to half
                the screen), never the window buttons after them. */}
            <div
              data-tauri-drag-region
              data-slot="toolbar-items"
              className="flex h-full min-w-0 flex-1 items-center gap-2 overflow-hidden"
            >
              {rows.status ? (
                <>
                  {/* Equal-growth sides keep the pill centred while there is room. */}
                  <div
                    data-tauri-drag-region
                    className="flex h-full min-w-fit flex-1 basis-0 items-center gap-1"
                  >
                    <LeftItems items={rows.left} text={text} />
                  </div>
                  <StatusPill />
                  <div
                    data-tauri-drag-region
                    className="flex h-full min-w-fit flex-1 basis-0 items-center justify-end gap-1"
                  >
                    <Items items={rows.right} text={text} />
                  </div>
                </>
              ) : (
                <div
                  data-tauri-drag-region
                  className="flex h-full min-w-0 flex-1 items-center gap-1"
                >
                  <LeftItems items={rows.left} text={text} />
                </div>
              )}
            </div>
            {ownTitleBar && <WindowControls className="ml-3" />}
          </header>
        </ContextMenuTrigger>
        <ContextMenuContent className="min-w-44">
          <ContextMenuRadioGroup
            value={display}
            onValueChange={(v) => setPref("toolbarDisplay", v as ToolbarDisplay)}
          >
            {TOOLBAR_DISPLAYS.map((d) => (
              <ContextMenuRadioItem key={d} value={d}>
                {TOOLBAR_DISPLAY_LABEL[d]}
              </ContextMenuRadioItem>
            ))}
          </ContextMenuRadioGroup>
          <ContextMenuSeparator />
          <ContextMenuItem onSelect={() => setCustomizing(true)}>
            Customize Toolbar…
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
      <CustomizeToolbarDialog open={customizing} onOpenChange={setCustomizing} />
    </>
  );
}
