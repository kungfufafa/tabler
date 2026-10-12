import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc, type ConnectionInfo, type Driver } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { refreshRestoredDatabase, resetBackupStore, startJob } from "./store";

const toast = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  message: vi.fn(),
  warning: vi.fn(),
}));
vi.mock("sonner", () => ({ toast }));

const guard = vi.hoisted(() => ({
  guardExecution: vi.fn(async () => true),
  READ_ONLY_MESSAGE: "This connection is read-only",
}));
vi.mock("@/features/safety/guard", () => guard);

function open(connectionId: string, profileId: string, driver: Driver, database: string) {
  const info: ConnectionInfo = {
    connectionId,
    profileId,
    name: profileId,
    color: null,
    driver,
    serverVersion: "1",
    currentDatabase: database,
    currentUser: null,
    tunnel: null,
    readOnly: false,
  };
  useConnectionsStore.getState().add(info);
}

/** Order of the backend cache resets and the renderer's table-list invalidations. */
function track(qc: QueryClient) {
  const calls: string[] = [];
  vi.spyOn(ipc.schema, "invalidate").mockImplementation(async (id) => {
    calls.push(`backend ${id}`);
  });
  const original = qc.invalidateQueries.bind(qc);
  vi.spyOn(qc, "invalidateQueries").mockImplementation((filters, options) => {
    const key = filters?.queryKey;
    if (key?.[0] === "tables") calls.push(`tables ${String(key[1])}`);
    return original(filters, options);
  });
  return calls;
}

beforeEach(() => {
  resetBackupStore();
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("refreshRestoredDatabase", () => {
  it("reloads the PostgreSQL connections of the profile on the restored database", async () => {
    open("c-a", "p1", "postgres", "analytics");
    open("c-b", "p1", "postgres", "tabler_dev");
    open("c-c", "p2", "postgres", "analytics");
    const qc = new QueryClient();
    const calls = track(qc);
    await refreshRestoredDatabase(qc, { id: "p1", driver: "postgres" }, "analytics");
    // The backend's cache goes first, so the refetch does not read the old schema.
    expect(calls).toEqual(["backend c-a", "tables c-a"]);
  });

  it("reloads every MySQL connection of the profile (each lists all databases)", async () => {
    open("c-a", "p1", "mysql", "shop");
    open("c-b", "p1", "mysql", "sys");
    open("c-c", "p2", "mysql", "shop");
    const qc = new QueryClient();
    const calls = track(qc);
    await refreshRestoredDatabase(qc, { id: "p1", driver: "mysql" }, "shop");
    expect(calls).toEqual(["backend c-a", "backend c-b", "tables c-a", "tables c-b"]);
  });
});

describe("startJob", () => {
  it("reloads the restored database's schema after a successful restore only", async () => {
    const profile = await ipc.profiles.get("p-local-pg");
    open("c-a", profile.id, "postgres", "analytics");
    const qc = new QueryClient();
    const calls = track(qc);
    const run = vi
      .spyOn(ipc.backup, "run")
      .mockResolvedValue({ path: "/x/app.dump", bytes: 10, durationMs: 5, errors: 0 });
    const job = {
      profile,
      database: "analytics",
      binaryPath: "/opt/homebrew/opt/postgresql@17/bin/pg_restore",
      flags: [],
      gzip: false,
      path: "/x/app.dump",
      queryClient: qc,
    };
    await startJob({ ...job, mode: "backup" });
    expect(calls).toEqual([]);
    await startJob({ ...job, mode: "restore" });
    expect(run).toHaveBeenCalledTimes(2);
    expect(calls).toEqual(["backend c-a", "tables c-a"]);
  });
});
