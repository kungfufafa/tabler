import { Plug } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { EmptyState } from "@/components/EmptyState";
import { Kbd } from "@/components/Kbd";
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
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { capsOf } from "@/lib/caps";
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
import { setMatchStrategy, useSidebarPrefs } from "./sidebarPrefs";
import { useLazySchemaTree } from "./useLazySchemaTree";
import { useCurrentSchema, useSchemas, useTables, useTreeRows } from "./useSchemaTree";
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
      className="border-sidebar-border bg-surface-2 flex h-8 w-full items-center gap-0.5 rounded-lg border p-0.5"
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
              "focus-visible:ring-sidebar-ring h-full min-w-0 flex-1 truncate rounded-md border border-transparent px-1 text-xs leading-4 transition-colors outline-none focus-visible:ring-1 focus-visible:ring-inset",
              checked
                ? "border-sidebar-border bg-surface text-foreground font-medium forced-colors:border-[Highlight]"
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
function DatabaseItemsPane({
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
  const schemas = useSchemas(connectionId);
  const schema = useCurrentSchema(connectionId);
  const tables = useTables(connectionId, lazy ? null : schema);
  // The tables query waits for a schema, so loading also covers the schema list: otherwise the
  // tree says "No tables" while the schemas are still on their way.
  const loading = schemas.isPending || (schema !== null && tables.isPending);
  const error = tables.isError
    ? tables.error.message
    : schemas.isError
      ? schemas.error.message
      : null;
  const collapsed = useSidebarPrefs((s) => s.collapsed);
  const showRecent = useSidebarPrefs((s) => s.options.showRecent);
  const showRowCounts = useSidebarPrefs((s) => s.options.showRowCounts);
  const groupByKind = useSidebarPrefs((s) => s.options.groupByKind);
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
    groupByKind,
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
  // Every object of the database, in schemas not loaded too, starting from this search.
  const searchInDatabase = () => usePaletteStore.getState().openPalette("objects", search.trim());
  /** Error state's Retry: loads again what failed (the schema list also roots the lazy tree). */
  const retry = () => {
    if (schemas.isError) void schemas.refetch();
    if (tables.isError) void tables.refetch();
  };
  /** The search field's menu: Search in Database…, Matching Strategy ▸, Settings ▸. */
  const menu = () =>
    searchMenu(
      {
        strategy,
        settings: searchSettingsEntries(connection.info.driver, settings, set),
        canSearchDatabase: true,
      },
      { searchInDatabase, setStrategy: setMatchStrategy },
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
      <div className="flex shrink-0 px-2 pb-2">
        <SidebarSearch
          ref={searchRef}
          value={search}
          onChange={setSearch}
          onArrowDown={() => treeRef.current?.focusFirstRow()}
          onEnter={() => treeRef.current?.openFirstRow()}
          hint={searchHint(strategy)}
          menu={menu}
        />
      </div>
      <SchemaTree
        ref={treeRef}
        connectionId={connectionId}
        rows={rows}
        loading={lazy ? lazyTree.loading : loading}
        error={lazy ? lazyTree.error : error}
        search={search}
        pinScope={scope}
        pinned={pinned}
        showRowCounts={showRowCounts}
        groups={groups}
        strategy={strategy}
        lazyKey={lazy ? lazyKey(connectionId, connection.info.currentDatabase) : null}
        onRetry={retry}
        onSearchDatabase={searchInDatabase}
      />
    </div>
  );
}

function ItemsPane(props: Parameters<typeof DatabaseItemsPane>[0]) {
  const { info } = props.connection;
  if (capsOf(info).namespaces.databases && info.currentDatabase === null)
    return (
      // TablePlus (Windows 27.2, MySQL without a database): this line alone, no button; the
      // database list opens from Ctrl+K or the toolbar.
      <div
        hidden={props.hidden}
        className="text-muted-foreground flex min-h-0 flex-1 items-center justify-center p-4 text-center text-xs"
      >
        <p>
          No database selected, press{" "}
          <Kbd
            combo={shortcutFor("palette.databases") ?? "Mod+K"}
            className="mx-0.5 align-middle"
          />{" "}
          to select a database
        </p>
      </div>
    );
  return <DatabaseItemsPane {...props} />;
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
    <aside
      aria-label="Database sidebar"
      className="bg-sidebar text-sidebar-foreground flex h-full flex-col"
      data-scope="sidebar"
    >
      {active ? (
        <>
          <div className="flex shrink-0 items-center p-2">
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
