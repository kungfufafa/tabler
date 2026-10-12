import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc, type ConnectionInfo, type ConnectionProfile } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { guardJob } from "./guard";

const toast = vi.hoisted(() => ({ error: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

const guard = vi.hoisted(() => ({
  guardExecution: vi.fn(async () => true),
  READ_ONLY_MESSAGE: "This connection is read-only",
}));
vi.mock("@/features/safety/guard", () => guard);

const RESTORE = { kind: "operation", label: "Restore app.dump into app", write: true } as const;
const BACKUP = { kind: "operation", label: "Backup app", write: false } as const;

function openConnection(profile: ConnectionProfile, readOnly: boolean): string {
  const info: ConnectionInfo = {
    connectionId: `c-${profile.id}`,
    profileId: profile.id,
    name: profile.name,
    color: null,
    driver: profile.driver,
    serverVersion: "17.6",
    currentDatabase: "app",
    currentUser: "app",
    tunnel: null,
    readOnly,
  };
  useConnectionsStore.getState().add(info);
  return info.connectionId;
}

beforeEach(() => {
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
});

afterEach(() => {
  toast.error.mockReset();
  guard.guardExecution.mockClear();
});

describe("guardJob", () => {
  it("refuses a restore on a read-only connection, whatever its level", async () => {
    const prod = await ipc.profiles.get("p-prod-pg");
    expect(prod.options.readOnly).toBe(true);
    const silent = { ...prod, options: { ...prod.options, safeMode: "silent" as const } };
    // Silent and not open: nothing would have asked before.
    expect(await guardJob(silent, RESTORE)).toBe(false);
    expect(toast.error).toHaveBeenCalledWith("This connection is read-only", expect.anything());
    // Open (connected before Read-only was turned on): still refused, without a prompt.
    openConnection(prod, false);
    expect(await guardJob(prod, RESTORE)).toBe(false);
    expect(guard.guardExecution).not.toHaveBeenCalled();
  });

  it("lets a backup of a read-only connection through its safe-mode check", async () => {
    const prod = await ipc.profiles.get("p-prod-pg");
    // Alert Mode 2 does not ask for a backup (a read), so no connection is needed.
    expect(await guardJob(prod, BACKUP)).toBe(true);
    const id = openConnection(prod, true);
    expect(await guardJob(prod, BACKUP)).toBe(true);
    expect(guard.guardExecution).toHaveBeenCalledWith(id, BACKUP);
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("checks a restore of a writable connection through its open connection", async () => {
    const local = await ipc.profiles.get("p-local-pg");
    const id = openConnection(local, false);
    guard.guardExecution.mockResolvedValueOnce(false);
    expect(await guardJob(local, RESTORE)).toBe(false);
    expect(guard.guardExecution).toHaveBeenCalledWith(id, RESTORE);
    expect(toast.error).not.toHaveBeenCalled();
  });
});
