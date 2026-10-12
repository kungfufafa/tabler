import { beforeEach, describe, expect, it, vi } from "vitest";
import { emptyStaging, insertRow } from "@/features/staging/model";
import { useStructureStore } from "@/features/structure/store";
import type { DdlOp } from "@/lib/ipc";
import {
  canGoBack,
  canGoForward,
  dirtyTabs,
  maxQueryNumber,
  pickNeighbour,
  registerUnsavedSource,
  setWindowCloseHooks,
  unsavedCount,
  useTabsStore,
  workspaceTabs,
  type Tab,
} from "./tabs";
import { useTableTabsStore } from "./tableTabs";

const users = { schema: "public", name: "users" };
const posts = { schema: "public", name: "posts" };
const orders = { schema: "public", name: "orders" };

const ids = () => useTabsStore.getState().tabs.map((t) => t.id);
const state = () => useTabsStore.getState();
const tabOf = (id: string) => state().tabs.find((t) => t.id === id)!;
const previewOf = (id: string) => {
  const tab = tabOf(id);
  return tab.kind === "query" ? undefined : tab.preview;
};

/** Stages `n` DDL ops (table renames, enough for counting) on a table's tab. */
function stageDdl(tabId: string, n: number) {
  for (let i = 0; i < n; i++)
    useStructureStore
      .getState()
      .stage(tabId, (ops): DdlOp[] => [
        ...ops,
        { op: "renameTable", table: users, newName: `u${i}` },
      ]);
}

/** Stages `n` inserted rows on a table tab. */
function stage(tabId: string, n: number) {
  let staging = emptyStaging();
  for (let i = 0; i < n; i++) staging = insertRow(staging, `new:${i}`, []);
  useTableTabsStore.getState().update(tabId, { staging });
}

