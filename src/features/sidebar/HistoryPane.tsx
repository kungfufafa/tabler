import { useMutation, useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import {
  BookmarkPlus,
  CalendarX,
  Copy,
  FolderSearch,
  History,
  Layers,
  ListFilter,
  Loader2,
  Play,
  SquareStack,
  TextCursorInput,
  Trash2,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent } from "react";
import { toast } from "sonner";
import { EmptyState } from "@/components/EmptyState";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { dialectOf, insertSql, runQuery } from "@/features/editor/controller";
import { HISTORY_RETENTION_LABELS } from "@/features/editor/editorSettings";
import { dayKey, groupByDay, HISTORY_KEY } from "@/features/editor/historyGroups";
import { firstLine } from "@/features/editor/labels";
import { openSqlInNewQueryTab } from "@/features/editor/openSql";
import { openAddToQueries } from "@/features/queries/dialogStore";
import { invalidateConnectionSchema, reportHeldTables } from "@/features/structure/invalidate";
import { ipc, toIpcError, type HistoryEntry } from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { cn } from "@/lib/utils";
import type { OpenConnection } from "@/stores/connections";
import { HISTORY_RETENTIONS, setPref, usePrefs, type HistoryRetention } from "@/stores/prefs";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import {
  entriesSql,
  entryTime,
  entryTitle,
  HISTORY_SEARCH_OPTIONS_KEY,
  historyMatcher,
  isMcpEntry,
  readHistorySearchOptions,
  writeHistorySearchOptions,
  type HistorySearchOptions,
} from "./historyEntry";
import { McpBadge } from "./McpBadge";
import { setSidebarOption, useSidebarPrefs } from "./sidebarPrefs";
import { SidebarSearch } from "./SidebarSearch";

const SEARCH_DEBOUNCE_MS = 200;
const LIMIT = 200;
export { HISTORY_SEARCH_OPTIONS_KEY };

function useDebounced<T>(value: T, ms: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const id = setTimeout(() => setDebounced(value), ms);
    return () => clearTimeout(id);
  }, [value, ms]);
  return debounced;
}

function copySql(sql: string, count = 1) {
  void navigator.clipboard?.writeText(sql).then(
    () => toast.success(count > 1 ? `Copied ${count} statements` : "Copied SQL"),
    () => toast.error("Could not copy"),
  );
}

function hooks(qc: QueryClient, connectionId: string) {
  return {
    onHistoryChanged: () => void qc.invalidateQueries({ queryKey: HISTORY_KEY }),
    onSchemaChanged: () => reportHeldTables(invalidateConnectionSchema(qc, connectionId)),
  };
}

/** Opens `sql` in a new query tab of the connection; with `run`, runs it there. */
function openEntry(qc: QueryClient, connection: OpenConnection, sql: string, run: boolean) {
  const { connectionId, driver } = connection.info;
  const tabId = openSqlInNewQueryTab(connectionId, sql, dialectOf(driver));
  useUiStore.getState().setView("workspace");
  if (!run) return;
  void runQuery(tabId, "auto", hooks(qc, connectionId));
}

/**
 * "Insert to SQL Editor": at the cursor of the connection's active query tab, or into a new query
 * tab when the active tab is something else.
 */
function insertEntry(qc: QueryClient, connection: OpenConnection, sql: string) {
  const tabs = useTabsStore.getState();
  const active = tabs.tabs.find((t) => t.id === tabs.activeId);
  if (active?.kind === "query" && active.connectionId === connection.info.connectionId) {
    useUiStore.getState().setView("workspace");
    void insertSql(active.id, sql);
    return;
  }
  openEntry(qc, connection, sql, false);
}

function revealHistoryStore() {
  void ipc.locations
    .reveal("data")
    .catch((e: unknown) =>
      toast.error("Could not show the history file", { description: toIpcError(e).message }),
    );
}

/** `YYYY-MM-DD` one month before today (the Clear History Older Than… default). */
function monthAgo(): string {
  const d = new Date();
  d.setMonth(d.getMonth() - 1);
  return dayKey(d);
}

