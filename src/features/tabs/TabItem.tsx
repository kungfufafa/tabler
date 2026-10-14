import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
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
import { groupOf, isTableTab, useTabsStore, type Tab } from "@/stores/tabs";
import { useTableTabsStore } from "@/stores/tableTabs";
import { moveTabToNewWindow } from "./windowHandoff";
import { startTabRename, stopTabRename, useTabRenameStore } from "./renameStore";
import { tabIcon } from "./tabIcon";

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
    if (!isTableTab(tab)) return 0;
    const slice = s.slices[tab.id];
    return slice ? pendingCount(slice.staging) : 0;
  });
  const ddl = useStructureStore((s) => (isTableTab(tab) ? (s.byTab[tab.id]?.ops.length ?? 0) : 0));
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
  // Split workspace: the tab's group, and whether the other group exists.
  const group = useTabsStore((s) => groupOf(s.splits, tab));
  const split = useTabsStore((s) => !!s.splits[tab.connectionId]);
  const store = useTabsStore.getState;
  const item = "py-0.5 text-xs";
  return (
    <ContextMenuContent
      className="min-w-48"
      // Rename Tab… edits the title in the tab: focus goes there, not back to the tab.
      onCloseAutoFocus={(e) => {
        if (useTabRenameStore.getState().renamingId === tab.id) e.preventDefault();
      }}
    >
      {tab.kind !== "query" && tab.preview && (
        <>
          <ContextMenuItem className={item} onSelect={() => store().pin(tab.id)}>
            Keep Open
          </ContextMenuItem>
          <ContextMenuSeparator />
        </>
      )}
      <ContextMenuItem className={item} onSelect={() => startTabRename(tab.id)}>
        Rename Tab…
      </ContextMenuItem>
      <ContextMenuSeparator />
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
        Close All Tabs
        <Shortcut id="tab.close-all" />
      </ContextMenuItem>
      <ContextMenuSeparator />
      {/* A table tab's copy starts with its filters, sort and column layout (not its staged edits). */}
      {(tab.kind === "query" || isTableTab(tab)) && (
        <ContextMenuItem className={item} onSelect={() => store().duplicate(tab.id)}>
          Duplicate Tab
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
      <ContextMenuSeparator />
      <ContextMenuItem className={item} onSelect={() => store().splitRight(tab.id)}>
        Split Right
        <Shortcut id="workspace.split-right" />
      </ContextMenuItem>
      {split ? (
        <ContextMenuItem
          className={item}
          onSelect={() => store().moveToGroup(tab.id, group === 0 ? 1 : 0)}
        >
          {group === 0 ? "Move to Right Group" : "Move to Left Group"}
        </ContextMenuItem>
      ) : (
        <ContextMenuItem
          className={item}
          disabled={count < 2}
          onSelect={() => store().moveToGroup(tab.id, 1)}
        >
          Move to Right Group
        </ContextMenuItem>
      )}
      <ContextMenuItem className={item} onSelect={() => void moveTabToNewWindow(tab.id)}>
        Move Tab to New Window
      </ContextMenuItem>
    </ContextMenuContent>
  );
}

/** The tab's title as a text field (Rename Tab…): ↩ or leaving it renames, ⎋ keeps the name. */
function TitleInput({ tab }: { tab: Tab }) {
  const [value, setValue] = useState(tab.title);
  const ref = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  useEffect(() => {
    // A closing context menu hands focus back to its trigger first; take it after that.
    const timer = setTimeout(() => {
      ref.current?.focus();
      ref.current?.select();
    }, 0);
    return () => clearTimeout(timer);
  }, []);
  const finish = (rename: boolean) => {
    if (done.current) return;
    done.current = true;
    if (rename) useTabsStore.getState().rename(tab.id, value);
    stopTabRename();
  };
  return (
    <input
      ref={ref}
      aria-label="Tab name"
      value={value}
      spellCheck={false}
      autoComplete="off"
      onChange={(e) => setValue(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          finish(true);
        } else if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          finish(false);
        }
      }}
      onBlur={() => finish(true)}
      // Typing and selecting text must not drag, activate or close the tab.
      onPointerDown={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      className="bg-background ring-brand h-5 w-full min-w-0 rounded px-1 text-center text-xs ring-1 outline-none select-text"
    />
  );
}

export function TabItem({
  tab,
  active,
  groupFocused = true,
  onActivate,
  onClose,
}: {
  tab: Tab;
  /** The tab its group shows. */
  active: boolean;
  /** False in the group of a split workspace that does not have the focus. */
  groupFocused?: boolean;
  onActivate: () => void;
  onClose: () => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: tab.id,
  });
  const color = useConnectionsStore((s) => s.byId[tab.connectionId]?.info.color ?? null);
  const pin = useTabsStore((s) => s.pin);
  const renaming = useTabRenameStore((s) => s.renamingId === tab.id);
  const unsaved = useUnsaved(tab);
  const Icon = tabIcon(tab);
  const subtitle = isTableTab(tab) ? (tab.table.schema ?? undefined) : undefined;
  const name = subtitle ? `${subtitle}.${tab.title}` : tab.title;
  const preview = isTableTab(tab) && tab.preview === true;
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
            // A subtle line in the connection's colour under the visible tab (fainter in the
            // group of a split workspace that does not have the focus).
            boxShadow: active
              ? `inset 0 -2px 0 color-mix(in oklab, ${accentColor(color)} ${groupFocused ? 75 : 30}%, transparent)`
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
          // A preview tab (italic) becomes a regular one on double-click; a query tab's title
          // turns into a text field to rename it.
          onDoubleClick={(e) => {
            if (e.target instanceof Element && e.target.closest("button")) return;
            if (preview) pin(tab.id);
            else if (tab.kind === "query") startTabRename(tab.id);
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
              className={cn(
                "size-3.5 shrink-0",
                active && groupFocused ? "text-brand" : "opacity-70",
              )}
              aria-hidden
            />
            {renaming ? (
              <TitleInput tab={tab} />
            ) : (
              <span className={cn("truncate", preview && "italic")}>{tab.title}</span>
            )}
          </span>
        </div>
      </ContextMenuTrigger>
      <TabMenu tab={tab} />
    </ContextMenu>
  );
}
