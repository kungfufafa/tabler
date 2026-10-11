import { ChevronsUpDown, Plug, Unplug } from "lucide-react";
import { ColorDot } from "@/components/ColorDot";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useGuardedDisconnect } from "@/features/connections/useConnections";
import { cn } from "@/lib/utils";
import { useShallow } from "zustand/react/shallow";
import { useActiveConnection, useConnectionsStore } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";

const STATUS_CLASS = {
  connected: "bg-success",
  lost: "bg-error",
  reconnecting: "bg-warning",
  disconnected: "bg-muted-foreground",
} as const;

export function ConnectionSwitcher() {
  const active = useActiveConnection();
  const all = useConnectionsStore(useShallow((s) => s.order.map((id) => s.byId[id]!)));
  const setActive = useConnectionsStore((s) => s.setActive);
  const setView = useUiStore((s) => s.setView);
  const disconnect = useGuardedDisconnect();
  if (!active) return null;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          className="h-9 w-full justify-start gap-2 rounded-none px-3 font-medium"
        >
          <ColorDot color={active.info.color} size={10} />
          <span className="truncate">{active.info.name}</span>
          <span
            className={cn("size-1.5 rounded-full", STATUS_CLASS[active.status])}
            title={active.status}
          />
          <ChevronsUpDown className="text-muted-foreground ml-auto size-3.5" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-64">
        <DropdownMenuLabel>Open connections</DropdownMenuLabel>
        {all.map((c) => (
          <DropdownMenuItem
            key={c.info.connectionId}
            onSelect={() => setActive(c.info.connectionId)}
          >
            <ColorDot color={c.info.color} />
            <span className="truncate">{c.info.name}</span>
            <span className="text-muted-foreground ml-auto text-[10px]">
              {c.info.currentDatabase ?? ""}
            </span>
          </DropdownMenuItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => setView("launcher")}>
          <Plug /> Connection manager…
        </DropdownMenuItem>
        <DropdownMenuItem
          variant="destructive"
          onSelect={() => void disconnect(active.info.connectionId)}
        >
          <Unplug /> Disconnect {active.info.name}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
