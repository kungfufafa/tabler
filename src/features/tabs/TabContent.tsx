import { EmptyState } from "@/components/EmptyState";
import { Kbd } from "@/components/Kbd";
import { QuietLoading } from "@/components/QuietLoading";
import { LacksFieldsContext } from "@/features/datagrid/lacksFields";
import { rowsCanLackFields } from "@/features/drivers/catalog";
import { useDriverUi } from "@/features/drivers/registry";
import { QueryTab } from "@/features/editor/QueryTab";
import { actions, ActionsEnabledContext, type ActionId } from "@/features/palette/actions";
import { StructureTab } from "@/features/structure/StructureTab";
import { BottomBarFrame } from "@/features/table-tab/BarParts";
import { TableTab } from "@/features/table-tab/TableTab";
import { capsOf } from "@/lib/caps";
import { shortcutFor, useKeymapVersion } from "@/lib/keys/keymap";
import { useConnectionsStore } from "@/stores/connections";
import { useActiveTab, useTabsStore, type Tab, type TabGroup } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { getToolTab } from "./toolTabs";
import { ViewSwitch } from "./ViewSwitch";

/** One of the empty workspace's actions, with its (possibly remapped) shortcut. */
function HintAction({ id, label }: { id: ActionId; label: string }) {
  const combo = shortcutFor(id);
  return (
    <button
      type="button"
      onClick={() => void actions.run(id)}
      className="hover:bg-accent hover:text-foreground focus-visible:ring-ring flex h-7 items-center gap-2 rounded-md px-2 transition-colors outline-none focus-visible:ring-1"
    >
      {label}
      {combo && <Kbd combo={combo} />}
    </button>
  );
}

/**
 * The connected workspace before any table opens. TablePlus leaves the centre blank; a short hint
 * with the ways in is friendlier, and only points at the sidebar while the sidebar shows.
 */
function WorkspaceHint() {
  const sidebarOpen = useUiStore((s) => s.sidebarOpen);
  // The shortcuts shown are the current (possibly remapped) ones.
  useKeymapVersion();
  return (
    <div
      data-slot="workspace-hint"
      className="text-muted-foreground flex h-full flex-col items-center justify-center gap-2 p-6 text-center text-xs"
    >
      <p>{sidebarOpen ? "Select an item in the sidebar, or:" : "Nothing is open yet."}</p>
      <div className="flex flex-wrap items-center justify-center gap-1">
        <HintAction id="palette.open" label="Open anything" />
        <HintAction id="tab.new-query" label="New query" />
        {!sidebarOpen && <HintAction id="ui.toggle-sidebar" label="Show sidebar" />}
      </div>
    </div>
  );
}

/** A tool tab's registered component (a placeholder when its feature is not there). */
function ToolTabBody({ tab, focused }: { tab: Extract<Tab, { kind: "tool" }>; focused: boolean }) {
  const def = getToolTab(tab.tool);
  if (!def)
    return (
      <div data-slot="tool-tab-missing" className="h-full">
        <EmptyState
          title="This tab cannot be shown"
          description={`Nothing in this build opens the “${tab.tool}” tool.`}
        />
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
  if (ui === undefined) return <QuietLoading header data-slot="lane-loading" />;
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
  // Whether its rows can lack a field (`isMissingValue`): a session of a driver beyond the core ones.
  const lacksFields = useConnectionsStore((s) => rowsCanLackFields(s.byId[tab.connectionId]?.info));
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
  return (
    <ActionsEnabledContext.Provider value={focused}>
      <LacksFieldsContext.Provider value={lacksFields}>{body}</LacksFieldsContext.Provider>
    </ActionsEnabledContext.Provider>
  );
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
