import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import {
  IpcError,
  ipc,
  isConnectionFailure,
  onConnectionFailure,
  type ConnStatus,
  type ConnectionInfo,
} from "@/lib/ipc";
import { registerMock } from "@/lib/ipc/mock";
import type { ErrorPayload } from "@/lib/ipc/transport";
import { useConnectionsStore } from "@/stores/connections";
import { createFailureProbe, FAILURE_PROBE_INTERVAL_MS } from "./useConnectionEvents";

const err = (payload: Partial<ErrorPayload> & { code: string }) =>
  new IpcError({ message: "boom", ...payload });

describe("isConnectionFailure", () => {
  const args = { connectionId: "c1" };
  const cases: Array<[string, string, Partial<ErrorPayload> & { code: string }, boolean]> = [
    ["dead PostgreSQL client", "browse_table", { code: "NOT_CONNECTED" }, true],
    ["socket error", "run_sql", { code: "CONNECT_FAILED" }, true],
    ["silent server", "list_tables", { code: "TIMEOUT" }, true],
    ["tunnel gone", "get_table", { code: "SSH_TUNNEL" }, true],
    ["connection exception", "run_sql", { code: "DB_ERROR", sqlstate: "08006" }, true],
    ["MySQL network error", "run_sql", { code: "DB_ERROR", sqlstate: "08S01" }, true],
    ["admin shutdown", "browse_table", { code: "DB_ERROR", sqlstate: "57P01" }, true],
    ["crash shutdown", "browse_table", { code: "DB_ERROR", sqlstate: "57P02" }, true],
    ["cannot connect now", "browse_table", { code: "DB_ERROR", sqlstate: "57P03" }, true],
    ["server gone away", "run_sql", { code: "DB_ERROR", vendorCode: 2006 }, true],
    ["lost during query", "run_sql", { code: "DB_ERROR", vendorCode: 2013 }, true],
    ["syntax error", "run_sql", { code: "DB_ERROR", sqlstate: "42601" }, false],
    ["unique violation", "apply_changes", { code: "DB_ERROR", sqlstate: "23505" }, false],
    ["query cancelled (57014)", "run_sql", { code: "DB_ERROR", sqlstate: "57014" }, false],
    ["other 57P code", "run_sql", { code: "DB_ERROR", sqlstate: "57P04" }, false],
    ["MySQL duplicate entry", "run_sql", { code: "DB_ERROR", vendorCode: 1062 }, false],
    ["cancelled", "run_sql", { code: "CANCELLED" }, false],
    ["file error", "export_data", { code: "IO" }, false],
    ["missing table", "get_table", { code: "NOT_FOUND" }, false],
    ["the probe itself", "ping", { code: "NOT_CONNECTED" }, false],
    ["disconnect", "disconnect", { code: "NOT_CONNECTED" }, false],
    ["cancel", "cancel_query", { code: "TIMEOUT" }, false],
    ["export cancel", "cancel_export", { code: "NOT_CONNECTED" }, false],
    ["host key", "accept_host_key", { code: "SSH_TUNNEL" }, false],
  ];
  it.each(cases)("%s (%s)", (_label, cmd, payload, expected) => {
    expect(isConnectionFailure(cmd, args, err(payload))).toBe(expected);
  });

  it("needs a connection id", () => {
    expect(isConnectionFailure("connect", { profileId: "p" }, err({ code: "TIMEOUT" }))).toBe(
      false,
    );
    expect(isConnectionFailure("test_connection", {}, err({ code: "CONNECT_FAILED" }))).toBe(false);
    expect(isConnectionFailure("run_sql", undefined, err({ code: "NOT_CONNECTED" }))).toBe(false);
  });
});

describe("onConnectionFailure", () => {
  it("reports failing commands of a connection and survives a throwing listener", async () => {
    const seen: string[] = [];
    const offBad = onConnectionFailure(() => {
      throw new Error("listener bug");
    });
    const off = onConnectionFailure((id, e) => seen.push(`${id}:${e.code}`));
    // The mock answers NOT_CONNECTED for a connection it does not know.
    await expect(ipc.schema.tables("nope", "public")).rejects.toMatchObject({
      code: "NOT_CONNECTED",
    });
    expect(seen).toEqual(["nope:NOT_CONNECTED"]);
    off();
    offBad();
    await expect(ipc.schema.tables("nope", "public")).rejects.toBeInstanceOf(IpcError);
    expect(seen).toHaveLength(1);
  });
});

