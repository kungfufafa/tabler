import { Plug } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { EmptyState } from "@/components/EmptyState";
import { Button } from "@/components/ui/button";
import { searchMenu, searchSettingsEntries } from "@/features/objects/menu";
import { searchHint } from "@/features/objects/search";
import { useAction } from "@/features/palette/actions";
import { usePaletteStore } from "@/features/palette/store";
import { QueriesPane } from "@/features/queries/QueriesPane";
import {
  showSidebarPane,
  useSidebarPaneId,
  type SidebarPaneId,
} from "@/features/queries/sidebarPane";
import { cn } from "@/lib/utils";
import { useActiveConnection, type OpenConnection } from "@/stores/connections";
import { pinScope, usePinned } from "@/stores/pins";
import { EMPTY_RECENT, useRecent } from "@/stores/recent";
import { useUiStore } from "@/stores/ui";
import { GroupDialogHost } from "./GroupDialog";
import { useGroups } from "./groups";
import { HistoryPane } from "./HistoryPane";
import { lazyKey, useLazyLoad } from "./lazyLoad";
import { SchemaTree, type SchemaTreeHandle } from "./SchemaTree";
import { SidebarFooter } from "./SidebarFooter";
import { SidebarSearch } from "./SidebarSearch";
import { SidebarTopBar } from "./SidebarTopBar";
import { setMatchStrategy, useSidebarPrefs } from "./sidebarPrefs";
import { useLazySchemaTree } from "./useLazySchemaTree";
import { useCurrentSchema, useTables, useTreeRows } from "./useSchemaTree";
import { useSidebarSettings } from "./useSidebarSettings";

const PANES: Array<{ id: SidebarPaneId; label: string }> = [
  { id: "items", label: "Items" },
  { id: "queries", label: "Queries" },
  { id: "history", label: "History" },
];

