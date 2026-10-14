import { PointerSensor, useSensor, useSensors, type DragEndEvent } from "@dnd-kit/core";
import { groupOf, useTabsStore, type TabGroup } from "@/stores/tabs";

/** Droppable id of a group's strip (dropping a tab on its empty part moves it there). */
export const groupDropId = (group: TabGroup) => `tab-group:${group}`;

/**
 * Where a dragged tab goes: onto a tab of its own group it is reordered (`move`, positions in the
 * whole strip), onto a tab or the strip of the other group of a split workspace it moves there.
 */
export function dropTab(activeId: string, overId: string | null): void {
  if (!overId || overId === activeId) return;
  const s = useTabsStore.getState();
  const tab = s.tabs.find((t) => t.id === activeId);
  if (!tab) return;
  const from = groupOf(s.splits, tab);
  const strip = /^tab-group:([01])$/.exec(overId);
  if (strip) {
    const group = Number(strip[1]) as TabGroup;
    if (group !== from) s.moveToGroup(activeId, group);
    return;
  }
  const over = s.tabs.find((t) => t.id === overId);
  if (!over || over.connectionId !== tab.connectionId) return;
  const to = groupOf(s.splits, over);
  if (to !== from) {
    s.moveToGroup(activeId, to, overId);
    return;
  }
  const a = s.tabs.findIndex((t) => t.id === activeId);
  const b = s.tabs.findIndex((t) => t.id === overId);
  if (a >= 0 && b >= 0) s.move(a, b);
}

export const onTabDragEnd = (e: DragEndEvent) =>
  dropTab(String(e.active.id), e.over ? String(e.over.id) : null);

/** The pointer sensor every tab strip drags with (a short move before a drag starts). */
export function useTabSensors() {
  return useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 6 } }));
}
