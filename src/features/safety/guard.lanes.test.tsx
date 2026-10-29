import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fakeLane } from "@/features/drivers/__fixtures__/fakeLane";
import { CORE_KINDS } from "@/features/drivers/catalog";
import { registerLaneOverride } from "@/features/drivers/registry";
import { BASE_CAPS } from "@/lib/caps";
import { ipc, type ConnectionInfo, type DatabaseKind, type Driver } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { useAppInfoStore } from "@/stores/appInfo";
import { useConnectionsStore } from "@/stores/connections";
import { guardExecution, sessionWrites, type GuardRequest } from "./guard";
import { SafetyHost } from "./SafetyHost";
import { useSafetyPromptStore } from "./store";

// Safe Mode on the sessions the core classifier cannot read (F9b): a native store's console
// through its lane's classifier (the fake lane stands in for MongoDB's), and an extension SQL
// dialect through the portable classifier over its lexer profile (Redshift's).

const query = (...statements: string[]): GuardRequest => ({ kind: "query", statements });

function info(driver: Driver, patch: Partial<ConnectionInfo> = {}): ConnectionInfo {
  return {
    connectionId: "c-lane",
    profileId: "p-lane",
    name: "Lane Store",
    color: null,
    driver,
    serverVersion: "1.0",
    currentDatabase: null,
    currentUser: null,
    tunnel: null,
    readOnly: false,
    ...patch,
  };
}

/** Redshift: the PostgreSQL driver speaking its own dialect. */
const redshift = info("postgres", { capabilities: { ...BASE_CAPS.postgres, dialect: "redshift" } });

const built = (...kinds: DatabaseKind[]) =>
  useAppInfoStore.setState({
    info: { version: "1", platform: "test", debug: true, kinds: [...CORE_KINDS, ...kinds] },
  });

beforeEach(() => {
  registerLaneOverride("mongodb", fakeLane("mongodb", "mongodb"));
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useSafetyPromptStore.setState({ queue: [] });
});
afterEach(() => {
  registerLaneOverride("mongodb", null);
  useAppInfoStore.setState({ info: null });
  vi.restoreAllMocks();
});

describe("sessionWrites", () => {
  it("reads a core session at once, as before", () => {
    expect(sessionWrites(info("postgres"), query("SELECT 1"))).toBe(false);
    expect(sessionWrites(info("mysql"), query("DELETE FROM t"))).toBe(true);
    // Commits and operations never need a classifier.
    expect(sessionWrites(redshift, { kind: "commit", statements: ["SELECT 1"] })).toBe(true);
    expect(sessionWrites(info("mongodb"), { kind: "operation", label: "x", write: false })).toBe(
      false,
    );
  });

  it("reads a native console with its lane's classifier, once the lane is built", async () => {
    built("mongodb");
    await expect(sessionWrites(info("mongodb"), query("find orders", "count users"))).resolves.toBe(
      false,
    );
    await expect(sessionWrites(info("mongodb"), query("find orders", "drop users"))).resolves.toBe(
      true,
    );
    await expect(sessionWrites(info("mongodb"), query())).resolves.toBe(true);
  });

  it("counts every console command as a write without the lane", async () => {
    await expect(sessionWrites(info("mongodb"), query("find orders"))).resolves.toBe(true);
  });

  it("reads an extension SQL dialect with the portable classifier", async () => {
    await expect(sessionWrites(redshift, query("SELECT 1"))).resolves.toBe(false);
    await expect(
      sessionWrites(redshift, query("SELECT 1", "INSERT INTO t VALUES (1)")),
    ).resolves.toBe(true);
  });
});

describe("guardExecution on a native console", () => {
  it("prompts only for the write in Alert Mode 2", async () => {
    built("mongodb");
    const qc = new QueryClient();
    render(
      <QueryClientProvider client={qc}>
        <SafetyHost />
      </QueryClientProvider>,
    );
    vi.spyOn(ipc.profiles, "get").mockResolvedValue({
      ...SEED_PROFILES[0]!,
      id: "p-lane",
      driver: "mongodb",
      options: { ...SEED_PROFILES[0]!.options, safeMode: "alertNonSelect" },
    });
    useConnectionsStore.getState().add(info("mongodb"));
    await expect(guardExecution("c-lane", query("find orders"))).resolves.toBe(true);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    const write = guardExecution("c-lane", query('insert users {"_id": "u3"}'));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByRole("list", { name: "Queries" })).toHaveTextContent("insert users");
    fireEvent.click(within(dialog).getByRole("button", { name: "Send" }));
    await expect(write).resolves.toBe(true);
  });

  it("refuses the write on a read-only connection, and lets the read through", async () => {
    built("mongodb");
    vi.spyOn(ipc.profiles, "get").mockResolvedValue({ ...SEED_PROFILES[0]!, id: "p-lane" });
    useConnectionsStore.getState().add(info("mongodb", { readOnly: true }));
    await expect(guardExecution("c-lane", query("count orders"))).resolves.toBe(true);
    await expect(guardExecution("c-lane", query("delete orders o1"))).resolves.toBe(false);
  });
});
