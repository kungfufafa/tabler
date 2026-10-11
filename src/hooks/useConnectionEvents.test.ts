import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc, type ConnStatus, type ConnectionInfo } from "@/lib/ipc";
import { useConnectionsStore, type OpenConnection } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import {
  applyStatus,
  bannerState,
  isReconnecting,
  PING_TIMEOUT_MS,
  pingWithDeadline,
  reconnectConnection,
  startKeepalive,
  statusFromPing,
  type KeepaliveOptions,
} from "./useConnectionEvents";

const toasts = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast: Object.assign(vi.fn(), toasts) }));

function info(connectionId: string, profileId = "p-local-pg"): ConnectionInfo {
  return {
    connectionId,
    profileId,
    name: "Local PG",
    color: null,
    driver: "postgres",
    serverVersion: "17",
    currentDatabase: "postgres",
    currentUser: "postgres",
    tunnel: null,
    readOnly: false,
  };
}

function open(status: ConnStatus, message: string | null = null): OpenConnection {
  return { info: info("c1"), status, message, currentSchema: null };
}

const testClient = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });

/**
 * A mounted query (like the open tab's grid) whose first fetch fails when `fails` is set; returns
 * how many times it has fetched.
 */
async function mountQuery(qc: QueryClient, queryKey: string[], fails: boolean) {
  let calls = 0;
  const observer = new QueryObserver(qc, {
    queryKey,
    queryFn: () => {
      calls++;
      return fails && calls === 1
        ? Promise.reject(new Error("connection lost"))
        : Promise.resolve([]);
    },
  });
  const unsubscribe = observer.subscribe(() => {});
  await vi.waitFor(() => expect(qc.getQueryState(queryKey)?.fetchStatus).toBe("idle"));
  return { calls: () => calls, unsubscribe };
}

function resetStores() {
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTabsStore.setState({
    tabs: [],
    activeId: null,
    closed: [],
    queryCounter: 0,
    closeRequest: null,
  });
}

describe("bannerState", () => {
  it("is hidden for healthy connections", () => {
    expect(bannerState(null, false)).toBeNull();
    expect(bannerState(open("connected"), false)).toBeNull();
    expect(bannerState(open("disconnected"), false)).toBeNull();
  });

  it("offers a reconnect for a lost connection", () => {
    expect(bannerState(open("lost", "server closed the connection"), false)).toEqual({
      title: "Connection to Local PG lost",
      message: "server closed the connection",
      spinning: false,
      canReconnect: true,
    });
  });

  it("spins while reconnecting and disables the button only for its own attempt", () => {
    expect(bannerState(open("reconnecting"), false)).toMatchObject({
      title: "Reconnecting to Local PG…",
      spinning: true,
      canReconnect: true,
    });
    expect(bannerState(open("lost"), true)).toMatchObject({ spinning: true, canReconnect: false });
  });
});

describe("statusFromPing", () => {
  it("maps failures and unknown sessions to lost", () => {
    expect(statusFromPing("connected", null)).toBe("lost");
    expect(statusFromPing("connected", "disconnected")).toBe("lost");
    expect(statusFromPing("connected", "lost")).toBe("lost");
  });
  it("reports recovery and ignores no-ops", () => {
    expect(statusFromPing("lost", "connected")).toBe("connected");
    expect(statusFromPing("connected", "connected")).toBeNull();
    expect(statusFromPing("lost", "lost")).toBeNull();
  });
  it("does not interrupt a reconnect in flight", () => {
    expect(statusFromPing("reconnecting", null)).toBeNull();
    expect(statusFromPing("reconnecting", "connected")).toBe("connected");
  });
});

