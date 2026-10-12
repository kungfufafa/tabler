import { useMutation, useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { Copy, History, Layers, Loader2, Play, SquareStack, Trash2 } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { toast } from "sonner";
import { EmptyState } from "@/components/EmptyState";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { dialectOf, runQuery } from "@/features/editor/controller";
import { groupByDay, HISTORY_KEY } from "@/features/editor/historyGroups";
import { firstLine } from "@/features/editor/labels";
import { openSqlInNewQueryTab } from "@/features/editor/openSql";
import { invalidateConnectionSchema, reportHeldTables } from "@/features/structure/invalidate";
import { ipc, toIpcError, type HistoryEntry } from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { cn } from "@/lib/utils";
import type { OpenConnection } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import { entryTime, entryTitle } from "./historyEntry";
import { setSidebarOption, useSidebarPrefs } from "./sidebarPrefs";
import { SidebarSearch } from "./SidebarSearch";

const SEARCH_DEBOUNCE_MS = 200;
/** A click waits this long for a second one: a double-click runs instead of only opening. */
export const HISTORY_CLICK_DELAY_MS = 250;
const LIMIT = 200;

function useDebounced<T>(value: T, ms: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const id = setTimeout(() => setDebounced(value), ms);
    return () => clearTimeout(id);
  }, [value, ms]);
  return debounced;
}

function copySql(sql: string) {
  void navigator.clipboard?.writeText(sql).then(
    () => toast.success("Copied SQL"),
    () => toast.error("Could not copy"),
  );
}

/** Opens `sql` in a new query tab of the connection; with `run`, runs it there. */
function openEntry(qc: QueryClient, connection: OpenConnection, sql: string, run: boolean) {
  const { connectionId, driver } = connection.info;
  const tabId = openSqlInNewQueryTab(connectionId, sql, dialectOf(driver));
  useUiStore.getState().setView("workspace");
  if (!run) return;
  void runQuery(tabId, "auto", {
    onHistoryChanged: () => void qc.invalidateQueries({ queryKey: HISTORY_KEY }),
    onSchemaChanged: () => reportHeldTables(invalidateConnectionSchema(qc, connectionId)),
  });
}

/**
 * The sidebar's History pane: the statements the backend recorded, newest first, grouped by day.
 * Lists the active connection's history unless "All connections" is on. A click opens the SQL in a
 * new query tab, a double-click (or ⌘↩) opens and runs it.
 */
