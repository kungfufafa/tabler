import { QueryClient, QueryClientProvider, QueryObserver } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useConnectionDialog } from "@/features/connections/store";
import { useSettingsDialog } from "@/features/settings/store";
import { insertRow, emptyStaging } from "@/features/staging/model";
import { ipc, type ConnectionInfo, type ConnectionProfile } from "@/lib/ipc";
import { profile } from "@/lib/ipc/mock/fixtures";
import { useConnectionsStore } from "@/stores/connections";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import {
  disconnectGuarded,
  PALETTE_ONLY_ACTIONS,
  paletteCommandItems,
  reloadSchema,
  usePaletteCommands,
} from "./commands";
import type { ActionId } from "./actions";
import { usePaletteStore } from "./store";

const toast = vi.hoisted(() => ({
  loading: vi.fn(),
  dismiss: vi.fn(),
  success: vi.fn(),
  error: vi.fn(),
}));
vi.mock("sonner", () => ({ toast }));

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (v: T) => void;
  reject: (e: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function info(connectionId: string, p: ConnectionProfile): ConnectionInfo {
  return {
    connectionId,
    profileId: p.id,
    name: p.name,
    color: p.color,
    driver: p.driver,
    serverVersion: "PostgreSQL 17",
    currentDatabase: "app",
    currentUser: "me",
    tunnel: null,
    readOnly: false,
  };
}

const p1 = profile({ id: "p1", name: "Local PG", driver: "postgres" });
const p2 = profile({ id: "p2", name: "Shop MySQL", driver: "mysql" });

function renderCommands() {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  qc.setQueryData(["profiles"], [p1, p2]);
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  );
  const { result } = renderHook(() => usePaletteCommands(), { wrapper });
  return { qc, commands: () => result.current };
}

/** Each `ipc.connections.connect` call waits until the test settles it. */
function holdConnects() {
  const calls: Array<{ profileId: string } & Deferred<ConnectionInfo>> = [];
  vi.spyOn(ipc.connections, "connect").mockImplementation((profileId) => {
    const d = deferred<ConnectionInfo>();
    calls.push({ profileId, ...d });
    return d.promise;
  });
  return calls;
}

function stageEdits(tabId: string, n: number) {
  let staging = emptyStaging();
  for (let i = 0; i < n; i++) staging = insertRow(staging, `new:${i}`, []);
  useTableTabsStore.getState().update(tabId, { staging });
}

beforeEach(() => {
  let nextToast = 0;
  toast.loading.mockImplementation(() => ++nextToast);
  usePaletteStore.setState({ open: false, session: 0, connecting: [] });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useTableTabsStore.setState({ slices: {} });
  useUiStore.setState({ view: "launcher" });
});

afterEach(() => {
  vi.restoreAllMocks();
  Object.values(toast).forEach((fn) => fn.mockReset());
});

describe("palette connect", () => {
  it("dismisses every 'Connecting…' toast when connects overlap", async () => {
    const calls = holdConnects();
    const { commands } = renderCommands();
    let first!: Promise<void>;
    let second!: Promise<void>;
    act(() => {
      first = commands().connect(p1);
      second = commands().connect(p2);
    });
    await waitFor(() => expect(calls).toHaveLength(2));
    expect(toast.loading).toHaveBeenCalledTimes(2);
    expect(usePaletteStore.getState().connecting).toEqual(["p1", "p2"]);

    calls[0]!.resolve(info("c1", p1));
    calls[1]!.reject(new Error("connection refused"));
    await act(async () => {
      await first;
      await second;
    });
    expect(toast.dismiss.mock.calls.map((c) => c[0]).sort()).toEqual([1, 2]);
    // The failure is reported once, by useConnect.
    expect(toast.error).toHaveBeenCalledWith("Could not connect to Shop MySQL", {
      description: "connection refused",
    });
    expect(usePaletteStore.getState().connecting).toEqual([]);
    expect(useConnectionsStore.getState().order).toEqual(["c1"]);
  });

  it("ignores a repeat selection of a profile that is still connecting", async () => {
    const calls = holdConnects();
    const { commands } = renderCommands();
    let first!: Promise<void>;
    act(() => {
      first = commands().connect(p1);
      void commands().connect(p1);
    });
    await waitFor(() => expect(calls).toHaveLength(1));
    calls[0]!.resolve(info("c1", p1));
    await act(async () => {
      await first;
    });
    expect(ipc.connections.connect).toHaveBeenCalledTimes(1);
    expect(toast.loading).toHaveBeenCalledTimes(1);
    const { byId, order } = useConnectionsStore.getState();
    expect(order.map((id) => byId[id]!.info.profileId)).toEqual(["p1"]);

    // Once settled it can be connected again (e.g. after a failure).
    expect(usePaletteStore.getState().beginConnect("p1")).toBe(true);
  });
});

