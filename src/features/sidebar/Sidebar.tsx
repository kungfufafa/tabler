import { Database } from "lucide-react";

export function Sidebar() {
  return (
    <aside className="bg-sidebar text-sidebar-foreground flex h-full flex-col">
      <div className="text-muted-foreground flex h-9 shrink-0 items-center px-3 text-xs font-semibold tracking-wide uppercase">
        Connections
      </div>
      <div className="text-muted-foreground flex flex-1 flex-col items-center justify-center gap-2 px-4 text-center text-xs">
        <Database className="size-6 opacity-50" />
        <p>No connection selected</p>
      </div>
    </aside>
  );
}
