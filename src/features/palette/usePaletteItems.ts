import { useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { connectionSummary } from "@/features/connections/schema";
import { useProfiles } from "@/features/connections/useConnections";
import { useCurrentSchema, useSchemas, useTables } from "@/features/sidebar/useSchemaTree";
import { pendingCount } from "@/features/staging/model";
import { KEYMAP } from "@/lib/keys/keymap";
import { useActiveConnection, useConnectionsStore } from "@/stores/connections";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { actions, type ActionId } from "./actions";
import { paletteCommandItems, type PaletteCommands } from "./commands";
import {
  actionApplies,
  buildSections,
  connectionItems,
  keymapActionItems,
  tabItems,
  tableItems,
  type PaletteSection,
} from "./items";
import { readMru } from "./mru";
import { usePaletteStore } from "./store";

export interface PaletteData {
  sections: PaletteSection[];
  /** Schema whose tables are listed (heading of the Tables group). */
  schema: string | null;
  tablesLoading: boolean;
  connection: { name: string; color: string | null } | null;
}

/** Action ids that have a handler right now. Snapshotted on mount, i.e. when the palette opens. */
function availableActions(): ReadonlySet<ActionId> {
  return new Set(KEYMAP.map((d) => d.id).filter((id) => actions.has(id)));
}

/**
 * Gathers everything the palette lists (tables of the current schema, open tabs, saved
 * connections, actions) and ranks it for `query`. Mount only while the palette is open.
 */
export function usePaletteItems(query: string, commands: PaletteCommands): PaletteData {
  const active = useActiveConnection();
  const connectionId = active?.info.connectionId ?? null;
  const profileId = active?.info.profileId ?? "";
  const schemas = useSchemas(connectionId);
  const schema = useCurrentSchema(connectionId);
  const tables = useTables(connectionId, schema);
  const profiles = useProfiles();
  const tabs = useTabsStore((s) => s.tabs);
  const activeTabId = useTabsStore((s) => s.activeId);
  const closedCount = useTabsStore((s) => s.closed.length);
  const slices = useTableTabsStore((s) => s.slices);
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
            open: (t, kind) => commands.openTable(connectionId, t, kind),
          })
        : [],
    [tables.data, connectionId, profileId, commands],
  );

  const tabGroup = useMemo(() => {
    const names = new Map(openConnections.map((c) => [c.info.connectionId, c.info.name]));
    // Same rule as the tab strip (`unsavedCount`), read from the subscribed slices.
    const isDirty = (tab: Tab) => {
      if (tab.kind === "query") return tab.dirty === true;
      const staging = tab.kind === "table" ? slices[tab.id]?.staging : undefined;
      return staging !== undefined && pendingCount(staging) > 0;
    };
    return tabItems(tabs, {
      activeId: activeTabId,
      isDirty,
      connectionName: names.size > 1 ? (id) => names.get(id) ?? null : undefined,
      activate: commands.activateTab,
    });
  }, [tabs, activeTabId, slices, openConnections, commands]);

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

  const sections = useMemo(
    () =>
      buildSections(
        {
          tables: tableGroup,
          tabs: tabGroup,
          connections: connectionGroup,
          actions: actionGroup,
        },
        query,
        { mru },
      ),
    [tableGroup, tabGroup, connectionGroup, actionGroup, query, mru],
  );

  return {
    sections,
    schema,
    tablesLoading:
      connectionId !== null && (schemas.isPending || (schema !== null && tables.isPending)),
    connection: active ? { name: active.info.name, color: active.info.color } : null,
  };
}
