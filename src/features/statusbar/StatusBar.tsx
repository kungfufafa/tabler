import { useQuery } from "@tanstack/react-query";
import { ColorDot } from "@/components/ColorDot";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import { actions } from "@/features/palette/actions";
import { pendingCount } from "@/features/staging/model";
import { ipc } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { formatDuration } from "@/lib/format/time";
import { useActiveConnection } from "@/stores/connections";
import { useActiveTab } from "@/stores/tabs";
import { useTableTab } from "@/stores/tableTabs";

const STATUS = {
  connected: { label: "Connected", dot: "bg-success" },
  lost: { label: "Connection lost", dot: "bg-error" },
  reconnecting: { label: "Reconnecting…", dot: "bg-warning" },
  disconnected: { label: "Disconnected", dot: "bg-muted-foreground" },
} as const;

export function StatusBar() {
  const info = useQuery({ queryKey: ["app-info"], queryFn: ipc.appInfo, staleTime: Infinity });
  const active = useActiveConnection();
  const tab = useActiveTab();
  const tableState = useTableTab(tab?.kind === "table" ? tab.id : "");
  const pending = tab?.kind === "table" ? pendingCount(tableState.staging) : 0;
  const status = active ? STATUS[active.status] : null;
  return (
    <footer className="border-border bg-surface-2 text-muted-foreground flex h-6 shrink-0 items-center gap-3 overflow-hidden border-t px-3 text-[11px] whitespace-nowrap">
      {active && status ? (
        <>
          <span
            className="flex shrink-0 items-center gap-1.5"
            title={active.message ?? status.label}
          >
            <span className={cn("inline-block size-1.5 rounded-full", status.dot)} />
            <ColorDot color={active.info.color} size={6} />
            <span className="text-foreground/80 font-medium">{active.info.name}</span>
          </span>
          {active.info.currentDatabase && (
            <span className="truncate font-mono">{active.info.currentDatabase}</span>
          )}
          <span className="min-w-0 truncate">{active.info.serverVersion}</span>
        </>
      ) : (
        <span className="flex items-center gap-1.5">
          <span className="bg-muted-foreground/60 inline-block size-1.5 rounded-full" />
          Not connected
        </span>
      )}
      <span className="flex-1" />
      {pending > 0 && (
        <span className="flex items-center gap-1">
          <span className="bg-changed text-foreground rounded-full px-1.5 py-px font-medium">
            {pending} pending change{pending === 1 ? "" : "s"}
          </span>
          <Button
            variant="ghost"
            size="xs"
            className="h-5 text-[11px]"
            onClick={() => void actions.run("grid.discard")}
          >
            Discard
          </Button>
          <Button
            size="xs"
            className="h-5 text-[11px]"
            onClick={() => void actions.run("grid.commit")}
          >
            Commit{" "}
            <Kbd
              combo="Mod+S"
              className="border-primary-foreground/30 bg-transparent text-inherit"
            />
          </Button>
        </span>
      )}
      {tab?.kind === "table" && tableState.page && (
        <span className="tabular-nums" title={tableState.page.sql}>
          {tableState.page.rows.toLocaleString()} rows ·{" "}
          {formatDuration(tableState.page.durationMs)}
        </span>
      )}
      {info.data && (
        <span className="tabular-nums">
          Tabler {info.data.version}
          {info.data.platform === "mock" ? " · mock" : ""}
        </span>
      )}
    </footer>
  );
}
