import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as deeplink from "@/features/deeplink/open";
import { resetLockForTests, useLockStore } from "@/features/lock/store";
import * as viewer from "@/features/sqlviewer/open";
import { ipc } from "@/lib/ipc";
// Loaded up front: the first IPC call would otherwise wait for every mock module to load.
import "@/lib/ipc/mock";
import { resetMockRecent } from "@/lib/ipc/mock/recent";
import { mockEmit } from "@/lib/ipc/mock/transport";
import { useConnectionsStore } from "@/stores/connections";
import { RecentHost } from "./RecentHost";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() } }));

function wrap(children: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

const flush = () => new Promise((r) => setTimeout(r, 0));

/** Renders the host and waits until its `menu:action` listener is installed. */
async function mountHost() {
  render(wrap(<RecentHost />));
  await flush();
  await flush();
}

beforeEach(() => {
  localStorage.clear();
  resetLockForTests();
  resetMockRecent();
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
});
afterEach(() => vi.restoreAllMocks());

describe("RecentHost and the app lock", () => {
  it("does nothing for a recent connection or file while the app is locked", async () => {
    const connect = vi.spyOn(ipc.connections, "connect");
    const inViewer = vi.spyOn(viewer, "openSqlViewer").mockImplementation(() => {});
    const inTab = vi.spyOn(deeplink, "openSqlFile").mockResolvedValue(undefined);
    useLockStore.setState({ status: "locked", reason: "user", enabled: true });
    await mountHost();

    mockEmit("menu:action", "recent.file:/Users/me/secret/payroll.sql");
    mockEmit("menu:action", "recent.connection:p-local-pg");
    await flush();
    await flush();

    expect(inViewer).not.toHaveBeenCalled();
    expect(inTab).not.toHaveBeenCalled();
    expect(connect).not.toHaveBeenCalled();
  });

  it("opens them again once unlocked", async () => {
    const connect = vi.spyOn(ipc.connections, "connect");
    const inViewer = vi.spyOn(viewer, "openSqlViewer").mockImplementation(() => {});
    useLockStore.setState({ status: "locked", reason: "user", enabled: true });
    await mountHost();

    mockEmit("menu:action", "recent.file:/Users/me/secret/payroll.sql");
    expect(inViewer).not.toHaveBeenCalled();

    useLockStore.setState({ status: "unlocked", reason: null });
    mockEmit("menu:action", "recent.file:/Users/me/secret/payroll.sql");
    expect(inViewer).toHaveBeenCalledWith("/Users/me/secret/payroll.sql");

    mockEmit("menu:action", "recent.connection:p-local-pg");
    await vi.waitFor(() => expect(connect).toHaveBeenCalledWith("p-local-pg"));
  });

  it("opens a recent file in a query tab of the active connection when unlocked only", async () => {
    const inTab = vi.spyOn(deeplink, "openSqlFile").mockResolvedValue(undefined);
    useConnectionsStore.getState().add({
      connectionId: "c1",
      profileId: "p-local-pg",
      name: "Local",
      color: null,
      driver: "postgres",
      serverVersion: "17",
      currentDatabase: "app",
      currentUser: "me",
      tunnel: null,
      readOnly: false,
    });
    useLockStore.setState({ status: "locked", reason: "idle", enabled: true });
    await mountHost();

    mockEmit("menu:action", "recent.file:/q/a.sql");
    expect(inTab).not.toHaveBeenCalled();
    useLockStore.setState({ status: "unlocked", reason: null });
    mockEmit("menu:action", "recent.file:/q/a.sql");
    expect(inTab).toHaveBeenCalledWith("c1", "/q/a.sql");
  });
});
