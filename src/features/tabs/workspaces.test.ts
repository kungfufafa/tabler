import { beforeEach, describe, expect, it } from "vitest";
import type { ConnectionInfo } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import {
  cycleWorkspace,
  moveWorkspace,
  railOrder,
  reorderWorkspaces,
  sortWorkspaces,
  stepWorkspace,
  workspaceEntries,
  workspaceInitials,
} from "./workspaces";

const info = (id: string, name: string, db: string | null = null): ConnectionInfo => ({
  connectionId: id,
  profileId: `p-${id}`,
  name,
  color: null,
  driver: "postgres",
  serverVersion: "17",
  currentDatabase: db,
  currentUser: null,
  tunnel: null,
  readOnly: false,
});

describe("stepWorkspace", () => {
  it("wraps around the rail order", () => {
    const order = ["a", "b", "c"];
    expect(stepWorkspace(order, "a", 1)).toBe("b");
    expect(stepWorkspace(order, "c", 1)).toBe("a");
    expect(stepWorkspace(order, "a", -1)).toBe("c");
    expect(stepWorkspace(order, null, 1)).toBe("a");
    expect(stepWorkspace(order, null, -1)).toBe("c");
    expect(stepWorkspace(["a"], "a", 1)).toBeNull();
    expect(stepWorkspace([], null, 1)).toBeNull();
  });
});

describe("workspaces", () => {
  beforeEach(() => {
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    const s = useConnectionsStore.getState();
    s.add(info("a", "Local Postgres", "tabler_dev"));
    s.add(info("b", "Shop MySQL", "shop"));
    s.setActive("a");
    useUiStore.setState({ view: "workspace" });
  });

  it("selects the next / previous workspace, and comes back from the connection manager", () => {
    cycleWorkspace(1);
    expect(useConnectionsStore.getState().activeId).toBe("b");
    cycleWorkspace(1);
    expect(useConnectionsStore.getState().activeId).toBe("a");
    cycleWorkspace(-1);
    expect(useConnectionsStore.getState().activeId).toBe("b");
    useUiStore.setState({ view: "launcher" });
    cycleWorkspace(1);
    expect(useConnectionsStore.getState().activeId).toBe("b");
    expect(useUiStore.getState().view).toBe("workspace");
  });

  it("lists the open workspaces with their database and tab count, filtered", () => {
    const { order, byId } = useConnectionsStore.getState();
    const counts = new Map([["b", 3]]);
    expect(workspaceEntries(order, byId, counts)).toEqual([
      {
        connectionId: "a",
        name: "Local Postgres",
        color: null,
        driver: "postgres",
        database: "tabler_dev",
        tabCount: 0,
      },
      {
        connectionId: "b",
        name: "Shop MySQL",
        color: null,
        driver: "postgres",
        database: "shop",
        tabCount: 3,
      },
    ]);
    expect(workspaceEntries(order, byId, counts, "SHOP").map((e) => e.connectionId)).toEqual(["b"]);
    expect(workspaceEntries(order, byId, counts, "local dev").map((e) => e.connectionId)).toEqual([
      "a",
    ]);
  });
});

describe("rail order", () => {
  const conn = (id: string, profileId: string, db: string | null) => ({
    info: { ...info(id, id, db), profileId },
    status: "connected" as const,
    message: null,
    currentSchema: null,
  });
  const byId = {
    a: conn("a", "p1", "app"),
    b: conn("b", "p1", "shop"),
    c: conn("c", "p2", "logs"),
  };
  const key = (profileId: string, db: string) => `${profileId}\u0000${db}`;

  it("puts dragged workspaces first, by profile and database, then the rest as opened", () => {
    expect(sortWorkspaces(["a", "b", "c"], byId, [])).toEqual(["a", "b", "c"]);
    expect(sortWorkspaces(["a", "b", "c"], byId, [key("p2", "logs"), key("p1", "shop")])).toEqual([
      "c",
      "b",
      "a",
    ]);
  });

  it("keeps the dragged order, and the saved keys of closed workspaces after it", () => {
    const saved = [key("p9", "gone")];
    expect(reorderWorkspaces(["a", "b", "c"], byId, saved, 2, 0)).toEqual([
      key("p2", "logs"),
      key("p1", "app"),
      key("p1", "shop"),
      key("p9", "gone"),
    ]);
    expect(reorderWorkspaces(["a"], byId, saved, 0, 5)).toEqual(saved);
  });

  it("drags the live rail and cycles in that order", () => {
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    useUiStore.setState({ railOrder: [] });
    const s = useConnectionsStore.getState();
    s.add(info("x", "First", "one"));
    s.add(info("y", "Second", "two"));
    moveWorkspace(1, 0);
    expect(railOrder()).toEqual(["y", "x"]);
    s.setActive("y");
    useUiStore.setState({ view: "workspace" });
    cycleWorkspace(1);
    expect(useConnectionsStore.getState().activeId).toBe("x");
  });

  it("names a workspace's icon after its database", () => {
    expect(workspaceInitials("tabler_dev")).toBe("TD");
    expect(workspaceInitials("ShopOrders")).toBe("SO");
    expect(workspaceInitials("shop")).toBe("SH");
    expect(workspaceInitials("/Users/me/data/sample.sqlite")).toBe("SA");
    expect(workspaceInitials("my-app.db")).toBe("MA");
    expect(workspaceInitials("--")).toBe("?");
  });
});
