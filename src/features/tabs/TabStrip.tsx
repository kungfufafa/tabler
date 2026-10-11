import {
  closestCenter,
  DndContext,
  PointerSensor,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import { horizontalListSortingStrategy, SortableContext } from "@dnd-kit/sortable";
import { useEffect, useRef } from "react";
import { actions } from "@/features/palette/actions";
import { useUnsavedGuard } from "@/hooks/useUnsavedGuard";
import { isTauri } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { TabItem } from "./TabItem";
import { UnsavedChangesDialog } from "./UnsavedChangesDialog";
import { wheelToScrollLeft } from "./wheel";

/**
 * Empty space after the tabs: drags the window like the rest of the titlebar, and a double-click
 * opens a new query tab. It is deliberately not a `data-tauri-drag-region` (that would also
 * maximize the window on double-click); dragging is started by hand instead.
 */
function StripFiller() {
  return (
    <div
      data-slot="tabstrip-filler"
      aria-hidden
      className="h-full min-w-0 flex-1"
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

export function TabStrip() {
  useUnsavedGuard();
  const tabs = useTabsStore((s) => s.tabs);
  const activeId = useTabsStore((s) => s.activeId);
  const activate = useTabsStore((s) => s.activate);
  const close = useTabsStore((s) => s.close);
  const move = useTabsStore((s) => s.move);
  const workspace = useUiStore((s) => s.view === "workspace");
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
    const from = tabs.findIndex((t) => t.id === e.active.id);
    const to = tabs.findIndex((t) => t.id === e.over!.id);
    if (from >= 0 && to >= 0) move(from, to);
  };

  const onWheel = (e: React.WheelEvent) => {
    const list = listRef.current;
    if (!list) return;
    const dx = wheelToScrollLeft(e, list.clientWidth);
    if (dx !== 0) list.scrollLeft += dx;
  };

  const showStrip = tabs.length > 0 || (workspace && hasConnection);
  return (
    <>
      {showStrip && (
        // Takes nearly all free titlebar space so the empty area belongs to the strip; the
        // titlebar's own drag filler keeps its minimum width next to the right-hand buttons.
        <div className="flex h-full min-w-0 flex-[999_1_0%] items-stretch" onWheel={onWheel}>
          <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
            <SortableContext items={tabs.map((t) => t.id)} strategy={horizontalListSortingStrategy}>
              <div
                ref={listRef}
                role="tablist"
                aria-label="Open tabs"
                className="flex h-full min-w-0 [scrollbar-width:none] items-stretch overflow-x-auto overflow-y-hidden [&::-webkit-scrollbar]:hidden"
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
              </div>
            </SortableContext>
          </DndContext>
          <StripFiller />
        </div>
      )}
      <UnsavedChangesDialog />
    </>
  );
}
