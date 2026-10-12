import { useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { connectionSummary } from "@/features/connections/schema";
import { useProfiles } from "@/features/connections/useConnections";
import { useSwitchingDatabase } from "@/features/sidebar/switchDatabase";
import {
  useCurrentSchema,
  useDatabases,
  useSchemas,
  useTables,
} from "@/features/sidebar/useSchemaTree";
import { pendingCount } from "@/features/staging/model";
import { useStructureStore } from "@/features/structure/store";
import { KEYMAP } from "@/lib/keys/keymap";
import { useActiveConnection, useConnectionsStore } from "@/stores/connections";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { actions, type ActionId } from "./actions";
import { paletteCommandItems, type PaletteCommands } from "./commands";
import {
  actionApplies,
  buildOpenAnything,
  buildSections,
  connectionItems,
  databaseItems,
  isCommandQuery,
  keymapActionItems,
  schemaItems,
  tabItems,
  tableItems,
  type PaletteGroupId,
  type PaletteSection,
} from "./items";
import { readMru } from "./mru";
import { usePaletteStore, type PaletteMode } from "./store";

export interface PaletteData {
  sections: PaletteSection[];
  /** Schema whose tables are listed. */
  schema: string | null;
  connection: { name: string; color: string | null } | null;
  /** Open Anything lists commands: the query starts with `>`. */
  commandMode: boolean;
}

/** Action ids that have a handler right now. Snapshotted on mount, i.e. when the palette opens. */
function availableActions(): ReadonlySet<ActionId> {
  return new Set(KEYMAP.map((d) => d.id).filter((id) => actions.has(id)));
}

/** Group order of the narrowed palettes (⌘K, ⇧⌘K). */
const DATABASE_ORDER: readonly PaletteGroupId[] = ["databases", "schemas"];
const CONNECTION_ORDER: readonly PaletteGroupId[] = ["connections"];

/**
 * Gathers everything the palette lists in `mode` and ranks it for `query`. Open Anything (⌘P):
 * one flat list of the current schema's tables, the open tabs, schemas, databases and saved
 * connections, or of the actions once the query starts with `>`. ⌘K: databases and schemas.
 * ⇧⌘K: connections. Mount only while the palette is open.
 */
export function usePaletteItems(
  query: string,
  commands: PaletteCommands,
  mode: PaletteMode = "all",
): PaletteData {
  const active = useActiveConnection();
  const connectionId = active?.info.connectionId ?? null;
  const profileId = active?.info.profileId ?? "";
  const driver = active?.info.driver ?? null;
  const currentDatabase = active?.info.currentDatabase ?? null;
  const databases = useDatabases(connectionId);
  const schemas = useSchemas(connectionId);
  const schema = useCurrentSchema(connectionId);
  const tables = useTables(connectionId, schema);
  const switching = useSwitchingDatabase(connectionId);
  const profiles = useProfiles();
  const tabs = useTabsStore((s) => s.tabs);
  const activeTabId = useTabsStore((s) => s.activeId);
  const closedCount = useTabsStore((s) => s.closed.length);
  const slices = useTableTabsStore((s) => s.slices);
  const structure = useStructureStore((s) => s.byTab);
  const openConnections = useConnectionsStore(
    useShallow((s) => s.order.map((id) => s.byId[id]).filter((c) => c !== undefined)),
  );
  const themePref = useUiStore((s) => s.themePref);
  const connecting = usePaletteStore((s) => s.connecting);
  const [mru] = useState(readMru);
  const [available] = useState(availableActions);

  const tableGroup = useMemo(
    () =>
      connectionId
        ? tableItems(tables.data ?? [], {
            profileId,
            open: (t, options) => commands.openTable(connectionId, t, options),
          })
        : [],
    [tables.data, connectionId, profileId, commands],
  );

  const tabGroup = useMemo(() => {
    const names = new Map(openConnections.map((c) => [c.info.connectionId, c.info.name]));
    // Same rule as the tab strip (`unsavedCount`), read from the subscribed slices: a table tab
    // keeps its staged rows and its staged DDL whichever view it shows.
    const isDirty = (tab: Tab) => {
      if (tab.kind === "query") return tab.dirty === true;
      const staging = slices[tab.id]?.staging;
      const rows = staging !== undefined ? pendingCount(staging) : 0;
      return rows + (structure[tab.id]?.ops.length ?? 0) > 0;
    };
    return tabItems(tabs, {
      activeId: activeTabId,
      isDirty,
      connectionName: names.size > 1 ? (id) => names.get(id) ?? null : undefined,
      activate: commands.activateTab,
    });
  }, [tabs, activeTabId, slices, structure, openConnections, commands]);

  // Same rules as the sidebar's DatabasePicker: PostgreSQL schemas and SQLite attached databases
  // (only when there is a choice), PostgreSQL / MySQL databases.
  const schemaGroup = useMemo(() => {
    const list = schemas.data ?? [];
    if (!connectionId || (driver !== "postgres" && driver !== "sqlite") || list.length < 2)
      return [];
    return schemaItems(list, {
      profileId,
      database: currentDatabase,
      current: schema,
      select: (name) => commands.selectSchema(connectionId, name),
    });
  }, [schemas.data, connectionId, driver, profileId, currentDatabase, schema, commands]);

  const databaseGroup = useMemo(() => {
    if (!connectionId || driver === "sqlite") return [];
    return databaseItems(databases.data ?? [], {
      profileId,
      current: currentDatabase,
      switching,
      switchTo: (name) => commands.switchDatabase(connectionId, name),
    });
  }, [databases.data, connectionId, driver, profileId, currentDatabase, switching, commands]);

  const connectionGroup = useMemo(() => {
    const openByProfile = new Map(
      openConnections.map((c) => [c.info.profileId, c.info.connectionId]),
    );
    return connectionItems(profiles.data ?? [], {
      openByProfile,
      connecting: new Set(connecting),
      activeConnectionId: connectionId,
      summary: connectionSummary,
      connect: commands.connect,
      switchTo: commands.switchConnection,
    });
  }, [profiles.data, openConnections, connecting, connectionId, commands]);

  const activeName = active?.info.name ?? null;
  const actionGroup = useMemo(() => {
    const ctx = { hasConnection: connectionId !== null, tabCount: tabs.length, closedCount };
    return [
      ...keymapActionItems(
        KEYMAP,
        (id) => available.has(id) && actionApplies(id, ctx),
        commands.runAction,
      ),
      ...paletteCommandItems({
        themePref,
        active:
          connectionId && activeName !== null ? { connectionId, name: activeName, schema } : null,
        commands,
      }),
    ];
  }, [available, tabs.length, closedCount, themePref, connectionId, activeName, schema, commands]);

  const tablesLoading =
    connectionId !== null && (schemas.isPending || (schema !== null && tables.isPending));
  // ⌘K lists the databases of PostgreSQL / MySQL and the attached databases of SQLite.
  const databasesLoading =
    connectionId !== null && (driver === "sqlite" ? schemas.isPending : databases.isPending);
  const databasesPrimary: PaletteGroupId = driver === "sqlite" ? "schemas" : "databases";
  const profilesLoading = profiles.isPending;

  const sections = useMemo(() => {
    if (mode === "databases")
      return buildSections({ databases: databaseGroup, schemas: schemaGroup }, query, {
        mru,
        order: DATABASE_ORDER,
        loading: databasesLoading ? [databasesPrimary] : [],
      });
    if (mode === "connections")
      return buildSections({ connections: connectionGroup }, query, {
        mru,
        order: CONNECTION_ORDER,
        loading: profilesLoading ? ["connections"] : [],
      });
    const loading: PaletteGroupId[] = [];
    if (tablesLoading) loading.push("tables");
    if (profilesLoading) loading.push("connections");
    return buildOpenAnything(
      {
        tables: tableGroup,
        tabs: tabGroup,
        schemas: schemaGroup,
        databases: databaseGroup,
        connections: connectionGroup,
        actions: actionGroup,
      },
      query,
      // Without a connection there are no tables to browse: list the saved connections.
      { mru, browse: connectionId ? ["tables"] : ["connections"], loading },
    );
  }, [
    mode,
    connectionId,
    tableGroup,
    tabGroup,
    schemaGroup,
    databaseGroup,
    connectionGroup,
    actionGroup,
    query,
    mru,
    tablesLoading,
    databasesLoading,
    databasesPrimary,
    profilesLoading,
  ]);

  return {
    sections,
    schema,
    connection: active ? { name: active.info.name, color: active.info.color } : null,
    commandMode: mode === "all" && isCommandQuery(query),
  };
}
