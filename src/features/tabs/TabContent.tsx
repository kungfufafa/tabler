import { Kbd } from "@/components/Kbd";
import { Skeleton } from "@/components/ui/skeleton";
import { useDriverUi } from "@/features/drivers/registry";
import { QueryTab } from "@/features/editor/QueryTab";
import { ActionsEnabledContext } from "@/features/palette/actions";
import { StructureTab } from "@/features/structure/StructureTab";
import { BottomBarFrame } from "@/features/table-tab/BarParts";
import { TableTab } from "@/features/table-tab/TableTab";
import { capsOf } from "@/lib/caps";
import { shortcutFor } from "@/lib/keys/keymap";
import { useConnectionsStore } from "@/stores/connections";
import { useActiveTab, useTabsStore, type Tab, type TabGroup } from "@/stores/tabs";
import { getToolTab } from "./toolTabs";
import { ViewSwitch } from "./ViewSwitch";

/** The connected workspace before any table opens (TablePlus shows a single centred hint). */
function WorkspaceHint() {
  const openAnything = shortcutFor("palette.open") ?? "Mod+P";
  return (
    <div
      data-slot="workspace-hint"
      className="text-muted-foreground flex h-full items-center justify-center p-6 text-center text-xs"
    >
      <p>
        Select an item in the sidebar, or press{" "}
        <Kbd combo={openAnything} className="mx-0.5 align-middle" /> to open anything
      </p>
    </div>
  );
}

/** A tool tab's registered component (a placeholder when its feature is not there). */
function ToolTabBody({ tab, focused }: { tab: Extract<Tab, { kind: "tool" }>; focused: boolean }) {
  const def = getToolTab(tab.tool);
  if (!def)
    return (
      <div
        data-slot="tool-tab-missing"
        className="text-muted-foreground flex h-full items-center justify-center p-6 text-xs"
      >
        This tab cannot be shown ({tab.tool}).
      </div>
    );
  const Component = def.component;
  return <Component tab={tab} params={tab.params} focused={focused} />;
}

/**
 * A table tab's data: the grid, or the view a lane gives its driver in its place
 * (`DriverUi.TableView`: a key browser, a document list), above the Data | Structure switch. The
 * core drivers have no lane: their grid shows at once.
 */
function TableTabBody({ tab }: { tab: Extract<Tab, { kind: "table" }> }) {
  const info = useConnectionsStore((s) => s.byId[tab.connectionId]?.info);
  const ui = useDriverUi(info?.driver ?? null);
  // The lane's module is still loading.
  if (ui === undefined)
    return (
      <div className="flex flex-col gap-2 p-4" data-slot="lane-loading">
        <Skeleton className="h-5 w-48" />
        <Skeleton className="h-64 w-full" />
      </div>
    );
  const View = ui?.TableView;
  if (!View || !info) return <TableTab tab={tab} />;
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="min-h-0 flex-1">
        <View connectionId={tab.connectionId} table={tab.table} caps={capsOf(info)} />
      </div>
      <BottomBarFrame aria-label="Table data">
        <ViewSwitch tabId={tab.id} view="table" />
      </BottomBarFrame>
    </div>
  );
}

/**
 * One tab's content. `focused`: false for the tab of a split workspace's other group; its
 * shortcuts and menu items are off (`ActionsEnabledContext`) until that group gets the focus.
 */
export function TabBody({ tab, focused = true }: { tab: Tab; focused?: boolean }) {
  let body;
  switch (tab.kind) {
    case "table":
      body = <TableTabBody key={tab.id} tab={tab} />;
      break;
    case "structure":
      body = <StructureTab key={tab.id} tab={tab} />;
      break;
    case "query":
      body = <QueryTab key={tab.id} tab={tab} />;
      break;
    case "tool":
      body = <ToolTabBody key={tab.id} tab={tab} focused={focused} />;
      break;
  }
  return <ActionsEnabledContext.Provider value={focused}>{body}</ActionsEnabledContext.Provider>;
}

/** The tab a group of a split workspace shows (`group` set), else the active tab. */
export function TabContent({ connectionId, group }: { connectionId?: string; group?: TabGroup }) {
  const active = useActiveTab();
  const shown = useTabsStore((s) => {
    if (connectionId === undefined || group === undefined) return null;
    const id = s.splits[connectionId]?.shown[group];
    return id ? (s.tabs.find((t) => t.id === id) ?? null) : null;
  });
  if (group === undefined || connectionId === undefined) {
    if (!active) return <WorkspaceHint />;
    return <TabBody tab={active} />;
  }
  if (!shown) return <WorkspaceHint />;
  return <TabBody tab={shown} focused={active?.id === shown.id} />;
}
