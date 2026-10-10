import {
  closestCenter,
  DndContext,
  PointerSensor,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import { horizontalListSortingStrategy, SortableContext } from "@dnd-kit/sortable";
import { useTabsStore } from "@/stores/tabs";
import { TabItem } from "./TabItem";

export function TabStrip() {
  const tabs = useTabsStore((s) => s.tabs);
  const activeId = useTabsStore((s) => s.activeId);
  const activate = useTabsStore((s) => s.activate);
  const close = useTabsStore((s) => s.close);
  const move = useTabsStore((s) => s.move);
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 6 } }));

  const onDragEnd = (e: DragEndEvent) => {
    if (!e.over || e.active.id === e.over.id) return;
    const from = tabs.findIndex((t) => t.id === e.active.id);
    const to = tabs.findIndex((t) => t.id === e.over!.id);
    if (from >= 0 && to >= 0) move(from, to);
  };

  if (tabs.length === 0) return null;
  return (
    <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
      <SortableContext items={tabs.map((t) => t.id)} strategy={horizontalListSortingStrategy}>
        <div
          role="tablist"
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
  );
}
