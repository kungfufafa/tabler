import { useVirtualizer } from "@tanstack/react-virtual";
import {
  ChevronRight,
  Copy,
  Eraser,
  PencilLine,
  Pin,
  PinOff,
  SquarePlus,
  SquareStack,
  Table2,
  TableProperties,
  Trash2,
} from "lucide-react";
import {
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
  type KeyboardEvent,
  type Ref,
} from "react";
import { toast } from "sonner";
import { EmptyState } from "@/components/EmptyState";
import { TableIcon } from "@/components/TypeIcon";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Skeleton } from "@/components/ui/skeleton";
import { TableDialogs, type TableDialogState } from "@/features/structure/TableDialogs";
import { isAlterable, isViewKind } from "@/features/structure/tableActions";
import { formatCount } from "@/lib/format/time";
import { tableKey, type TableRef, type TableSummary } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { EMPTY_PINS, togglePin } from "@/stores/pins";
import { recordRecent } from "@/stores/recent";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { setSectionCollapsed, toggleSection } from "./sidebarPrefs";
import { useCurrentSchema, type TreeRow } from "./useSchemaTree";

type HeaderRow = Extract<TreeRow, { type: "header" }>;

/** Row height of the tree (headers and tables). */
export const ROW_H = 22;

/**
 * Keyboard focus is a thin inset ring, never a fill: the one filled row is the open table
 * (TablePlus). Inset, so the neighbouring rows (absolutely positioned) do not cover it.
 */
const FOCUS_RING =
  "outline-none ring-inset focus-visible:ring-1 focus-visible:ring-sidebar-ring/70";

export interface SchemaTreeHandle {
  /** Focuses the first table row (scrolling it into view); false when the tree lists none. */
  focusFirstRow: () => boolean;
}

/**
 * Recording an opened table reorders (or grows) the Recent section, which moves rows under the
 * pointer. Mouse opens are recorded once the double-click window has passed, so the second click
 * of a double-click still lands on the row it was aimed at.
 */
export const RECENT_DELAY_MS = 500;

let recentQueue: Array<[scope: string, table: TableRef]> = [];
let recentTimer: ReturnType<typeof setTimeout> | null = null;

function flushRecent(): void {
  if (recentTimer !== null) clearTimeout(recentTimer);
  recentTimer = null;
  const queued = recentQueue;
  recentQueue = [];
  for (const [scope, table] of queued) recordRecent(scope, table);
}

/** Records `table` as recently opened in `scope`, after `RECENT_DELAY_MS` when `later`. */
function rememberOpened(scope: string, table: TableRef, later: boolean): void {
  if (!later) {
    flushRecent();
    recordRecent(scope, table);
    return;
  }
  recentQueue.push([scope, table]);
  if (recentTimer !== null) clearTimeout(recentTimer);
  recentTimer = setTimeout(flushRecent, RECENT_DELAY_MS);
}

/** Index of the next table row after `from` in direction `step` (headers are skipped). */
function nextTableRow(rows: readonly TreeRow[], from: number, step: 1 | -1): number | null {
  for (let i = from + step; i >= 0 && i < rows.length; i += step)
    if (rows[i]!.type === "table") return i;
  return null;
}

function copyText(text: string) {
  void navigator.clipboard?.writeText(text).then(
    () => toast.success("Copied", { description: text }),
    () => toast.error("Could not copy"),
  );
}