describe("in-band errors of a dropped connection", () => {
  // These commands resolve even when the server is gone; the error travels in the result.
  const wire = (payload: Partial<ErrorPayload> & { code: string }) => ({
    message: "boom",
    detail: null,
    hint: null,
    sqlstate: null,
    vendorCode: null,
    position: null,
    statementIndex: 0,
    data: null,
    ...payload,
  });
  const statement = (error: ReturnType<typeof wire> | null) => ({
    index: 0,
    sql: "select 1",
    span: { start: 0, end: 8, line: 1 },
    kind: "query",
    columns: [],
    resultSets: 0,
    rowCount: 0,
    truncated: false,
    affected: null,
    lastInsertId: null,
    startedAt: "2026-01-01T00:00:00Z",
    durationMs: 1,
    error,
    notices: [],
  });

  let seen: string[];
  let off: () => void;
  beforeEach(() => {
    seen = [];
    off = onConnectionFailure((id, e) => seen.push(`${id}:${e.code}:${e.payload.sqlstate ?? ""}`));
  });
  afterEach(() => off());

  it("reports a run_sql statement error once per run", async () => {
    const dead = wire({ code: "NOT_CONNECTED" });
    registerMock("run_sql", (args) => {
      const channel = args.onEvent as { send?: (e: unknown) => void };
      channel.send?.({ event: "failed", data: { index: 0, error: dead } });
      channel.send?.({ event: "done", data: { totalMs: 1 } });
      return [statement(dead)];
    });
    const events: string[] = [];
    const results = await ipc.query.run("c1", "q1", "select 1", {}, (e) => events.push(e.event));
    expect(results[0]?.error?.code).toBe("NOT_CONNECTED");
    // The caller still gets every event.
    expect(events).toEqual(["failed", "done"]);
    expect(seen).toEqual(["c1:NOT_CONNECTED:"]);
  });

  it("reports a fatal statement error found only in the results", async () => {
    registerMock("run_sql", () => [
      statement(null),
      statement(wire({ code: "DB_ERROR", sqlstate: "08006" })),
    ]);
    await ipc.query.run("c1", "q1", "select 1; select 2", { stopOnError: false }, () => {});
    expect(seen).toEqual(["c1:DB_ERROR:08006"]);
  });

  it("ignores ordinary statement errors", async () => {
    registerMock("run_sql", (args) => {
      const error = wire({ code: "DB_ERROR", sqlstate: "42601" });
      (args.onEvent as { send?: (e: unknown) => void }).send?.({
        event: "failed",
        data: { index: 0, error },
      });
      return [statement(error)];
    });
    await ipc.query.run("c1", "q1", "selec 1", {}, () => {});
    expect(seen).toEqual([]);
  });

  it("reports a failed grid commit and a failed DDL apply", async () => {
    registerMock("apply_changes", () => ({
      success: false,
      results: [],
      sql: [],
      error: wire({ code: "DB_ERROR", sqlstate: "57P01" }),
      durationMs: 1,
    }));
    registerMock("apply_ddl", () => ({
      executed: [],
      failedIndex: null,
      error: wire({ code: "NOT_CONNECTED" }),
      durationMs: 1,
    }));
    const commit = await ipc.mutate.apply("c1", {
      table: { schema: "public", name: "t" },
      changes: [],
    } as never);
    expect(commit.success).toBe(false);
    const ddl = await ipc.ddl.apply("c2", { schema: "public", name: "t" }, []);
    expect(ddl.error?.code).toBe("NOT_CONNECTED");
    expect(seen).toEqual(["c1:DB_ERROR:57P01", "c2:NOT_CONNECTED:"]);
  });

  it("ignores a commit that failed on a constraint", async () => {
    registerMock("apply_changes", () => ({
      success: false,
      results: [],
      sql: [],
      error: wire({ code: "DB_ERROR", sqlstate: "23505" }),
      durationMs: 1,
    }));
    await ipc.mutate.apply("c1", {} as never);
    expect(seen).toEqual([]);
  });
});

