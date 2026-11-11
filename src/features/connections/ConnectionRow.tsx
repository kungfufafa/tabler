import { useDraggable } from "@dnd-kit/core";
import { MoreHorizontal, Network } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ContextMenu, ContextMenuContent, ContextMenuTrigger } from "@/components/ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { kindOf } from "@/features/drivers/catalog";
import { useKindUi } from "@/features/drivers/registry";
import { relativeTime } from "@/lib/format/time";
import type { ConnectionProfile } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { DriverBadge, GroupBreadcrumb, TagPill } from "./badges";
import { connectionRowId } from "./list";
import { ContextMenuEntries, DropdownMenuEntries, type MenuEntry } from "./menuModel";
import { connectionDetail } from "./schema";

/** Indent per group level of the launcher list (headers and rows). */
export const INDENT_PX = 16;

/**
 * One saved connection in the launcher list: driver badge tinted with the connection
 * colour, name + tag + read-only, `host : port : user : database` below, last connect on the right.
 * Selection and keyboard handling live in the list (`Launcher`); a click selects, a double-click
 * connects, right-click or "…" opens the actions (`menu`). The row can be dragged onto a group
 * header to move it there. Inside nested groups it is indented by its group's level; search
 * results show the group path as a breadcrumb instead.
 *
 * The selected row uses a neutral fill until focus enters the connection pane. A ring marks the
 * current row while navigating with the keyboard, including before the first selection.
 */
export function ConnectionRow({
  profile,
  selected,
  current,
  connecting,
  connected,
  menu,
  indent = 0,
  breadcrumb,
  onSelect,
  onConnect,
}: {
  profile: ConnectionProfile;
  selected: boolean;
  /** The row ↑/↓/↩ start from (the list's active descendant). */
  current: boolean;
  connecting: boolean;
  connected: boolean;
  menu: readonly MenuEntry[];
  /** Nesting level of its group (0: top level or no group). */
  indent?: number;
  /** The group path to show (search results). */
  breadcrumb?: string;
  onSelect: () => void;
  onConnect: () => void;
}) {
  const last = profile.lastConnectedAt;
  const drag = useDraggable({ id: profile.id, data: { profile } });
  // A database of a lane writes its own detail line ("account / warehouse") once loaded.
  const kindUi = useKindUi(kindOf(profile));
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          ref={drag.setNodeRef}
          id={connectionRowId(profile.id)}
          role="option"
          aria-selected={selected}
          data-current={current || undefined}
          data-dragging={drag.isDragging || undefined}
          onClick={onSelect}
          onDoubleClick={onConnect}
          onContextMenu={onSelect}
          onPointerDown={
            drag.listeners?.onPointerDown as React.PointerEventHandler<HTMLElement> | undefined
          }
          style={indent > 0 ? { marginLeft: indent * INDENT_PX } : undefined}
          className={cn(
            "relative flex h-12 items-center gap-2 rounded-md px-2",
            selected
              ? [
                  "bg-accent group-focus-within/pane:bg-brand-soft group-focus-within/pane:before:bg-brand",
                  "before:pointer-events-none before:absolute before:inset-y-3 before:left-0 before:w-0.5 before:rounded-full",
                  "forced-colors:outline forced-colors:-outline-offset-1 forced-colors:outline-[Highlight]",
                ]
              : "hover:bg-row-hover",
            current &&
              (selected
                ? "group-focus-visible/list:ring-brand/70 group-focus-visible/list:ring-1 group-focus-visible/list:ring-inset"
                : [
                    "group-focus/list:ring-brand group-focus/list:ring-1 group-focus/list:ring-inset",
                    "forced-colors:group-focus/list:outline forced-colors:group-focus/list:-outline-offset-1 forced-colors:group-focus/list:outline-[Highlight]",
                  ]),
            drag.isDragging && "opacity-50",
          )}
        >
          <DriverBadge kind={kindOf(profile)} color={profile.color} busy={connecting} />
          <div className="min-w-0 flex-1">
            <div className="flex min-w-0 items-center gap-2">
              <span className="truncate text-sm leading-4 font-semibold" title={profile.name}>
                {profile.name}
              </span>
              {profile.options.tag && (
                <TagPill
                  tag={profile.options.tag}
                  className="block max-w-[min(7rem,30%)] truncate rounded-sm leading-4"
                />
              )}
              {profile.options.readOnly && (
                <span className="border-border text-muted-foreground inline-flex h-4 shrink-0 items-center rounded-sm border px-1 text-[10px] leading-none font-medium">
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
            <div className="text-muted-foreground mt-0.5 flex min-w-0 items-center gap-1 font-mono text-[12px] leading-4">
              {profile.ssh && (
                <Network className="size-3 shrink-0" aria-label="Over SSH" role="img" />
              )}
              <span className="truncate">{connectionDetail(profile, kindUi)}</span>
              {breadcrumb && (
                <GroupBreadcrumb
                  path={breadcrumb}
                  className="ml-auto max-w-[min(10rem,40%)] shrink-0 overflow-hidden pl-1 font-sans text-xs"
                />
              )}
            </div>
          </div>
          {last && (
            <span
              className="text-muted-foreground hidden max-w-24 shrink-0 truncate text-[12px] tabular-nums sm:block"
              title={`Last connected ${new Date(last).toLocaleString()}`}
            >
              {relativeTime(last)}
            </span>
          )}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                size="icon-sm"
                tabIndex={-1}
                className="text-muted-foreground"
                onDoubleClick={(e) => e.stopPropagation()}
                // A press on "…" must not start dragging the row.
                onPointerDown={(e) => e.stopPropagation()}
                aria-label="Connection actions"
              >
                <MoreHorizontal />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent
              align="end"
              className="w-52"
              onClick={(e) => e.stopPropagation()}
              onDoubleClick={(e) => e.stopPropagation()}
            >
              <DropdownMenuEntries entries={menu} />
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent className="w-52">
        <ContextMenuEntries entries={menu} />
      </ContextMenuContent>
    </ContextMenu>
  );
}
