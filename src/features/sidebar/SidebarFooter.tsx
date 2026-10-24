import { ChevronsUpDown, Plus, Settings2 } from "lucide-react";
import { useEffect, useState } from "react";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { coreDialect } from "@/features/drivers/catalog";
import { runNew, type ObjectContext } from "@/features/objects/actions";
import { plusMenu, settingsEntries } from "@/features/objects/menu";
import { DropdownMenuEntries } from "@/features/objects/MenuEntries";
import { PG_SYSTEM_SCHEMAS, useSystemSchemas } from "@/features/objects/queries";
import { TableDialogs, type TableDialogState } from "@/features/structure/TableDialogs";
import { shortcutFor, useKeymapVersion } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { useConnectionsStore, type OpenConnection } from "@/stores/connections";
import { pinScope } from "@/stores/pins";
import { setSidebarOption, useSidebarPrefs } from "./sidebarPrefs";
import { useCurrentSchema, useSchemas } from "./useSchemaTree";
import { useSidebarSettings } from "./useSidebarSettings";

const iconButton =
  "text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground data-[state=open]:bg-sidebar-accent data-[state=open]:text-foreground focus-visible:ring-ring/50 flex h-6 shrink-0 items-center justify-center gap-0.5 rounded-md px-1 outline-none focus-visible:ring-2";

/**
 * The schema switcher at the bottom (TablePlus): PostgreSQL schemas (plus the system schemas when
 * "Show System Schemas" is on, which its menu toggles too), SQLite's attached databases (main,
 * temp, …; shown only when there is more than one).
 */
export function SchemaPicker({ connection }: { connection: OpenConnection }) {
  const id = connection.info.connectionId;
  const driver = connection.info.driver;
  const schemas = useSchemas(id);
  const showSystemPref = useSidebarPrefs((s) => s.options.showSystemSchemas);
  const showSystem = showSystemPref && driver === "postgres";
  const system = useSystemSchemas(id, showSystem);
  const current = useCurrentSchema(id);
  const setCurrentSchema = useConnectionsStore((s) => s.setCurrentSchema);
  // Hiding the system schemas while one is shown goes back to the default schema.
  useEffect(() => {
    if (!showSystem && current !== null && PG_SYSTEM_SCHEMAS.includes(current))
      setCurrentSchema(id, null);
  }, [showSystem, current, id, setCurrentSchema]);
  if (driver !== "postgres" && driver !== "sqlite") return null;
  if (!schemas.data) return null;
  const list = [...schemas.data];
  if (showSystem)
    for (const s of system.data ?? []) if (!list.some((x) => x.name === s.name)) list.push(s);
  // PostgreSQL keeps it for its "Show System Schemas" toggle.
  if (list.length < 2 && driver !== "postgres") return null;
  const label = driver === "sqlite" ? "Attached database" : "Schema";
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label={label}
        title={label}
        className={cn(iconButton, "max-w-40 min-w-0 px-1.5 text-[11px]")}
      >
        <span className="truncate">{current ?? label}</span>
        <ChevronsUpDown aria-hidden className="size-3 shrink-0 opacity-70" />
      </DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="end" className="max-h-80 min-w-44">
        <DropdownMenuRadioGroup
          value={current ?? ""}
          onValueChange={(v) => setCurrentSchema(id, v)}
        >
          {list.map((s) => (
            <DropdownMenuRadioItem
              key={s.name}
              value={s.name}
              className={cn(
                "text-xs",
                PG_SYSTEM_SCHEMAS.includes(s.name) && "text-muted-foreground",
              )}
            >
              {s.name}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        {driver === "postgres" && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuCheckboxItem
              checked={showSystemPref}
              onCheckedChange={(v) => setSidebarOption("showSystemSchemas", v === true)}
              className="text-xs"
            >
              Show System Schemas
            </DropdownMenuCheckboxItem>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/**
 * The sidebar's bottom bar (32px, like the main area's): view options (⚙), "+" (New Table… …
 * New Schema…, New Query, Settings ▸) and the schema picker.
 */
export function SidebarFooter({ connection }: { connection: OpenConnection }) {
  // Read at render: the user can remap it (Settings ▸ Shortcuts).
  useKeymapVersion();
  const newQueryCombo = shortcutFor("tab.new-query");
  const { settings, set } = useSidebarSettings(connection);
  const lazy = settings.lazyLoad === true;
  const currentSchema = useCurrentSchema(connection.info.connectionId);
  const [dialog, setDialog] = useState<TableDialogState | null>(null);
  const { driver, readOnly, connectionId } = connection.info;
  const dialect = coreDialect(driver);
  const ctx: ObjectContext = {
    connectionId,
    driver,
    readOnly,
    schema: currentSchema,
    pinScope: pinScope(connection.info.profileId, connection.info.currentDatabase),
  };
  const settingItems = settingsEntries(driver, settings, set);
  const plus = plusMenu(
    driver,
    readOnly,
    settingItems,
    {
      newItem: (what) =>
        runNew(
          ctx,
          what,
          // Without the dialog (another driver), New ▸ Table… says it is not available.
          dialect ? () => setDialog({ kind: "create", schema: currentSchema }) : undefined,
        ),
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
        {/* Lazy load lists the schemas in the tree itself. */}
        {!lazy && <SchemaPicker connection={connection} />}
      </div>
      {/* Their SQL is a core dialect's: none for other drivers. */}
      {dialect && (
        <TableDialogs
          connectionId={connectionId}
          dialect={dialect}
          state={dialog}
          onClose={() => setDialog(null)}
        />
      )}
    </div>
  );
}
