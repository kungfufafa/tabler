import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { ConnectionInfo } from "../../bindings/ConnectionInfo";
import type { ConnectionProfile } from "../../bindings/ConnectionProfile";
import type { ExplainResult } from "../../bindings/ExplainResult";
import type { FilterGroup } from "../../bindings/FilterGroup";
import type { ProfileInput } from "../../bindings/ProfileInput";
import type { TableRef } from "../../bindings/TableRef";
import { mockProfiles } from "../handlers";
import { mockError, mockInvoke } from "../transport";
import { fakeMockDriver } from "./fake";
import { buildMockDrivers, enableMockDrivers, registerMockDriver, resetMockBuild } from ".";
import type { MockDriver, MockSession } from "./types";

// The mock's Phase B hooks (README, Change requests, Phase B): the count's exact flag, a session
// opened again on Switch Database, the form's profile and read-only refusal of `driver_util`,
// the session's own database name, view definitions, explains and plugin runs, and the
// PostgreSQL flavors' engines.

beforeAll(async () => {
  await import("..");
});

afterEach(() => {
  vi.restoreAllMocks();
  registerMockDriver("mongodb", null);
  resetMockBuild();
  mockProfiles.delete("p-fake-mongodb");
});

const fakeProfile = (): ConnectionProfile => fakeMockDriver("mongodb").seedProfiles()[0]!;

/** The fake store with `patch` over each session it opens; `opened` records their profiles. */
function withSession(patch: Partial<MockSession>, driver: Partial<MockDriver> = {}) {
  const fake = fakeMockDriver("mongodb");
  const opened: ConnectionProfile[] = [];
  registerMockDriver("mongodb", {
    ...fake,
    connect: (profile) => {
      opened.push(profile);
      return { ...fake.connect(profile), ...patch };
    },
    ...driver,
  });
  buildMockDrivers(["mongodb"]);
  mockProfiles.set("p-fake-mongodb", fakeProfile());
  return opened;
}

const connect = () => mockInvoke<ConnectionInfo>("connect", { profileId: "p-fake-mongodb" });

describe("the mock's session hooks", () => {
  it("passes the count's exact flag", async () => {
    const count = vi.fn((_t: TableRef, _f: FilterGroup | null, exact?: boolean) => ({
      count: exact ? 3 : 1000,
      exact: exact === true,
    }));
    withSession({ count });
    const { connectionId } = await connect();
    const table = { schema: null, name: "orders" };
    expect(await mockInvoke("count_table_rows", { connectionId, table })).toEqual({
      count: 1000,
      exact: false,
    });
    expect(await mockInvoke("count_table_rows", { connectionId, table, exact: true })).toEqual({
      count: 3,
      exact: true,
    });
    await mockInvoke("disconnect", { connectionId });
  });

  it("opens the session again on Switch Database, and shows the session's database", async () => {
    const opened = withSession({ currentDatabase: "catalog" });
    const info = await connect();
    expect(info.currentDatabase).toBe("catalog");
    const switched = await mockInvoke<ConnectionInfo>("switch_database", {
      connectionId: info.connectionId,
      database: "other",
    });
    expect(opened.map((p) => p.database)).toEqual(["app", "other"]);
    expect(switched.currentDatabase).toBe("other");
    await mockInvoke("disconnect", { connectionId: info.connectionId });
  });

  it("answers view definitions and explains through the session", async () => {
    const plan: ExplainResult = { columns: [], rows: [], text: "plan", durationMs: 1 };
    const explain = vi.fn(() => plan);
    withSession({ viewDefinition: (t) => `view ${t.name}`, explain });
    const { connectionId } = await connect();
    const table = { schema: null, name: "v" };
    expect(await mockInvoke("get_view_definition", { connectionId, table })).toBe("view v");
    expect(await mockInvoke("explain", { connectionId, sql: "find users", mode: "plan" })).toBe(
      plan,
    );
    expect(explain).toHaveBeenCalledWith("find users", "plan");
    await mockInvoke("disconnect", { connectionId });
  });

  it("lets the session refuse a plugin's run first", async () => {
    withSession({
      admitPluginRun: (text) => {
        if (text.includes("drop")) mockError("UNSUPPORTED", "unsupported: not from a plugin");
      },
    });
    const { connectionId } = await connect();
    const run = (sql: string, origin: "user" | "plugin") =>
      mockInvoke("run_sql", { connectionId, queryId: "q", sql, options: { origin }, onEvent: {} });
    await expect(run("drop users", "plugin")).rejects.toMatchObject({ code: "UNSUPPORTED" });
    await expect(run("count users", "plugin")).resolves.toHaveLength(1);
    await expect(run("drop users", "user")).resolves.toHaveLength(1);
    await mockInvoke("disconnect", { connectionId });
  });
});

describe("the mock's driver_util", () => {
  it("gives the call the form's input and refuses a write on a read-only profile", async () => {
    const util = vi.fn((_m: string, _a: unknown, input: ProfileInput) => input.password);
    withSession({}, { util, utilKind: (m) => (m === "probe" ? "read" : "write") });
    const profile = fakeProfile();
    const input = { profile, password: "pw" };
    expect(await mockInvoke("driver_util", { input, method: "probe", args: null })).toBe("pw");
    const ro = { profile: { ...profile, options: { ...profile.options, readOnly: true } } };
    expect(await mockInvoke("driver_util", { input: ro, method: "probe", args: null })).toBe(
      undefined,
    );
    await expect(
      mockInvoke("driver_util", { input: ro, method: "createFile", args: null }),
    ).rejects.toMatchObject({ code: "INVALID_INPUT", message: "This connection is read-only" });
    expect(util).toHaveBeenCalledTimes(2);
  });
});

describe("the PostgreSQL flavors' engines", () => {
  it("connect a flavor's profile through its lane's mock, with its caps", async () => {
    const added = await enableMockDrivers(["cockroachdb"]);
    const seeded = added.find((p) => p.options.pgFlavor === "cockroachdb");
    expect(seeded).toBeDefined();
    const info = await mockInvoke<ConnectionInfo>("connect", { profileId: seeded!.id });
    expect(info.capabilities?.flavor).toBe("cockroachdb");
    await mockInvoke("disconnect", { connectionId: info.connectionId });
    mockProfiles.delete(seeded!.id);
  });
});
