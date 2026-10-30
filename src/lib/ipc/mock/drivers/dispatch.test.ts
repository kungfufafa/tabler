import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { CORE_KINDS } from "@/features/drivers/catalog";
import type { AppInfo } from "../../bindings/AppInfo";
import type { ApplyResult } from "../../bindings/ApplyResult";
import type { BrowsePage } from "../../bindings/BrowsePage";
import type { ConnectionInfo } from "../../bindings/ConnectionInfo";
import type { ConnectionProfile } from "../../bindings/ConnectionProfile";
import type { KillOutcome } from "../../bindings/KillOutcome";
import type { ProcessRow } from "../../bindings/ProcessRow";
import type { QueryEvent } from "../../bindings/QueryEvent";
import type { RoutineSummary } from "../../bindings/RoutineSummary";
import type { SessionDetails } from "../../bindings/SessionDetails";
import type { StatementResult } from "../../bindings/StatementResult";
import type { TableSummary } from "../../bindings/TableSummary";
import type { UserSummary } from "../../bindings/UserSummary";
import { SEED_PROFILES } from "../fixtures";
import { mockProfiles } from "../handlers";
import { mockInvoke } from "../transport";
import { fakeMockDriver } from "./fake";
import {
  buildMockDrivers,
  enableMockDrivers,
  isMockBuilt,
  mockBuiltDrivers,
  mockBuiltKinds,
  registerMockDriver,
  resetMockBuild,
} from ".";

// The mock's handlers send a connection of a driver beyond the core three to its mock driver
// (F9b); the fake document store stands in for MongoDB's.

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

async function connectFake(): Promise<ConnectionInfo> {
  registerMockDriver("mongodb", fakeMockDriver("mongodb"));
  buildMockDrivers(["mongodb"]);
  mockProfiles.set("p-fake-mongodb", fakeProfile());
  return mockInvoke<ConnectionInfo>("connect", { profileId: "p-fake-mongodb" });
}

describe("the mock's build", () => {
  it("is the core drivers by default", async () => {
    const info = await mockInvoke<AppInfo>("app_info", {});
    expect(info.kinds).toEqual(CORE_KINDS);
    expect(info.drivers).toEqual(["postgres", "mysql", "sqlite"]);
    expect(isMockBuilt("mongodb")).toBe(false);
  });

  it("takes drivers and databases by id, in the picker's order", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    // A core driver's id adds nothing: its flavors are databases (or a lane) of their own.
    expect(buildMockDrivers(["redis", "cockroachdb", "nope", "postgres"])).toEqual([
      "redis",
      "cockroachdb",
    ]);
    expect(mockBuiltKinds()).toEqual([
      "postgres",
      "cockroachdb",
      "mysql",
      "mariadb",
      "sqlite",
      "redis",
    ]);
    expect(buildMockDrivers(["pg-flavors"])).toEqual(["greenplum", "redshift"]);
    // CockroachDB is the PostgreSQL driver.
    expect(mockBuiltDrivers()).toEqual(["postgres", "mysql", "sqlite", "redis"]);
    const info = await mockInvoke<AppInfo>("app_info", {});
    expect(info.kinds).toContain("redis");
  });

  it("seeds the profiles of ?mockDrivers= next to the core seed", async () => {
    registerMockDriver("mongodb", fakeMockDriver("mongodb"));
    const added = await enableMockDrivers(["mongodb"]);
    expect(added.map((p) => p.id)).toEqual(["p-fake-mongodb"]);
    const listed = await mockInvoke<ConnectionProfile[]>("list_profiles", {});
    expect(listed.map((p) => p.id)).toContain("p-fake-mongodb");
    expect(SEED_PROFILES).toHaveLength(6);
  });

  it("refuses a database it was not built with", async () => {
    registerMockDriver("mongodb", fakeMockDriver("mongodb"));
    mockProfiles.set("p-fake-mongodb", fakeProfile());
    await expect(mockInvoke("connect", { profileId: "p-fake-mongodb" })).rejects.toMatchObject({
      code: "UNSUPPORTED",
      message: "unsupported: MongoDB is not built into this Tabler",
    });
  });

  it("refuses a built database whose lane has no mock yet", async () => {
    buildMockDrivers(["mongodb"]);
    mockProfiles.set("p-fake-mongodb", fakeProfile());
    await expect(mockInvoke("connect", { profileId: "p-fake-mongodb" })).rejects.toMatchObject({
      code: "UNSUPPORTED",
      message: "unsupported: the mock has no MongoDB driver yet",
    });
  });
});

