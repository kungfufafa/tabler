import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emptyStaging, insertRow } from "@/features/staging/model";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";
import { switchDatabase, useDatabaseSwitchStore } from "./switchDatabase";

const toast = vi.hoisted(() => ({ error: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

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

function setup() {
  useConnectionsStore.getState().add(info);
  useConnectionsStore.getState().setCurrentSchema("c1", "inventory");
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData(["rows", "c1", "public.orders"], { rows: [] });
  const tab = useTabsStore.getState().openTable("c1", { schema: "public", name: "orders" });
  return { qc, tab };
}

/** Each backend switch waits until the test settles it. */
function holdSwitch() {
  let resolve!: (info: ConnectionInfo) => void;
  const spy = vi.spyOn(ipc.connections, "switchDatabase").mockImplementation(
    () =>
      new Promise((res) => {
        resolve = res;
      }),
  );
  return { spy, resolve: (database: string) => resolve({ ...info, currentDatabase: database }) };
}

beforeEach(() => {
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useTableTabsStore.setState({ slices: {} });
  useDatabaseSwitchStore.setState({ pending: {} });
});

afterEach(() => {
  vi.restoreAllMocks();
  toast.error.mockReset();
});

describe("switchDatabase", () => {
  it("switches, drops the connection's tabs and everything cached for the old database", async () => {
    const { qc } = setup();
    const spy = vi
      .spyOn(ipc.connections, "switchDatabase")
      .mockResolvedValue({ ...info, currentDatabase: "shop" });
    await expect(switchDatabase(qc, "c1", "shop")).resolves.toBe(true);
    expect(spy).toHaveBeenCalledWith("c1", "shop");
    const conn = useConnectionsStore.getState().byId.c1!;
    expect(conn.info.currentDatabase).toBe("shop");
    // The new database picks its own default schema.
    expect(conn.currentSchema).toBeNull();
    expect(useTabsStore.getState().tabs).toEqual([]);
    expect(qc.getQueryData(["rows", "c1", "public.orders"])).toBeUndefined();
    expect(useDatabaseSwitchStore.getState().pending).toEqual({});
  });

  it("does nothing for the current database", async () => {
    const { qc, tab } = setup();
    const spy = vi.spyOn(ipc.connections, "switchDatabase");
    await expect(switchDatabase(qc, "c1", "app")).resolves.toBe(true);
    expect(spy).not.toHaveBeenCalled();
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual([tab]);
  });

  it("asks before dropping unsaved work, and keeps everything on cancel", async () => {
    const { qc, tab } = setup();
    useTableTabsStore.getState().update(tab, { staging: insertRow(emptyStaging(), "n", []) });
    const spy = vi
      .spyOn(ipc.connections, "switchDatabase")
      .mockResolvedValue({ ...info, currentDatabase: "shop" });

    const cancelled = switchDatabase(qc, "c1", "shop");
    expect(useTabsStore.getState().closeRequest).toMatchObject({ kind: "tabs", total: 1 });
    useTabsStore.getState().cancelClose();
    await expect(cancelled).resolves.toBe(false);
    expect(spy).not.toHaveBeenCalled();
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual([tab]);

    const confirmed = switchDatabase(qc, "c1", "shop");
    useTabsStore.getState().confirmClose();
    await expect(confirmed).resolves.toBe(true);
    expect(spy).toHaveBeenCalledOnce();
    expect(useConnectionsStore.getState().byId.c1!.info.currentDatabase).toBe("shop");
  });

  it("reports a failed switch and leaves the connection as it was", async () => {
    const { qc } = setup();
    vi.spyOn(ipc.connections, "switchDatabase").mockRejectedValue(new Error("no such database"));
    await expect(switchDatabase(qc, "c1", "gone")).resolves.toBe(false);
    expect(toast.error).toHaveBeenCalledWith("Could not switch database", {
      description: "no such database",
    });
    const conn = useConnectionsStore.getState().byId.c1!;
    expect(conn.info.currentDatabase).toBe("app");
    expect(conn.currentSchema).toBe("inventory");
    expect(useDatabaseSwitchStore.getState().pending).toEqual({});
  });

  it("runs one switch at a time per connection", async () => {
    const { qc } = setup();
    const held = holdSwitch();
    const first = switchDatabase(qc, "c1", "shop");
    await vi.waitFor(() => expect(held.spy).toHaveBeenCalledOnce());
    expect(useDatabaseSwitchStore.getState().pending).toEqual({ c1: "shop" });
    await expect(switchDatabase(qc, "c1", "sys")).resolves.toBe(false);
    held.resolve("shop");
    await expect(first).resolves.toBe(true);
    expect(held.spy).toHaveBeenCalledOnce();
    expect(useDatabaseSwitchStore.getState().pending).toEqual({});
  });

  it("ignores connections that are gone", async () => {
    const spy = vi.spyOn(ipc.connections, "switchDatabase");
    await expect(switchDatabase(new QueryClient(), "c9", "shop")).resolves.toBe(false);
    expect(spy).not.toHaveBeenCalled();
  });
});
