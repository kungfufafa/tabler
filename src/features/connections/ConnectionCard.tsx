import { Copy, Loader2, MoreHorizontal, Pencil, Plug, Trash2 } from "lucide-react";
import { ColorDot } from "@/components/ColorDot";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import { DRIVER_SHORT } from "./colors";
import { connectionSummary } from "./schema";

export function ConnectionCard({
  profile,
  connecting,
  connected,
  onConnect,
  onEdit,
  onDuplicate,
  onDelete,
}: {
  profile: ConnectionProfile;
  connecting: boolean;
  connected: boolean;
  onConnect: () => void;
  onEdit: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
}) {
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onConnect}
      onDoubleClick={onConnect}
      onKeyDown={(e) => {
        if (e.key === "Enter") onConnect();
      }}
      className={cn(
        "group border-border bg-surface relative flex items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors",
        "hover:border-border-strong hover:bg-row-hover focus-visible:ring-ring/50 focus-visible:ring-2 focus-visible:outline-none",
        connected && "border-brand/40",
      )}
    >
      <div className="bg-surface-2 text-muted-foreground flex size-8 shrink-0 items-center justify-center rounded-md font-mono text-[11px] font-semibold">
        {connecting ? (
          <Loader2 className="text-brand size-4 animate-spin" />
        ) : (
          DRIVER_SHORT[profile.driver]
        )}
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <ColorDot color={profile.color} />
          <span className="truncate text-sm font-medium">{profile.name}</span>
          {profile.options.readOnly && (
            <Badge variant="outline" className="h-4 px-1 text-[10px]">
              read-only
            </Badge>
          )}
          {connected && (
            <Badge variant="secondary" className="text-success h-4 px-1 text-[10px]">
              open
            </Badge>
          )}
        </div>
        <div className="text-muted-foreground mt-0.5 truncate font-mono text-xs">
          {connectionSummary(profile)}
        </div>
      </div>
      <div className="text-muted-foreground hidden text-right text-[11px] sm:block">
        {relativeTime(profile.lastConnectedAt)}
      </div>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="icon-xs"
            className="opacity-0 group-hover:opacity-100 data-[state=open]:opacity-100"
            onClick={(e) => e.stopPropagation()}
            aria-label="Connection actions"
          >
            <MoreHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" onClick={(e) => e.stopPropagation()}>
          <DropdownMenuItem onSelect={onConnect}>
            <Plug /> Connect
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={onEdit}>
            <Pencil /> Edit…
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={onDuplicate}>
            <Copy /> Duplicate
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem variant="destructive" onSelect={onDelete}>
            <Trash2 /> Delete
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
