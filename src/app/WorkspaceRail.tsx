import {
  closestCenter,
  DndContext,
  PointerSensor,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import { SortableContext, useSortable, verticalListSortingStrategy } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { Plus } from "lucide-react";
import {
  ContextMenu,
  ContextMenuCheckboxItem,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useDisconnect } from "@/features/connections/useConnections";
import { actions } from "@/features/palette/actions";
import { disconnectGuarded } from "@/features/palette/commands";
import { openProfileInNewWindow } from "@/features/tabs/windowHandoff";
import { moveWorkspace, useRailOrder, workspaceInitials } from "@/features/tabs/workspaces";
import { accentColor } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import { databaseLabel, serverLabel } from "./connectionLabels";
import { ToolbarTip } from "./ToolbarButton";

function RailItem({
  connectionId,
  active,
  onDisconnect,
}: {
  connectionId: string;
  active: boolean;
  onDisconnect: (connectionId: string) => void;
}) {
  const conn = useConnectionsStore((s) => s.byId[connectionId]);
  const showDatabase = useUiStore((s) => s.railShowDatabase);
  const showConnection = useUiStore((s) => s.railShowConnection);
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: connectionId,
  });
  if (!conn) return null;
  const { info } = conn;
  const accent = accentColor(info.color);
  const problem = conn.status === "lost" || conn.status === "reconnecting";
  const database = databaseLabel(info);
  // The database's initials (TablePlus's database icons); the connection's without one.
  const initials = workspaceInitials(database || info.name);
  const label = database ? `${database} (${info.name})` : info.name;
  return (
    <ContextMenu>
      <Tooltip>
        <ContextMenuTrigger asChild>
          <TooltipTrigger asChild>
            <button
              ref={setNodeRef}
              type="button"
              data-connection-id={connectionId}
              aria-label={label}
              aria-current={active ? "true" : undefined}
              style={{ transform: CSS.Translate.toString(transform), transition }}
              {...attributes}
              {...listeners}
              onClick={() => {
                useConnectionsStore.getState().setActive(connectionId);
                useUiStore.getState().setView("workspace");
              }}
              className={cn(
                "group/rail focus-visible:ring-ring/50 flex w-12 flex-col items-center gap-0.5 rounded-md py-1 outline-none focus-visible:ring-2",
                active ? "text-foreground" : "text-muted-foreground hover:text-foreground",
                isDragging && "z-10 opacity-80",
              )}
            >
              <span
                aria-hidden
                className={cn(
                  "relative flex size-8 items-center justify-center rounded-lg text-[11px] font-bold tracking-wide transition-shadow",
                  !active && "group-hover/rail:brightness-95 dark:group-hover/rail:brightness-110",
                )}
                style={{
                  backgroundColor: `color-mix(in oklab, ${accent} ${active ? 26 : 16}%, var(--surface))`,
                  color: `color-mix(in oklab, ${accent} 65%, var(--foreground))`,
                  boxShadow: active
                    ? `0 0 0 2px var(--sidebar), 0 0 0 3.5px ${accent}`
                    : `inset 0 0 0 1px color-mix(in oklab, ${accent} 30%, transparent)`,
                }}
              >
                {initials}
                {problem && (
                  <span
                    data-slot="rail-problem"
                    className="bg-warning ring-sidebar absolute -top-0.5 -right-0.5 size-2 rounded-full ring-2"
                  />
                )}
              </span>
              {showDatabase && database && (
                <span
                  aria-hidden
                  data-slot="rail-database"
                  className={cn(
                    "w-full truncate px-0.5 text-center text-[10px] leading-tight",
                    active && "font-medium",
                  )}
                >
                  {database}
                </span>
              )}
              {showConnection && (
                <span
                  aria-hidden
                  data-slot="rail-connection"
                  className={cn(
                    "w-full truncate px-0.5 text-center leading-tight",
                    showDatabase && database
                      ? "text-muted-foreground text-[9px]"
                      : cn("text-[10px]", active && "font-medium"),
                  )}
                >
                  {info.name}
                </span>
              )}
            </button>
          </TooltipTrigger>
        </ContextMenuTrigger>
        <TooltipContent side="right" className="flex flex-col items-start gap-0.5">
          <span className="font-medium">{database || info.name}</span>
          {database && <span>{info.name}</span>}
          <span className="opacity-70">{serverLabel(info)}</span>
        </TooltipContent>
      </Tooltip>
      <ContextMenuContent className="min-w-44">
        <ContextMenuCheckboxItem
          className="py-0.5 text-xs"
          checked={showDatabase}
          onSelect={(e) => {
            e.preventDefault();
            useUiStore.getState().toggleRailDatabase();
          }}
        >
          Show Database Name
        </ContextMenuCheckboxItem>
        <ContextMenuCheckboxItem
          className="py-0.5 text-xs"
          checked={showConnection}
          onSelect={(e) => {
            e.preventDefault();
            useUiStore.getState().toggleRailConnection();
          }}
        >
          Show Connection Name
        </ContextMenuCheckboxItem>
        <ContextMenuSeparator />
        <ContextMenuItem
          className="py-0.5 text-xs"
          onSelect={() =>
            void openProfileInNewWindow(
              info.profileId,
              info.driver === "sqlite" ? null : info.currentDatabase,
            )
          }
        >
          Open in New Window
        </ContextMenuItem>
        <ContextMenuItem className="py-0.5 text-xs" onSelect={() => onDisconnect(connectionId)}>
          Disconnect
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

/**
 * TablePlus's workspace rail at the window's far left, shown while any connection is open (even
 * one, so the connection list is always a click away): one icon per workspace — the database's
 * initials in the connection's colour, with the database and connection names under it (the
 * right-click menu hides either) — that switches to it, drags to reorder (kept across restarts)
 * and right-clicks to open it in a new window or disconnect; a `+` opens the connection manager
 * (highlighted while it shows).
 */
export function WorkspaceRail() {
  const order = useRailOrder();
  const activeId = useConnectionsStore((s) => s.activeId);
  const launcher = useUiStore((s) => s.view === "launcher");
  const { mutate: disconnect } = useDisconnect();
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 6 } }));
  if (order.length === 0) return null;
  const onDragEnd = (e: DragEndEvent) => {
    if (!e.over || e.active.id === e.over.id) return;
    moveWorkspace(order.indexOf(String(e.active.id)), order.indexOf(String(e.over.id)));
  };
  return (
    <nav
      aria-label="Open connections"
      data-slot="workspace-rail"
      className="bg-sidebar border-border flex w-13 shrink-0 flex-col items-center border-r"
    >
      <div className="flex min-h-0 w-full flex-1 [scrollbar-width:none] flex-col items-center gap-1.5 overflow-y-auto py-2 [&::-webkit-scrollbar]:hidden">
        <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
          <SortableContext items={order} strategy={verticalListSortingStrategy}>
            {order.map((id) => (
              <RailItem
                key={id}
                connectionId={id}
                active={!launcher && id === activeId}
                onDisconnect={(connectionId) => void disconnectGuarded(connectionId, disconnect)}
              />
            ))}
          </SortableContext>
        </DndContext>
      </div>
      <div className="border-border flex w-full justify-center border-t py-2">
        <ToolbarTip label="Connection manager" action="ui.launcher" side="right">
          <button
            type="button"
            aria-label="Connection manager"
            aria-current={launcher ? "true" : undefined}
            onClick={() => {
              if (!launcher) void actions.run("ui.launcher");
            }}
            className={cn(
              "border-border text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-ring/50 flex size-8 items-center justify-center rounded-lg border border-dashed outline-none focus-visible:ring-2",
              launcher && "border-brand text-brand bg-brand-soft border-solid",
            )}
          >
            <Plus className="size-4" />
          </button>
        </ToolbarTip>
      </div>
    </nav>
  );
}
