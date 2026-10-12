import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { ConnectionInfo } from "@/lib/ipc";
import { installActiveSync } from "./activeSync";
import { useConnectionsStore, type OpenConnection } from "./connections";
import { useTabsStore } from "./tabs";
import { useTableTabsStore } from "./tableTabs";

const users = { schema: "public", name: "users" };
const posts = { schema: "public", name: "posts" };

function info(connectionId: string): ConnectionInfo {
  return {
    connectionId,
    profileId: `p-${connectionId}`,
    name: connectionId,
    color: null,
    driver: "postgres",
    serverVersion: "17",
    currentDatabase: "postgres",
    currentUser: "postgres",
    tunnel: null,
    readOnly: false,
  };
}

const tabs = () => useTabsStore.getState();
const conns = () => useConnectionsStore.getState();

describe("installActiveSync", () => {
  let uninstall: () => void = () => {};

  beforeEach(() => {
    useTabsStore.setState({
      tabs: [],
      activeId: null,
      closed: [],
      queryCounter: 0,
      closeRequest: null,
    });
    useTableTabsStore.setState({ slices: {} });
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    uninstall = installActiveSync();
    // Two open connections; B was opened last, so it is active.
    conns().add(info("A"));
    conns().add(info("B"));
  });
  afterEach(() => uninstall());

  it("activating another connection's tab makes that connection active", () => {
    conns().setActive("A");
    const a = tabs().openTable("A", users);
    const b = tabs().openTable("B", posts);
    expect(conns().activeId).toBe("B");
    tabs().activate(a);
    expect(conns().activeId).toBe("A");
    tabs().next();
    expect(tabs().activeId).toBe(b);
    expect(conns().activeId).toBe("B");
  });

  it("switching connections restores that connection's last tab", () => {
    conns().setActive("A");
    tabs().openTable("A", users);
    const a2 = tabs().newQuery("A");
    const b = tabs().openTable("B", posts);
    expect(conns().activeId).toBe("B");
    conns().setActive("A");
    expect(tabs().activeId).toBe(a2);
    conns().setActive("B");
    expect(tabs().activeId).toBe(b);
  });

  it("falls back to the connection's first tab when its last one was closed", () => {
    conns().setActive("A");
    const a1 = tabs().openTable("A", users);
    const a2 = tabs().newQuery("A");
    tabs().openTable("B", posts);
    tabs().close(a2);
    conns().setActive("A");
    expect(tabs().activeId).toBe(a1);
  });

  it("a connection without tabs clears the active tab", () => {
    tabs().openTable("B", posts);
    conns().setActive("A");
    expect(tabs().activeId).toBeNull();
    expect(conns().activeId).toBe("A");
    // A fresh connect (add makes it active) shows its empty workspace too.
    conns().setActive("B");
    expect(tabs().activeId).not.toBeNull();
    conns().add(info("C"));
    expect(tabs().activeId).toBeNull();
  });

  it("closing A's last tab while B has tabs leaves no tab active and A active", () => {
    conns().setActive("A");
    const a = tabs().openTable("A", users);
    tabs().openTable("B", posts);
    tabs().activate(a);
    expect(conns().activeId).toBe("A");
    tabs().close(a);
    expect(tabs().activeId).toBeNull();
    expect(conns().activeId).toBe("A");
  });

  it("Close Others on another connection's tab shows it and switches to its connection", () => {
    conns().setActive("A");
    const a = tabs().openTable("A", users);
    const b = tabs().openTable("B", posts);
    tabs().activate(a);
    expect(conns().activeId).toBe("A");
    tabs().closeOthers(b);
    expect(tabs().tabs.map((t) => t.id)).toEqual([b]);
    expect(tabs().activeId).toBe(b);
    expect(conns().activeId).toBe("B");
  });

  it("follows tabs moved onto a reconnected connection", () => {
    conns().setActive("A");
    tabs().openTable("A", users);
    const a2 = tabs().newQuery("A");
    tabs().openTable("B", posts);
    tabs().retargetConnection("A", "A2");
    useConnectionsStore.setState((s) => {
      const byId: Record<string, OpenConnection> = {
        ...s.byId,
        A2: { ...s.byId.A!, info: info("A2") },
      };
      delete byId.A;
      return { byId, order: s.order.map((id) => (id === "A" ? "A2" : id)) };
    });
    conns().setActive("A2");
    expect(tabs().activeId).toBe(a2);
  });

  it("ignores tabs of a connection that is not open", () => {
    tabs().open({ id: "x", kind: "query", connectionId: "gone", title: "Query" });
    expect(tabs().activeId).toBe("x");
    expect(conns().activeId).toBe("B");
  });

  it("does not loop", () => {
    conns().setActive("A");
    const a = tabs().openTable("A", users);
    const b = tabs().openTable("B", posts);
    let tabUpdates = 0;
    let connUpdates = 0;
    const offTabs = useTabsStore.subscribe(() => tabUpdates++);
    const offConns = useConnectionsStore.subscribe(() => connUpdates++);
    tabs().activate(a);
    expect([tabUpdates, connUpdates]).toEqual([1, 1]);
    conns().setActive("B");
    expect(tabs().activeId).toBe(b);
    expect([tabUpdates, connUpdates]).toEqual([2, 2]);
    offTabs();
    offConns();
  });

  it("stops syncing once uninstalled", () => {
    const b = tabs().openTable("B", posts);
    uninstall();
    conns().setActive("A");
    expect(tabs().activeId).toBe(b);
  });
});
