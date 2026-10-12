import { Plus } from "lucide-react";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useDisconnect } from "@/features/connections/useConnections";
import { actions } from "@/features/palette/actions";
import { disconnectGuarded } from "@/features/palette/commands";
import { accentColor } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import { DRIVER_SHORT, serverLabel } from "./connectionLabels";
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
  if (!conn) return null;
  const { info } = conn;
  const accent = accentColor(info.color);
  const problem = conn.status === "lost" || conn.status === "reconnecting";
  return (
    <ContextMenu>
      <Tooltip>
        <ContextMenuTrigger asChild>
          <TooltipTrigger asChild>
            <button
              type="button"
              data-connection-id={connectionId}
              aria-label={info.name}
              aria-current={active ? "true" : undefined}
              onClick={() => {
                useConnectionsStore.getState().setActive(connectionId);
                useUiStore.getState().setView("workspace");
              }}
              className={cn(
                "group/rail focus-visible:ring-ring/50 flex w-12 flex-col items-center gap-1 rounded-md py-1 outline-none focus-visible:ring-2",
                active ? "text-foreground" : "text-muted-foreground hover:text-foreground",
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
                {DRIVER_SHORT[info.driver]}
                {problem && (
                  <span
                    data-slot="rail-problem"
                    className="bg-warning ring-sidebar absolute -top-0.5 -right-0.5 size-2 rounded-full ring-2"
                  />
                )}
              </span>
              <span
                aria-hidden
                className={cn(
                  "w-full truncate px-0.5 text-center text-[10px] leading-tight",
                  active && "font-medium",
                )}
              >
                {info.name}
              </span>
            </button>
          </TooltipTrigger>
        </ContextMenuTrigger>
        <TooltipContent side="right" className="flex flex-col items-start gap-0.5">
          <span className="font-medium">{info.name}</span>
          <span className="opacity-70">{serverLabel(info)}</span>
        </TooltipContent>
      </Tooltip>
      <ContextMenuContent className="min-w-36">
        <ContextMenuItem className="py-0.5 text-xs" onSelect={() => onDisconnect(connectionId)}>
          Disconnect
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

/**
 * TablePlus's workspace rail at the window's far left, shown while any connection is open (even
 * one, so the connection list is always a click away): one tinted badge per connection (click to
 * switch, right-click to disconnect) and a `+` that opens the connection manager (highlighted
 * while it shows).
 */
export function WorkspaceRail() {
  const order = useConnectionsStore((s) => s.order);
  const activeId = useConnectionsStore((s) => s.activeId);
  const launcher = useUiStore((s) => s.view === "launcher");
  const { mutate: disconnect } = useDisconnect();
  if (order.length === 0) return null;
  return (
    <nav
      aria-label="Open connections"
      data-slot="workspace-rail"
      className="bg-sidebar border-border flex w-13 shrink-0 flex-col items-center border-r"
    >
      <div className="flex min-h-0 w-full flex-1 [scrollbar-width:none] flex-col items-center gap-1.5 overflow-y-auto py-2 [&::-webkit-scrollbar]:hidden">
        {order.map((id) => (
          <RailItem
            key={id}
            connectionId={id}
            active={!launcher && id === activeId}
            onDisconnect={(connectionId) => void disconnectGuarded(connectionId, disconnect)}
          />
        ))}
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