export function HistoryPane({ connection }: { connection: OpenConnection }) {
  const qc = useQueryClient();
  const allConnections = useSidebarPrefs((s) => s.options.historyAllConnections);
  const profileId = allConnections ? null : connection.info.profileId;
  const [search, setSearch] = useState("");
  const term = useDebounced(search.trim(), SEARCH_DEBOUNCE_MS);
  const history = useQuery({
    queryKey: [...HISTORY_KEY, "sidebar", profileId, term],
    queryFn: () => ipc.history.list({ profileId, search: term || null, limit: LIMIT }),
    placeholderData: (prev) => prev,
  });
  // The fetch is debounced: filter by what is typed right now so the list follows each keystroke.
  const needle = search.trim().toLowerCase();
  const entries = useMemo(
    () => (history.data ?? []).filter((e) => !needle || e.sql.toLowerCase().includes(needle)),
    [history.data, needle],
  );
  const settled = term === search.trim() && !history.isPlaceholderData;
  const groups = useMemo(() => groupByDay(entries), [entries]);
  const listRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const clickTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (clickTimer.current) clearTimeout(clickTimer.current);
    },
    [],
  );
  // Relative times ("5 min ago") are recomputed every minute.
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 60_000);
    return () => clearInterval(id);
  }, []);

  const remove = useMutation({
    mutationFn: (ids: number[]) => ipc.history.delete(ids),
    onSuccess: () => void qc.invalidateQueries({ queryKey: HISTORY_KEY }),
    onError: (e) =>
      toast.error("Could not delete the entry", { description: toIpcError(e).message }),
  });

  const cancelClick = () => {
    if (clickTimer.current) clearTimeout(clickTimer.current);
    clickTimer.current = null;
  };
  const open = (e: HistoryEntry, run = false) => {
    cancelClick();
    openEntry(qc, connection, e.sql, run);
  };

  const rowsOf = () =>
    Array.from(listRef.current?.querySelectorAll<HTMLElement>("[data-history-row]") ?? []);
  const focusRow = (index: number) => {
    const rows = rowsOf();
    rows[Math.max(0, Math.min(rows.length - 1, index))]?.focus();
  };

  const onRowKeyDown = (ev: KeyboardEvent<HTMLButtonElement>, e: HistoryEntry) => {
    const mod = ev.metaKey || ev.ctrlKey;
    if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
      ev.preventDefault();
      const rows = rowsOf();
      const i = rows.indexOf(ev.currentTarget);
      if (ev.key === "ArrowUp" && i <= 0) searchRef.current?.focus();
      else focusRow(i + (ev.key === "ArrowDown" ? 1 : -1));
    } else if (ev.key === "Enter" && mod) {
      ev.preventDefault();
      open(e, true);
    } else if ((ev.key === "Backspace" && mod) || ev.key === "Delete") {
      ev.preventDefault();
      const rows = rowsOf();
      const i = rows.indexOf(ev.currentTarget);
      remove.mutate([e.id], {
        // Keep the keyboard in the list: the entry that took the deleted one's place.
        onSuccess: () => requestAnimationFrame(() => focusRow(i)),
      });
    }
  };

  let body;
  if (history.isError) {
    body = <EmptyState title="Could not load history" description={history.error.message} />;
  } else if (entries.length === 0 && settled && !history.isPending) {
    body = (
      <EmptyState
        icon={History}
        title={term ? "No matches" : "No history yet"}
        description={
          term
            ? `Nothing you ran contains “${term}”.`
            : allConnections
              ? "Statements you run appear here."
              : `Statements you run on ${connection.info.name} appear here.`
        }
      />
    );
  } else {
    body = groups.map((g) => (
      <div key={g.key} role="group" aria-label={g.label}>
        <div
          aria-hidden
          className="bg-sidebar text-muted-foreground sticky top-0 z-[1] flex h-5.5 items-center px-2.5 text-[11px] font-semibold"
        >
          {g.label}
        </div>
        {g.entries.map((e) => (
          <ContextMenu key={e.id}>
            <ContextMenuTrigger asChild>
              <button
                type="button"
                data-history-row
                title={entryTitle(e)}
                onClick={(ev) => {
                  // From the keyboard (Enter / Space): open at once.
                  if (ev.detail === 0) return open(e);
                  if (ev.detail > 1) return;
                  cancelClick();
                  clickTimer.current = setTimeout(() => open(e), HISTORY_CLICK_DELAY_MS);
                }}
                onDoubleClick={() => open(e, true)}
                onKeyDown={(ev) => onRowKeyDown(ev, e)}
                className="hover:bg-sidebar-accent/60 focus-visible:bg-sidebar-accent flex h-5.5 w-full items-center gap-1.5 pr-2.5 pl-3 text-left outline-none"
              >
                <span
                  role="img"
                  aria-label={e.success ? "Succeeded" : "Failed"}
                  className={cn(
                    "size-1.5 shrink-0 rounded-full",
                    e.success ? "bg-success" : "bg-error",
                  )}
                />
                <span className="min-w-0 flex-1 truncate font-mono text-[11px]">
                  {firstLine(e.sql, 200)}
                </span>
                <span className="text-muted-foreground/80 shrink-0 text-[10px] tabular-nums">
                  {entryTime(e.startedAt, now)}
                </span>
              </button>
            </ContextMenuTrigger>
            <ContextMenuContent>
              <ContextMenuItem onSelect={() => open(e)}>
                <SquareStack /> Open in New Tab
              </ContextMenuItem>
              <ContextMenuItem onSelect={() => open(e, true)}>
                <Play /> Run in New Tab
                <ContextMenuShortcut>{formatCombo("Mod+Enter")}</ContextMenuShortcut>
              </ContextMenuItem>
              <ContextMenuItem onSelect={() => copySql(e.sql)}>
                <Copy /> Copy SQL
              </ContextMenuItem>
              <ContextMenuSeparator />
              <ContextMenuItem variant="destructive" onSelect={() => remove.mutate([e.id])}>
                <Trash2 /> Delete Entry
                <ContextMenuShortcut>{formatCombo("Mod+Backspace")}</ContextMenuShortcut>
              </ContextMenuItem>
            </ContextMenuContent>
          </ContextMenu>
        ))}
      </div>
    ));
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-1 px-2 pb-1.5">
        <SidebarSearch
          ref={searchRef}
          value={search}
          onChange={setSearch}
          onArrowDown={() => focusRow(0)}
          placeholder="Search history…"
          label="Search history"
          kbdHint={false}
        />
        <button
          type="button"
          aria-pressed={allConnections}
          aria-label="All connections"
          title={
            allConnections
              ? "Showing every connection's history"
              : `Showing ${connection.info.name} only — click for all connections`
          }
          onClick={() => setSidebarOption("historyAllConnections", !allConnections)}
          className={cn(
            "focus-visible:ring-ring/50 flex size-6.5 shrink-0 items-center justify-center rounded-md outline-none focus-visible:ring-2",
            allConnections
              ? "bg-sidebar-accent text-sidebar-accent-foreground"
              : "text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground",
          )}
        >
          <Layers className="size-3.5" />
        </button>
      </div>
      <div
        ref={listRef}
        className="relative min-h-0 flex-1 overflow-auto pb-1"
        aria-label="Query history"
        role="region"
      >
        {history.isFetching && !history.isPending && (
          <Loader2
            aria-hidden
            className="text-muted-foreground absolute top-1 right-2 z-[2] size-3 animate-spin"
          />
        )}
        {body}
      </div>
    </div>
  );
}