describe("createFailureProbe", () => {
  let status: ConnStatus | undefined;
  let ping: Mock<(id: string) => Promise<ConnStatus>>;
  let onResult: Mock<(id: string, status: ConnStatus | null, error?: unknown) => void>;
  let clock: number;

  beforeEach(() => {
    vi.useFakeTimers();
    status = "connected";
    clock = 0;
    ping = vi.fn(async (): Promise<ConnStatus> => "lost");
    onResult = vi.fn();
  });
  afterEach(() => vi.useRealTimers());

  const make = () =>
    createFailureProbe({
      ping,
      status: () => status,
      onResult,
      now: () => clock,
    });

  it("probes a connected connection once per interval", async () => {
    const probe = make();
    expect(probe("c1")).toBe(true);
    await vi.runAllTimersAsync();
    expect(onResult).toHaveBeenCalledWith("c1", "lost");
    clock += FAILURE_PROBE_INTERVAL_MS - 1;
    expect(probe("c1")).toBe(false);
    clock += 1;
    expect(probe("c1")).toBe(true);
    // Each connection has its own budget.
    expect(probe("c2")).toBe(true);
    expect(ping).toHaveBeenCalledTimes(3);
  });

  it("keeps at most one probe in flight per connection", async () => {
    let answer: (s: ConnStatus) => void = () => {};
    ping.mockImplementation(() => new Promise<ConnStatus>((r) => (answer = r)));
    const probe = make();
    expect(probe("c1")).toBe(true);
    clock += FAILURE_PROBE_INTERVAL_MS * 2;
    expect(probe("c1")).toBe(false);
    answer("connected");
    await vi.runAllTimersAsync();
    expect(onResult).toHaveBeenCalledWith("c1", "connected");
    expect(probe("c1")).toBe(true);
  });

  it("only probes connections that still look connected", () => {
    const probe = make();
    for (const s of ["lost", "reconnecting", "disconnected", undefined] as const) {
      status = s;
      expect(probe("c1")).toBe(false);
    }
    expect(ping).not.toHaveBeenCalled();
  });

  it("reports a ping that never answers as a failure", async () => {
    ping.mockImplementation(() => new Promise<ConnStatus>(() => {}));
    const probe = make();
    probe("c1");
    await vi.advanceTimersByTimeAsync(8_000);
    expect(onResult).toHaveBeenCalledWith("c1", null, expect.objectContaining({ code: "TIMEOUT" }));
  });

  it("ignores NOT_CONNECTED from the ping itself", async () => {
    ping.mockRejectedValue(new IpcError({ code: "NOT_CONNECTED", message: "not connected" }));
    const probe = make();
    probe("c1");
    await vi.runAllTimersAsync();
    expect(onResult).not.toHaveBeenCalled();
  });
});

describe("a failing query flips the connection to lost", () => {
  function info(connectionId: string): ConnectionInfo {
    return {
      connectionId,
      profileId: "p-local-pg",
      name: "Local PG",
      color: null,
      driver: "postgres",
      serverVersion: "17",
      currentDatabase: "postgres",
      currentUser: "postgres",
      tunnel: null,
      readOnly: false,
    };
  }

  it("probes through the real IPC path", async () => {
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    useConnectionsStore.getState().add(info("gone"));
    const results: Array<ConnStatus | null> = [];
    const probe = createFailureProbe({
      ping: (id) => ipc.connections.ping(id),
      status: (id) => useConnectionsStore.getState().byId[id]?.status,
      onResult: (_id, s) => results.push(s),
    });
    const off = onConnectionFailure((id) => void probe(id));
    await expect(
      ipc.browse.cell("gone", { schema: null, name: "t" }, { kind: "rowid", value: 1 }, "x"),
    ).rejects.toMatchObject({ code: "NOT_CONNECTED" });
    await vi.waitFor(() => expect(results).toEqual(["disconnected"]));
    off();
  });
});