describe("applyStatus", () => {
  beforeEach(() => {
    resetStores();
    vi.clearAllMocks();
    useConnectionsStore.getState().add(info("c1"));
  });

  it("toasts once on loss and once on recovery", () => {
    applyStatus("c1", "lost", "boom");
    applyStatus("c1", "lost", "boom");
    expect(useConnectionsStore.getState().byId.c1).toMatchObject({
      status: "lost",
      message: "boom",
    });
    expect(toasts.error).toHaveBeenCalledTimes(1);
    applyStatus("c1", "connected");
    expect(toasts.success).toHaveBeenCalledTimes(1);
  });

  it("ignores unknown connections", () => {
    applyStatus("nope", "lost");
    expect(toasts.error).not.toHaveBeenCalled();
  });

  it("refetches the connection's failed queries when it comes back", async () => {
    const qc = testClient();
    const failed = await mountQuery(qc, ["rows", "c1", "t"], true);
    const healthy = await mountQuery(qc, ["tables", "c1", "public"], false);
    const other = await mountQuery(qc, ["rows", "c2", "t"], true);
    expect(qc.getQueryState(["rows", "c1", "t"])?.status).toBe("error");

    applyStatus("c1", "lost", "boom", qc);
    applyStatus("c1", "connected", null, qc);
    await vi.waitFor(() => expect(qc.getQueryState(["rows", "c1", "t"])?.status).toBe("success"));
    expect(failed.calls()).toBe(2);
    // Healthy data and other connections are left alone.
    expect(healthy.calls()).toBe(1);
    expect(other.calls()).toBe(1);
    [failed, healthy, other].forEach((q) => q.unsubscribe());
  });
});

describe("pingWithDeadline", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("passes a prompt answer through and times out a silent server", async () => {
    await expect(pingWithDeadline(Promise.resolve("connected"))).resolves.toBe("connected");
    const silent = pingWithDeadline(new Promise<ConnStatus>(() => {}), 5_000);
    const settled = vi.fn();
    silent.catch(settled);
    await vi.advanceTimersByTimeAsync(4_999);
    expect(settled).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(settled).toHaveBeenCalledWith(expect.objectContaining({ code: "TIMEOUT" }));
  });
});

