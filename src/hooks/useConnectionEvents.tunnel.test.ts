import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnStatus, ConnectionInfo, TunnelStatusEvent } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import {
  applyStatus,
  applyTunnelStatus,
  resetTunnelState,
  TUNNEL_DOWN_GRACE_MS,
} from "./useConnectionEvents";

const toasts = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast: Object.assign(vi.fn(), toasts) }));

const info: ConnectionInfo = {
  connectionId: "c1",
  profileId: "p-staging-ssh",
  name: "Staging via SSH",
  color: null,
  driver: "postgres",
  serverVersion: "17",
  currentDatabase: "app_staging",
  currentUser: "app",
  tunnel: { localPort: 50123 },
  readOnly: false,
};

function openConnection(status: ConnStatus = "connected") {
  useConnectionsStore.setState({
    byId: { c1: { info, status, message: null, currentSchema: null } },
    order: ["c1"],
    activeId: "c1",
  });
}

const tunnel = (status: TunnelStatusEvent["status"], message: string | null = null) =>
  applyTunnelStatus({ connectionId: "c1", status, localPort: 50123, message });

const TOAST_ID = "connection-status:p-staging-ssh";

beforeEach(() => {
  vi.useFakeTimers();
  resetTunnelState();
  openConnection();
});

afterEach(() => {
  resetTunnelState();
  vi.useRealTimers();
  Object.values(toasts).forEach((fn) => fn.mockReset());
});

describe("tunnel:status", () => {
  it("announces the tunnel instead of a generic 'connection lost'", () => {
    tunnel("down", "the SSH session ended");
    expect(toasts.error).not.toHaveBeenCalled();
    applyStatus("c1", "lost", "SSH tunnel down: the SSH session ended");
    expect(toasts.error).toHaveBeenCalledOnce();
    expect(toasts.error).toHaveBeenCalledWith("SSH tunnel to Staging via SSH closed", {
      id: TOAST_ID,
      description: "the SSH session ended",
    });
    expect(useConnectionsStore.getState().byId.c1?.status).toBe("lost");
    // The grace timer was dropped with the pending report.
    vi.advanceTimersByTime(TUNNEL_DOWN_GRACE_MS * 2);
    expect(toasts.error).toHaveBeenCalledOnce();
  });

  it("announces right away when the connection is already lost, replacing that toast", () => {
    applyStatus("c1", "lost", "SSH tunnel down: keepalive timeout");
    expect(toasts.error).toHaveBeenLastCalledWith("Connection to Staging via SSH lost", {
      id: TOAST_ID,
      description: "SSH tunnel down: keepalive timeout",
    });
    tunnel("error", "the SSH server stopped answering keepalives");
    expect(toasts.error).toHaveBeenLastCalledWith("SSH tunnel to Staging via SSH closed", {
      id: TOAST_ID,
      description: "the SSH server stopped answering keepalives",
    });
    // Repeated reports while it stays down are not announced again.
    tunnel("down", "tunnel closed");
    expect(toasts.error).toHaveBeenCalledTimes(2);
  });

  it("falls back to announcing after a grace period", () => {
    tunnel("down", "tunnel closed");
    vi.advanceTimersByTime(TUNNEL_DOWN_GRACE_MS - 1);
    expect(toasts.error).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(toasts.error).toHaveBeenCalledWith("SSH tunnel to Staging via SSH closed", {
      id: TOAST_ID,
      description: "tunnel closed",
    });
  });

  it("stays silent for a deliberate disconnect", () => {
    tunnel("down", "tunnel closed");
    applyStatus("c1", "disconnected");
    vi.advanceTimersByTime(TUNNEL_DOWN_GRACE_MS * 2);
    expect(toasts.error).not.toHaveBeenCalled();

    openConnection();
    tunnel("down", "tunnel closed");
    // useDisconnect drops the connection from the store when the command returns.
    useConnectionsStore.getState().remove("c1");
    vi.advanceTimersByTime(TUNNEL_DOWN_GRACE_MS * 2);
    expect(toasts.error).not.toHaveBeenCalled();
  });

  it("ignores tunnels of unknown connections and 'up'", () => {
    applyTunnelStatus({ connectionId: "other", status: "down", localPort: 1, message: null });
    tunnel("up");
    vi.advanceTimersByTime(TUNNEL_DOWN_GRACE_MS * 2);
    expect(toasts.error).not.toHaveBeenCalled();
    expect(toasts.success).not.toHaveBeenCalled();
  });

  it("announces again after the tunnel came back up", () => {
    openConnection("lost");
    tunnel("down", "first");
    expect(toasts.error).toHaveBeenCalledTimes(1);
    tunnel("up");
    applyStatus("c1", "connected");
    tunnel("down", "second");
    applyStatus("c1", "lost", "SSH tunnel down: second");
    expect(toasts.error).toHaveBeenCalledTimes(2);
    expect(toasts.error).toHaveBeenLastCalledWith("SSH tunnel to Staging via SSH closed", {
      id: TOAST_ID,
      description: "second",
    });
  });
});