describe("tabs store", () => {
  beforeEach(() => {
    useTabsStore.setState({
      tabs: [],
      activeId: null,
      closed: [],
      queryCounter: 0,
      closeRequest: null,
      history: [],
      historyIndex: -1,
    });
    useTableTabsStore.setState({ slices: {} });
    useStructureStore.setState({ byTab: {} });
  });

  it("keeps one tab per table and connection, switching its view in place", () => {
    const s = state();
    const a = s.openTable("c1", users);
    const b = s.openTable("c1", users);
    const c = s.openTable("c1", users, "structure");
    const d = s.openTable("c2", users);
    expect(a).toBe(b);
    expect(c).toBe(a);
    expect(tabOf(a).kind).toBe("structure");
    expect(d).not.toBe(a);
    expect(state().tabs).toHaveLength(2);
    expect(state().activeId).toBe(d);
    expect(s.openTable("c1", users)).toBe(a);
    expect(tabOf(a).kind).toBe("table");
    expect(state().activeId).toBe(a);
  });

  it("switches a table's tab between data and structure, keeping both views' work", () => {
    const s = state();
    const a = s.openTable("c1", users);
    const q = s.newQuery("c1");
    stage(a, 2);
    s.setView(a, "structure");
    expect(tabOf(a)).toMatchObject({ id: a, kind: "structure", title: "users" });
    expect(ids()).toEqual([a, q]);
    // Switching does not activate the tab.
    expect(state().activeId).toBe(q);
    stageDdl(a, 1);
    s.setView(a, "table");
    expect(tabOf(a).kind).toBe("table");
    expect(useTableTabsStore.getState().slices[a]?.staging.changes.size).toBe(2);
    expect(useStructureStore.getState().byTab[a]?.ops).toHaveLength(1);
    // Same view, query tabs and unknown ids leave the state alone.
    const before = state().tabs;
    s.setView(a, "table");
    s.setView(q, "structure");
    s.setView("nope", "structure");
    expect(state().tabs).toBe(before);
    expect(tabOf(q).kind).toBe("query");
  });

  describe("preview tabs", () => {
    it("replaces the connection's preview tab in place", () => {
      const s = state();
      const q = s.newQuery("c1");
      const a = s.openTable("c1", users, "table", { preview: true });
      const n = s.newQuery("c1");
      expect(previewOf(a)).toBe(true);
      const b = s.openTable("c1", posts, "table", { preview: true });
      expect(b).not.toBe(a);
      expect(ids()).toEqual([q, b, n]);
      expect(tabOf(b)).toMatchObject({ kind: "table", title: "posts", preview: true });
      expect(state().activeId).toBe(b);
      // A replaced preview is not a closed tab (⌘⇧T does not bring it back).
      expect(state().closed).toHaveLength(0);
      // Another connection gets its own preview tab.
      const c = s.openTable("c2", users, "table", { preview: true });
      expect(ids()).toEqual([q, b, n, c]);
    });

    it("drops the replaced preview tab's per-tab state", () => {
      const s = state();
      const a = s.openTable("c1", users, "table", { preview: true });
      useTableTabsStore.getState().update(a, { staging: emptyStaging() });
      s.openTable("c1", posts, "table", { preview: true });
      expect(useTableTabsStore.getState().slices[a]).toBeUndefined();
    });

    it("pins a preview tab that holds unsaved work instead of replacing it", () => {
      const s = state();
      const a = s.openTable("c1", users, "table", { preview: true });
      stage(a, 1);
      const b = s.openTable("c1", posts, "table", { preview: true });
      expect(ids()).toEqual([a, b]);
      expect(previewOf(a)).toBeUndefined();
      expect(previewOf(b)).toBe(true);

      // Staged DDL counts too.
      stageDdl(b, 1);
      const c = s.openTable("c1", orders, "table", { preview: true });
      expect(ids()).toEqual([a, b, c]);
      expect(previewOf(b)).toBeUndefined();
    });

    it("keeps a table's open tab: a preview open activates it, a regular open pins it", () => {
      const s = state();
      const a = s.openTable("c1", users);
      const p = s.openTable("c1", posts, "table", { preview: true });
      expect(s.openTable("c1", users, "table", { preview: true })).toBe(a);
      expect(previewOf(a)).toBeUndefined();
      expect(ids()).toEqual([a, p]);
      // Clicking the preview's own table again keeps it a preview…
      expect(s.openTable("c1", posts, "table", { preview: true })).toBe(p);
      expect(previewOf(p)).toBe(true);
      // …a double-click (regular open) or the structure view from the menu pins it.
      expect(s.openTable("c1", posts, "structure")).toBe(p);
      expect(tabOf(p)).toMatchObject({ kind: "structure" });
      expect(previewOf(p)).toBeUndefined();
    });

    it("pin turns a preview tab into a regular one", () => {
      const s = state();
      const a = s.openTable("c1", users, "table", { preview: true });
      const q = s.newQuery("c1");
      s.pin(a);
      expect(tabOf(a)).not.toHaveProperty("preview");
      const b = s.openTable("c1", posts, "table", { preview: true });
      expect(ids()).toEqual([a, q, b]);
      // Pinning a regular or query tab changes nothing.
      const before = state().tabs;
      s.pin(a);
      s.pin(q);
      expect(state().tabs).toBe(before);
    });

    it("reopens a closed preview tab as a regular one", () => {
      const s = state();
      const a = s.openTable("c1", users, "table", { preview: true });
      s.close(a);
      expect(state().closed[0]?.id).toBe(a);
      s.reopenClosed();
      expect(ids()).toEqual([a]);
      expect(tabOf(a)).not.toHaveProperty("preview");
      // So the next preview open does not replace it.
      const b = s.openTable("c1", posts, "table", { preview: true });
      expect(ids()).toEqual([a, b]);
    });
  });

  it("adds registered unsaved sources (e.g. staged result edits) to a tab's count", () => {
    const s = state();
    const q = s.newQuery("c1");
    expect(unsavedCount(tabOf(q))).toBe(0);
    const remove = registerUnsavedSource((tab) => (tab.id === q ? 3 : 0));
    expect(unsavedCount(tabOf(q))).toBe(3);
    expect(dirtyTabs(state().tabs)).toEqual([{ id: q, kind: "query", title: "Query 1", count: 3 }]);
    remove();
    expect(unsavedCount(tabOf(q))).toBe(0);
  });

  it("counts staged rows and staged DDL of a table's tab together, in either view", () => {
    const s = state();
    const a = s.openTable("c1", users);
    const q = s.newQuery("c1");
    stage(a, 2);
    stageDdl(a, 3);
    expect(unsavedCount(tabOf(a))).toBe(5);
    s.setView(a, "structure");
    expect(unsavedCount(tabOf(a))).toBe(5);
    expect(unsavedCount(tabOf(q))).toBe(0);
    s.setDirty(q, true);
    expect(dirtyTabs(state().tabs)).toEqual([
      { id: a, kind: "structure", title: "users", count: 5 },
      { id: q, kind: "query", title: "Query 1", count: 1 },
    ]);
    s.closeAll();
    expect(state().closeRequest?.total).toBe(6);
    state().confirmClose();
    expect(useTableTabsStore.getState().slices[a]).toBeUndefined();
    expect(useStructureStore.getState().byTab[a]).toBeUndefined();
  });

  it("activates the right-hand neighbour when closing the active tab, else the left", () => {
    const s = state();
    const a = s.openTable("c1", users);
    const b = s.openTable("c1", posts);
    const q = s.newQuery("c1");
    s.activate(b);
    s.close(b);
    expect(state().activeId).toBe(q);
    s.close(q);
    expect(state().activeId).toBe(a);
    s.reopenClosed();
    expect(ids()).toEqual([a, q]);
    expect(state().activeId).toBe(q);
  });

  it("keeps the active tab when closing an inactive one", () => {
    const s = state();
    const a = s.openTable("c1", users);
    const b = s.openTable("c1", posts);
    s.activate(a);
    s.close(b);
    expect(state().activeId).toBe(a);
  });

  it("navigates and reorders", () => {
    const s = state();
    const a = s.openTable("c1", users);
    const b = s.openTable("c1", posts);
    const c = s.newQuery("c1");
    s.activateIndex(0);
    expect(state().activeId).toBe(a);
    s.prev();
    expect(state().activeId).toBe(c);
    s.next();
    expect(state().activeId).toBe(a);
    s.activateIndex(8);
    expect(state().activeId).toBe(c);
    s.move(0, 2);
    expect(ids()).toEqual([b, c, a]);
    expect(state().tabs[1]?.title).toBe("Query 1");
  });

  describe("pickNeighbour", () => {
    const tabs = ["a", "b", "c", "d", "e"].map((id): Tab => ({
      id,
      kind: "query",
      connectionId: "c1",
      title: id,
    }));
    it("prefers the nearest surviving tab to the right", () => {
      expect(pickNeighbour(tabs, "b", new Set(["b", "c"]))).toBe("d");
    });
    it("falls back to the left when nothing survives on the right", () => {
      expect(pickNeighbour(tabs, "d", new Set(["d", "e"]))).toBe("c");
    });
    it("keeps a surviving active tab and returns null when everything closes", () => {
      expect(pickNeighbour(tabs, "a", new Set(["b"]))).toBe("a");
      expect(pickNeighbour(tabs, "a", new Set(tabs.map((t) => t.id)))).toBeNull();
    });
    it("returns null when nothing was active", () => {
      expect(pickNeighbour(tabs, null, new Set(["b"]))).toBeNull();
      expect(pickNeighbour(tabs, "gone", new Set(["b"]))).toBeNull();
    });

    describe("with several connections", () => {
      const mixed = (
        [
          ["a1", "A"],
          ["b1", "B"],
          ["a2", "A"],
          ["b2", "B"],
          ["a3", "A"],
        ] as const
      ).map(([id, connectionId]): Tab => ({ id, kind: "query", connectionId, title: id }));
      it("prefers a surviving tab of the same connection, right then left", () => {
        expect(pickNeighbour(mixed, "a2", new Set(["a2"]))).toBe("a3");
        expect(pickNeighbour(mixed, "a3", new Set(["a3"]))).toBe("a2");
        expect(pickNeighbour(mixed, "a2", new Set(["a2", "a3"]))).toBe("a1");
        expect(pickNeighbour(mixed, "b1", new Set(["b1"]))).toBe("b2");
      });
      it("never jumps to another connection's tab", () => {
        expect(pickNeighbour(mixed, "a2", new Set(["a1", "a2", "a3"]))).toBeNull();
        expect(pickNeighbour(mixed, "b2", new Set(["b1", "b2"]))).toBeNull();
      });
    });
  });

  describe("close guard", () => {
    it("closes clean tabs immediately", () => {
      const s = state();
      const a = s.openTable("c1", users);
      expect(s.requestClose([a])).toBe(true);
      expect(state().tabs).toHaveLength(0);
      expect(state().closeRequest).toBeNull();
    });

    it("asks before closing a table tab with pending edits and discards them on confirm", () => {
      const s = state();
      const a = s.openTable("c1", users);
      stage(a, 3);
      expect(s.requestClose([a])).toBe(false);
      expect(ids()).toEqual([a]);
      expect(state().closeRequest).toMatchObject({
        kind: "tabs",
        ids: [a],
        total: 3,
        dirty: [{ id: a, kind: "table", title: "users", count: 3 }],
      });
      state().confirmClose();
      expect(state().tabs).toHaveLength(0);
      expect(state().closeRequest).toBeNull();
      expect(useTableTabsStore.getState().slices[a]).toBeUndefined();
    });

    it("asks before closing a structure tab with staged DDL and drops it on confirm", () => {
      const s = state();
      const a = s.openTable("c1", users, "structure");
      stageDdl(a, 2);
      expect(s.requestClose([a])).toBe(false);
      expect(state().closeRequest).toMatchObject({
        total: 2,
        dirty: [{ id: a, kind: "structure", title: "users", count: 2 }],
      });
      expect(s.requestWindowClose()).toBe(false);
      state().cancelClose();
      state().forceClose([a]);
      expect(useStructureStore.getState().byTab[a]).toBeUndefined();
    });

    it("keeps everything on cancel", () => {
      const s = state();
      const q = s.newQuery("c1");
      s.setDirty(q, true);
      s.close(q);
      expect(state().closeRequest?.total).toBe(1);
      state().cancelClose();
      expect(ids()).toEqual([q]);
      expect(state().closeRequest).toBeNull();
    });

    it("guards close-all and sums the pending count across tabs", () => {
      const s = state();
      const a = s.openTable("c1", users);
      const b = s.openTable("c1", posts);
      const q = s.newQuery("c1");
      stage(a, 2);
      s.setDirty(q, true);
      s.closeAll();
      const req = state().closeRequest!;
      expect(req.ids).toEqual([a, b, q]);
      expect(req.dirty.map((d) => d.id)).toEqual([a, q]);
      expect(req.total).toBe(3);
      // A second request while the dialog is up is ignored.
      expect(state().requestClose([b])).toBe(false);
      expect(state().closeRequest).toBe(req);
      state().confirmClose();
      expect(state().tabs).toHaveLength(0);
      expect(state().activeId).toBeNull();
    });

    it("closes others and tabs to the right, guarding only when needed", () => {
      const s = state();
      const a = s.openTable("c1", users);
      const b = s.openTable("c1", posts);
      const c = s.openTable("c1", orders);
      s.newQuery("c1");
      s.activate(a);
      s.closeToRight(b);
      expect(ids()).toEqual([a, b]);
      expect(state().activeId).toBe(a);

      const c2 = s.openTable("c1", orders);
      expect(c2).not.toBe(c);
      stage(c2, 1);
      s.activate(a);
      s.closeOthers(b);
      expect(state().closeRequest?.ids).toEqual([a, c2]);
      state().confirmClose();
      expect(ids()).toEqual([b]);
      expect(state().activeId).toBe(b);
    });

    it("Close Others closes only the kept tab's workspace and shows the kept tab", () => {
      const s = state();
      const a1 = s.openTable("A", users);
      const b1 = s.openTable("B", posts);
      const b2 = s.newQuery("B");
      // Right-clicking b1 does not activate it.
      s.activate(a1);
      s.closeOthers(b1);
      expect(ids()).toEqual([a1, b1]);
      expect(ids()).not.toContain(b2);
      expect(state().activeId).toBe(b1);
    });

    it("Close Others asks about the workspace's unsaved tabs, then shows the kept tab", () => {
      const s = state();
      const a1 = s.openTable("A", users);
      const a2 = s.openTable("A", orders);
      const b1 = s.openTable("B", posts);
      stage(a2, 1);
      s.activate(b1);
      s.closeOthers(a1);
      expect(state().closeRequest).toMatchObject({ ids: [a2], activateAfter: a1 });
      expect(state().activeId).toBe(b1);
      state().confirmClose();
      expect(ids()).toEqual([a1, b1]);
      expect(state().activeId).toBe(a1);
    });

    it("Close Others leaves the active tab alone when the dialog is cancelled", () => {
      const s = state();
      const a1 = s.openTable("A", users);
      const a2 = s.openTable("A", orders);
      stage(a2, 1);
      s.activate(a2);
      s.closeOthers(a1);
      state().cancelClose();
      expect(ids()).toEqual([a1, a2]);
      expect(state().activeId).toBe(a2);
      // A later, unrelated close does not inherit the kept tab.
      stage(a2, 0);
      expect(state().requestClose([a2])).toBe(true);
      expect(state().activeId).toBe(a1);
    });

    it("keeps tab navigation and Close All inside one workspace", () => {
      const s = state();
      const a1 = s.openTable("A", users);
      const a2 = s.openTable("A", orders);
      const b1 = s.openTable("B", posts);
      s.activate(a2);
      state().next();
      expect(state().activeId).toBe(a1);
      state().prev();
      expect(state().activeId).toBe(a2);
      state().activateIndex(0, "B");
      expect(state().activeId).toBe(b1);
      state().activateIndex(8, "A");
      expect(state().activeId).toBe(a2);
      state().closeAll("A");
      expect(ids()).toEqual([b1]);
      // Without a workspace (nothing scoped) the tab actions see every tab.
      expect(workspaceTabs(state().tabs, null).map((t) => t.id)).toEqual([b1]);
    });

    it("guards the window only when something is unsaved", () => {
      const s = state();
      const a = s.openTable("c1", users);
      expect(s.requestWindowClose()).toBe(true);
      stage(a, 1);
      expect(s.requestWindowClose()).toBe(false);
      expect(state().closeRequest).toMatchObject({ kind: "window", total: 1 });
      // Confirming a window close does not touch the tabs; the window goes away instead.
      expect(state().confirmClose()?.kind).toBe("window");
      expect(ids()).toEqual([a]);
    });
  });

  it("closing the last tab of a connection leaves no tab active, even if others remain", () => {
    const s = state();
    const a = s.openTable("c1", users);
    const b = s.openTable("c2", posts);
    s.activate(a);
    s.close(a);
    expect(ids()).toEqual([b]);
    expect(state().activeId).toBeNull();
  });

  it("clears the active tab", () => {
    const s = state();
    const a = s.openTable("c1", users);
    s.clearActive();
    expect(state().activeId).toBeNull();
    expect(ids()).toEqual([a]);
    const before = state();
    s.clearActive();
    expect(state()).toBe(before);
  });

  it("tracks the dirty flag on query tabs only", () => {
    const s = state();
    const a = s.openTable("c1", users);
    const q = s.newQuery("c1");
    s.setDirty(q, true);
    s.setDirty(a, true);
    expect(state().tabs.find((t) => t.id === q)).toMatchObject({ dirty: true });
    expect(state().tabs.find((t) => t.id === a)).not.toHaveProperty("dirty");
    const before = state().tabs;
    s.setDirty(q, true);
    expect(state().tabs).toBe(before);
  });

  it("duplicates query tabs next to the source", () => {
    const s = state();
    const q1 = s.newQuery("c1");
    const a = s.openTable("c1", users);
    const q2 = s.duplicate(q1)!;
    expect(ids()).toEqual([q1, q2, a]);
    expect(state().activeId).toBe(q2);
    expect(state().tabs[1]).toMatchObject({ kind: "query", connectionId: "c1", title: "Query 2" });
    expect(s.duplicate(a)).toBeNull();
  });

  it("retargets open and recently closed tabs to a new connection id", () => {
    const s = state();
    const a = s.openTable("old", users);
    const b = s.openTable("other", posts);
    const q = s.newQuery("old");
    stage(a, 2);
    s.close(q);
    s.retargetConnection("old", "new");
    expect(state().tabs.map((t) => t.connectionId)).toEqual(["new", "other"]);
    expect(state().closed.map((t) => t.connectionId)).toEqual(["new"]);
    // Tab ids are stable, so per-tab state (staged edits, filters) survives the reconnect.
    expect(ids()).toEqual([a, b]);
    expect(useTableTabsStore.getState().slices[a]?.staging.changes.size).toBe(2);
  });

  it("drops a connection's tabs, history and per-tab state without asking", () => {
    const s = state();
    const a = s.openTable("c1", users);
    const b = s.openTable("c2", posts);
    const q = s.newQuery("c1");
    s.close(q);
    const st = s.openTable("c1", orders, "structure");
    stage(a, 1);
    stageDdl(st, 1);
    s.activate(a);
    s.closeForConnection("c1");
    expect(ids()).toEqual([b]);
    // c2's tab is not picked here: the connection switch (activeSync) restores c2's own last tab.
    expect(state().activeId).toBeNull();
    expect(state().closed).toHaveLength(0);
    expect(useTableTabsStore.getState().slices[a]).toBeUndefined();
    expect(useStructureStore.getState().byTab[st]).toBeUndefined();
  });

  describe("back / forward history", () => {
    it("walks back and forward through previously active tabs", () => {
      const s = state();
      const a = s.openTable("c1", users);
      const b = s.openTable("c1", posts);
      const q = s.newQuery("c1");
      s.activate(a);
      expect(state().history).toEqual([a, b, q, a]);
      expect(canGoForward(state())).toBe(false);
      s.back();
      expect(state().activeId).toBe(q);
      s.back();
      expect(state().activeId).toBe(b);
      expect(canGoForward(state())).toBe(true);
      s.forward();
      expect(state().activeId).toBe(q);
      s.forward();
      expect(state().activeId).toBe(a);
      expect(canGoForward(state())).toBe(false);
      // Nothing further either way is a no-op.
      s.forward();
      expect(state().activeId).toBe(a);
    });

    it("drops the forward entries on a new activation, like a browser", () => {
      const s = state();
      const a = s.openTable("c1", users);
      const b = s.openTable("c1", posts);
      const q = s.newQuery("c1");
      s.back();
      s.back();
      expect(state().activeId).toBe(a);
      s.activate(q);
      expect(state().history).toEqual([a, q]);
      expect(canGoForward(state())).toBe(false);
      s.back();
      expect(state().activeId).toBe(a);
      expect(ids()).toContain(b);
    });

    it("records ⌘[ / ⌘] / ⌘1…9 and does not repeat the visible tab", () => {
      const s = state();
      const a = s.openTable("c1", users);
      const b = s.openTable("c1", posts);
      s.activate(b);
      s.openTable("c1", posts);
      expect(state().history).toEqual([a, b]);
      s.prev();
      s.activateIndex(1);
      expect(state().history).toEqual([a, b, a, b]);
    });

    it("skips closed tabs and leaves no trace of them", () => {
      const s = state();
      const a = s.openTable("c1", users);
      const b = s.openTable("c1", posts);
      const q = s.newQuery("c1");
      s.activate(a);
      s.close(q);
      expect(state().history).toEqual([a, b, a]);
      s.back();
      expect(state().activeId).toBe(b);
      s.back();
      expect(state().activeId).toBe(a);
      expect(canGoBack(state())).toBe(false);
    });

    it("goes back to the last tab after its connection's last tab closed", () => {
      const s = state();
      const a = s.openTable("c1", users);
      const b = s.openTable("c2", posts);
      s.close(b);
      expect(state().activeId).toBeNull();
      expect(canGoBack(state())).toBe(true);
      s.back();
      expect(state().activeId).toBe(a);
      expect(canGoBack(state())).toBe(false);
    });

    it("keeps at most 50 entries", () => {
      const s = state();
      const a = s.openTable("c1", users);
      const b = s.openTable("c1", posts);
      for (let i = 0; i < 40; i++) {
        s.activate(a);
        s.activate(b);
      }
      expect(state().history).toHaveLength(50);
      expect(state().historyIndex).toBe(49);
      expect(state().history.at(-1)).toBe(b);
    });
  });
  describe("rename and restore", () => {
    it("renames with a trimmed title and ignores blank names", () => {
      const s = state();
      const q = s.newQuery("c1");
      s.rename(q, "  Daily report ");
      expect(tabOf(q).title).toBe("Daily report");
      s.rename(q, "   ");
      expect(tabOf(q).title).toBe("Daily report");
      const t = s.openTable("c1", users);
      s.rename(t, "People");
      expect(tabOf(t).title).toBe("People");
    });

    it("appends restored tabs without activating them and continues the query numbering", () => {
      const s = state();
      const a = s.openTable("c1", users);
      s.restoreTabs([
        { id: "r1", kind: "query", connectionId: "c2", title: "Query 12" },
        { id: "r2", kind: "structure", connectionId: "c2", table: posts, title: "posts" },
        { id: a, kind: "table", connectionId: "c1", table: users, title: "dup" },
      ]);
      expect(ids()).toEqual([a, "r1", "r2"]);
      expect(state().activeId).toBe(a);
      expect(tabOf(a).title).toBe("users");
      s.newQuery("c2");
      expect(state().tabs.at(-1)?.title).toBe("Query 13");
      expect(
        maxQueryNumber([
          { kind: "query", title: "Query 4" },
          { kind: "table", title: "Query 9" },
        ]),
      ).toBe(4);
    });

    it("lets the session keep auto-saved query tabs out of the window-close prompt", () => {
      const s = state();
      const q = s.newQuery("c1");
      const f = s.newQuery("c1");
      s.setDirty(q, true);
      const save = vi.fn();
      const closeAfterSave = vi.fn();
      const uninstall = setWindowCloseHooks({
        survivesClose: (tab) => tab.id === q,
        save,
        closeAfterSave,
      });
      expect(s.requestWindowClose()).toBe(false);
      expect(closeAfterSave).toHaveBeenCalledTimes(1);
      expect(state().closeRequest).toBeNull();
      s.setDirty(f, true);
      expect(s.requestWindowClose()).toBe(false);
      expect(save).toHaveBeenCalledTimes(1);
      expect(state().closeRequest?.dirty.map((d) => d.id)).toEqual([f]);
      s.cancelClose();
      uninstall();
      expect(s.requestWindowClose()).toBe(false);
      expect(state().closeRequest?.dirty.map((d) => d.id)).toEqual([q, f]);
    });
  });
});
