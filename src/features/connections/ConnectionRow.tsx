import { useDraggable } from "@dnd-kit/core";
import { MoreHorizontal, Network } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ContextMenu, ContextMenuContent, ContextMenuTrigger } from "@/components/ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { relativeTime } from "@/lib/format/time";
import type { ConnectionProfile } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { DriverBadge, GroupBreadcrumb, TagPill } from "./badges";
import { connectionRowId } from "./list";
import { ContextMenuEntries, DropdownMenuEntries, type MenuEntry } from "./menuModel";
import { connectionDetail } from "./schema";

/** Indent per group level of the launcher list (headers and rows). */
export const INDENT_PX = 14;

/**
 * One saved connection in the launcher list (44px): driver badge tinted with the connection
 * colour, name + tag + read-only, `host : port : user : database` below, last connect on the right.
 * Selection and keyboard handling live in the list (`Launcher`); a click selects, a double-click
 * connects, right-click or "…" opens the actions (`menu`). The row can be dragged onto a group
 * header to move it there. Inside nested groups it is indented by its group's level; search
 * results show the group path as a breadcrumb instead.
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
          style={indent > 0 ? { marginLeft: 6 + indent * INDENT_PX } : undefined}
          className={cn(
            "group/row relative mx-1.5 flex h-11 items-center gap-2.5 rounded-md pr-1.5 pl-2.5",
            selected ? "bg-accent group-focus-within/pane:bg-brand-soft" : "hover:bg-row-hover",
            !selected &&
              current &&
              "group-focus/list:ring-brand/45 group-focus/list:ring-1 group-focus/list:ring-inset",
            drag.isDragging && "opacity-50",
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
              {breadcrumb && (
                <GroupBreadcrumb path={breadcrumb} className="ml-auto max-w-[45%] text-[11px]" />
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
