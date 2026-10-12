import { beforeEach, describe, expect, it } from "vitest";
import type { ConnectionInfo } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import { cycleWorkspace, stepWorkspace, workspaceEntries } from "./workspaces";

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
