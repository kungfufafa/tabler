import { beforeEach, describe, expect, it, vi } from "vitest";
import { emptyStaging, insertRow } from "@/features/staging/model";
import type { ConnectionInfo } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";
import { closeConnectionTabs } from "./closeGuard";

function info(connectionId: string): ConnectionInfo {
  return {
    connectionId,
    profileId: `p-${connectionId}`,
    name: connectionId,
    color: null,
    driver: "postgres",
    serverVersion: "PostgreSQL 17",
    currentDatabase: "app",
    currentUser: "me",
    tunnel: null,
    readOnly: false,
  };
}

function setup() {
  useConnectionsStore.getState().add(info("c1"));
  useConnectionsStore.getState().add(info("c2"));
  const tabs = useTabsStore.getState();
  const orders = tabs.openTable("c1", { schema: "public", name: "orders" });
  const other = tabs.openTable("c2", { schema: "public", name: "items" });
  return { orders, other };
}

const stage = (tabId: string) =>
  useTableTabsStore.getState().update(tabId, { staging: insertRow(emptyStaging(), "n", []) });
const tabIds = () => useTabsStore.getState().tabs.map((t) => t.id);

beforeEach(() => {
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useTableTabsStore.setState({ slices: {} });
});

describe("closeConnectionTabs", () => {
  it("closes the connection's clean tabs and proceeds right away", async () => {
    const { other } = setup();
    const proceed = vi.fn();
    await expect(closeConnectionTabs("c1", proceed)).resolves.toBe(true);
    expect(proceed).toHaveBeenCalledOnce();
    expect(tabIds()).toEqual([other]);
  });

  it("proceeds once the user discards", async () => {
    const { orders, other } = setup();
    stage(orders);
    const proceed = vi.fn();
    const result = closeConnectionTabs("c1", proceed);
    expect(proceed).not.toHaveBeenCalled();
    useTabsStore.getState().confirmClose();
    await expect(result).resolves.toBe(true);
    expect(proceed).toHaveBeenCalledOnce();
    expect(tabIds()).toEqual([other]);
  });

  it("skips `proceed` when the connection went away while the prompt was open", async () => {
    const { orders, other } = setup();
    stage(orders);
    const proceed = vi.fn();
    const result = closeConnectionTabs("c1", proceed);
    // The connection dropped (e.g. lost and removed) before the user answered.
    useTabsStore.getState().closeForConnection("c1");
    useConnectionsStore.getState().remove("c1");
    await expect(result).resolves.toBe(false);
    expect(proceed).not.toHaveBeenCalled();
    expect(tabIds()).toEqual([other]);
  });
});