export function SchemaTree({
  connectionId,
  rows,
  loading,
  error,
  search,
  pinScope = null,
  pinned = EMPTY_PINS,
  showRowCounts = false,
  ref,
}: {
  connectionId: string;
  rows: TreeRow[];
  loading: boolean;
  error: string | null;
  search: string;
  /**
   * Where "Pin to Top" stores pins and opened tables are recorded as recent (`pinScope`); null
   * hides "Pin to Top" and records nothing.
   */
  pinScope?: string | null;
  /** Pinned `tableKey`s of `pinScope`. */
  pinned?: ReadonlySet<string>;
  /** Show the estimated row count on the right of each table row. */
  showRowCounts?: boolean;
  ref?: Ref<SchemaTreeHandle>;
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const openTable = useTabsStore((s) => s.openTable);
  const activeTab = useTabsStore((s) => s.tabs.find((t) => t.id === s.activeId) ?? null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_H,
    overscan: 12,
    getItemKey: (i) => rows[i]?.key ?? i,
  });
  // DDL actions of the context menu (New table / Rename / Truncate / Drop).
  const [dialog, setDialog] = useState<TableDialogState | null>(null);
  // Set when a context-menu item opened the structure view: the closing menu must then not hand
  // focus back to the sidebar, or the structure (which focuses itself) loses its keyboard
  // shortcuts. The data view leaves focus in the sidebar, so the menu may return it there.
  const openedFromMenu = useRef(false);
  const driver = useConnectionsStore((s) => s.byId[connectionId]?.info.driver ?? "postgres");
  const readOnly = useConnectionsStore((s) => s.byId[connectionId]?.info.readOnly ?? false);
  const currentSchema = useCurrentSchema(connectionId);
  // Row that ↑/↓ moved to while it was outside the rendered window: focused once rendered.
  const pendingFocus = useRef<number | null>(null);
  useEffect(() => {
    const index = pendingFocus.current;
    if (index === null) return;
    const el = parentRef.current?.querySelector<HTMLElement>(`[data-row-index="${index}"]`);
    if (!el) return;
    pendingFocus.current = null;
    el.focus({ preventScroll: true });
  });
  const focusRow = (index: number) => {
    virtualizer.scrollToIndex(index, { align: "auto" });
    const el = parentRef.current?.querySelector<HTMLElement>(`[data-row-index="${index}"]`);
    if (el) el.focus({ preventScroll: true });
    else pendingFocus.current = index;
  };
  useImperativeHandle(ref, () => ({
    focusFirstRow: () => {
      const first = nextTableRow(rows, -1, 1);
      if (first === null) return false;
      focusRow(first);
      return true;
    },
  }));
  const dialogs = (
    <TableDialogs
      connectionId={connectionId}
      dialect={driver}
      state={dialog}
      onClose={() => setDialog(null)}
    />
  );

  if (loading) {
    return (
      <div className="flex flex-col gap-1.5 px-3 py-2">
        {[0, 1, 2, 3, 4, 5].map((i) => (
          <Skeleton key={i} className="h-4 w-[70%]" />
        ))}
      </div>
    );
  }
  if (error) return <EmptyState title="Could not load tables" description={error} />;
  if (rows.length === 0) {
    return (
      <>
        <ContextMenu>
          <ContextMenuTrigger asChild>
            <div className="min-h-0 flex-1">
              <EmptyState
                icon={Table2}
                title={search ? "No tables match" : "No tables"}
                description={
                  search ? `Nothing matches “${search}”.` : "This schema has no tables or views."
                }
              />
            </div>
          </ContextMenuTrigger>
          <ContextMenuContent>
            <ContextMenuItem
              disabled={readOnly}
              onSelect={() => setDialog({ kind: "create", schema: currentSchema })}
            >
              <SquarePlus /> New table…
            </ContextMenuItem>
          </ContextMenuContent>
        </ContextMenu>
        {dialogs}
      </>
    );
  }

  const toRef = (t: TableSummary): TableRef => ({ schema: t.schema, name: t.name });
  /** The connection manager may be showing: an opened table must be seen. */
  const showWorkspace = () => useUiStore.getState().setView("workspace");
  /** Single click, ↑/↓, "Open": shows the table in the connection's (replaceable) preview tab. */
  const preview = (t: TableSummary) => {
    openTable(connectionId, toRef(t), "table", { preview: true });
    showWorkspace();
  };
  /** Double-click, middle-click, Enter, "Open in New Tab", "Structure": a regular tab. */
  const openPinned = (t: TableSummary, kind: "table" | "structure" = "table") => {
    openTable(connectionId, toRef(t), kind);
    showWorkspace();
  };
  const remember = (t: TableSummary, later = false) => {
    if (pinScope !== null) rememberOpened(pinScope, toRef(t), later);
  };

  /** ↑/↓ from a row or a header: focus and preview the next table (headers are skipped). */
  const onArrow = (e: KeyboardEvent, index: number) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const next = nextTableRow(rows, index, e.key === "ArrowDown" ? 1 : -1);
    const row = next === null ? undefined : rows[next];
    if (next === null || row?.type !== "table") return;
    // Focus stays in the sidebar (the grid does not take it), so ↑/↓ keep browsing. Browsing is
    // not recorded as recent: that would reorder the Recent section while it is being browsed.
    focusRow(next);
    preview(row.table);
  };

  const onRowKeyDown = (e: KeyboardEvent, index: number, t: TableSummary) => {
    if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
    if (e.key === "Enter") {
      // Instead of the button's click (a preview open): Enter keeps the tab.
      e.preventDefault();
      openPinned(t);
      remember(t);
      return;
    }
    onArrow(e, index);
  };

  /** ←/→ on a section header collapse / expand it; ↑/↓ move to the tables around it. */
  const onHeaderKeyDown = (e: KeyboardEvent, index: number, row: HeaderRow) => {
    if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      e.preventDefault();
      setSectionCollapsed(row.label, e.key === "ArrowLeft");
      return;
    }
    onArrow(e, index);
  };

  return (
    <div ref={parentRef} className="min-h-0 flex-1 overflow-auto" data-scope="sidebar">
      {dialogs}
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualizer.getVirtualItems().map((v) => {
          const row = rows[v.index]!;
          const style = {
            position: "absolute" as const,
            top: 0,
            left: 0,
            width: "100%",
            height: ROW_H,
            transform: `translateY(${v.start}px)`,
          };
          if (row.type === "header") {
            return (
              <button
                key={row.key}
                type="button"
                style={style}
                data-row-index={v.index}
                aria-expanded={!row.collapsed}
                onClick={() => toggleSection(row.label)}
                onKeyDown={(e) => onHeaderKeyDown(e, v.index, row)}
                className={cn(
                  "text-muted-foreground hover:text-foreground flex items-center gap-1 pr-3 pl-1.5 text-left text-[11px] font-semibold",
                  FOCUS_RING,
                )}
              >
                <ChevronRight
                  aria-hidden
                  className={cn(
                    "size-3 shrink-0 transition-transform duration-100",
                    !row.collapsed && "rotate-90",
                  )}
                />
                <span className="truncate">{row.label}</span>
                <span className="text-muted-foreground/70 ml-auto pl-2 font-normal tabular-nums">
                  {row.count}
                </span>
              </button>
            );
          }
          const t = row.table;
          const isPinned = pinned.has(tableKey(t));
          const isActive =
            activeTab !== null &&
            activeTab.kind !== "query" &&
            activeTab.connectionId === connectionId &&
            activeTab.table.name === t.name &&
            (activeTab.table.schema ?? "") === (t.schema ?? "");
          return (
            <ContextMenu key={row.key}>
              <ContextMenuTrigger asChild>
                <button
                  type="button"
                  style={style}
                  data-row-index={v.index}
                  onClick={(e) => {
                    preview(t);
                    // detail 0: activated from the keyboard (Space), nothing moves under a pointer.
                    remember(t, e.detail > 0);
                  }}
                  onDoubleClick={() => {
                    openPinned(t);
                    remember(t, true);
                  }}
                  onMouseDown={(e) => {
                    // Middle-click must not start autoscroll.
                    if (e.button === 1) e.preventDefault();
                  }}
                  onAuxClick={(e) => {
                    if (e.button !== 1) return;
                    e.preventDefault();
                    openPinned(t);
                    remember(t, true);
                  }}
                  onKeyDown={(e) => onRowKeyDown(e, v.index, t)}
                  title={t.comment ?? undefined}
                  data-active={isActive || undefined}
                  className={cn(
                    "flex items-center gap-1.5 pr-3 pl-5 text-left text-xs",
                    FOCUS_RING,
                    // The right-clicked row is outlined like a focused one while its menu is open.
                    "data-[state=open]:ring-sidebar-ring/70 data-[state=open]:ring-1",
                    isActive
                      ? "bg-sidebar-accent text-sidebar-accent-foreground font-medium"
                      : "hover:bg-black/[0.035] dark:hover:bg-white/[0.05]",
                  )}
                >
                  <TableIcon kind={t.kind} />
                  <span className="truncate">{t.name}</span>
                  {isPinned && (
                    <Pin
                      aria-label="Pinned"
                      className="text-muted-foreground/60 size-3 shrink-0 -rotate-45"
                    />
                  )}
                  {showRowCounts && t.rowEstimate !== null && (
                    <span className="text-muted-foreground/70 ml-auto pl-2 font-mono text-[10px] tabular-nums">
                      {formatCount(t.rowEstimate, t.rowEstimate > 1000)}
                    </span>
                  )}
                </button>
              </ContextMenuTrigger>
              <ContextMenuContent
                onCloseAutoFocus={(e) => {
                  if (!openedFromMenu.current) return;
                  openedFromMenu.current = false;
                  e.preventDefault();
                }}
              >
                <ContextMenuItem
                  onSelect={() => {
                    preview(t);
                    remember(t);
                  }}
                >
                  <Table2 /> Open
                </ContextMenuItem>
                <ContextMenuItem
                  onSelect={() => {
                    openPinned(t);
                    remember(t);
                  }}
                >
                  <SquareStack /> Open in New Tab
                </ContextMenuItem>
                <ContextMenuItem
                  onSelect={() => {
                    openedFromMenu.current = true;
                    openPinned(t, "structure");
                    remember(t);
                  }}
                >
                  <TableProperties /> Structure
                </ContextMenuItem>
                {pinScope !== null && (
                  <ContextMenuItem onSelect={() => togglePin(pinScope, t)}>
                    {isPinned ? (
                      <>
                        <PinOff /> Unpin
                      </>
                    ) : (
                      <>
                        <Pin /> Pin to Top
                      </>
                    )}
                  </ContextMenuItem>
                )}
                <ContextMenuSeparator />
                <ContextMenuItem
                  disabled={readOnly}
                  onSelect={() => setDialog({ kind: "create", schema: t.schema })}
                >
                  <SquarePlus /> New table…
                </ContextMenuItem>
                {isAlterable(t.kind) && (
                  <>
                    <ContextMenuItem
                      disabled={readOnly}
                      onSelect={() => setDialog({ kind: "rename", table: t })}
                    >
                      <PencilLine /> Rename…
                    </ContextMenuItem>
                    <ContextMenuItem
                      disabled={readOnly}
                      onSelect={() => setDialog({ kind: "truncate", table: t })}
                    >
                      <Eraser /> Truncate…
                    </ContextMenuItem>
                  </>
                )}
                {(isAlterable(t.kind) || isViewKind(t.kind)) && (
                  <ContextMenuItem
                    variant="destructive"
                    disabled={readOnly}
                    onSelect={() => setDialog({ kind: "drop", table: t })}
                  >
                    <Trash2 /> {isViewKind(t.kind) ? "Drop view…" : "Drop…"}
                  </ContextMenuItem>
                )}
                <ContextMenuSeparator />
                <ContextMenuItem onSelect={() => copyText(t.name)}>
                  <Copy /> Copy name
                </ContextMenuItem>
              </ContextMenuContent>
            </ContextMenu>
          );
        })}
      </div>
    </div>
  );
}
