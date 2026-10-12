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
import { useStructureStore } from "@/features/structure/store";
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

/**
 * Staged row edits plus staged DDL ops of a table's tab (both views keep their work while the other
 * one shows), or 1 for a dirty query tab. Same rule as `unsavedCount`, from subscribed state.
 */
function useUnsaved(tab: Tab): number {
  const rows = useTableTabsStore((s) => {
    if (tab.kind === "query") return 0;
    const slice = s.slices[tab.id];
    return slice ? pendingCount(slice.staging) : 0;
  });
  const ddl = useStructureStore((s) =>
    tab.kind === "query" ? 0 : (s.byTab[tab.id]?.ops.length ?? 0),
  );
  if (tab.kind === "query") return tab.dirty ? 1 : 0;
  return rows + ddl;
}

function TabMenu({ tab }: { tab: Tab }) {
  // Close Others / to the Right act within the tab's workspace (its connection).
  const count = useTabsStore(
    (s) => s.tabs.filter((t) => t.connectionId === tab.connectionId).length,
  );
  const isLast = useTabsStore((s) => {
    const own = s.tabs.filter((t) => t.connectionId === tab.connectionId);
    return own[own.length - 1]?.id === tab.id;
  });
  const canReopen = useTabsStore((s) => s.closed.length > 0);
  const store = useTabsStore.getState;
  const item = "py-0.5 text-xs";
  return (
    <ContextMenuContent className="min-w-48">
      {tab.kind !== "query" && tab.preview && (
        <>
          <ContextMenuItem className={item} onSelect={() => store().pin(tab.id)}>
            Keep Open
          </ContextMenuItem>
          <ContextMenuSeparator />
        </>
      )}
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
  const pin = useTabsStore((s) => s.pin);
  const unsaved = useUnsaved(tab);
  const Icon = ICONS[tab.kind];
  const subtitle = tab.kind === "query" ? undefined : (tab.table.schema ?? undefined);
  const name = subtitle ? `${subtitle}.${tab.title}` : tab.title;
  const preview = tab.kind !== "query" && tab.preview === true;
  const unsavedLabel =
    tab.kind === "query"
      ? "Unsaved changes"
      : `${unsaved} pending change${unsaved === 1 ? "" : "s"}`;
  const hint = preview ? " (preview: double-click to keep it open)" : "";
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          ref={setNodeRef}
          style={{
            transform: CSS.Translate.toString(transform),
            transition,
            // A subtle line in the connection's colour under the visible tab.
            boxShadow: active
              ? `inset 0 -2px 0 color-mix(in oklab, ${accentColor(color)} 75%, transparent)`
              : undefined,
          }}
          {...attributes}
          {...listeners}
          role="tab"
          aria-selected={active}
          aria-label={unsaved > 0 ? `${name} (${unsavedLabel})` : name}
          tabIndex={-1}
          data-tab-id={tab.id}
          data-preview={preview || undefined}
          title={(unsaved > 0 ? `${name} — ${unsavedLabel}` : name) + hint}
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
          // A preview tab (italic) becomes a regular one on double-click.
          onDoubleClick={() => {
            if (preview) pin(tab.id);
          }}
          className={cn(
            // macOS document tabs (TablePlus): the tabs share the whole bar in equal widths (one tab
            // spans it), down to 110px each; past that the row scrolls.
            "group border-border relative flex h-full min-w-[110px] flex-[1_1_0] cursor-default items-center border-r text-xs select-none last:border-r-0",
            active
              ? "bg-background text-foreground"
              : "text-muted-foreground hover:text-foreground hover:bg-black/[0.03] dark:hover:bg-white/[0.04]",
            isDragging && "z-10 opacity-80 shadow-md",
          )}
        >
          <button
            type="button"
            aria-label={`Close ${tab.title}`}
            // Not a tab stop: it is invisible until hover, and ⌘W / the context menu already
            // close tabs from the keyboard (one invisible stop per tab would sit before `+`).
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
              // macOS puts the close button at the tab's left edge.
              "text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:ring-ring/50 absolute top-1/2 left-1.5 flex size-4 -translate-y-1/2 items-center justify-center rounded outline-none focus-visible:ring-2",
              // Clean tabs show the close button on hover; tabs with unsaved work show a dot that
              // turns into the close button on hover. Focus (e.g. from assistive tech) reveals it
              // the same way.
              unsaved > 0 ? "" : "opacity-0 group-hover:opacity-100 focus-visible:opacity-100",
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
          <span className="flex min-w-0 flex-1 items-center justify-center gap-1.5 px-6">
            <Icon
              className={cn("size-3.5 shrink-0", active ? "text-brand" : "opacity-70")}
              aria-hidden
            />
            <span className={cn("truncate", preview && "italic")}>{tab.title}</span>
          </span>
        </div>
      </ContextMenuTrigger>
      <TabMenu tab={tab} />
    </ContextMenu>
  );
}
