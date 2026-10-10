import { useVirtualizer } from "@tanstack/react-virtual";
import { Copy, Table2, TableProperties } from "lucide-react";
import { useRef } from "react";
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
import { formatCount } from "@/lib/format/time";
import type { TableSummary } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useTabsStore } from "@/stores/tabs";
import type { TreeRow } from "./useSchemaTree";

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
}: {
  connectionId: string;
  rows: TreeRow[];
  loading: boolean;
  error: string | null;
  search: string;
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
      <EmptyState
        icon={Table2}
        title={search ? "No tables match" : "No tables"}
        description={
          search ? `Nothing matches “${search}”.` : "This schema has no tables or views."
        }
      />
    );
  }

  const open = (table: TableSummary, kind: "table" | "structure" = "table") => {
    openTable(connectionId, { schema: table.schema, name: table.name }, kind);
  };

  return (
    <div ref={parentRef} className="min-h-0 flex-1 overflow-auto" data-scope="sidebar">
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
                  {t.rowEstimate !== null && (
                    <span className="text-muted-foreground/70 ml-auto pl-2 font-mono text-[10px] tabular-nums">
                      {formatCount(t.rowEstimate, t.rowEstimate > 1000)}
                    </span>
                  )}
                </button>
              </ContextMenuTrigger>
              <ContextMenuContent>
                <ContextMenuItem onSelect={() => open(t)}>
                  <Table2 /> Open data
                </ContextMenuItem>
                <ContextMenuItem onSelect={() => open(t, "structure")}>
                  <TableProperties /> Structure
                </ContextMenuItem>
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