/** TablePlus's "Items | Queries | History" switch at the top of the sidebar. */
function PaneSwitch({ pane }: { pane: SidebarPaneId }) {
  return (
    <div
      role="radiogroup"
      aria-label="Sidebar pane"
      className="border-border bg-surface flex h-5.5 w-full items-center rounded-md border p-px"
    >
      {PANES.map((p) => {
        const checked = p.id === pane;
        return (
          <button
            key={p.id}
            type="button"
            role="radio"
            aria-checked={checked}
            onClick={() => showSidebarPane(p.id)}
            className={cn(
              "focus-visible:ring-ring/50 h-full flex-1 rounded-[5px] text-[11px] leading-none outline-none focus-visible:ring-2",
              checked
                ? "bg-background text-foreground font-medium shadow-xs"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            {p.label}
          </button>
        );
      })}
    </div>
  );
}

/**
 * The Items pane: the search field (with its menu) and the schema tree (Recent, Pinned, Tables,
 * Views…), or with "Lazy Load Schema Items" one node per schema / database.
 */
function ItemsPane({
  connection,
  hidden,
  focusSearch,
}: {
  connection: OpenConnection;
  hidden: boolean;
  /** Bumped to focus (and select) the search field. */
  focusSearch: number;
}) {
  const [search, setSearch] = useState("");
  const searchRef = useRef<HTMLInputElement>(null);
  const treeRef = useRef<SchemaTreeHandle>(null);
  const connectionId = connection.info.connectionId;
  const lazy = useLazyLoad(connection);
  const schema = useCurrentSchema(connectionId);
  const tables = useTables(connectionId, lazy ? null : schema);
  const collapsed = useSidebarPrefs((s) => s.collapsed);
  const showRecent = useSidebarPrefs((s) => s.options.showRecent);
  const showRowCounts = useSidebarPrefs((s) => s.options.showRowCounts);
  const strategy = useSidebarPrefs((s) => s.matchStrategy);
  const { settings, set } = useSidebarSettings(connection);
  // Pins and recent tables follow the saved connection and its database, not the (per-session)
  // connection id.
  const scope = pinScope(connection.info.profileId, connection.info.currentDatabase);
  const pinned = usePinned(scope);
  const recent = useRecent(scope);
  const groups = useGroups(scope);
  const shownRecent = showRecent ? recent : EMPTY_RECENT;
  const regularRows = useTreeRows(
    lazy ? undefined : tables.data,
    search,
    pinned,
    shownRecent,
    collapsed,
    groups,
    strategy,
  );
  const lazyTree = useLazySchemaTree(connection, lazy, {
    search,
    strategy,
    pinned,
    recent: shownRecent,
    collapsed,
    groups,
  });
  const rows = lazy ? lazyTree.rows : regularRows;
  /** The search field's menu: Search in Database…, Matching Strategy ▸, Settings ▸. */
  const menu = () =>
    searchMenu(
      {
        strategy,
        settings: searchSettingsEntries(connection.info.driver, settings, set),
        canSearchDatabase: true,
      },
      {
        // Every object of the database, in schemas not loaded too, starting from this search.
        searchInDatabase: () => usePaletteStore.getState().openPalette("objects", search.trim()),
        setStrategy: setMatchStrategy,
      },
    );

  // Only a bump after mounting focuses: a remount (another connection) must not take focus.
  const focusedFor = useRef(focusSearch);
  useEffect(() => {
    if (focusedFor.current === focusSearch) return;
    focusedFor.current = focusSearch;
    searchRef.current?.focus();
    searchRef.current?.select();
  }, [focusSearch]);

  return (
    <div className="flex min-h-0 flex-1 flex-col" hidden={hidden}>
      <div className="flex shrink-0 px-2 pb-1.5">
        <SidebarSearch
          ref={searchRef}
          value={search}
          onChange={setSearch}
          onArrowDown={() => treeRef.current?.focusFirstRow()}
          hint={searchHint(strategy)}
          menu={menu}
        />
      </div>
      <SchemaTree
        ref={treeRef}
        connectionId={connectionId}
        rows={rows}
        loading={lazy ? lazyTree.loading : tables.isPending && schema !== null}
        error={lazy ? lazyTree.error : tables.isError ? tables.error.message : null}
        search={search}
        pinScope={scope}
        pinned={pinned}
        showRowCounts={showRowCounts}
        groups={groups}
        strategy={strategy}
        lazyKey={lazy ? lazyKey(connectionId, connection.info.currentDatabase) : null}
      />
    </div>
  );
}

export function Sidebar() {
  const active = useActiveConnection();
  const setView = useUiStore((s) => s.setView);
  const pane = useSidebarPaneId();
  const [focusSearch, setFocusSearch] = useState(0);

  // ⇧⌘F: the Items search, from any pane.
  useAction("sidebar.focus-search", () => {
    showSidebarPane("items");
    setFocusSearch((n) => n + 1);
  });

  return (
    <aside className="bg-sidebar text-sidebar-foreground flex h-full flex-col" data-scope="sidebar">
      {/* Show Connections / Show Databases / Open Query Editor (disabled without a connection). */}
      <SidebarTopBar />
      {active ? (
        <>
          <div className="flex h-8 shrink-0 items-center px-2">
            <PaneSwitch pane={pane} />
          </div>
          <ItemsPane
            // Search text, scroll position and focus belong to one connection's tree.
            key={active.info.connectionId}
            connection={active}
            hidden={pane !== "items"}
            focusSearch={focusSearch}
          />
          {pane === "queries" && <QueriesPane connection={active} />}
          {pane === "history" && <HistoryPane connection={active} />}
          <SidebarFooter connection={active} />
          {/* New Group… / Edit Group…. */}
          <GroupDialogHost />
        </>
      ) : (
        <EmptyState
          icon={Plug}
          title="No connection"
          description="Open a connection to browse its tables."
        >
          <Button size="sm" variant="outline" onClick={() => setView("launcher")}>
            Connections
          </Button>
        </EmptyState>
      )}
    </aside>
  );
}
