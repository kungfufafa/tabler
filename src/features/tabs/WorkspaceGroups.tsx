import { closestCenter, DndContext } from "@dnd-kit/core";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { focusedGroup, useTabsStore, type TabGroup } from "@/stores/tabs";
import { TabContent } from "./TabContent";
import { onTabDragEnd, useTabSensors } from "./tabDrag";
import { TabStrip } from "./TabStrip";
import { useActiveSplit } from "./workspaces";

/** One group of a split workspace: its tab bar and the tab it shows. */
function GroupPane({ connectionId, group }: { connectionId: string; group: TabGroup }) {
  const focused = useTabsStore((s) => focusedGroup(s, connectionId) === group);
  // Clicking into (or tabbing into) the other group gives it the focus: its tab becomes active.
  const take = () => {
    const s = useTabsStore.getState();
    const shown = s.splits[connectionId]?.shown[group] ?? null;
    if (shown && s.activeId !== shown) s.activate(shown);
  };
  return (
    <div
      data-slot="tab-group"
      data-tab-group={group}
      data-focused={focused || undefined}
      onPointerDownCapture={take}
      onFocusCapture={take}
      className={cn(
        "flex h-full min-h-0 min-w-0 flex-col",
        !focused && "[&>[data-slot=tab-bar]]:opacity-80",
      )}
    >
      <TabStrip group={group} shared />
      <div className="min-h-0 flex-1">
        <TabContent connectionId={connectionId} group={group} />
      </div>
    </div>
  );
}

/**
 * The workspace's tab content. Split Right (⇧⌘D) shows two tab groups side by side, each with its
 * own tab bar; tabs drag from one bar to the other, and the split closes once a group has no tab
 * left. Unsplit, the shell's single tab bar sits above this.
 */
export function WorkspaceGroups() {
  const connectionId = useConnectionsStore((s) => s.activeId);
  const split = useActiveSplit();
  const sensors = useTabSensors();
  if (!split || connectionId === null) return <TabContent />;
  return (
    <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onTabDragEnd}>
      <ResizablePanelGroup orientation="horizontal" className="h-full min-h-0">
        <ResizablePanel id="tab-group-left" minSize={240}>
          <GroupPane connectionId={connectionId} group={0} />
        </ResizablePanel>
        <ResizableHandle />
        <ResizablePanel id="tab-group-right" minSize={240}>
          <GroupPane connectionId={connectionId} group={1} />
        </ResizablePanel>
      </ResizablePanelGroup>
    </DndContext>
  );
}