describe("reloadSchema", () => {
  /** An observed query that succeeds once, then fails like a lost connection. */
  function observe(qc: QueryClient, queryKey: readonly unknown[]) {
    let calls = 0;
    const observer = new QueryObserver(qc, {
      queryKey,
      queryFn: () => {
        calls++;
        if (calls === 1) return Promise.resolve(["public"]);
        return Promise.reject(new Error("connection lost"));
      },
    });
    const unsubscribe = observer.subscribe(() => {});
    return { unsubscribe, calls: () => calls };
  }

  it("rejects when a visible query fails to refetch", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const schemas = observe(qc, ["schemas", "c1"]);
    await waitFor(() => expect(qc.getQueryState(["schemas", "c1"])?.status).toBe("success"));
    await expect(reloadSchema(qc, "c1")).rejects.toThrow("connection lost");
    expect(schemas.calls()).toBe(2);
    schemas.unsubscribe();
  });

  it("rejects when the backend cache could not be dropped", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    vi.spyOn(ipc.schema, "invalidate").mockRejectedValue(new Error("NOT_CONNECTED"));
    await expect(reloadSchema(qc, "c1")).rejects.toThrow("NOT_CONNECTED");
  });

  it("reports failure instead of 'Schema reloaded' from the palette", async () => {
    const { qc, commands } = renderCommands();
    const schemas = observe(qc, ["schemas", "c1"]);
    await waitFor(() => expect(qc.getQueryState(["schemas", "c1"])?.status).toBe("success"));
    await act(() => commands().reloadSchema("c1"));
    expect(toast.success).not.toHaveBeenCalled();
    expect(toast.error).toHaveBeenCalledWith("Could not reload schema", {
      description: "connection lost",
    });
    schemas.unsubscribe();
  });

  it("confirms a successful reload", async () => {
    const { commands } = renderCommands();
    await act(() => commands().reloadSchema("c1"));
    expect(toast.success).toHaveBeenCalledWith("Schema reloaded");
    expect(toast.error).not.toHaveBeenCalled();
  });
});

describe("disconnectGuarded", () => {
  function setup() {
    useConnectionsStore.getState().add(info("c1", p1));
    useConnectionsStore.getState().add(info("c2", p2));
    const tabs = useTabsStore.getState();
    const orders = tabs.openTable("c1", { schema: "public", name: "orders" });
    const query = tabs.newQuery("c1");
    const other = tabs.openTable("c2", { schema: "shop", name: "items" });
    return { orders, query, other };
  }
  const tabIds = () => useTabsStore.getState().tabs.map((t) => t.id);

  it("disconnects right away when nothing is unsaved", async () => {
    const { other } = setup();
    const disconnect = vi.fn();
    await expect(disconnectGuarded("c1", disconnect)).resolves.toBe(true);
    expect(disconnect).toHaveBeenCalledWith("c1");
    expect(tabIds()).toEqual([other]);
  });

  it("asks before dropping staged edits and keeps the connection on cancel", async () => {
    const { orders, query, other } = setup();
    stageEdits(orders, 2);
    const disconnect = vi.fn();
    const result = disconnectGuarded("c1", disconnect);
    const request = useTabsStore.getState().closeRequest;
    expect(request).toMatchObject({ kind: "tabs", total: 2 });
    expect(disconnect).not.toHaveBeenCalled();

    useTabsStore.getState().cancelClose();
    await expect(result).resolves.toBe(false);
    expect(disconnect).not.toHaveBeenCalled();
    expect(tabIds()).toEqual([orders, query, other]);
    expect(useTableTabsStore.getState().slices[orders]?.staging.changes.size).toBe(2);
  });

  it("disconnects once the user discards the changes", async () => {
    const { orders, query, other } = setup();
    stageEdits(orders, 1);
    useTabsStore.getState().setDirty(query, true);
    const disconnect = vi.fn();
    const result = disconnectGuarded("c1", disconnect);
    expect(useTabsStore.getState().closeRequest?.dirty).toHaveLength(2);
    useTabsStore.getState().confirmClose();
    await expect(result).resolves.toBe(true);
    expect(disconnect).toHaveBeenCalledWith("c1");
    expect(tabIds()).toEqual([other]);
  });

  it("leaves the connection alone while another close prompt is pending", async () => {
    const { orders, other } = setup();
    stageEdits(orders, 1);
    useTableTabsStore.getState().update(other, { staging: insertRow(emptyStaging(), "n", []) });
    useTabsStore.getState().requestClose([other]);
    const disconnect = vi.fn();
    await expect(disconnectGuarded("c1", disconnect)).resolves.toBe(false);
    expect(disconnect).not.toHaveBeenCalled();
    expect(useTabsStore.getState().closeRequest?.ids).toEqual([other]);
  });
});