/**
 * The sidebar's History pane: the statements the backend recorded, newest first, grouped by day.
 * Lists the active connection's history unless "All connections" is on. A click only selects the
 * entry (⌘-click / ⇧-click select several, ⌘C copies them all); a double-click (or Enter) opens
 * its SQL in a new query tab and NEVER runs it: running is the context menu's Run in New Tab and
 * ⌘↩. The filter menu has Match Case / Whole Word / Regex, Keep History For, Clear History Older
 * Than…, Clear All History… and Show in Finder.
 */
export function HistoryPane({ connection }: { connection: OpenConnection }) {
  const qc = useQueryClient();
  const allConnections = useSidebarPrefs((s) => s.options.historyAllConnections);
  const retention = usePrefs((p) => p.historyRetention);
  const profileId = allConnections ? null : connection.info.profileId;
  const [search, setSearch] = useState("");
  const [options, setOptionsState] = useState(readHistorySearchOptions);
  const term = useDebounced(search.trim(), SEARCH_DEBOUNCE_MS);
  const typedMatcher = historyMatcher(search.trim(), options);
  const invalidRegex = typedMatcher === "invalid";
  const history = useQuery({
    queryKey: [...HISTORY_KEY, "sidebar", profileId, term, options],
    queryFn: () => ipc.history.list({ profileId, search: term || null, limit: LIMIT, ...options }),
    placeholderData: (prev) => prev,
    enabled: historyMatcher(term, options) !== "invalid",
  });
  // The fetch is debounced: filter by what is typed right now so the list follows each keystroke.
  const entries = useMemo(
    () =>
      typeof typedMatcher === "function"
        ? (history.data ?? []).filter((e) => typedMatcher(e.sql))
        : invalidRegex
          ? []
          : (history.data ?? []),
    // The matcher is rebuilt each render; what it depends on is listed instead.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [history.data, search, options],
  );
  const settled = term === search.trim() && !history.isPlaceholderData;
  const groups = useMemo(() => groupByDay(entries), [entries]);
  const [selected, setSelected] = useState<ReadonlySet<number>>(() => new Set());
  const anchor = useRef<number | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  const [olderThan, setOlderThan] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  // Relative times ("5 min ago") are recomputed every minute.
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 60_000);
    return () => clearInterval(id);
  }, []);
  // Entries that left the list leave the selection.
  useEffect(() => {
    setSelected((prev) => {
      if (prev.size === 0) return prev;
      const ids = new Set(entries.map((e) => e.id));
      const next = new Set([...prev].filter((id) => ids.has(id)));
      return next.size === prev.size ? prev : next;
    });
  }, [entries]);

  const setOptions = (patch: Partial<HistorySearchOptions>) =>
    setOptionsState((o) => {
      const next = { ...o, ...patch };
      writeHistorySearchOptions(next);
      return next;
    });

  const remove = useMutation({
    mutationFn: (ids: number[]) => ipc.history.delete(ids),
    onSuccess: () => void qc.invalidateQueries({ queryKey: HISTORY_KEY }),
    onError: (e) =>
      toast.error("Could not delete the entry", { description: toIpcError(e).message }),
  });

  const scope = allConnections ? "every connection" : connection.info.name;
  const clearAll = () => {
    setConfirmClear(false);
    void ipc.history.clear(profileId).then(
      () => {
        setSelected(new Set());
        void qc.invalidateQueries({ queryKey: HISTORY_KEY });
        toast.success("History cleared");
      },
      (e: unknown) =>
        toast.error("Could not clear history", { description: toIpcError(e).message }),
    );
  };
  const clearOlder = () => {
    const day = olderThan;
    setOlderThan(null);
    if (!day) return;
    const cutoff = new Date(`${day}T00:00:00`);
    if (Number.isNaN(cutoff.getTime())) return;
    void ipc.history.clearBefore(cutoff.toISOString(), profileId).then(
      (n) => {
        void qc.invalidateQueries({ queryKey: HISTORY_KEY });
        toast.success(
          n === 0
            ? "Nothing that old"
            : `Deleted ${n.toLocaleString()} ${n === 1 ? "entry" : "entries"}`,
        );
      },
      (e: unknown) =>
        toast.error("Could not clear history", { description: toIpcError(e).message }),
    );
  };

  const open = (e: HistoryEntry, run = false) => openEntry(qc, connection, e.sql, run);

  /** The entries an action on `e` applies to: the selection when `e` is in it, else `e`. */
  const targets = (e: HistoryEntry): HistoryEntry[] =>
    selected.has(e.id) && selected.size > 1 ? entries.filter((x) => selected.has(x.id)) : [e];

  const copyEntries = (e: HistoryEntry) => {
    const list = targets(e);
    copySql(list.length > 1 ? entriesSql(list) : e.sql, list.length);
  };

  const selectRange = (to: HistoryEntry) => {
    const ids = entries.map((x) => x.id);
    const from = anchor.current !== null ? ids.indexOf(anchor.current) : -1;
    const end = ids.indexOf(to.id);
    if (from < 0 || end < 0) {
      anchor.current = to.id;
      setSelected(new Set([to.id]));
      return;
    }
    const [a, b] = from < end ? [from, end] : [end, from];
    setSelected(new Set(ids.slice(a, b + 1)));
  };

  const onRowClick = (ev: MouseEvent<HTMLButtonElement>, e: HistoryEntry) => {
    if (ev.metaKey || ev.ctrlKey) {
      anchor.current = e.id;
      setSelected((prev) => {
        const next = new Set(prev);
        if (next.has(e.id)) next.delete(e.id);
        else next.add(e.id);
        return next;
      });
      return;
    }
    if (ev.shiftKey) {
      selectRange(e);
      return;
    }
    anchor.current = e.id;
    setSelected(new Set([e.id]));
    // A click only selects. From the keyboard (Enter / Space) it is the double-click: open.
    if (ev.detail === 0) open(e);
  };

  const rowsOf = () =>
    Array.from(listRef.current?.querySelectorAll<HTMLElement>("[data-history-row]") ?? []);
  const focusRow = (index: number) => {
    const rows = rowsOf();
    rows[Math.max(0, Math.min(rows.length - 1, index))]?.focus();
  };

  const onRowKeyDown = (ev: KeyboardEvent<HTMLButtonElement>, e: HistoryEntry) => {
    const mod = ev.metaKey || ev.ctrlKey;
    const key = ev.key.toLowerCase();
    if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
      ev.preventDefault();
      const rows = rowsOf();
      const i = rows.indexOf(ev.currentTarget);
      if (ev.key === "ArrowUp" && i <= 0) searchRef.current?.focus();
      else focusRow(i + (ev.key === "ArrowDown" ? 1 : -1));
    } else if (ev.key === "Enter" && mod) {
      ev.preventDefault();
      open(e, true);
    } else if (mod && key === "c") {
      ev.preventDefault();
      ev.stopPropagation();
      copyEntries(e);
    } else if (mod && key === "a") {
      ev.preventDefault();
      ev.stopPropagation();
      setSelected(new Set(entries.map((x) => x.id)));
    } else if (ev.key === "Escape" && selected.size > 0) {
      ev.preventDefault();
      ev.stopPropagation();
      setSelected(new Set());
    } else if ((ev.key === "Backspace" && mod) || ev.key === "Delete") {
      ev.preventDefault();
      const rows = rowsOf();
      const i = rows.indexOf(ev.currentTarget);
      remove.mutate(
        targets(e).map((x) => x.id),
        // Keep the keyboard in the list: the entry that took the deleted one's place.
        { onSuccess: () => requestAnimationFrame(() => focusRow(i)) },
      );
    }
  };

  /** Clear History Older Than…, Clear All History…, Show in Finder (both menus). */
  const clearActions = [
    {
      label: "Clear History Older Than…",
      icon: <CalendarX />,
      run: () => setOlderThan(monthAgo()),
      variant: "default" as const,
    },
    {
      label: "Clear All History…",
      icon: <Trash2 />,
      run: () => setConfirmClear(true),
      variant: "destructive" as const,
    },
    {
      label: "Show in Finder",
      icon: <FolderSearch />,
      run: revealHistoryStore,
      variant: "default" as const,
    },
  ];

  let body;
  if (invalidRegex) {
    body = (
      <EmptyState
        title="Invalid regular expression"
        description="Fix the pattern, or turn Regex off in the filter menu."
      />
    );
  } else if (history.isError) {
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
        {g.entries.map((e) => {
          const isSelected = selected.has(e.id);
          const many = isSelected && selected.size > 1 ? selected.size : 1;
          return (
            <ContextMenu
              key={e.id}
              onOpenChange={(o) => {
                // Right-clicking outside the selection selects that entry (as Finder does).
                if (o && !selected.has(e.id)) {
                  anchor.current = e.id;
                  setSelected(new Set([e.id]));
                }
              }}
            >
              <ContextMenuTrigger asChild>
                <button
                  type="button"
                  data-history-row
                  aria-selected={isSelected}
                  title={entryTitle(e)}
                  onClick={(ev) => onRowClick(ev, e)}
                  onDoubleClick={(ev) => {
                    // Opens a new SQL tab with the statement; it is never run from here.
                    if (!ev.metaKey && !ev.ctrlKey && !ev.shiftKey) open(e);
                  }}
                  onKeyDown={(ev) => onRowKeyDown(ev, e)}
                  className={cn(
                    "hover:bg-sidebar-accent/60 focus-visible:bg-sidebar-accent flex h-5.5 w-full items-center gap-1.5 pr-2.5 pl-3 text-left outline-none",
                    isSelected && selected.size > 1 && "bg-brand/15 hover:bg-brand/20",
                  )}
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
                  {isMcpEntry(e) && <McpBadge />}
                  <span className="text-muted-foreground/80 shrink-0 text-[10px] tabular-nums">
                    {entryTime(e.startedAt, now)}
                  </span>
                </button>
              </ContextMenuTrigger>
              <ContextMenuContent className="w-56">
                <ContextMenuItem onSelect={() => copyEntries(e)}>
                  <Copy /> {many > 1 ? `Copy ${many} Statements` : "Copy SQL"}
                  <ContextMenuShortcut>{formatCombo("Mod+C")}</ContextMenuShortcut>
                </ContextMenuItem>
                <ContextMenuItem onSelect={() => open(e, true)}>
                  <Play /> Run in New Tab
                  <ContextMenuShortcut>{formatCombo("Mod+Enter")}</ContextMenuShortcut>
                </ContextMenuItem>
                <ContextMenuItem onSelect={() => open(e)}>
                  <SquareStack /> Open in New Tab
                </ContextMenuItem>
                <ContextMenuItem onSelect={() => insertEntry(qc, connection, e.sql)}>
                  <TextCursorInput /> Insert to SQL Editor
                </ContextMenuItem>
                <ContextMenuItem
                  onSelect={() =>
                    openAddToQueries({
                      sql: e.sql,
                      connection: {
                        profileId: connection.info.profileId,
                        name: connection.info.name,
                      },
                    })
                  }
                >
                  <BookmarkPlus /> Add to Queries…
                </ContextMenuItem>
                <ContextMenuSeparator />
                <ContextMenuItem
                  variant="destructive"
                  onSelect={() => remove.mutate(targets(e).map((x) => x.id))}
                >
                  <Trash2 /> {many > 1 ? `Delete ${many} Entries` : "Delete Entry"}
                  <ContextMenuShortcut>{formatCombo("Mod+Backspace")}</ContextMenuShortcut>
                </ContextMenuItem>
                <ContextMenuSeparator />
                {clearActions.map((a) => (
                  <ContextMenuItem key={a.label} variant={a.variant} onSelect={a.run}>
                    {a.icon} {a.label}
                  </ContextMenuItem>
                ))}
              </ContextMenuContent>
            </ContextMenu>
          );
        })}
      </div>
    ));
  }

  const filtering = options.matchCase || options.wholeWord || options.regex;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-1 px-2 pb-1.5">
        <SidebarSearch
          ref={searchRef}
          value={search}
          onChange={setSearch}
          onArrowDown={() => focusRow(0)}
          placeholder={options.regex ? "Search history (regex)…" : "Search history…"}
          label="Search history"
          kbdHint={false}
        />
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label="History options"
              title={
                filtering
                  ? [
                      options.matchCase && "Match Case",
                      options.wholeWord && "Whole Word",
                      options.regex && "Regex",
                    ]
                      .filter(Boolean)
                      .join(", ")
                  : "Search options, retention, clear"
              }
              className={cn(
                "focus-visible:ring-ring/50 data-[state=open]:bg-sidebar-accent flex size-6.5 shrink-0 items-center justify-center rounded-md outline-none focus-visible:ring-2",
                filtering
                  ? "bg-sidebar-accent text-sidebar-accent-foreground"
                  : "text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground",
              )}
            >
              <ListFilter className="size-3.5" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-60 text-xs">
            <DropdownMenuLabel className="text-muted-foreground text-[11px] font-normal">
              Search
            </DropdownMenuLabel>
            <DropdownMenuCheckboxItem
              checked={options.matchCase}
              onCheckedChange={(v) => setOptions({ matchCase: v === true })}
              onSelect={(ev) => ev.preventDefault()}
            >
              Match Case
            </DropdownMenuCheckboxItem>
            <DropdownMenuCheckboxItem
              checked={options.wholeWord}
              onCheckedChange={(v) => setOptions({ wholeWord: v === true })}
              onSelect={(ev) => ev.preventDefault()}
            >
              Whole Word
            </DropdownMenuCheckboxItem>
            <DropdownMenuCheckboxItem
              checked={options.regex}
              onCheckedChange={(v) => setOptions({ regex: v === true })}
              onSelect={(ev) => ev.preventDefault()}
            >
              Regex
            </DropdownMenuCheckboxItem>
            <DropdownMenuSeparator />
            <DropdownMenuSub>
              <DropdownMenuSubTrigger>Keep History For</DropdownMenuSubTrigger>
              <DropdownMenuSubContent className="w-36">
                <DropdownMenuRadioGroup
                  value={retention}
                  onValueChange={(v) => setPref("historyRetention", v as HistoryRetention)}
                >
                  {HISTORY_RETENTIONS.map((r) => (
                    <DropdownMenuRadioItem key={r} value={r}>
                      {HISTORY_RETENTION_LABELS[r]}
                    </DropdownMenuRadioItem>
                  ))}
                </DropdownMenuRadioGroup>
              </DropdownMenuSubContent>
            </DropdownMenuSub>
            {clearActions.map((a) => (
              <DropdownMenuItem key={a.label} variant={a.variant} onSelect={a.run}>
                {a.icon} {a.label}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
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
        aria-multiselectable
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
      <AlertDialog open={confirmClear} onOpenChange={setConfirmClear}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle className="text-sm">Clear all history?</AlertDialogTitle>
            <AlertDialogDescription className="text-xs">
              {`Every statement recorded for ${scope} is deleted. This cannot be undone.`}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel size="sm">Cancel</AlertDialogCancel>
            <AlertDialogAction size="sm" variant="destructive" onClick={clearAll}>
              Clear History
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <AlertDialog open={olderThan !== null} onOpenChange={(o) => !o && setOlderThan(null)}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle className="text-sm">Clear history older than…</AlertDialogTitle>
            <AlertDialogDescription className="text-xs">
              {`Statements of ${scope} run before this day are deleted. This cannot be undone.`}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <input
            type="date"
            aria-label="Clear history before"
            value={olderThan ?? ""}
            max={dayKey(new Date())}
            onChange={(ev) => setOlderThan(ev.target.value)}
            className="border-input bg-background focus-visible:ring-ring/50 h-7 rounded-md border px-2 text-xs outline-none focus-visible:ring-2"
          />
          <AlertDialogFooter>
            <AlertDialogCancel size="sm">Cancel</AlertDialogCancel>
            <AlertDialogAction
              size="sm"
              variant="destructive"
              disabled={!olderThan}
              onClick={clearOlder}
            >
              Clear
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
