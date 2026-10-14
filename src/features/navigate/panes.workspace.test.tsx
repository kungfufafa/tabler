import { act, render, screen } from "@testing-library/react";
import { Gauge } from "lucide-react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { registerToolTab, type ToolTabProps } from "@/features/tabs/toolTabs";
import { WorkspaceGroups } from "@/features/tabs/WorkspaceGroups";
import type { ConnectionInfo } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { cyclePane, paneStops } from "./panes";

// The pane cycle against the REAL split workspace (WorkspaceGroups): each group is rendered with
// its tab bar, and `data-tab-group` sits on the group pane AND on the bar and the tab list inside
// it. Only the group panes are stops: the bars must not be.

const info: ConnectionInfo = {
  connectionId: "c1",
  profileId: "p1",
  name: "Local PG",
  color: null,
  driver: "postgres",
  serverVersion: "PostgreSQL 17",
  currentDatabase: "app",
  currentUser: "me",
  tunnel: null,
  readOnly: false,
};

/** A tab body that is a data grid, as a table tab is. */
function Body({ tab }: ToolTabProps) {
  return <div data-scope="grid" tabIndex={0} aria-label={`grid-${tab.title}`} />;
}

const tool = (id: string, title: string): Tab => ({
  id,
  kind: "tool",
  connectionId: "c1",
  tool: "note",
  title,
  params: { id },
});

let unregister: () => void = () => {};
beforeEach(() => {
  useTabsStore.setState({
    tabs: [],
    activeId: null,
    closed: [],
    queryCounter: 0,
    closeRequest: null,
    history: [],
    historyIndex: -1,
    splits: {},
  });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useConnectionsStore.getState().add(info);
  useUiStore.setState({ view: "workspace" });
  unregister = registerToolTab("note", { component: Body, icon: Gauge, title: () => "Note" });
  if (!Element.prototype.scrollIntoView) Element.prototype.scrollIntoView = () => {};
});
afterEach(() => unregister());

function renderShell() {
  return render(
    <TooltipProvider>
      <div data-panel id="sidebar">
        <button data-row-index="0">schema</button>
      </div>
      <div data-panel id="content">
        <WorkspaceGroups />
      </div>
      <div data-panel id="inspector">
        <button>Field</button>
      </div>
    </TooltipProvider>,
  );
}

describe("pane cycle in a real split workspace", () => {
  it("has the three nested data-tab-group elements per group the real tab bars render", () => {
    useTabsStore.getState().open(tool("a", "Alpha"));
    useTabsStore.getState().open(tool("b", "Beta"));
    useTabsStore.getState().moveToGroup("b", 1);
    renderShell();
    // the premise of the test: the attribute is on the group pane, its bar and its tab list
    expect(document.querySelectorAll("[data-tab-group='0']").length).toBeGreaterThan(1);
    expect(document.querySelectorAll("[data-tab-group='1']").length).toBeGreaterThan(1);
  });

  it("stops at the sidebar, the left group, the right group and the inspector, nowhere in a tab bar", () => {
    useTabsStore.getState().open(tool("a", "Alpha"));
    useTabsStore.getState().open(tool("b", "Beta"));
    useTabsStore.getState().moveToGroup("b", 1);
    renderShell();
    const stops = paneStops(document);
    expect(stops.map((s) => s.pane)).toEqual(["sidebar", "content", "content", "inspector"]);
    expect(stops.map((s) => s.zone.getAttribute("data-slot") ?? s.zone.id)).toEqual([
      "sidebar",
      "tab-group",
      "tab-group",
      "inspector",
    ]);
    expect(stops.map((s) => s.target.getAttribute("aria-label") ?? s.target.textContent)).toEqual([
      "schema",
      "grid-Alpha",
      "grid-Beta",
      "Field",
    ]);
  });

  it("⇧⌘] and ⇧⌘[ walk those four stops and wrap", () => {
    useTabsStore.getState().open(tool("a", "Alpha"));
    useTabsStore.getState().open(tool("b", "Beta"));
    useTabsStore.getState().moveToGroup("b", 1);
    renderShell();
    (document.activeElement as HTMLElement | null)?.blur();
    const at = () =>
      document.activeElement?.getAttribute("aria-label") ?? document.activeElement?.textContent;
    const forward: Array<string | null | undefined> = [];
    for (let i = 0; i < 5; i++) {
      act(() => void cyclePane(1, document));
      forward.push(at());
    }
    expect(forward).toEqual(["schema", "grid-Alpha", "grid-Beta", "Field", "schema"]);
    const back: Array<string | null | undefined> = [];
    for (let i = 0; i < 4; i++) {
      act(() => void cyclePane(-1, document));
      back.push(at());
    }
    expect(back).toEqual(["Field", "grid-Beta", "grid-Alpha", "schema"]);
  });

  it("focus on a tab-bar button counts as the group it belongs to", () => {
    useTabsStore.getState().open(tool("a", "Alpha"));
    useTabsStore.getState().open(tool("b", "Beta"));
    useTabsStore.getState().moveToGroup("b", 1);
    renderShell();
    screen.getByRole("button", { name: "Back" }).focus();
    // inside the left group's bar: next is the RIGHT group, not the bar's own buttons
    act(() => void cyclePane(1, document));
    expect(document.activeElement?.getAttribute("aria-label")).toBe("grid-Beta");
  });

  it("an unsplit workspace is one content stop", () => {
    useTabsStore.getState().open(tool("a", "Alpha"));
    renderShell();
    expect(paneStops(document).map((s) => s.pane)).toEqual(["sidebar", "content", "inspector"]);
  });
});
