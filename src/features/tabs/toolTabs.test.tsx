import { act, render, screen } from "@testing-library/react";
import { Gauge } from "lucide-react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { actions, useAction } from "@/features/palette/actions";
import { useTabsStore, type ToolTab } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { TabBody } from "./TabContent";
import {
  findToolTab,
  getToolTab,
  openToolTab,
  persistsToolTab,
  registerToolTab,
  restoreToolParams,
  type ToolTabProps,
} from "./toolTabs";

function Board({ tab, params, focused }: ToolTabProps<{ board: string }>) {
  // A tool tab's shortcuts are on only while its group has the focus.
  useAction("view.refresh", () => {});
  return (
    <p>
      {tab.title} / {params.board} / {focused ? "focused" : "unfocused"}
    </p>
  );
}

const state = () => useTabsStore.getState();

describe("tool tabs", () => {
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
    useUiStore.setState({ view: "launcher" });
    unregister = registerToolTab<{ board: string }>("metrics", {
      component: Board,
      icon: Gauge,
      title: (p) => `Metrics: ${p.board}`,
      persist: true,
      restore: (raw) =>
        typeof (raw as { board?: unknown } | null)?.board === "string"
          ? { board: (raw as { board: string }).board }
          : null,
    });
  });
  afterEach(() => unregister());

  it("registers a tool and checks restored params with it", () => {
    expect(getToolTab("metrics")?.icon).toBe(Gauge);
    expect(persistsToolTab("metrics")).toBe(true);
    expect(persistsToolTab("nope")).toBe(false);
    expect(restoreToolParams("metrics", { board: "db" })).toEqual({ board: "db" });
    expect(restoreToolParams("metrics", { board: 1 })).toBeNull();
    expect(restoreToolParams("nope", {})).toBeNull();
    unregister();
    expect(getToolTab("metrics")).toBeNull();
  });

  it("lets a tool decide per tab from its params whether the session keeps it", () => {
    const off = registerToolTab<{ keep: boolean } | null>("picky", {
      component: () => null,
      icon: Gauge,
      title: () => "Picky",
      persist: (p) => p?.keep === true,
    });
    expect(persistsToolTab("picky", { keep: true })).toBe(true);
    expect(persistsToolTab("picky", { keep: false })).toBe(false);
    expect(persistsToolTab("picky", null)).toBe(false);
    // Without params the predicate is asked with null.
    expect(persistsToolTab("picky")).toBe(false);
    off();
    // A predicate that throws leaves its tab out instead of breaking the session save.
    const broken = registerToolTab<unknown>("broken", {
      component: () => null,
      icon: Gauge,
      title: () => "Broken",
      persist: () => {
        throw new Error("boom");
      },
    });
    expect(persistsToolTab("broken", {})).toBe(false);
    broken();
    // The plain boolean forms still work whatever the params.
    expect(persistsToolTab("metrics", { board: "x" })).toBe(true);
    expect(persistsToolTab("nope", { board: "x" })).toBe(false);
  });

  it("opens a tab, and shows it again for the same params", () => {
    const a = openToolTab("c1", "metrics", { board: "db" });
    expect(useUiStore.getState().view).toBe("workspace");
    expect(state().tabs).toEqual([
      {
        id: a,
        kind: "tool",
        connectionId: "c1",
        tool: "metrics",
        title: "Metrics: db",
        params: { board: "db" },
      },
    ]);
    expect(openToolTab("c1", "metrics", { board: "db" })).toBe(a);
    const b = openToolTab("c1", "metrics", { board: "server" });
    expect(b).not.toBe(a);
    // Another connection gets its own.
    expect(openToolTab("c2", "metrics", { board: "db" })).not.toBe(a);
    // reuse "tool": any tab of the tool, its params replaced; reuse false: always a new one.
    expect(openToolTab("c1", "metrics", { board: "x" }, { reuse: "tool", title: "X" })).toBe(a);
    expect(state().tabs.find((t) => t.id === a)).toMatchObject({
      params: { board: "x" },
      title: "X",
    });
    expect(openToolTab("c1", "metrics", { board: "x" }, { reuse: false })).not.toBe(a);
    expect(state().activeId).not.toBe(b);
    expect(findToolTab(state().tabs, "c1", "metrics", { board: "server" })?.id).toBe(b);
  });

  it("renders the registered component; its actions only while focused", () => {
    const tab = state().tabs.find(() => false) ?? null;
    expect(tab).toBeNull();
    const id = openToolTab("c1", "metrics", { board: "db" });
    const t = state().tabs.find((x) => x.id === id) as ToolTab;
    const { rerender } = render(<TabBody tab={t} />);
    expect(screen.getByText("Metrics: db / db / focused")).toBeInTheDocument();
    expect(actions.has("view.refresh")).toBe(true);
    act(() => rerender(<TabBody tab={t} focused={false} />));
    expect(screen.getByText("Metrics: db / db / unfocused")).toBeInTheDocument();
    expect(actions.has("view.refresh")).toBe(false);
    const missing: ToolTab = { ...t, tool: "gone" };
    act(() => rerender(<TabBody tab={missing} />));
    expect(screen.getByText(/cannot be shown/)).toBeInTheDocument();
  });
});
