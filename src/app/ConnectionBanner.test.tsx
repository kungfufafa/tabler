import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { ConnectionBanner } from "./ConnectionBanner";

vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn(), message: vi.fn() }),
}));

function info(connectionId: string, profileId: string, name = "Local PG"): ConnectionInfo {
  return {
    connectionId,
    profileId,
    name,
    color: null,
    driver: "postgres",
    serverVersion: "17",
    currentDatabase: "postgres",
    currentUser: "postgres",
    tunnel: null,
    readOnly: false,
  };
}

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

function renderBanner() {
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <ConnectionBanner />
    </QueryClientProvider>,
  );
}

const reconnectButton = () => screen.getByRole("button", { name: "Reconnect" });

describe("ConnectionBanner", () => {
  beforeEach(() => {
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  });
  afterEach(() => vi.restoreAllMocks());

  it("lets a long title truncate instead of pushing Reconnect out of view", () => {
    const conns = useConnectionsStore.getState();
    conns.add(info("a", "pa", "Production EU-West replica (read-only)"));
    conns.setStatus("a", "lost", "server closed the connection unexpectedly");
    renderBanner();
    const title = document.querySelector("[data-slot=connection-banner-title]")!;
    const message = document.querySelector("[data-slot=connection-banner-message]")!;
    // jsdom has no layout, so pin the flex contract: the title may shrink (and truncates), the
    // message only takes what is left, and the button never shrinks.
    expect(title).toHaveClass("min-w-0", "truncate");
    expect(title).not.toHaveClass("shrink-0");
    expect(title).toHaveAttribute(
      "title",
      "Connection to Production EU-West replica (read-only) lost: server closed the connection unexpectedly",
    );
    expect(message).toHaveClass("min-w-0", "flex-1", "truncate");
    expect(reconnectButton()).toHaveClass("shrink-0");
  });

  it("keeps each connection's Reconnect disabled until its own attempt finishes", async () => {
    const conns = useConnectionsStore.getState();
    conns.add(info("a", "pa"));
    conns.add(info("b", "pb"));
    conns.setStatus("a", "lost");
    conns.setStatus("b", "lost");
    conns.setActive("a");
    vi.spyOn(ipc.connections, "ping").mockResolvedValue("lost");
    const opened = { pa: deferred<ConnectionInfo>(), pb: deferred<ConnectionInfo>() };
    const connect = vi
      .spyOn(ipc.connections, "connect")
      .mockImplementation((profileId) => opened[profileId as "pa" | "pb"].promise);
    vi.spyOn(ipc.connections, "disconnect").mockResolvedValue(undefined);
    renderBanner();

    fireEvent.click(reconnectButton());
    expect(reconnectButton()).toBeDisabled();
    act(() => useConnectionsStore.getState().setActive("b"));
    expect(reconnectButton()).toBeEnabled();
    fireEvent.click(reconnectButton());
    expect(reconnectButton()).toBeDisabled();
    await vi.waitFor(() => expect(connect).toHaveBeenCalledTimes(2));

    // A finishing must not re-enable B's button while B is still connecting.
    await act(async () => opened.pa.resolve(info("a2", "pa")));
    expect(useConnectionsStore.getState().activeId).toBe("b");
    expect(reconnectButton()).toBeDisabled();
    fireEvent.click(reconnectButton());
    expect(connect).toHaveBeenCalledTimes(2);

    await act(async () => opened.pb.resolve(info("b2", "pb")));
    expect(useConnectionsStore.getState().order).toEqual(["a2", "b2"]);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });
});
