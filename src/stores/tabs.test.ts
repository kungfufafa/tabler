import { beforeEach, describe, expect, it } from "vitest";
import { emptyStaging, insertRow } from "@/features/staging/model";
import { pickNeighbour, useTabsStore, type Tab } from "./tabs";
import { useTableTabsStore } from "./tableTabs";

const users = { schema: "public", name: "users" };
const posts = { schema: "public", name: "posts" };
const orders = { schema: "public", name: "orders" };

const ids = () => useTabsStore.getState().tabs.map((t) => t.id);
const state = () => useTabsStore.getState();

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
    });
    useTableTabsStore.setState({ slices: {} });
  });

  it("dedupes table tabs per connection and kind", () => {
    const s = state();
    const a = s.openTable("c1", users);
    const b = s.openTable("c1", users);
    const c = s.openTable("c1", users, "structure");
    const d = s.openTable("c2", users);
    expect(a).toBe(b);
    expect(c).not.toBe(a);
    expect(d).not.toBe(a);
    expect(state().tabs).toHaveLength(3);
    expect(state().activeId).toBe(d);
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
    stage(a, 1);
    s.activate(a);
    s.closeForConnection("c1");
    expect(ids()).toEqual([b]);
    expect(state().activeId).toBe(b);
    expect(state().closed).toHaveLength(0);
    expect(useTableTabsStore.getState().slices[a]).toBeUndefined();
  });
});
