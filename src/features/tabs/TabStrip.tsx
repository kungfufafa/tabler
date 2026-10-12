import {
  closestCenter,
  DndContext,
  PointerSensor,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import { horizontalListSortingStrategy, SortableContext } from "@dnd-kit/sortable";
import { ChevronLeft, ChevronRight, Plus } from "lucide-react";
import { useEffect, useMemo, useRef, type ReactNode } from "react";
import { Kbd } from "@/components/Kbd";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { actions } from "@/features/palette/actions";
import { isTauri } from "@/lib/ipc";
import { shortcutFor } from "@/lib/keys/keymap";
import { useConnectionsStore } from "@/stores/connections";
import { canGoBack, canGoForward, useTabsStore, workspaceTabs } from "@/stores/tabs";
import { TabItem } from "./TabItem";
import { wheelToScrollLeft } from "./wheel";

/**
 * The empty bar while no tab is open (tabs otherwise fill it): drags the window, and a
 * double-click opens a new query tab. It is deliberately not a `data-tauri-drag-region` (that would
 * also maximize the window on double-click); dragging is started by hand instead.
 */
function StripFiller() {
  return (
    <div
      data-slot="tabstrip-filler"
      aria-hidden
      className="h-full min-w-0 flex-[1_0_0]"
      onMouseDown={(e) => {
        if (e.button !== 0) return;
        if (e.detail === 2) {
          e.preventDefault();
          void actions.run("tab.new-query");
        } else if (e.detail === 1 && isTauri()) {
          void import("@tauri-apps/api/window")
            .then(({ getCurrentWindow }) => getCurrentWindow().startDragging())
            .catch(() => undefined);
        }
      }}
    />
  );
}

function BarButton({
  label,
  combo,
  disabled,
  onClick,
  children,
}: {
  label: string;
  combo?: string;
  disabled?: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  const button = (
    <button
      type="button"
      aria-label={label}
      disabled={disabled}
      onClick={onClick}
      className="text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-ring/50 flex size-5.5 shrink-0 items-center justify-center rounded outline-none focus-visible:ring-2 disabled:pointer-events-none disabled:opacity-35"
    >
      {children}
    </button>
  );
  // Always wrapped (a disabled button gets no pointer events, so no tooltip): toggling `disabled`
  // must not remount the button under the pointer or the keyboard focus.
  return (
    <Tooltip>
      <TooltipTrigger asChild>{button}</TooltipTrigger>
      <TooltipContent className="flex items-center gap-1.5">
        {label}
        {combo && <Kbd combo={combo} className="border-border/40 bg-transparent text-inherit" />}
      </TooltipContent>
    </Tooltip>
  );
}

/**
 * The workspace's tab bar (macOS document tabs, TablePlus style): ‹ › walk the tab history, the
 * tabs fill the row in equal widths, `+` opens a new query. Tabs drag to reorder, middle-click
 * closes, and the wheel scrolls the row when the tabs no longer fit.
 */
export function TabStrip() {
  // Each connection is its own workspace (TablePlus): the bar shows the active connection's tabs.
  const workspace = useConnectionsStore((s) => s.activeId);
  const allTabs = useTabsStore((s) => s.tabs);
  const tabs = useMemo(() => workspaceTabs(allTabs, workspace), [allTabs, workspace]);
  const activeId = useTabsStore((s) => s.activeId);
  const activate = useTabsStore((s) => s.activate);
  const close = useTabsStore((s) => s.close);
  const move = useTabsStore((s) => s.move);
  const back = useTabsStore((s) => s.back);
  const forward = useTabsStore((s) => s.forward);
  const backEnabled = useTabsStore(canGoBack);
  const forwardEnabled = useTabsStore(canGoForward);
  const hasConnection = useConnectionsStore((s) => s.activeId !== null);
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 6 } }));
  const listRef = useRef<HTMLDivElement>(null);

  // Keep the active tab visible (new tabs, ⌘1…9, ⌃Tab, reopen).
  useEffect(() => {
    const list = listRef.current;
    if (!list || !activeId) return;
    const el = Array.from(list.children).find(
      (c): c is HTMLElement => c instanceof HTMLElement && c.dataset.tabId === activeId,
    );
    el?.scrollIntoView?.({ inline: "nearest", block: "nearest" });
  }, [activeId, tabs.length]);

  const onDragEnd = (e: DragEndEvent) => {
    if (!e.over || e.active.id === e.over.id) return;
    // `move` takes positions in the whole strip (every workspace), not in the visible part.
    const from = allTabs.findIndex((t) => t.id === e.active.id);
    const to = allTabs.findIndex((t) => t.id === e.over!.id);
    if (from >= 0 && to >= 0) move(from, to);
  };

  const onWheel = (e: React.WheelEvent) => {
    const list = listRef.current;
    if (!list) return;
    const dx = wheelToScrollLeft(e, list.clientWidth);
    if (dx !== 0) list.scrollLeft += dx;
  };

  return (
    <div
      data-slot="tab-bar"
      // The bar is a shade darker than the window; the visible tab takes the window background.
      className="border-border bg-surface-2 flex h-7 shrink-0 items-stretch border-b dark:bg-black/20"
    >
      <div className="border-border flex shrink-0 items-center gap-px border-r px-1">
        <BarButton label="Back" disabled={!backEnabled} onClick={back}>
          <ChevronLeft className="size-3.5" />
        </BarButton>
        <BarButton label="Forward" disabled={!forwardEnabled} onClick={forward}>
          <ChevronRight className="size-3.5" />
        </BarButton>
      </div>
      <div className="flex min-w-0 flex-1 items-stretch" onWheel={onWheel}>
        <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
          <SortableContext items={tabs.map((t) => t.id)} strategy={horizontalListSortingStrategy}>
            <div
              ref={listRef}
              role="tablist"
              aria-label="Open tabs"
              className="flex h-full min-w-0 flex-1 [scrollbar-width:none] items-stretch overflow-x-auto overflow-y-hidden [&::-webkit-scrollbar]:hidden"
            >
              {tabs.map((t) => (
                <TabItem
                  key={t.id}
                  tab={t}
                  active={t.id === activeId}
                  onActivate={() => activate(t.id)}
                  onClose={() => close(t.id)}
                />
              ))}
              {tabs.length === 0 && <StripFiller />}
            </div>
          </SortableContext>
        </DndContext>
      </div>
      <div className="border-border flex shrink-0 items-center border-l px-1">
        <BarButton
          label="New query"
          combo={shortcutFor("tab.new-query")}
          disabled={!hasConnection}
          onClick={() => void actions.run("tab.new-query")}
        >
          <Plus className="size-3.5" />
        </BarButton>
      </div>
    </div>
  );
}
