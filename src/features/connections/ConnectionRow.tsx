import {
  Copy,
  Link2,
  MoreHorizontal,
  Network,
  Pencil,
  Plug,
  Trash2,
  type LucideIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { relativeTime } from "@/lib/format/time";
import type { ConnectionProfile } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { DriverBadge, TagPill } from "./badges";
import { connectionRowId } from "./list";
import { connectionDetail } from "./schema";

interface RowActions {
  onConnect: () => void;
  onEdit: () => void;
  onDuplicate: () => void;
  onCopyUrl: () => void;
  onDelete: () => void;
}

type MenuEntry =
  { label: string; icon: LucideIcon; run: () => void; destructive?: boolean } | "separator";

function menuEntries(a: RowActions): MenuEntry[] {
  return [
    { label: "Connect", icon: Plug, run: a.onConnect },
    { label: "Edit…", icon: Pencil, run: a.onEdit },
    { label: "Duplicate", icon: Copy, run: a.onDuplicate },
    { label: "Copy as URL", icon: Link2, run: a.onCopyUrl },
    "separator",
    { label: "Delete…", icon: Trash2, run: a.onDelete, destructive: true },
  ];
}

/**
 * One saved connection in the launcher list (44px): driver badge tinted with the connection
 * colour, name + tag + read-only, `host : port : user : database` below, last connect on the right.
 * Selection and keyboard handling live in the list (`Launcher`); a click selects, a double-click
 * connects, right-click or "…" opens the actions.
 *
 * Two marks: `selected` is a real selection (a click, ↑/↓…), filled with the accent while the
 * connection pane has the focus and grey otherwise; `current` is only the keyboard target before
 * anything is selected, drawn as a thin ring while the list itself has the focus.
 */
export function ConnectionRow({
  profile,
  selected,
  current,
  connecting,
  connected,
  onSelect,
  ...actions
}: RowActions & {
  profile: ConnectionProfile;
  selected: boolean;
  /** The row ↑/↓/↩ start from (the list's active descendant). */
  current: boolean;
  connecting: boolean;
  connected: boolean;
  onSelect: () => void;
}) {
  const entries = menuEntries(actions);
  const last = profile.lastConnectedAt;
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          id={connectionRowId(profile.id)}
          role="option"
          aria-selected={selected}
          data-current={current || undefined}
          onClick={onSelect}
          onDoubleClick={actions.onConnect}
          onContextMenu={onSelect}
          className={cn(
            "group/row relative mx-1.5 flex h-11 items-center gap-2.5 rounded-md pr-1.5 pl-2.5",
            selected ? "bg-accent group-focus-within/pane:bg-brand-soft" : "hover:bg-row-hover",
            !selected &&
              current &&
              "group-focus/list:ring-brand/45 group-focus/list:ring-1 group-focus/list:ring-inset",
          )}
        >
          <DriverBadge driver={profile.driver} color={profile.color} busy={connecting} />
          <div className="min-w-0 flex-1">
            <div className="flex min-w-0 items-center gap-1.5">
              <span className="truncate text-[13px] leading-4 font-semibold">{profile.name}</span>
              {profile.options.tag && <TagPill tag={profile.options.tag} />}
              {profile.options.readOnly && (
                <span className="border-border text-muted-foreground inline-flex h-4 shrink-0 items-center rounded-full border px-1.5 text-[10px] leading-none">
                  read-only
                </span>
              )}
              {connected && (
                <span
                  className="bg-success size-1.5 shrink-0 rounded-full"
                  title="Open"
                  aria-label="open"
                  role="img"
                />
              )}
            </div>
            <div className="text-muted-foreground mt-0.5 flex min-w-0 items-center gap-1 font-mono text-[11px] leading-3.5">
              {profile.ssh && (
                <Network
                  className="size-3 shrink-0"
                  aria-label="Over SSH"
                  role="img"
                  strokeWidth={2.25}
                />
              )}
              <span className="truncate">{connectionDetail(profile)}</span>
            </div>
          </div>
          {last && (
            <span
              className="text-muted-foreground hidden shrink-0 text-[11px] tabular-nums sm:block"
              title={`Last connected ${new Date(last).toLocaleString()}`}
            >
              {relativeTime(last)}
            </span>
          )}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                tabIndex={-1}
                className="opacity-0 group-hover/row:opacity-100 group-aria-selected/row:opacity-100 data-[state=open]:opacity-100"
                onDoubleClick={(e) => e.stopPropagation()}
                aria-label="Connection actions"
              >
                <MoreHorizontal />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent
              align="end"
              onClick={(e) => e.stopPropagation()}
              onDoubleClick={(e) => e.stopPropagation()}
            >
              {entries.map((e, i) =>
                e === "separator" ? (
                  <DropdownMenuSeparator key={i} />
                ) : (
                  <DropdownMenuItem
                    key={e.label}
                    variant={e.destructive ? "destructive" : "default"}
                    onSelect={e.run}
                  >
                    <e.icon /> {e.label}
                  </DropdownMenuItem>
                ),
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent className="w-48">
        {entries.map((e, i) =>
          e === "separator" ? (
            <ContextMenuSeparator key={i} />
          ) : (
            <ContextMenuItem
              key={e.label}
              variant={e.destructive ? "destructive" : "default"}
              onSelect={e.run}
            >
              <e.icon /> {e.label}
            </ContextMenuItem>
          ),
        )}
      </ContextMenuContent>
    </ContextMenu>
  );
}
