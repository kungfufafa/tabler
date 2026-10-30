import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { openCreateDatabase, openCreateSchema } from "@/features/admin/store";
import { useThemes } from "@/features/appearance/themesStore";
import { connectionSummary } from "@/features/connections/schema";
import { useProfiles } from "@/features/connections/useConnections";
import { openRoutine } from "@/features/objects/actions";
import { objectKeys, useRoutines } from "@/features/objects/queries";
import { openSavedQuery, runSavedQuery } from "@/features/queries/open";
import { ensureSavedQueries, queriesFor, useSavedQueriesStore } from "@/features/queries/store";
import { loadLazyNodes, revealLazyNode, useLazyLoad } from "@/features/sidebar/lazyLoad";
import { useSidebarPrefs } from "@/features/sidebar/sidebarPrefs";
import { useSwitchingDatabase } from "@/features/sidebar/switchDatabase";
import { useLazyItems } from "@/features/sidebar/useLazySchemaTree";
import { openProfileInNewWindow } from "@/features/tabs/windowHandoff";
import {
  useCurrentSchema,
  useDatabases,
  useSchemas,
  useTables,
} from "@/features/sidebar/useSchemaTree";
import { pendingCount } from "@/features/staging/model";
import { useStructureStore } from "@/features/structure/store";
import { capsOf } from "@/lib/caps";
import { ipc, type Feature, type RoutineSummary, type TableSummary } from "@/lib/ipc";
import { KEYMAP } from "@/lib/keys/keymap";
import { useActiveConnection, useConnectionsStore } from "@/stores/connections";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { actions, type ActionId } from "./actions";
import { databaseMenu, schemaMenu } from "./adminItems";
import { PALETTE_ONLY_ACTIONS, paletteCommandItems, type PaletteCommands } from "./commands";
import { extraPaletteItems } from "./extraItems";
import {
  actionApplies,
  buildOpenAnything,
  buildSections,
  connectionItems,
  createItem,
  databaseItems,
  isCommandQuery,
  keymapActionItems,
  loadAllSchemasItem,
  routineItems,
  savedQueryItems,
  schemaItems,
  showSchemaInObjects,
  tabItems,
  tableItems,
  type PaletteGroupId,
  type PaletteSection,
} from "./items";
import { readMru } from "./mru";
import { usePaletteStore, type PaletteMode } from "./store";

export interface PaletteData {
  sections: PaletteSection[];
  /** What is searched: the schema whose tables are listed, or the loaded schemas / database. */
  schema: string | null;
  connection: { name: string; color: string | null } | null;
  /** Open Anything lists commands: the query starts with `>`. */
  commandMode: boolean;
}

/** Action ids that have a handler right now. Snapshotted on mount, i.e. when the palette opens. */
function availableActions(): ReadonlySet<ActionId> {
  const ids = [...KEYMAP.map((d) => d.id), ...PALETTE_ONLY_ACTIONS];
  return new Set(ids.filter((id) => actions.has(id)));
}

/** Group order of the narrowed palettes (⌘K, ⇧⌘K). */
const DATABASE_ORDER: readonly PaletteGroupId[] = ["databases", "schemas"];
const CONNECTION_ORDER: readonly PaletteGroupId[] = ["connections"];

const NO_TABLES: readonly TableSummary[] = [];
const NO_ROUTINES: readonly RoutineSummary[] = [];

