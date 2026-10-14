import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetLockForTests, useLockStore } from "@/features/lock/store";
import { usePaletteStore } from "@/features/palette/store";
import { ipc } from "@/lib/ipc";
// Loaded up front: the first IPC call would otherwise wait for every mock module to load.
import "@/lib/ipc/mock";
import { mockOpen } from "@/lib/ipc/mock/deeplink";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { DeepLinkHost } from "./DeepLinkHost";
import { useDeepLinkStore } from "./store";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() } }));

function wrap(children: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

/** The mock's saved SQLite connection ("sample.sqlite"): a file link that connects it. */
const SAVED_SQLITE = "/Users/me/dev/fixtures/sample.sqlite";

const flush = () => new Promise((r) => setTimeout(r, 0));

/** Waits until the connect a link started is over (nothing leaks into the next test). */
const settled = () =>
  vi.waitFor(() => {
    expect(usePaletteStore.getState().connecting).toEqual([]);
    expect(useConnectionsStore.getState().order).toHaveLength(1);
  });

/** Renders the host and waits until its `deeplink:open` listener is installed. */
async function mountHost() {
  render(wrap(<DeepLinkHost />));
  await flush();
  await flush();
}

beforeEach(() => {
  localStorage.clear();
  resetLockForTests();
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTabsStore.setState({ tabs: [], activeId: null });
  useDeepLinkStore.setState({ temporary: {}, pendingFile: null });
});
afterEach(() => vi.restoreAllMocks());

describe("DeepLinkHost and the app lock", () => {
  it("leaves the queue alone while locked and opens it after the unlock", async () => {
    const take = vi.spyOn(ipc.deeplink, "takePending");
    const connect = vi.spyOn(ipc.connections, "connect");
    useLockStore.setState({ status: "locked", reason: "startup", enabled: true });
    // A saved connection (the mock's sample.sqlite) opened from Finder.
    mockOpen(SAVED_SQLITE);
    await mountHost();
    mockOpen(SAVED_SQLITE);
    await flush();
    await flush();

    expect(take).not.toHaveBeenCalled();
    expect(connect).not.toHaveBeenCalled();
    expect(useConnectionsStore.getState().order).toHaveLength(0);

    act(() => useLockStore.setState({ status: "unlocked", reason: null }));
    await vi.waitFor(() => expect(connect).toHaveBeenCalledTimes(1));
    expect(connect).toHaveBeenCalledWith("p-sample-sqlite", null);
    // Both queued requests were taken (one connection: the second finds it open).
    await vi.waitFor(() => expect(take).toHaveBeenCalled());
    await vi.waitFor(() => expect(useConnectionsStore.getState().order).toHaveLength(1));
    expect(await ipc.deeplink.takePending()).toEqual([]);
    await settled();
  });

  it("opens a link right away when the app is not locked", async () => {
    const connect = vi.spyOn(ipc.connections, "connect");
    await mountHost();
    mockOpen(SAVED_SQLITE);
    await vi.waitFor(() => expect(connect).toHaveBeenCalledWith("p-sample-sqlite", null));
    await settled();
  });

  it("holds requests taken just before the lock engaged until the unlock", async () => {
    const connect = vi.spyOn(ipc.connections, "connect");
    mockOpen(SAVED_SQLITE);
    // The lock engages while the request is being taken (the passcode status answered late).
    const take = ipc.deeplink.takePending.bind(ipc.deeplink);
    vi.spyOn(ipc.deeplink, "takePending").mockImplementationOnce(async () => {
      const requests = await take();
      useLockStore.setState({ status: "locked", reason: "startup", enabled: true });
      return requests;
    });
    await mountHost();
    expect(connect).not.toHaveBeenCalled();

    act(() => useLockStore.setState({ status: "unlocked", reason: null }));
    await vi.waitFor(() => expect(connect).toHaveBeenCalledTimes(1));
    await settled();
  });
});
