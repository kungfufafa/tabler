import { Plug } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { EmptyState } from "@/components/EmptyState";
import { Button } from "@/components/ui/button";
import { useAction } from "@/features/palette/actions";
import { cn } from "@/lib/utils";
import { useActiveConnection, type OpenConnection } from "@/stores/connections";
import { pinScope, usePinned } from "@/stores/pins";
import { EMPTY_RECENT, useRecent } from "@/stores/recent";
import { useUiStore } from "@/stores/ui";
import { HistoryPane } from "./HistoryPane";
import { SchemaTree, type SchemaTreeHandle } from "./SchemaTree";
import { SidebarFooter } from "./SidebarFooter";
import { SidebarSearch } from "./SidebarSearch";
import { setSidebarPane, useSidebarPrefs, type SidebarPane } from "./sidebarPrefs";
import { useCurrentSchema, useTables, useTreeRows } from "./useSchemaTree";

const PANES: Array<{ id: SidebarPane; label: string }> = [
  { id: "items", label: "Items" },
  { id: "history", label: "History" },
];

/** TablePlus's "Items | History" switch at the top of the sidebar. */
function PaneSwitch({ pane }: { pane: SidebarPane }) {
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
            onClick={() => setSidebarPane(p.id)}
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

/** The Items pane: the search field and the schema tree (Recent, Pinned, Tables, Views…). */
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
  const schema = useCurrentSchema(connectionId);
  const tables = useTables(connectionId, schema);
  const collapsed = useSidebarPrefs((s) => s.collapsed);
  const showRecent = useSidebarPrefs((s) => s.options.showRecent);
  const showRowCounts = useSidebarPrefs((s) => s.options.showRowCounts);
  // Pins and recent tables follow the saved connection and its database, not the (per-session)
  // connection id.
  const scope = pinScope(connection.info.profileId, connection.info.currentDatabase);
  const pinned = usePinned(scope);
  const recent = useRecent(scope);
  const rows = useTreeRows(
    tables.data,
    search,
    pinned,
    showRecent ? recent : EMPTY_RECENT,
    collapsed,
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
        />
      </div>
      <SchemaTree
        ref={treeRef}
        connectionId={connectionId}
        rows={rows}
        loading={tables.isPending && schema !== null}
        error={tables.isError ? tables.error.message : null}
        search={search}
        pinScope={scope}
        pinned={pinned}
        showRowCounts={showRowCounts}
      />
    </div>
  );
}

export function Sidebar() {
  const active = useActiveConnection();
  const setView = useUiStore((s) => s.setView);
  const pane = useSidebarPrefs((s) => s.pane);
  const [focusSearch, setFocusSearch] = useState(0);

  // ⇧⌘F: the Items search, from either pane.
  useAction("sidebar.focus-search", () => {
    setSidebarPane("items");
    setFocusSearch((n) => n + 1);
  });

  return (
    <aside className="bg-sidebar text-sidebar-foreground flex h-full flex-col" data-scope="sidebar">
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
          {pane === "history" && <HistoryPane connection={active} />}
          <SidebarFooter connection={active} />
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
