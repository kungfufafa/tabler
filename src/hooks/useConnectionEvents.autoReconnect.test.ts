import { afterEach, describe, expect, it, vi } from "vitest";
import type { ConnStatus } from "@/lib/ipc";
import { AUTO_RECONNECT_INTERVAL_MS, startAutoReconnect } from "./useConnectionEvents";

function setup(statuses: Record<string, ConnStatus>, ping: (id: string) => Promise<ConnStatus>) {
  let now = 0;
  const target = new EventTarget() as unknown as Window;
  let visibility: DocumentVisibilityState = "visible";
  const docTarget = new EventTarget();
  const doc = {
    get visibilityState() {
      return visibility;
    },
    addEventListener: docTarget.addEventListener.bind(docTarget),
    removeEventListener: docTarget.removeEventListener.bind(docTarget),
  } as unknown as Document;
  const reconnect = vi.fn(async (_id: string) => "reconnected");
  const busy = vi.fn((_id: string) => false);
  const pingFn = vi.fn(ping);
  const stop = startAutoReconnect({
    connections: () => Object.entries(statuses).map(([id, status]) => ({ id, status })),
    ping: pingFn,
    reconnect,
    busy,
    target,
    doc,
    now: () => now,
  });
  return {
    reconnect,
    ping: pingFn,
    busy,
    stop,
    fire: (name: string) => target.dispatchEvent(new Event(name)),
    show: () => {
      visibility = "visible";
      docTarget.dispatchEvent(new Event("visibilitychange"));
    },
    hide: () => {
      visibility = "hidden";
      docTarget.dispatchEvent(new Event("visibilitychange"));
    },
    advance: (ms: number) => {
      now += ms;
    },
  };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

afterEach(() => vi.restoreAllMocks());

describe("startAutoReconnect", () => {
  it("reconnects lost connections when the window gains focus, at most once per interval", async () => {
    const t = setup({ a: "lost", b: "reconnecting", c: "disconnected" }, async () => "connected");
    t.fire("focus");
    expect(t.reconnect).toHaveBeenCalledTimes(1);
    expect(t.reconnect).toHaveBeenCalledWith("a");
    t.fire("focus");
    expect(t.reconnect).toHaveBeenCalledTimes(1);
    t.advance(AUTO_RECONNECT_INTERVAL_MS);
    t.fire("focus");
    expect(t.reconnect).toHaveBeenCalledTimes(2);
    t.stop();
  });

  it("also on network change and when the window becomes visible", () => {
    const t = setup({ a: "lost" }, async () => "connected");
    t.fire("online");
    expect(t.reconnect).toHaveBeenCalledTimes(1);
    t.advance(AUTO_RECONNECT_INTERVAL_MS);
    t.hide();
    expect(t.reconnect).toHaveBeenCalledTimes(1);
    t.show();
    expect(t.reconnect).toHaveBeenCalledTimes(2);
    t.stop();
  });

  it("pings connected ones and reconnects those whose ping fails", async () => {
    const results: Record<string, () => Promise<ConnStatus>> = {
      ok: async () => "connected",
      gone: async () => "lost",
      dead: async () => Promise.reject(new Error("timeout")),
    };
    const t = setup({ ok: "connected", gone: "connected", dead: "connected" }, (id) =>
      results[id]!(),
    );
    t.fire("focus");
    await flush();
    expect(t.ping).toHaveBeenCalledTimes(3);
    expect(t.reconnect.mock.calls.map((c) => c[0]).sort()).toEqual(["dead", "gone"]);
    t.stop();
  });

  it("leaves a connection alone while its reconnect is running, and stops listening", () => {
    const t = setup({ a: "lost" }, async () => "connected");
    t.busy.mockReturnValue(true);
    t.fire("focus");
    expect(t.reconnect).not.toHaveBeenCalled();
    t.busy.mockReturnValue(false);
    t.advance(AUTO_RECONNECT_INTERVAL_MS);
    t.stop();
    t.fire("focus");
    expect(t.reconnect).not.toHaveBeenCalled();
  });
});
