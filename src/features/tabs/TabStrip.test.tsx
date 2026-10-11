import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import { emptyStaging, insertRow } from "@/features/staging/model";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useUiStore } from "@/stores/ui";
import { TabStrip } from "./TabStrip";
import { wheelToScrollLeft } from "./wheel";

const users = { schema: "public", name: "users" };
const posts = { schema: "public", name: "posts" };

function stage(tabId: string, n: number) {
  let staging = emptyStaging();
  for (let i = 0; i < n; i++) staging = insertRow(staging, `new:${i}`, []);
  useTableTabsStore.getState().update(tabId, { staging });
}

const tabEl = (name: RegExp) => screen.getByRole("tab", { name });
const ids = () => useTabsStore.getState().tabs.map((t) => t.id);

describe("TabStrip", () => {
  const scrollIntoView = vi.fn();
  const originalScrollIntoView = Element.prototype.scrollIntoView;
  let unregister: Array<() => void> = [];
  afterAll(() => {
    Element.prototype.scrollIntoView = originalScrollIntoView;
  });

  beforeEach(() => {
    useTabsStore.setState({
      tabs: [],
      activeId: null,
      closed: [],
      queryCounter: 0,
      closeRequest: null,
    });
    useTableTabsStore.setState({ slices: {} });
    useConnectionsStore.setState({ byId: {}, order: [], activeId: "c1" });
    useUiStore.setState({ view: "workspace" });
    Element.prototype.scrollIntoView = scrollIntoView;
    scrollIntoView.mockClear();
  });
  afterEach(() => {
    unregister.forEach((u) => u());
    unregister = [];
  });

  it("marks tabs with pending edits or a dirty buffer", () => {
    const s = useTabsStore.getState();
    const a = s.openTable("c1", users);
    s.openTable("c1", posts);
    const q = s.newQuery("c1");
    stage(a, 2);
    s.setDirty(q, true);
    render(<TabStrip />);
    expect(tabEl(/^public\.users \(2 pending changes\)$/)).toBeInTheDocument();
    expect(tabEl(/^Query 1 \(Unsaved changes\)$/)).toBeInTheDocument();
    expect(tabEl(/^public\.posts$/)).toBeInTheDocument();
    expect(document.querySelectorAll("[data-slot=tab-dirty-dot]")).toHaveLength(2);
  });

  it("scrolls the active tab into view when it changes", () => {
    const s = useTabsStore.getState();
    const a = s.openTable("c1", users);
    s.openTable("c1", posts);
    render(<TabStrip />);
    scrollIntoView.mockClear();
    act(() => useTabsStore.getState().activate(a));
    expect(scrollIntoView).toHaveBeenCalledWith({ inline: "nearest", block: "nearest" });
    expect(scrollIntoView.mock.contexts.at(-1)).toBe(tabEl(/users/));
  });

  it("asks before closing a tab with pending edits; Cancel keeps it, Discard closes it", () => {
    const a = useTabsStore.getState().openTable("c1", users);
    stage(a, 2);
    render(<TabStrip />);
    fireEvent.click(screen.getByRole("button", { name: "Close users" }));
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText("Discard 2 pending changes?")).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(ids()).toEqual([a]);

    fireEvent.click(screen.getByRole("button", { name: "Close users" }));
    fireEvent.click(screen.getByRole("button", { name: /Discard/ }));
    expect(ids()).toEqual([]);
    expect(useTableTabsStore.getState().slices[a]).toBeUndefined();
  });

  it("Review… focuses the dirty table tab and opens the commit preview", async () => {
    // "grid.preview", not "grid.commit": with "Confirm before commit" off, ⌘S applies right away.
    const commit = vi.fn();
    const preview = vi.fn();
    unregister.push(actions.register("grid.commit", commit));
    unregister.push(actions.register("grid.preview", preview));
    const s = useTabsStore.getState();
    const a = s.openTable("c1", users);
    s.newQuery("c1");
    stage(a, 1);
    render(<TabStrip />);
    act(() => useTabsStore.getState().closeAll());
    fireEvent.click(screen.getByRole("button", { name: "Review…" }));
    expect(useTabsStore.getState().activeId).toBe(a);
    expect(useTabsStore.getState().closeRequest).toBeNull();
    expect(ids()).toHaveLength(2);
    await act(() => new Promise((r) => setTimeout(r, 0)));
    expect(preview).toHaveBeenCalledTimes(1);
    expect(commit).not.toHaveBeenCalled();
  });

  it("closes other tabs from the context menu", () => {
    const s = useTabsStore.getState();
    s.openTable("c1", users);
    const b = s.openTable("c1", posts);
    s.newQuery("c1");
    render(<TabStrip />);
    fireEvent.contextMenu(tabEl(/posts/));
    fireEvent.click(screen.getByRole("menuitem", { name: "Close Others" }));
    expect(ids()).toEqual([b]);
    expect(useTabsStore.getState().activeId).toBe(b);
  });

  it("offers Duplicate only on query tabs", () => {
    const s = useTabsStore.getState();
    s.openTable("c1", users);
    const q = s.newQuery("c1");
    render(<TabStrip />);
    fireEvent.contextMenu(tabEl(/users/));
    expect(screen.queryByRole("menuitem", { name: "Duplicate" })).not.toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    fireEvent.contextMenu(tabEl(/Query 1/));
    fireEvent.click(screen.getByRole("menuitem", { name: "Duplicate" }));
    expect(ids()).toHaveLength(3);
    expect(ids()[2]).not.toBe(q);
  });

  it("opens a new query on double-click in the empty strip, even with no tabs", () => {
    const newQuery = vi.fn();
    unregister.push(actions.register("tab.new-query", newQuery));
    render(<TabStrip />);
    const filler = document.querySelector("[data-slot=tabstrip-filler]")!;
    fireEvent.mouseDown(filler, { button: 0, detail: 1 });
    expect(newQuery).not.toHaveBeenCalled();
    fireEvent.mouseDown(filler, { button: 0, detail: 2 });
    expect(newQuery).toHaveBeenCalledTimes(1);
  });

  it("closes a tab on middle-click", () => {
    const s = useTabsStore.getState();
    s.openTable("c1", users);
    const b = s.openTable("c1", posts);
    render(<TabStrip />);
    fireEvent.pointerDown(tabEl(/users/), { button: 1 });
    expect(ids()).toEqual([b]);
  });

  it("keeps the (hover-only) close buttons out of the Tab order", async () => {
    const s = useTabsStore.getState();
    const a = s.openTable("c1", users);
    s.openTable("c1", posts);
    render(
      <>
        <button type="button">Connections</button>
        <TabStrip />
        <button type="button">Search</button>
      </>,
    );
    const user = userEvent.setup();
    screen.getByRole("button", { name: "Connections" }).focus();
    await user.tab();
    expect(screen.getByRole("button", { name: "Search" })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(ids()).toContain(a);
    expect(screen.getByRole("button", { name: "Close users" })).toHaveAttribute("tabindex", "-1");
  });

  it("scrolls horizontally with a vertical mouse wheel", () => {
    useTabsStore.getState().openTable("c1", users);
    render(<TabStrip />);
    const list = screen.getByRole("tablist");
    fireEvent.wheel(list, { deltaY: 40 });
    expect(list.scrollLeft).toBe(40);
  });
});

describe("wheelToScrollLeft", () => {
  it("converts vertical wheel deltas and leaves horizontal gestures alone", () => {
    expect(wheelToScrollLeft({ deltaX: 0, deltaY: 30, deltaMode: 0 }, 500)).toBe(30);
    expect(wheelToScrollLeft({ deltaX: 0, deltaY: -3, deltaMode: 1 }, 500)).toBe(-48);
    expect(wheelToScrollLeft({ deltaX: 0, deltaY: 1, deltaMode: 2 }, 500)).toBe(500);
    expect(wheelToScrollLeft({ deltaX: 20, deltaY: 5, deltaMode: 0 }, 500)).toBe(0);
  });
});
