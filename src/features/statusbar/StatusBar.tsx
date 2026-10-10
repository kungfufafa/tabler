import { useQuery } from "@tanstack/react-query";
import { ColorDot } from "@/components/ColorDot";
import { ipc } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useActiveConnection } from "@/stores/connections";

const STATUS = {
  connected: { label: "Connected", dot: "bg-success" },
  lost: { label: "Connection lost", dot: "bg-error" },
  reconnecting: { label: "Reconnecting…", dot: "bg-warning" },
  disconnected: { label: "Disconnected", dot: "bg-muted-foreground" },
} as const;

export function StatusBar() {
  const info = useQuery({ queryKey: ["app-info"], queryFn: ipc.appInfo, staleTime: Infinity });
  const active = useActiveConnection();
  const status = active ? STATUS[active.status] : null;
  return (
    <footer className="border-border bg-surface-2 text-muted-foreground flex h-6 shrink-0 items-center gap-3 border-t px-3 text-[11px]">
      {active && status ? (
        <>
          <span className="flex items-center gap-1.5" title={active.message ?? status.label}>
            <span className={cn("inline-block size-1.5 rounded-full", status.dot)} />
            <ColorDot color={active.info.color} size={6} />
            <span className="text-foreground/80 font-medium">{active.info.name}</span>
          </span>
          {active.info.currentDatabase && (
            <span className="truncate font-mono">{active.info.currentDatabase}</span>
          )}
          <span className="truncate">{active.info.serverVersion}</span>
        </>
      ) : (
        <span className="flex items-center gap-1.5">
          <span className="bg-muted-foreground/60 inline-block size-1.5 rounded-full" />
          Not connected
        </span>
      )}
      <span className="flex-1" />
      {info.data && (
        <span className="tabular-nums">
          Tabler {info.data.version}
          {info.data.platform === "mock" ? " · mock" : ""}
        </span>
      )}
    </footer>
  );
}
