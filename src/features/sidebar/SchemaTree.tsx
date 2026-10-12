import { useVirtualizer } from "@tanstack/react-virtual";
import {
  Copy,
  Eraser,
  PencilLine,
  Pin,
  PinOff,
  SquarePlus,
  Table2,
  TableProperties,
  Trash2,
} from "lucide-react";
import { useRef, useState } from "react";
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
import { tableKey, type TableSummary } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { EMPTY_PINS, togglePin } from "@/stores/pins";
import { useTabsStore } from "@/stores/tabs";
import { useCurrentSchema, type TreeRow } from "./useSchemaTree";

const ROW_H = 24;

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
}: {
  connectionId: string;
  rows: TreeRow[];
  loading: boolean;
  error: string | null;
  search: string;
  /** Where "Pin to Top" stores pins (`pinScope`); null hides it. */
  pinScope?: string | null;
  /** Pinned `tableKey`s of `pinScope`. */
  pinned?: ReadonlySet<string>;
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
  // Set when a context-menu item opened a tab: the closing menu must then not hand focus back to
  // the sidebar, or the new tab (which focuses itself) loses its keyboard shortcuts.
  const openedFromMenu = useRef(false);
  const driver = useConnectionsStore((s) => s.byId[connectionId]?.info.driver ?? "postgres");
  const readOnly = useConnectionsStore((s) => s.byId[connectionId]?.info.readOnly ?? false);
  const currentSchema = useCurrentSchema(connectionId);
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

  const open = (table: TableSummary, kind: "table" | "structure" = "table") => {
    openTable(connectionId, { schema: table.schema, name: table.name }, kind);
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
              <div
                key={row.key}
                style={style}
                className="text-muted-foreground flex items-center px-3 text-[10px] font-semibold tracking-wide uppercase"
              >
                {row.label}
                <span className="ml-1 font-normal tracking-normal normal-case">({row.count})</span>
              </div>
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
                  onClick={() => open(t)}
                  title={t.comment ?? undefined}
                  className={cn(
                    "hover:bg-sidebar-accent/60 focus-visible:bg-sidebar-accent/60 flex items-center gap-1.5 px-3 text-left text-xs outline-none",
                    isActive && "bg-sidebar-accent font-medium",
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
                  {t.rowEstimate !== null && (
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
                    openedFromMenu.current = true;
                    open(t);
                  }}
                >
                  <Table2 /> Open data
                </ContextMenuItem>
                <ContextMenuItem
                  onSelect={() => {
                    openedFromMenu.current = true;
                    open(t, "structure");
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