/**
 * Gathers everything the palette lists in `mode` and ranks it for `query`. Open Anything (⌘P):
 * one flat list of the current schema's tables (with lazy load: those of every loaded schema,
 * plus a "Load all schemas" row), the open tabs, schemas, databases and saved connections, or of
 * the actions once the query starts with `>`. ⌘K: databases and schemas. ⇧⌘K: connections.
 * Search in Database…: every table, view and routine of the database (`ipc.objects.databaseItems`).
 * Mount only while the palette is open.
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
  // What the session offers is ANDed in (every feature for the core sessions).
  const features = active ? capsOf(active.info).features : undefined;
  const offers = (f: Feature) => !features || features.includes(f);
  const currentDatabase = active?.info.currentDatabase ?? null;
  const databases = useDatabases(connectionId);
  const schemas = useSchemas(connectionId);
  const schema = useCurrentSchema(connectionId);
  const lazy = useLazyLoad(active);
  const lazyItems = useLazyItems(active, lazy && mode === "all", {
    routines: driver !== "sqlite" && offers("routines"),
  });
  const tables = useTables(connectionId, lazy ? null : schema);
  const switching = useSwitchingDatabase(connectionId);
  const routines = useRoutines(
    connectionId,
    schema,
    mode === "all" && !lazy && driver !== "sqlite" && offers("routines"),
  );
  // Search in Database…: one backend call for every schema (loaded in the sidebar or not).
  const includeSystem = useSidebarPrefs((s) => s.options.showSystemSchemas);
  const everything = useQuery({
    queryKey: objectKeys.databaseItems(connectionId ?? "", includeSystem),
    queryFn: () => ipc.objects.databaseItems(connectionId!, includeSystem),
    enabled: mode === "objects" && connectionId !== null,
    staleTime: 30_000,
  });
  const savedQueries = useSavedQueriesStore((s) => s.queries);
  const qc = useQueryClient();
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
  const customThemes = useThemes((s) => s.themes);
  const connecting = usePaletteStore((s) => s.connecting);
  const [mru] = useState(readMru);
  const [available] = useState(availableActions);

  // Lazy load: the tables / routines of the loaded schemas only (TablePlus #3309).
  const lazyNodes = lazyItems.nodes;
  const lazyTables = useMemo(
    () => (lazy ? lazyNodes.flatMap((n) => n.tables ?? []) : NO_TABLES),
    [lazy, lazyNodes],
  );
  const lazyRoutines = useMemo(
    () => (lazy ? lazyNodes.flatMap((n) => n.routines ?? []) : NO_ROUTINES),
    [lazy, lazyNodes],
  );
  const loadedCount = lazyNodes.filter((n) => n.tables !== undefined).length;
  const lazyKey = lazyItems.key;

  /**
   * An object of another schema was opened: the sidebar follows it, so it is highlighted there
   * (lazy load expands its node; otherwise the schema switcher moves).
   */
  const reveal = useMemo(
    () => (target: string | null) => {
      if (!connectionId || target === null || target === schema) return;
      if (lazy) {
        revealLazyNode(lazyKey, target);
        commands.selectSchema(connectionId, target);
      } else if (driver === "postgres" || driver === "sqlite") {
        commands.selectSchema(connectionId, target);
      }
    },
    [connectionId, schema, lazy, lazyKey, driver, commands],
  );

  const tableGroup = useMemo(() => {
    if (!connectionId) return [];
    const list =
      mode === "objects" ? (everything.data?.tables ?? NO_TABLES) : lazy ? lazyTables : tables.data;
    const showSchema =
      mode === "objects" ? showSchemaInObjects(driver, list ?? NO_TABLES) : lazy && loadedCount > 1;
    return tableItems(list ?? NO_TABLES, {
      profileId,
      showSchema,
      open: (t, options) => {
        reveal(t.schema);
        commands.openTable(connectionId, t, options);
      },
    });
  }, [
    mode,
    everything.data,
    lazy,
    lazyTables,
    loadedCount,
    tables.data,
    connectionId,
    driver,
    profileId,
    commands,
    reveal,
  ]);

  useEffect(() => {
    if (mode === "all") void ensureSavedQueries();
  }, [mode]);

  const routineGroup = useMemo(() => {
    if (!connectionId || driver === null) return [];
    const ctx = {
      connectionId,
      driver,
      readOnly: active?.info.readOnly ?? false,
      schema,
      pinScope: null,
    };
    const list =
      mode === "objects"
        ? (everything.data?.routines ?? NO_ROUTINES)
        : lazy
          ? lazyRoutines
          : (routines.data ?? NO_ROUTINES);
    const showSchema =
      mode === "objects" ? showSchemaInObjects(driver, list) : lazy && loadedCount > 1;
    return routineItems(list, { profileId, showSchema, open: (r) => openRoutine(ctx, r) });
  }, [
    mode,
    everything.data,
    lazy,
    lazyRoutines,
    loadedCount,
    routines.data,
    connectionId,
    driver,
    active?.info.readOnly,
    schema,
    profileId,
  ]);

  const queryGroup = useMemo(
    () =>
      active
        ? savedQueryItems(queriesFor(savedQueries, active.info.profileId), {
            open: (q) => openSavedQuery(active, q),
            run: (q) => runSavedQuery(qc, active, q),
          })
        : [],
    [savedQueries, active, qc],
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
    const select = (name: string) => commands.selectSchema(connectionId, name);
    return schemaItems(list, {
      profileId,
      database: currentDatabase,
      current: schema,
      select,
      menu: (name) => schemaMenu({ connectionId, driver, show: select, features }, name),
    });
  }, [schemas.data, connectionId, driver, profileId, currentDatabase, schema, commands, features]);

  const databaseGroup = useMemo(() => {
    if (!connectionId || driver === null || driver === "sqlite") return [];
    // Opening another database is switching to it.
    if (features && !features.includes("switchDatabase")) return [];
    // ↩ / double-click open a new workspace; "Open in Current Workspace" switches this one.
    const open = (name: string) => commands.switchDatabase(connectionId, name);
    return databaseItems(databases.data ?? [], {
      profileId,
      current: currentDatabase,
      switching,
      switchTo: (name) => commands.openDatabase(connectionId, name),
      menu: (name, current) =>
        databaseMenu({ connectionId, driver, open, features }, name, current),
    });
  }, [
    databases.data,
    connectionId,
    driver,
    profileId,
    currentDatabase,
    switching,
    commands,
    features,
  ]);

  // ⌘K's "+" rows: New Database… (PostgreSQL, MySQL) once the list is in, New Schema… (PG).
  const createRows = useMemo(() => {
    if (!connectionId) return { databases: [], schemas: [] };
    return {
      databases:
        (driver === "postgres" || driver === "mysql") &&
        databases.data &&
        (!features || features.includes("createDatabase"))
          ? [createItem("databases", { profileId, run: () => openCreateDatabase(connectionId) })]
          : [],
      schemas:
        driver === "postgres" && schemas.data && (!features || features.includes("createSchema"))
          ? [createItem("schemas", { profileId, run: () => openCreateSchema(connectionId) })]
          : [],
    };
  }, [connectionId, driver, profileId, databases.data, schemas.data, features]);

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
      menu: (p) => [{ label: "Open in New Window", run: () => void openProfileInNewWindow(p.id) }],
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
        customThemes,
        active:
          connectionId && activeName !== null ? { connectionId, name: activeName, schema } : null,
        commands,
        has: (id) => available.has(id),
      }),
      // Other features' commands (Plugins: ">Plugin: <name>: <command>").
      ...extraPaletteItems(),
    ];
  }, [
    available,
    tabs.length,
    closedCount,
    themePref,
    customThemes,
    connectionId,
    activeName,
    schema,
    commands,
  ]);

  const tablesLoading =
    connectionId !== null &&
    (mode === "objects"
      ? everything.isPending
      : lazy
        ? lazyItems.pending || lazyNodes.some((n) => n.loading && n.tables === undefined)
        : schemas.isPending || (schema !== null && tables.isPending));
  const notLoaded = lazy ? lazyItems.notLoaded.length : 0;
  const lazyNames = lazyItems.names;
  // ⌘K lists the databases of PostgreSQL / MySQL and the attached databases of SQLite.
  const databasesLoading =
    connectionId !== null && (driver === "sqlite" ? schemas.isPending : databases.isPending);
  const databasesPrimary: PaletteGroupId = driver === "sqlite" ? "schemas" : "databases";
  const profilesLoading = profiles.isPending;

  const sections = useMemo(() => {
    if (mode === "databases")
      return buildSections(
        {
          databases: [...databaseGroup, ...createRows.databases],
          schemas: [...schemaGroup, ...createRows.schemas],
        },
        query,
        {
          mru,
          order: DATABASE_ORDER,
          loading: databasesLoading ? [databasesPrimary] : [],
        },
      );
    if (mode === "connections")
      return buildSections({ connections: connectionGroup }, query, {
        mru,
        order: CONNECTION_ORDER,
        loading: profilesLoading ? ["connections"] : [],
      });
    if (mode === "objects")
      return buildOpenAnything({ tables: tableGroup, routines: routineGroup }, query, {
        mru,
        browse: ["tables", "routines"],
        loading: tablesLoading ? ["tables"] : [],
      });
    const loading: PaletteGroupId[] = [];
    if (tablesLoading) loading.push("tables");
    if (profilesLoading) loading.push("connections");
    const sections = buildOpenAnything(
      {
        tables: tableGroup,
        routines: routineGroup,
        tabs: tabGroup,
        queries: queryGroup,
        schemas: schemaGroup,
        databases: databaseGroup,
        connections: connectionGroup,
        actions: actionGroup,
      },
      query,
      // Without a connection there are no tables to browse: list the saved connections.
      { mru, browse: connectionId ? ["tables"] : ["connections"], loading },
    );
    // Lazy load: the schemas not loaded yet are not searched; this row loads them.
    if (notLoaded > 0 && !isCommandQuery(query))
      sections.push({
        id: "more",
        items: [
          {
            item: loadAllSchemasItem({
              profileId,
              notLoaded,
              run: () => loadLazyNodes(lazyKey, lazyNames),
            }),
            ranges: [],
          },
        ],
        more: 0,
      });
    return sections;
  }, [
    notLoaded,
    lazyKey,
    lazyNames,
    profileId,
    mode,
    connectionId,
    tableGroup,
    routineGroup,
    tabGroup,
    queryGroup,
    schemaGroup,
    databaseGroup,
    connectionGroup,
    actionGroup,
    query,
    mru,
    tablesLoading,
    createRows,
    databasesLoading,
    databasesPrimary,
    profilesLoading,
  ]);

  return {
    sections,
    schema:
      mode === "objects"
        ? (currentDatabase ?? "all schemas")
        : lazy
          ? `${loadedCount} of ${lazyNames.length} schemas`
          : schema,
    connection: active ? { name: active.info.name, color: active.info.color } : null,
    commandMode: mode === "all" && isCommandQuery(query),
  };
}