describe("palette navigation commands", () => {
  it("opens the New connection form in the connection manager", () => {
    useUiStore.setState({ view: "workspace" });
    useConnectionDialog.setState({ open: false, profile: p1 });
    const { commands } = renderCommands();
    act(() => commands().newConnection());
    expect(useUiStore.getState().view).toBe("launcher");
    expect(useConnectionDialog.getState()).toMatchObject({ open: true, profile: null });
  });

  it("opens tables in the preview tab or in a tab of their own", () => {
    useConnectionsStore.getState().add(info("c1", p1));
    const { commands } = renderCommands();
    const table = { schema: "public", name: "orders" };
    act(() => commands().openTable("c1", table, { preview: true }));
    act(() => commands().openTable("c1", { schema: "public", name: "users" }, { preview: true }));
    act(() => commands().openTable("c1", { schema: "public", name: "items" }));
    expect(
      useTabsStore
        .getState()
        .tabs.map((t) => [t.title, t.kind, t.kind !== "query" && t.preview === true]),
    ).toEqual([
      ["users", "table", true],
      ["items", "table", false],
    ]);
    expect(useUiStore.getState().view).toBe("workspace");
  });

  it("selects a schema and switches databases", async () => {
    useConnectionsStore.getState().add(info("c1", p1));
    const spy = vi
      .spyOn(ipc.connections, "switchDatabase")
      .mockResolvedValue({ ...info("c1", p1), currentDatabase: "shop" });
    const { commands } = renderCommands();
    act(() => commands().selectSchema("c1", "inventory"));
    expect(useConnectionsStore.getState().byId.c1!.currentSchema).toBe("inventory");
    expect(useUiStore.getState().view).toBe("workspace");

    useUiStore.setState({ view: "launcher" });
    await act(() => commands().switchDatabase("c1", "shop"));
    expect(spy).toHaveBeenCalledWith("c1", "shop");
    const conn = useConnectionsStore.getState().byId.c1!;
    expect(conn.info.currentDatabase).toBe("shop");
    expect(conn.currentSchema).toBeNull();
    expect(useUiStore.getState().view).toBe("workspace");
  });

  it("stays put when a database switch fails", async () => {
    useConnectionsStore.getState().add(info("c1", p1));
    vi.spyOn(ipc.connections, "switchDatabase").mockRejectedValue(new Error("denied"));
    const { commands } = renderCommands();
    await act(() => commands().switchDatabase("c1", "shop"));
    expect(toast.error).toHaveBeenCalledWith("Could not switch database", {
      description: "denied",
    });
    expect(useUiStore.getState().view).toBe("launcher");
  });

  it("opens Settings on the Shortcuts section", () => {
    useSettingsDialog.setState({ open: false, section: "general" });
    const { commands } = renderCommands();
    act(() => commands().openShortcuts());
    expect(useSettingsDialog.getState()).toMatchObject({ open: true, section: "shortcuts" });
  });
});

describe("palette-only actions", () => {
  const noop = () => undefined;
  const items = (has: (id: ActionId) => boolean, active: boolean) =>
    paletteCommandItems({
      themePref: "system",
      active: active ? { connectionId: "c1", name: "Local PG", schema: null } : null,
      commands: {
        setTheme: noop,
        newConnection: noop,
        openShortcuts: noop,
        disconnect: () => Promise.resolve(true),
        reloadSchema: () => Promise.resolve(),
        runAction: vi.fn(),
      },
      has,
    });

  it("lists Connection Manager (its key moved to New SQL Viewer) and Ask Assistant…", () => {
    expect(PALETTE_ONLY_ACTIONS).toContain("ui.launcher");
    expect(PALETTE_ONLY_ACTIONS).toContain("assistant.ask");
    const all = items(() => true, true);
    const manager = all.find((i) => i.key === "action:ui.launcher");
    expect(manager).toMatchObject({ label: "Connection Manager", group: "actions" });
    expect(manager?.detail).toBeUndefined();
    expect(manager?.keywords?.join(" ")).toMatch(/launcher/);
    const ask = all.find((i) => i.key === "action:assistant.ask");
    expect(ask).toMatchObject({ label: "Ask Assistant…", detail: "Local PG" });
    expect(ask?.keywords?.join(" ")).toMatch(/assistant/);
  });

  it("lists them only while they have a handler, and runs them through the registry", () => {
    expect(items((id) => id !== "assistant.ask", false).map((i) => i.key)).toContain(
      "action:ui.launcher",
    );
    const keys = items((id) => id !== "assistant.ask", false).map((i) => i.key);
    expect(keys).not.toContain("action:assistant.ask");
    expect(items(() => false, false).map((i) => i.key)).not.toContain("action:ui.launcher");
  });
});
