import { beforeEach, describe, expect, it } from "vitest";
import { useTabsStore } from "./tabs";

const users = { schema: "public", name: "users" };
const posts = { schema: "public", name: "posts" };

describe("tabs store", () => {
  beforeEach(() =>
    useTabsStore.setState({ tabs: [], activeId: null, closed: [], queryCounter: 0 }),
  );

  it("dedupes table tabs per connection and kind", () => {
    const s = useTabsStore.getState();
    const a = s.openTable("c1", users);
    const b = s.openTable("c1", users);
    const c = s.openTable("c1", users, "structure");
    const d = s.openTable("c2", users);
    expect(a).toBe(b);
    expect(c).not.toBe(a);
    expect(d).not.toBe(a);
    expect(useTabsStore.getState().tabs).toHaveLength(3);
    expect(useTabsStore.getState().activeId).toBe(d);
  });

  it("activates the neighbour when closing the active tab and can reopen it", () => {
    const s = useTabsStore.getState();
    const a = s.openTable("c1", users);
    const b = s.openTable("c1", posts);
    const q = s.newQuery("c1");
    s.activate(b);
    s.close(b);
    expect(useTabsStore.getState().activeId).toBe(q);
    s.close(q);
    expect(useTabsStore.getState().activeId).toBe(a);
    s.reopenClosed();
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual([a, q]);
    expect(useTabsStore.getState().activeId).toBe(q);
  });

  it("navigates and reorders", () => {
    const s = useTabsStore.getState();
    const a = s.openTable("c1", users);
    const b = s.openTable("c1", posts);
    const c = s.newQuery("c1");
    s.activateIndex(0);
    expect(useTabsStore.getState().activeId).toBe(a);
    s.prev();
    expect(useTabsStore.getState().activeId).toBe(c);
    s.next();
    expect(useTabsStore.getState().activeId).toBe(a);
    s.activateIndex(8);
    expect(useTabsStore.getState().activeId).toBe(c);
    s.move(0, 2);
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual([b, c, a]);
    expect(useTabsStore.getState().tabs[1]?.title).toBe("Query 1");
  });
});
