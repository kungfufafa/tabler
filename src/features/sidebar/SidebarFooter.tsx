import { Plus, Settings2 } from "lucide-react";
import { useEffect, useState } from "react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { runNew, type ObjectContext } from "@/features/objects/actions";
import { plusMenu, settingsEntries, type SidebarSettings } from "@/features/objects/menu";
import { DropdownMenuEntries } from "@/features/objects/MenuEntries";
import { PG_SYSTEM_SCHEMAS, useSystemSchemas } from "@/features/objects/queries";
import { TableDialogs, type TableDialogState } from "@/features/structure/TableDialogs";
import { shortcutFor, useKeymapVersion } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { useConnectionsStore, type OpenConnection } from "@/stores/connections";
import { pinScope } from "@/stores/pins";
import { setSidebarOption, useSidebarPrefs } from "./sidebarPrefs";
import { useCurrentSchema, useSchemas } from "./useSchemaTree";

const iconButton =
  "text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground data-[state=open]:bg-sidebar-accent data-[state=open]:text-foreground focus-visible:ring-ring/50 flex h-6 shrink-0 items-center justify-center gap-0.5 rounded-md px-1 outline-none focus-visible:ring-2";

/**
 * Schema selector: PostgreSQL schemas (plus the system schemas when "Show System Schemas" is on),
 * SQLite's attached databases (main, temp, …). Shown only when there is more than one to pick
 * from.
 */
export function SchemaPicker({ connection }: { connection: OpenConnection }) {
  const id = connection.info.connectionId;
  const driver = connection.info.driver;
  const schemas = useSchemas(id);
  const showSystem = useSidebarPrefs((s) => s.options.showSystemSchemas) && driver === "postgres";
  const system = useSystemSchemas(id, showSystem);
  const current = useCurrentSchema(id);
  const setCurrentSchema = useConnectionsStore((s) => s.setCurrentSchema);
  // Hiding the system schemas while one is shown goes back to the default schema.
  useEffect(() => {
    if (!showSystem && current !== null && PG_SYSTEM_SCHEMAS.includes(current))
      setCurrentSchema(id, null);
  }, [showSystem, current, id, setCurrentSchema]);
  if (driver !== "postgres" && driver !== "sqlite") return null;
  const list = [...(schemas.data ?? [])];
  if (showSystem)
    for (const s of system.data ?? []) if (!list.some((x) => x.name === s.name)) list.push(s);
  if (list.length < 2) return null;
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
        {list.map((s) => (
          <SelectItem
            key={s.name}
            value={s.name}
            className={cn("text-xs", PG_SYSTEM_SCHEMAS.includes(s.name) && "text-muted-foreground")}
          >
            {s.name}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

/** The sidebar's view options (⚙ and "+" ▸ Settings), as `settingsEntries` takes them. */
function useSidebarSettings(): SidebarSettings {
  const options = useSidebarPrefs((s) => s.options);
  return {
    showRecent: options.showRecent,
    showFunctions: options.showFunctions,
    showSequences: options.showSequences,
    showSystemSchemas: options.showSystemSchemas,
    showRowCounts: options.showRowCounts,
  };
}

/**
 * The sidebar's bottom bar (32px, like the main area's): view options (⚙), "+" (New Table… …
 * New Schema…, New Query, Settings ▸) and the schema picker.
 */
export function SidebarFooter({ connection }: { connection: OpenConnection }) {
  // Read at render: the user can remap it (Settings ▸ Shortcuts).
  useKeymapVersion();
  const newQueryCombo = shortcutFor("tab.new-query");
  const settings = useSidebarSettings();
  const currentSchema = useCurrentSchema(connection.info.connectionId);
  const [dialog, setDialog] = useState<TableDialogState | null>(null);
  const { driver, readOnly, connectionId } = connection.info;
  const ctx: ObjectContext = {
    connectionId,
    driver,
    readOnly,
    schema: currentSchema,
    pinScope: pinScope(connection.info.profileId, connection.info.currentDatabase),
  };
  const settingItems = settingsEntries(driver, settings, (key, value) =>
    setSidebarOption(key, value),
  );
  const plus = plusMenu(
    driver,
    readOnly,
    settingItems,
    {
      newItem: (what) =>
        runNew(ctx, what, () => setDialog({ kind: "create", schema: currentSchema })),
    },
    newQueryCombo,
  );
  return (
    <div className="border-sidebar-border bg-sidebar flex h-8 shrink-0 items-center gap-0.5 border-t px-1.5">
      <DropdownMenu>
        <DropdownMenuTrigger className={cn(iconButton, "w-6")} aria-label="Sidebar options">
          <Settings2 className="size-3.5" />
        </DropdownMenuTrigger>
        <DropdownMenuContent side="top" align="start" className="w-52">
          <DropdownMenuLabel className="text-muted-foreground text-[11px]">
            Sidebar
          </DropdownMenuLabel>
          <DropdownMenuEntries entries={settingItems} />
        </DropdownMenuContent>
      </DropdownMenu>
      <DropdownMenu>
        <DropdownMenuTrigger className={cn(iconButton, "w-6")} aria-label="New item">
          <Plus className="size-3.5" />
        </DropdownMenuTrigger>
        <DropdownMenuContent side="top" align="start" className="w-56">
          <DropdownMenuEntries entries={plus} />
        </DropdownMenuContent>
      </DropdownMenu>
      <div className="ml-auto flex min-w-0 justify-end">
        <SchemaPicker connection={connection} />
      </div>
      <TableDialogs
        connectionId={connectionId}
        dialect={driver}
        state={dialog}
        onClose={() => setDialog(null)}
      />
    </div>
  );
}
