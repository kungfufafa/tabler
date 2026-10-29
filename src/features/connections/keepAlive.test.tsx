import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BASE_CAPS } from "@/lib/caps";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { DEFAULT_PREFS, resetPrefsForTests, sanitizePrefs, setPref } from "@/stores/prefs";
import { KEEP_ALIVE_INTERVAL_MS, keepAliveTargets, useKeepAlive } from "./keepAlive";

vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn(), message: vi.fn() }),
}));

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

function mount() {
  const qc = new QueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  );
  return renderHook(() => useKeepAlive(), { wrapper });
}

beforeEach(() => {
  vi.useFakeTimers();
  resetPrefsForTests();
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useConnectionsStore.getState().add(info("a"));
  useConnectionsStore.getState().add(info("b"));
  useConnectionsStore.getState().setStatus("b", "lost", "gone");
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("Keep connection alive", () => {
  it("is on by default and only takes booleans", () => {
    expect(DEFAULT_PREFS.keepAlive).toBe(true);
    expect(sanitizePrefs({ keepAlive: false }).keepAlive).toBe(false);
    expect(sanitizePrefs({ keepAlive: "yes" }).keepAlive).toBe(true);
  });

  it("pings each connected session every 30 s", async () => {
    const ping = vi.spyOn(ipc.connections, "ping").mockResolvedValue("connected");
    mount();
    expect(keepAliveTargets()).toEqual(["a"]);
    await act(async () => vi.advanceTimersByTimeAsync(KEEP_ALIVE_INTERVAL_MS - 1));
    expect(ping).not.toHaveBeenCalled();
    await act(async () => vi.advanceTimersByTimeAsync(1));
    expect(ping).toHaveBeenCalledTimes(1);
    expect(ping).toHaveBeenCalledWith("a");
    await act(async () => vi.advanceTimersByTimeAsync(KEEP_ALIVE_INTERVAL_MS));
    expect(ping).toHaveBeenCalledTimes(2);
  });

  it("marks a connection lost when its ping fails", async () => {
    vi.spyOn(ipc.connections, "ping").mockRejectedValue(
      Object.assign(new Error("server closed the connection"), { code: "NOT_CONNECTED" }),
    );
    mount();
    await act(async () => vi.advanceTimersByTimeAsync(KEEP_ALIVE_INTERVAL_MS));
    expect(useConnectionsStore.getState().byId.a?.status).toBe("lost");
  });

  it("stops when turned off", async () => {
    const ping = vi.spyOn(ipc.connections, "ping").mockResolvedValue("connected");
    mount();
    act(() => setPref("keepAlive", false));
    await act(async () => vi.advanceTimersByTimeAsync(KEEP_ALIVE_INTERVAL_MS * 3));
    expect(ping).not.toHaveBeenCalled();
  });
});

describe("Keep connection alive, by the session's caps", () => {
  const withKeepalive = (connectionId: string, keepaliveSecs: number | null): ConnectionInfo => {
    const base = info(connectionId);
    const caps = BASE_CAPS.postgres;
    return { ...base, capabilities: { ...caps, session: { ...caps.session, keepaliveSecs } } };
  };

  it("never pings a session whose ping would bill or wake it", async () => {
    const ping = vi.spyOn(ipc.connections, "ping").mockResolvedValue("connected");
    useConnectionsStore.getState().add(withKeepalive("warehouse", null));
    mount();
    expect(keepAliveTargets()).toEqual(["a"]);
    await act(async () => vi.advanceTimersByTimeAsync(KEEP_ALIVE_INTERVAL_MS * 2));
    expect(ping.mock.calls.map(([id]) => id)).toEqual(["a", "a"]);
  });

  it("pings a session with a longer interval every other beat", async () => {
    const ping = vi.spyOn(ipc.connections, "ping").mockResolvedValue("connected");
    useConnectionsStore.getState().add(withKeepalive("search", 60));
    mount();
    await act(async () => vi.advanceTimersByTimeAsync(KEEP_ALIVE_INTERVAL_MS * 4));
    const calls = ping.mock.calls.map(([id]) => id);
    expect(calls.filter((id) => id === "a")).toHaveLength(4);
    expect(calls.filter((id) => id === "search")).toHaveLength(2);
  });
});