describe("a connection of a mock driver", () => {
  it("tests and connects with the driver's version and caps", async () => {
    registerMockDriver("mongodb", fakeMockDriver("mongodb"));
    buildMockDrivers(["mongodb"]);
    const test = await mockInvoke<{ ok: boolean; serverVersion: string }>("test_connection", {
      input: { profile: fakeProfile(), password: null },
    });
    expect(test).toMatchObject({ ok: true, serverVersion: "FakeStore 1.0" });
    const info = await connectFake();
    expect(info.serverVersion).toBe("FakeStore 1.0");
    expect(info.capabilities?.namespaces.labels.tables).toBe("Collections");
    await mockInvoke("disconnect", { connectionId: info.connectionId });
  });

  it("lists, browses by cursor, counts and fetches through the session", async () => {
    const { connectionId } = await connectFake();
    const tables = await mockInvoke<TableSummary[]>("list_tables", { connectionId, schema: "" });
    expect(tables.map((t) => t.name)).toEqual(["orders", "users"]);
    const orders = { schema: null, name: "orders" };
    const req = { table: orders, columns: null, filter: null, sort: [], limit: 10, offset: 0 };
    const first = await mockInvoke<BrowsePage>("browse_table", { connectionId, req });
    expect(first.nextCursor).toBe("10");
    const second = await mockInvoke<BrowsePage>("browse_table", {
      connectionId,
      req: { ...req, cursor: first.nextCursor },
    });
    expect(second.rows[0]?.[0]).toEqual({ t: "text", v: "o11" });
    expect(await mockInvoke("count_table_rows", { connectionId, table: orders })).toEqual({
      count: 25,
      exact: true,
    });
    const key = { kind: "pk", value: { _id: { t: "text", v: "o2" } } };
    expect(
      await mockInvoke("fetch_cell", { connectionId, table: orders, key, column: "status" }),
    ).toEqual({ t: "text", v: "paid" });
    await mockInvoke("disconnect", { connectionId });
  });

  it("previews and applies staged changes one at a time", async () => {
    const { connectionId } = await connectFake();
    const key = (id: string) => ({ kind: "pk", value: { _id: { t: "text", v: id } } });
    const req = {
      table: { schema: null, name: "orders" },
      returning: false,
      changes: [
        { op: "update", rowId: "1", key: key("o1"), changes: { status: { t: "text", v: "paid" } } },
        {
          op: "update",
          rowId: "2",
          key: key("o2"),
          changes: { status: { t: "text", v: "__fail__" } },
        },
      ],
    };
    const preview = await mockInvoke<string[]>("preview_changes", { connectionId, req });
    expect(preview[0]).toBe('db.orders.updateOne({_id: "o1"}, {$set: {status: "paid"}})');
    const result = await mockInvoke<ApplyResult>("apply_changes", { connectionId, req });
    expect(result.results.map((r) => r.error === null)).toEqual([true, false]);
    await mockInvoke("disconnect", { connectionId });
  });

  it("runs console lines as one batch, numbered, and stops at a failure", async () => {
    const { connectionId } = await connectFake();
    const events: QueryEvent[] = [];
    const results = await mockInvoke<StatementResult[]>("run_sql", {
      connectionId,
      queryId: "q1",
      sql: "count orders\n\n  find users\ndrop users\ncount users",
      options: {},
      onEvent: { send: (e: QueryEvent) => events.push(e) },
    });
    expect(results.map((r) => [r.index, r.sql, r.error === null])).toEqual([
      [0, "count orders", true],
      [1, "find users", true],
      [2, "drop users", false],
    ]);
    expect(results[1]?.span).toEqual({ start: 16, end: 26, line: 3 });
    const started = events.filter((e) => e.event === "started").map((e) => e.data);
    expect(started.map((d) => d.index)).toEqual([0, 1, 2]);
    expect(events.at(-1)?.event).toBe("done");
    await mockInvoke("disconnect", { connectionId });
  });

  it("ends the batch once, whatever a statement's own run sends", async () => {
    // A session that ends each statement's run with a `done` of its own.
    const fake = fakeMockDriver("mongodb");
    registerMockDriver("mongodb", {
      ...fake,
      connect: (profile) => {
        const session = fake.connect(profile);
        return {
          ...session,
          run: async (statement, emit) => {
            await session.run(statement, emit);
            emit({ event: "done", data: { totalMs: 1 } });
          },
        };
      },
    });
    buildMockDrivers(["mongodb"]);
    mockProfiles.set("p-fake-mongodb", fakeProfile());
    const { connectionId } = await mockInvoke<ConnectionInfo>("connect", {
      profileId: "p-fake-mongodb",
    });
    const events: QueryEvent[] = [];
    await mockInvoke("run_sql", {
      connectionId,
      queryId: "q2",
      sql: "count orders\ncount users",
      options: {},
      onEvent: { send: (e: QueryEvent) => events.push(e) },
    });
    expect(events.filter((e) => e.event === "done")).toHaveLength(1);
    expect(events.at(-1)?.event).toBe("done");
    await mockInvoke("disconnect", { connectionId });
  });

  it("answers Running Queries, routines, users and session details through the session's hooks", async () => {
    // Without the hooks, refused like the backend's missing hook.
    let { connectionId } = await connectFake();
    for (const [command, args] of [
      ["admin_list_processes", {}],
      ["admin_kill_processes", { ids: ["t1"], scope: "query" }],
      ["list_routines", { schema: "app" }],
      ["admin_list_users", { includeNologin: false }],
      ["session_details", {}],
    ] as const) {
      await expect(mockInvoke(command, { connectionId, ...args })).rejects.toMatchObject({
        code: "UNSUPPORTED",
      });
    }
    await mockInvoke("disconnect", { connectionId });

    const fake = fakeMockDriver("mongodb");
    const process: ProcessRow = {
      id: "t1",
      user: null,
      database: null,
      application: "tabler",
      client: null,
      state: "running",
      command: null,
      startedAt: null,
      durationMs: 5,
      waitEvent: null,
      query: "find users",
      current: false,
      own: true,
    };
    const routine: RoutineSummary = {
      schema: "app",
      name: "f",
      kind: "function",
      arguments: "",
      identityArguments: "",
      returnType: null,
      language: "js",
      comment: null,
    };
    const user: UserSummary = { name: "ada", host: null, canLogin: true, superuser: false };
    const details: SessionDetails = {
      tls: true,
      tlsVersion: null,
      tlsCipher: null,
      encoding: null,
    };
    registerMockDriver("mongodb", {
      ...fake,
      connect: (profile) => ({
        ...fake.connect(profile),
        listProcesses: () => [process],
        killProcesses: (ids, scope) =>
          ids.map((id) => ({ id, ok: scope === "query", message: null })),
        listRoutines: (schema) => [{ ...routine, schema }],
        routineDefinition: (r) => `function ${r.name}() {}`,
        listUsers: (includeNologin) => (includeNologin ? [user] : []),
        loadUser: (u) => ({
          state: {
            name: u.name,
            host: null,
            globalPrivileges: [],
            limits: null,
            attributes: null,
            databasePrivileges: [],
            schemaPrivileges: [],
            tablePrivileges: [],
            partialTablePrivileges: [],
          },
          schemas: [],
        }),
        sessionDetails: () => details,
      }),
    });
    buildMockDrivers(["mongodb"]);
    ({ connectionId } = await mockInvoke<ConnectionInfo>("connect", {
      profileId: "p-fake-mongodb",
    }));
    expect(await mockInvoke("admin_list_processes", { connectionId })).toEqual([process]);
    expect(
      await mockInvoke<KillOutcome[]>("admin_kill_processes", {
        connectionId,
        ids: ["t1"],
        scope: "query",
      }),
    ).toEqual([{ id: "t1", ok: true, message: null }]);
    expect(await mockInvoke("list_routines", { connectionId, schema: "app" })).toEqual([routine]);
    expect(
      await mockInvoke("get_routine_definition", {
        connectionId,
        routine: { schema: "app", name: "f", kind: "function", identityArguments: "" },
      }),
    ).toBe("function f() {}");
    expect(await mockInvoke("admin_list_users", { connectionId, includeNologin: true })).toEqual([
      user,
    ]);
    expect(
      await mockInvoke("admin_get_user", { connectionId, user: { name: "ada", host: null } }),
    ).toMatchObject({ state: { name: "ada" } });
    expect(await mockInvoke("session_details", { connectionId })).toEqual(details);
    await mockInvoke("disconnect", { connectionId });
  });

  it("answers the session's and the form's own calls", async () => {
    const { connectionId } = await connectFake();
    expect(
      await mockInvoke("session_call", {
        connectionId,
        method: "stats",
        args: { collection: "users" },
      }),
    ).toEqual({ collection: "users", count: 2 });
    expect(
      await mockInvoke("driver_util", {
        input: { profile: fakeProfile() },
        method: "listDatabases",
        args: null,
      }),
    ).toEqual(["app"]);
    await mockInvoke("disconnect", { connectionId });
    await expect(
      mockInvoke("session_call", { connectionId, method: "stats", args: null }),
    ).rejects.toMatchObject({ code: "NOT_CONNECTED" });
  });
});
