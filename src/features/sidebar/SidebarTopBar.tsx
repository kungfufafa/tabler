import { Cable, Database } from "lucide-react";
import { ToolbarButton } from "@/app/ToolbarButton";
import { useKeymapVersion } from "@/lib/keys/keymap";
import { useActiveConnection } from "@/stores/connections";

/**
 * TablePlus's quick navigation buttons above the Items | Queries | History switch (spec §2.2):
 * Show Connections (⇧⌘K, the connection palette) and Show Databases (⌘K, the database palette).
 * TablePlus's third one, Open Query Editor, is left out: the tab strip's + already opens a new SQL
 * tab. Each tooltip shows the key as it is bound right now. Show Databases belongs to a
 * connection, so it is disabled while none is open; Show Connections is how you open one and is
 * always enabled.
 */
export function SidebarTopBar() {
  // Read at render: the user can remap the keys (Settings ▸ Shortcuts).
  useKeymapVersion();
  const hasConnection = useActiveConnection() !== null;
  return (
    <div
      role="toolbar"
      aria-label="Quick navigation"
      data-slot="sidebar-top-bar"
      className="flex h-8 shrink-0 items-center gap-0.5 px-2"
    >
      <ToolbarButton label="Show Connections" action="palette.connections">
        <Cable className="size-3.5" />
      </ToolbarButton>
      <ToolbarButton label="Show Databases" action="palette.databases" disabled={!hasConnection}>
        <Database className="size-3.5" />
      </ToolbarButton>
    </div>
  );
}
