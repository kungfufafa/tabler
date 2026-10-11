import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { FileCode2, Table2, TableProperties, X } from "lucide-react";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { pendingCount } from "@/features/staging/model";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import type { ActionId } from "@/features/palette/actions";
import { accentColor } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { useTableTabsStore } from "@/stores/tableTabs";

const ICONS = { table: Table2, structure: TableProperties, query: FileCode2 } as const;

function Shortcut({ id }: { id: ActionId }) {
  const keys = shortcutFor(id);
  return keys ? (
    <ContextMenuShortcut className="tracking-normal">{formatCombo(keys)}</ContextMenuShortcut>
  ) : null;
}

/** Staged row edits (table tabs) or 1 for a dirty query tab. */
function useUnsaved(tab: Tab): number {
  const pending = useTableTabsStore((s) => {
    if (tab.kind !== "table") return 0;
    const slice = s.slices[tab.id];
    return slice ? pendingCount(slice.staging) : 0;
  });
  return tab.kind === "query" ? (tab.dirty ? 1 : 0) : pending;
}

function TabMenu({ tab }: { tab: Tab }) {
  const count = useTabsStore((s) => s.tabs.length);
  const isLast = useTabsStore((s) => s.tabs.at(-1)?.id === tab.id);
  const canReopen = useTabsStore((s) => s.closed.length > 0);
  const store = useTabsStore.getState;
  const item = "py-0.5 text-xs";
  return (
    <ContextMenuContent className="min-w-48">
      <ContextMenuItem className={item} onSelect={() => store().close(tab.id)}>
        Close
        <Shortcut id="tab.close" />
      </ContextMenuItem>
      <ContextMenuItem
        className={item}
        disabled={count < 2}
        onSelect={() => store().closeOthers(tab.id)}
      >
        Close Others
      </ContextMenuItem>
      <ContextMenuItem
        className={item}
        disabled={isLast}
        onSelect={() => store().closeToRight(tab.id)}
      >
        Close Tabs to the Right
      </ContextMenuItem>
      <ContextMenuItem className={item} onSelect={() => store().closeAll()}>
        Close All
        <Shortcut id="tab.close-all" />
      </ContextMenuItem>
      <ContextMenuSeparator />
      {tab.kind === "query" && (
        <ContextMenuItem className={item} onSelect={() => store().duplicate(tab.id)}>
          Duplicate
        </ContextMenuItem>
      )}
      <ContextMenuItem
        className={item}
        disabled={!canReopen}
        onSelect={() => store().reopenClosed()}
      >
        Reopen Closed Tab
        <Shortcut id="tab.reopen" />
      </ContextMenuItem>
    </ContextMenuContent>
  );
}

export function TabItem({
  tab,
  active,
  onActivate,
  onClose,
}: {
  tab: Tab;
  active: boolean;
  onActivate: () => void;
  onClose: () => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: tab.id,
  });
  const color = useConnectionsStore((s) => s.byId[tab.connectionId]?.info.color ?? null);
  const unsaved = useUnsaved(tab);
  const Icon = ICONS[tab.kind];
  const subtitle = tab.kind === "query" ? undefined : (tab.table.schema ?? undefined);
  const name = subtitle ? `${subtitle}.${tab.title}` : tab.title;
  const unsavedLabel =
    tab.kind === "table"
      ? `${unsaved} pending change${unsaved === 1 ? "" : "s"}`
      : "Unsaved changes";
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
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
          aria-label={unsaved > 0 ? `${name} (${unsavedLabel})` : name}
          tabIndex={-1}
          data-tab-id={tab.id}
          title={unsaved > 0 ? `${name} — ${unsavedLabel}` : name}
          onPointerDown={(e) => {
            if (e.button === 1) {
              // Middle-click closes (and must not start autoscroll).
              e.preventDefault();
              onClose();
              return;
            }
            if (e.button !== 0) return;
            onActivate();
            listeners?.onPointerDown?.(e);
          }}
          onMouseDown={(e) => {
            if (e.button === 1) e.preventDefault();
          }}
          className={cn(
            "group border-border relative flex h-full max-w-55 min-w-28 shrink-0 cursor-default items-center gap-1.5 border-r border-b-2 pr-1.5 pl-2.5 text-xs select-none",
            active
              ? "bg-background text-foreground"
              : "text-muted-foreground hover:bg-surface hover:text-foreground",
            isDragging && "z-10 opacity-80 shadow-md",
          )}
        >
          <Icon className={cn("size-3.5 shrink-0", active ? "text-brand" : "")} />
          <span className="truncate">{tab.title}</span>
          <button
            type="button"
            aria-label={`Close ${tab.title}`}
            // Not a tab stop: it is invisible until hover, and ⌘W / the context menu already
            // close tabs from the keyboard (one invisible stop per tab would sit before Search).
            tabIndex={-1}
            onPointerDown={(e) => {
              e.stopPropagation();
              if (e.button === 1) {
                e.preventDefault();
                onClose();
              }
            }}
            onClick={(e) => {
              e.stopPropagation();
              onClose();
            }}
            className={cn(
              "text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:ring-ring/50 ml-auto flex size-4 shrink-0 items-center justify-center rounded outline-none focus-visible:ring-2",
              // Clean tabs show the close button on hover (always, faintly, when active); tabs with
              // unsaved work show a dot that turns into the close button on hover. Focus (e.g. from
              // assistive tech) reveals it the same way.
              unsaved > 0
                ? ""
                : cn(
                    "opacity-0 group-hover:opacity-100 focus-visible:opacity-100",
                    active && "opacity-60",
                  ),
            )}
          >
            {unsaved > 0 && (
              <span
                data-slot="tab-dirty-dot"
                className="bg-warning size-2 rounded-full group-hover:hidden group-has-[button:focus-visible]:hidden"
              />
            )}
            <X
              className={cn(
                "size-3",
                unsaved > 0 && "hidden group-hover:block group-has-[button:focus-visible]:block",
              )}
            />
          </button>
        </div>
      </ContextMenuTrigger>
      <TabMenu tab={tab} />
    </ContextMenu>
  );
}
