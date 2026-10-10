import { useQuery } from "@tanstack/react-query";
import { ipc } from "@/lib/ipc";

export function StatusBar() {
  const info = useQuery({ queryKey: ["app-info"], queryFn: ipc.appInfo, staleTime: Infinity });
  return (
    <footer className="border-border bg-surface-2 text-muted-foreground flex h-6 shrink-0 items-center gap-3 border-t px-3 text-xs">
      <span className="flex items-center gap-1.5">
        <span className="bg-muted-foreground/60 inline-block size-1.5 rounded-full" />
        Not connected
      </span>
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
