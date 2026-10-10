import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { FileCode2, Table2, TableProperties, X } from "lucide-react";
import { accentColor } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import type { Tab } from "@/stores/tabs";

const ICONS = { table: Table2, structure: TableProperties, query: FileCode2 } as const;

export function TabItem({
  tab,
  active,
  dirty,
  onActivate,
  onClose,
}: {
  tab: Tab;
  active: boolean;
  dirty?: boolean;
  onActivate: () => void;
  onClose: () => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: tab.id,
  });
  const color = useConnectionsStore((s) => s.byId[tab.connectionId]?.info.color ?? null);
  const Icon = ICONS[tab.kind];
  const subtitle = tab.kind === "query" ? undefined : (tab.table.schema ?? undefined);
  return (
    <div
      ref={setNodeRef}
      style={{
        transform: CSS.Translate.toString(transform),
        transition,
        borderBottomColor: active ? accentColor(color) : "transparent",
      }}
      {...attributes}
      {...listeners}
      role="tab"
      aria-selected={active}
      tabIndex={-1}
      title={subtitle ? `${subtitle}.${tab.title}` : tab.title}
      onPointerDown={(e) => {
        if (e.button === 1) {
          e.preventDefault();
          onClose();
          return;
        }
        onActivate();
        listeners?.onPointerDown?.(e);
      }}
      className={cn(
        "group border-border relative flex h-full max-w-55 min-w-28 shrink-0 cursor-default items-center gap-1.5 border-r border-b-2 px-2.5 text-xs select-none",
        active
          ? "bg-background text-foreground"
          : "text-muted-foreground hover:bg-surface hover:text-foreground",
        isDragging && "z-10 opacity-80 shadow-md",
      )}
    >
      <Icon className={cn("size-3.5 shrink-0", active ? "text-brand" : "")} />
      <span className="truncate">{tab.title}</span>
      {dirty && (
        <span className="bg-warning size-1.5 shrink-0 rounded-full" title="Unsaved changes" />
      )}
      <button
        type="button"
        aria-label="Close tab"
        onPointerDown={(e) => e.stopPropagation()}
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
        className={cn(
          "text-muted-foreground hover:bg-accent hover:text-foreground ml-auto flex size-4 shrink-0 items-center justify-center rounded opacity-0 group-hover:opacity-100",
          active && "opacity-60",
        )}
      >
        <X className="size-3" />
      </button>
    </div>
  );
}