describe("startKeepalive", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function fakeDoc(initial: DocumentVisibilityState = "visible") {
    const target = new EventTarget();
    const doc = {
      visibilityState: initial,
      addEventListener: target.addEventListener.bind(target),
      removeEventListener: target.removeEventListener.bind(target),
    };
    return {
      doc: doc as unknown as NonNullable<KeepaliveOptions["doc"]>,
      set(v: DocumentVisibilityState) {
        doc.visibilityState = v;
        target.dispatchEvent(new Event("visibilitychange"));
      },
    };
  }

  function setup(ids = ["a", "b"], initial: DocumentVisibilityState = "visible") {
    const ping = vi.fn(async (_id: string): Promise<ConnStatus> => "connected");
    const onResult = vi.fn();
    const { doc, set } = fakeDoc(initial);
    const stop = startKeepalive({
      ping,
      connectionIds: () => ids,
      onResult,
      intervalMs: 60_000,
      doc,
    });
    return { ping, onResult, stop, setVisibility: set };
  }

  it("pings every open connection once per interval while visible", async () => {
    const { ping, onResult, stop } = setup();
    await vi.advanceTimersByTimeAsync(59_999);
    expect(ping).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(ping.mock.calls.map((c) => c[0])).toEqual(["a", "b"]);
    expect(onResult).toHaveBeenCalledWith("a", "connected");
    await vi.advanceTimersByTimeAsync(60_000);
    expect(ping).toHaveBeenCalledTimes(4);
    stop();
  });

  it("pauses while hidden and catches up when shown after a missed beat", async () => {
    const { ping, setVisibility, stop } = setup(["a"]);
    await vi.advanceTimersByTimeAsync(30_000);
    setVisibility("hidden");
    await vi.advanceTimersByTimeAsync(120_000);
    expect(ping).not.toHaveBeenCalled();
    setVisibility("visible");
    expect(ping).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(ping).toHaveBeenCalledTimes(2);
    stop();
  });

  it("does not ping early when the window comes back quickly", async () => {
    const { ping, setVisibility, stop } = setup(["a"]);
    await vi.advanceTimersByTimeAsync(10_000);
    setVisibility("hidden");
    await vi.advanceTimersByTimeAsync(10_000);
    setVisibility("visible");
    expect(ping).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(ping).toHaveBeenCalledTimes(1);
    stop();
  });

  it("starts paused when hidden", async () => {
    const { ping, stop } = setup(["a"], "hidden");
    await vi.advanceTimersByTimeAsync(180_000);
    expect(ping).not.toHaveBeenCalled();
    stop();
  });

  it("reports errors, skips connections whose ping is still in flight, and stops cleanly", async () => {
    let release: (s: ConnStatus) => void = () => {};
    const ping = vi.fn((id: string) =>
      id === "slow"
        ? new Promise<ConnStatus>((r) => (release = r))
        : Promise.reject(new Error("socket closed")),
    );
    const onResult = vi.fn();
    const { doc } = fakeDoc();
    const stop = startKeepalive({
      ping,
      connectionIds: () => ["slow", "bad"],
      onResult,
      intervalMs: 1_000,
      doc,
    });
    await vi.advanceTimersByTimeAsync(2_000);
    expect(ping.mock.calls.map((c) => c[0])).toEqual(["slow", "bad", "bad"]);
    expect(onResult).toHaveBeenCalledWith("bad", null, expect.any(Error));
    stop();
    release("connected");
    await vi.advanceTimersByTimeAsync(5_000);
    expect(onResult).not.toHaveBeenCalledWith("slow", "connected");
    expect(ping).toHaveBeenCalledTimes(3);
  });

  it("reports a ping that never answers (half-open socket) after the deadline", async () => {
    let release: (s: ConnStatus) => void = () => {};
    const ping = vi.fn(() => new Promise<ConnStatus>((r) => (release = r)));
    const onResult = vi.fn();
    const { doc } = fakeDoc();
    const stop = startKeepalive({
      ping,
      connectionIds: () => ["a"],
      onResult,
      intervalMs: 60_000,
      timeoutMs: 8_000,
      doc,
    });
    await vi.advanceTimersByTimeAsync(60_000 + 7_999);
    expect(ping).toHaveBeenCalledTimes(1);
    expect(onResult).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(onResult).toHaveBeenCalledTimes(1);
    expect(onResult).toHaveBeenCalledWith("a", null, expect.objectContaining({ code: "TIMEOUT" }));
    // The hung ping is not stacked with more pings on later beats…
    await vi.advanceTimersByTimeAsync(10 * 60_000);
    expect(ping).toHaveBeenCalledTimes(1);
    expect(onResult).toHaveBeenCalledTimes(1);
    // …and its late answer still counts (the network came back).
    release("connected");
    await vi.advanceTimersByTimeAsync(0);
    expect(onResult).toHaveBeenLastCalledWith("a", "connected");
    await vi.advanceTimersByTimeAsync(60_000);
    expect(ping).toHaveBeenCalledTimes(2);
    stop();
  });

  it("does not report a deadline that expires after stopping", async () => {
    const onResult = vi.fn();
    const { doc } = fakeDoc();
    const stop = startKeepalive({
      ping: () => new Promise<ConnStatus>(() => {}),
      connectionIds: () => ["a"],
      onResult,
      intervalMs: 1_000,
      timeoutMs: 8_000,
      doc,
    });
    await vi.advanceTimersByTimeAsync(1_000);
    stop();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(onResult).not.toHaveBeenCalled();
  });
});

