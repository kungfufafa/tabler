import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emptyStaging, insertRow } from "@/features/staging/model";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";
import * as passwords from "@/features/connections/passwordPromptStore";
import { usePaletteStore } from "@/features/palette/store";
import { railOrder } from "@/features/tabs/workspaces";
import { profile } from "@/lib/ipc/mock/fixtures";
import { useUiStore } from "@/stores/ui";
import {
  connectWorkspace,
  openDatabaseWorkspace,
  switchDatabase,
  useDatabaseSwitchStore,
  workspaceOf,
} from "./switchDatabase";

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

describe("per-database workspaces", () => {
  beforeEach(() => {
    useUiStore.setState({ railOrder: [], view: "workspace" });
    usePaletteStore.setState({ connecting: [] });
  });

  const opened = (connectionId: string, database: string): ConnectionInfo => ({
    ...info,
    connectionId,
    currentDatabase: database,
  });

  it("opens another database in a new workspace next to this one", async () => {
    const { qc, tab } = setup();
    useConnectionsStore.getState().add({ ...info, connectionId: "c2", profileId: "p2" });
    const connect = vi.spyOn(ipc.connections, "connect").mockResolvedValue(opened("c3", "shop"));
    await expect(openDatabaseWorkspace(qc, "c1", "shop")).resolves.toBe(true);
    expect(connect).toHaveBeenCalledWith("p1", null, "shop");
    const conns = useConnectionsStore.getState();
    expect(conns.activeId).toBe("c3");
    // This workspace keeps its database and its tabs.
    expect(conns.byId.c1!.info.currentDatabase).toBe("app");
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual([tab]);
    expect(railOrder()).toEqual(["c1", "c3", "c2"]);
    expect(workspaceOf("p1", "shop")).toBe("c3");
    expect(useDatabaseSwitchStore.getState().pending).toEqual({});

    // Open already: shown, not opened twice.
    useConnectionsStore.getState().setActive("c1");
    await expect(openDatabaseWorkspace(qc, "c1", "shop")).resolves.toBe(true);
    expect(useConnectionsStore.getState().activeId).toBe("c3");
    await expect(openDatabaseWorkspace(qc, "c3", "app")).resolves.toBe(true);
    expect(useConnectionsStore.getState().activeId).toBe("c1");
    expect(connect).toHaveBeenCalledOnce();
  });

  it("reports a database that cannot be opened", async () => {
    const { qc } = setup();
    vi.spyOn(ipc.connections, "connect").mockRejectedValue(new Error("no such database"));
    await expect(openDatabaseWorkspace(qc, "c1", "gone")).resolves.toBe(false);
    expect(toast.error).toHaveBeenCalledWith("Could not open gone", {
      description: "no such database",
    });
    expect(useConnectionsStore.getState().order).toEqual(["c1"]);
  });

  it("asks an 'Ask every time' profile's password only when the backend does not have it", async () => {
    const qc = new QueryClient();
    const asking = profile({ id: "p1", name: "Local PG", driver: "postgres" });
    qc.setQueryData(
      ["profiles"],
      [{ ...asking, options: { ...asking.options, passwordMode: "ask" } }],
    );
    const connect = vi
      .spyOn(ipc.connections, "connect")
      .mockRejectedValueOnce({ code: "AUTH_FAILED", message: "enter the password" })
      .mockResolvedValue(opened("c5", "app"));
    const asked = vi.spyOn(passwords, "askConnectionPassword").mockResolvedValue("secret");
    await expect(connectWorkspace(qc, "p1", "app")).resolves.toBe("c5");
    expect(connect.mock.calls).toEqual([
      ["p1", null, "app"],
      ["p1", "secret", "app"],
    ]);
    expect(asked).toHaveBeenCalledOnce();
    expect(usePaletteStore.getState().connecting).toEqual([]);
  });

  it("keeps a workspace's place in the rail when it switches database in place", async () => {
    const { qc } = setup();
    useConnectionsStore.getState().add({ ...info, connectionId: "c2", profileId: "p2" });
    useUiStore.setState({ railOrder: ["p2\u0000app", "p1\u0000app"] });
    expect(railOrder()).toEqual(["c2", "c1"]);
    vi.spyOn(ipc.connections, "switchDatabase").mockResolvedValue({
      ...info,
      currentDatabase: "shop",
    });
    await switchDatabase(qc, "c1", "shop");
    expect(useUiStore.getState().railOrder).toEqual(["p2\u0000app", "p1\u0000shop"]);
    expect(railOrder()).toEqual(["c2", "c1"]);
  });
});
