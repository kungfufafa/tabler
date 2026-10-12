import { Plus, Settings2, SquarePlus, SquareTerminal } from "lucide-react";
import { useState } from "react";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { actions } from "@/features/palette/actions";
import { TableDialogs, type TableDialogState } from "@/features/structure/TableDialogs";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { useConnectionsStore, type OpenConnection } from "@/stores/connections";
import { setSidebarOption, useSidebarPrefs } from "./sidebarPrefs";
import { useCurrentSchema, useSchemas } from "./useSchemaTree";

const NEW_QUERY_COMBO = shortcutFor("tab.new-query");

const iconButton =
  "text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground data-[state=open]:bg-sidebar-accent data-[state=open]:text-foreground focus-visible:ring-ring/50 flex h-6 shrink-0 items-center justify-center gap-0.5 rounded-md px-1 outline-none focus-visible:ring-2";

/**
 * Schema selector: PostgreSQL schemas, SQLite's attached databases (main, temp, …). Shown only when
 * there is more than one to pick from.
 */
export function SchemaPicker({ connection }: { connection: OpenConnection }) {
  const id = connection.info.connectionId;
  const schemas = useSchemas(id);
  const current = useCurrentSchema(id);
  const setCurrentSchema = useConnectionsStore((s) => s.setCurrentSchema);
  const driver = connection.info.driver;
  if (driver !== "postgres" && driver !== "sqlite") return null;
  if ((schemas.data?.length ?? 0) < 2) return null;
  return (
    <Select value={current ?? ""} onValueChange={(v) => setCurrentSchema(id, v)}>
      <SelectTrigger
        size="sm"
        aria-label={driver === "sqlite" ? "Attached database" : "Schema"}
        title={driver === "sqlite" ? "Attached database" : "Schema"}
        className="hover:bg-sidebar-accent/60 min-w-0 gap-1 border-transparent bg-transparent px-1.5 text-[11px] shadow-none data-[size=sm]:h-6 dark:bg-transparent [&_svg:not([class*='size-'])]:size-3"
      >
        <SelectValue placeholder="Schema" />
      </SelectTrigger>
      <SelectContent align="end">
        {(schemas.data ?? []).map((s) => (
          <SelectItem key={s.name} value={s.name} className="text-xs">
            {s.name}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

/**
 * The sidebar's bottom bar (32px, like the main area's): view options (⚙), "+" (New Table…, New
 * Query) and the schema picker.
 */
export function SidebarFooter({ connection }: { connection: OpenConnection }) {
  const options = useSidebarPrefs((s) => s.options);
  const currentSchema = useCurrentSchema(connection.info.connectionId);
  const [dialog, setDialog] = useState<TableDialogState | null>(null);
  return (
    <div className="border-sidebar-border bg-sidebar flex h-8 shrink-0 items-center gap-0.5 border-t px-1.5">
      <DropdownMenu>
        <DropdownMenuTrigger className={cn(iconButton, "w-6")} aria-label="Sidebar options">
          <Settings2 className="size-3.5" />
        </DropdownMenuTrigger>
        <DropdownMenuContent side="top" align="start" className="w-48">
          <DropdownMenuLabel className="text-muted-foreground text-[11px]">
            Sidebar
          </DropdownMenuLabel>
          <DropdownMenuCheckboxItem
            checked={options.showRecent}
            onCheckedChange={(v) => setSidebarOption("showRecent", v === true)}
          >
            Show Recent
          </DropdownMenuCheckboxItem>
          <DropdownMenuCheckboxItem
            checked={options.showRowCounts}
            onCheckedChange={(v) => setSidebarOption("showRowCounts", v === true)}
          >
            Show row counts
          </DropdownMenuCheckboxItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <DropdownMenu>
        <DropdownMenuTrigger className={cn(iconButton, "w-6")} aria-label="New item">
          <Plus className="size-3.5" />
        </DropdownMenuTrigger>
        <DropdownMenuContent side="top" align="start" className="w-48">
          <DropdownMenuItem
            disabled={connection.info.readOnly}
            onSelect={() => setDialog({ kind: "create", schema: currentSchema })}
          >
            <SquarePlus /> New Table…
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => void actions.run("tab.new-query")}>
            <SquareTerminal /> New Query
            {NEW_QUERY_COMBO && (
              <DropdownMenuShortcut>{formatCombo(NEW_QUERY_COMBO)}</DropdownMenuShortcut>
            )}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <div className="ml-auto flex min-w-0 justify-end">
        <SchemaPicker connection={connection} />
      </div>
      <TableDialogs
        connectionId={connection.info.connectionId}
        dialect={connection.info.driver}
        state={dialog}
        onClose={() => setDialog(null)}
      />
    </div>
  );
}