describe("reconnectConnection", () => {
  beforeEach(() => {
    resetStores();
    vi.clearAllMocks();
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  /** A promise with its resolve/reject handles. */
  function deferred<T>() {
    let resolve!: (v: T) => void;
    let reject!: (e: unknown) => void;
    const promise = new Promise<T>((res, rej) => {
      resolve = res;
      reject = rej;
    });
    return { promise, resolve, reject };
  }

  /** A real (mock-backend) session on the profile's default database, switched to `database`. */
  async function lostSessionOn(database: string) {
    const live = await ipc.connections.connect("p-local-pg");
    const switched = await ipc.connections.switchDatabase(live.connectionId, database);
    const conns = useConnectionsStore.getState();
    conns.add(live);
    conns.setInfo(switched);
    conns.setCurrentSchema(live.connectionId, "public");
    const tab = useTabsStore.getState().openTable(live.connectionId, {
      schema: "public",
      name: "users",
    });
    // The server dropped the session.
    await ipc.connections.disconnect(live.connectionId);
    conns.setStatus(live.connectionId, "lost", "server closed the connection unexpectedly");
    return { oldId: live.connectionId, defaultDb: live.currentDatabase, tab };
  }

  it("keeps the database the user had switched to on the new session", async () => {
    const { oldId, defaultDb, tab } = await lostSessionOn("ecommerce_sales");
    expect(defaultDb).not.toBe("ecommerce_sales");

    await expect(reconnectConnection(oldId, new QueryClient())).resolves.toBe("reconnected");

    const after = useConnectionsStore.getState();
    const fresh = after.activeId!;
    expect(fresh).not.toBe(oldId);
    expect(after.byId[fresh]?.info.currentDatabase).toBe("ecommerce_sales");
    expect(after.byId[fresh]?.currentSchema).toBe("public");
    const backend = (await ipc.connections.list()).find((c) => c.connectionId === fresh);
    expect(backend?.currentDatabase).toBe("ecommerce_sales");
    expect(useTabsStore.getState().tabs.find((t) => t.id === tab)?.connectionId).toBe(fresh);
  });

  it("leaves the tabs alone when the database cannot be reopened", async () => {
    const { oldId, tab } = await lostSessionOn("ecommerce_sales");
    const before = new Set((await ipc.connections.list()).map((c) => c.connectionId));
    vi.spyOn(ipc.connections, "switchDatabase").mockRejectedValue({
      code: "QUERY_FAILED",
      message: 'database "ecommerce_sales" does not exist',
    });
    const disconnect = vi.spyOn(ipc.connections, "disconnect");

    await expect(reconnectConnection(oldId, new QueryClient())).resolves.toBe("failed");

    const after = useConnectionsStore.getState();
    expect(after.order).toEqual([oldId]);
    expect(after.byId[oldId]?.status).toBe("lost");
    expect(after.byId[oldId]?.message).toMatch(/Could not reopen database “ecommerce_sales”/);
    expect(useTabsStore.getState().tabs.find((t) => t.id === tab)?.connectionId).toBe(oldId);
    // The session opened on the default database is closed again.
    expect(disconnect).toHaveBeenCalledTimes(1);
    const leftover = (await ipc.connections.list()).filter((c) => !before.has(c.connectionId));
    expect(leftover).toEqual([]);
    expect(toasts.error).toHaveBeenCalledWith(
      "Could not reconnect to Local Postgres",
      expect.anything(),
    );
  });

  it("does not wait minutes on a ping that never answers", async () => {
    vi.useFakeTimers();
    useConnectionsStore.getState().add(info("dead"));
    useConnectionsStore.getState().setStatus("dead", "lost");
    vi.spyOn(ipc.connections, "ping").mockReturnValue(new Promise<ConnStatus>(() => {}));
    const connect = vi.spyOn(ipc.connections, "connect");

    const outcome = reconnectConnection("dead", new QueryClient());
    await vi.advanceTimersByTimeAsync(PING_TIMEOUT_MS - 1);
    expect(connect).not.toHaveBeenCalled();
    expect(useConnectionsStore.getState().byId.dead?.status).toBe("reconnecting");
    await vi.advanceTimersByTimeAsync(1_000);
    expect(connect).toHaveBeenCalledWith("p-local-pg");
    await expect(outcome).resolves.toBe("reconnected");
  });

  it("shares one attempt per connection", async () => {
    useConnectionsStore.getState().add(info("dead"));
    useConnectionsStore.getState().setStatus("dead", "lost");
    vi.spyOn(ipc.connections, "ping").mockResolvedValue("lost");
    const opened = deferred<ConnectionInfo>();
    const connect = vi.spyOn(ipc.connections, "connect").mockReturnValue(opened.promise);
    const qc = new QueryClient();

    const first = reconnectConnection("dead", qc);
    const second = reconnectConnection("dead", qc);
    expect(second).toBe(first);
    expect(isReconnecting("dead")).toBe(true);
    await vi.waitFor(() => expect(connect).toHaveBeenCalledTimes(1));
    opened.resolve(info("fresh"));
    await expect(first).resolves.toBe("reconnected");
    expect(isReconnecting("dead")).toBe(false);
    expect(connect).toHaveBeenCalledTimes(1);
    expect(useConnectionsStore.getState().order).toEqual(["fresh"]);
  });

  it("closes the new session instead of resurrecting a connection the user disconnected", async () => {
    useConnectionsStore.getState().add(info("dead"));
    useConnectionsStore.getState().setStatus("dead", "lost");
    useTabsStore.getState().newQuery("dead");
    vi.spyOn(ipc.connections, "ping").mockResolvedValue("lost");
    const opened = deferred<ConnectionInfo>();
    const connect = vi.spyOn(ipc.connections, "connect").mockReturnValue(opened.promise);
    const disconnect = vi.spyOn(ipc.connections, "disconnect").mockResolvedValue(undefined);

    const outcome = reconnectConnection("dead", new QueryClient());
    await vi.waitFor(() => expect(connect).toHaveBeenCalled());
    // What useDisconnect does.
    useTabsStore.getState().closeForConnection("dead");
    useConnectionsStore.getState().remove("dead");
    opened.resolve(info("fresh"));

    await expect(outcome).resolves.toBe("failed");
    expect(useConnectionsStore.getState()).toMatchObject({ order: [], activeId: null, byId: {} });
    expect(disconnect).toHaveBeenCalledWith("fresh");
    expect(toasts.success).not.toHaveBeenCalled();
    expect(toasts.error).not.toHaveBeenCalled();
  });

  it("refetches what failed while the connection was down when the old session answers", async () => {
    const live = await ipc.connections.connect("p-local-pg");
    useConnectionsStore.getState().add(live);
    useConnectionsStore.getState().setStatus(live.connectionId, "lost", "timeout");
    const qc = testClient();
    const grid = await mountQuery(qc, ["rows", live.connectionId, "public.users"], true);
    await expect(reconnectConnection(live.connectionId, qc)).resolves.toBe("alive");
    await vi.waitFor(() => expect(grid.calls()).toBe(2));
    expect(qc.getQueryState(["rows", live.connectionId, "public.users"])?.status).toBe("success");
    grid.unsubscribe();
  });

  it("only flips the status back when the old session answers a ping", async () => {
    const live = await ipc.connections.connect("p-local-pg");
    useConnectionsStore.getState().add(live);
    useConnectionsStore.getState().setStatus(live.connectionId, "lost", "timeout");
    const qc = new QueryClient();
    await expect(reconnectConnection(live.connectionId, qc)).resolves.toBe("alive");
    expect(useConnectionsStore.getState().byId[live.connectionId]?.status).toBe("connected");
    expect(toasts.success).toHaveBeenCalledWith(`Reconnected to ${live.name}`, expect.anything());
  });

  it("opens a fresh connection and moves tabs, focus, order and schema onto it", async () => {
    const conns = useConnectionsStore.getState();
    conns.add(info("dead"));
    conns.add(info("other", "p-shop-mysql"));
    conns.setActive("dead");
    conns.setCurrentSchema("dead", "inventory");
    conns.setStatus("dead", "lost", "broken pipe");
    const tabs = useTabsStore.getState();
    const t1 = tabs.openTable("dead", { schema: "inventory", name: "items" });
    const t2 = tabs.newQuery("other");
    const qc = new QueryClient();
    qc.setQueryData(["tables", "dead", "inventory"], []);
    qc.setQueryData(["tables", "other", "public"], []);

    await expect(reconnectConnection("dead", qc)).resolves.toBe("reconnected");

    const after = useConnectionsStore.getState();
    const fresh = after.order[0]!;
    expect(fresh).not.toBe("dead");
    expect(after.order).toEqual([fresh, "other"]);
    expect(after.activeId).toBe(fresh);
    expect(after.byId.dead).toBeUndefined();
    expect(after.byId[fresh]).toMatchObject({ status: "connected", currentSchema: "inventory" });
    expect(useTabsStore.getState().tabs.map((t) => [t.id, t.connectionId])).toEqual([
      [t1, fresh],
      [t2, "other"],
    ]);
    expect(qc.getQueryData(["tables", "dead", "inventory"])).toBeUndefined();
    expect(qc.getQueryData(["tables", "other", "public"])).toEqual([]);
    expect(toasts.success).toHaveBeenCalled();
  });

  it("keeps the connection marked lost when reconnecting fails", async () => {
    useConnectionsStore.getState().add(info("dead", "no-such-profile"));
    useConnectionsStore.getState().setStatus("dead", "lost");
    await expect(reconnectConnection("dead", new QueryClient())).resolves.toBe("failed");
    const conn = useConnectionsStore.getState().byId.dead;
    expect(conn?.status).toBe("lost");
    expect(conn?.message).toMatch(/no-such-profile/);
    expect(toasts.error).toHaveBeenCalledWith(
      "Could not reconnect to Local PG",
      expect.objectContaining({ description: expect.stringMatching(/no-such-profile/) }),
    );
  });
});
