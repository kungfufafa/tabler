import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { ipc, type SqlLogEvent } from "@/lib/ipc";
import type { UserChange } from "@/lib/ipc/bindings/UserChange";
import { registerMock } from "@/lib/ipc/mock";
import { mockHandler } from "@/lib/ipc/mock/transport";
import { drainMockTrace, resetMockTrace } from "@/lib/ipc/mock/trace";
import { resetTraceLedger } from "@/lib/ipc/sqlTrace";
import { useConnectionsStore } from "@/stores/connections";
import { useConsoleStore } from "./store";

// What the console shows of the statements that are not the SQL editor's: the Explain command and
// the transaction around an Analyze, a SQL dump import, and user changes (which carry a password).
// Against the in-memory mock, which records what the real backend's sessions record.

let c = "";
const entries = () => useConsoleStore.getState().entries;
const texts = () => entries().map((e) => e.sql);
const restoreDrain = () =>
  registerMock("drain_sql_trace", ({ connectionId }) => drainMockTrace(connectionId as string));

beforeAll(async () => {
  const info = await ipc.connections.connect("p-local-pg");
  c = info.connectionId;
  useConnectionsStore.getState().add(info);
});

afterAll(async () => {
  await ipc.connections.disconnect(c);
  useConnectionsStore.getState().remove(c);
});

beforeEach(() => {
  useConsoleStore.getState().clear();
  resetMockTrace();
  resetTraceLedger();
});

afterEach(restoreDrain);

describe("the explain command", () => {
  it("logs the EXPLAIN the backend wrapped around the statement, not the statement itself", async () => {
    await ipc.query.explain(c, "select * from users;", "plan");
    expect(texts()).toEqual(["EXPLAIN (VERBOSE, COSTS, FORMAT TEXT) select * from users"]);
    expect(entries()[0]).toMatchObject({ category: "data", meta: false });
    expect(entries()[0]!.rows).toBeGreaterThan(0);
  });

  it("logs an Analyze with the transaction it runs in", async () => {
    await ipc.query.explain(c, "update users set bio = 'x'", "analyze");
    expect(texts()).toEqual([
      "BEGIN",
      "EXPLAIN (ANALYZE, BUFFERS, VERBOSE, FORMAT TEXT) update users set bio = 'x'",
      "ROLLBACK",
    ]);
  });

  it("describes the statement when it never reached the server", async () => {
    await expect(ipc.query.explain(c, "bogus things", "plan")).rejects.toBeTruthy();
    expect(entries()).toEqual([
      expect.objectContaining({
        sql: "-- explain bogus things",
        meta: true,
        label: "EXPLAIN",
        error: expect.any(String),
      }),
    ]);
  });
});

describe("a SQL dump import", () => {
  const request = (path: string) => ({ importId: "fx3-dump", path, stopOnError: false });

  it("logs the statements of the dump, not a description of the file", async () => {
    const path = "/tmp/fx3-dump.sql";
    await ipc.imports.previewSql(c, path);
    await ipc.imports.sql(c, request(path), () => undefined);
    expect(entries().length).toBeGreaterThan(3);
    expect(entries().every((e) => !e.meta && !e.sql.startsWith("--"))).toBe(true);
    expect(texts()[0]).toBe("SET NAMES utf8mb4");
    // The DROP and the CREATE are structure, the INSERTs data.
    expect(
      entries()
        .map((e) => e.category)
        .slice(0, 4),
    ).toEqual(["data", "structure", "structure", "data"]);
    expect(texts().some((t) => t.startsWith("INSERT INTO `demo_items`"))).toBe(true);
  });

  it("logs the failing statement with its error", async () => {
    const path = "/tmp/fx3-bad-dump.sql";
    await ipc.imports.previewSql(c, path);
    await ipc.imports.sql(c, request(path), () => undefined);
    const failed = entries().filter((e) => e.error !== undefined);
    expect(failed.map((e) => e.sql)).toEqual(["INSERT INTO `missing_table` VALUES (1)"]);
    expect(failed[0]!.error).toContain("missing_table");
  });

  it("falls back to the description when the backend recorded nothing", async () => {
    await ipc.imports.sql(c, request("/tmp/fx3-unpreviewed.sql"), () => undefined);
    expect(entries().map((e) => [e.sql, e.meta])).toEqual([
      ["-- import SQL dump fx3-unpreviewed.sql", true],
    ]);
  });
});

describe("a user change with a password", () => {
  const PASSWORD = "fx3-s3cr3t it's";
  // The mock keeps the accounts it created: every test creates its own.
  let n = 0;
  const change = (password: string | null, name = `fx3_role_${n++}`): UserChange => ({
    original: null,
    edited: {
      name,
      host: null,
      globalPrivileges: [],
      limits: null,
      attributes: {
        superuser: false,
        createDb: false,
        createRole: false,
        login: true,
        replication: false,
        inherit: true,
        connectionLimit: -1,
        validUntil: null,
      },
      databasePrivileges: [],
      schemaPrivileges: [],
      tablePrivileges: [],
      partialTablePrivileges: [],
    },
    password,
  });
  const leaked = (list: readonly SqlLogEvent[]) =>
    list.some((e) => `${e.sql} ${e.error ?? ""}`.includes("fx3-s3cr3t") || e.sql.includes("it''s"));

  it("logs the masked statements once, from the backend's trace and the result alike", async () => {
    const result = await ipc.admin.applyUser(c, change(PASSWORD));
    expect(result.error).toBeNull();
    expect(result.executed.join("")).toContain("********");
    // The mock records the statements as the real backend does (masked): the call's own entry and
    // the trace's are the same lines, logged once.
    expect(texts()).toEqual([result.executed.join(";\n")]);
    expect(leaked(entries())).toBe(false);
  });

  it("masks the password even when a statement reaches the log with it (a second lock)", async () => {
    const second = change(PASSWORD);
    const [shown] = await ipc.admin.previewUser(c, second);
    // A backend that recorded the statement as it ran, password and all: the quote doubled as the
    // literal wrote it.
    const raw = (shown ?? "").replace("'********'", "'fx3-s3cr3t it''s'");
    expect(raw).toContain("fx3-s3cr3t");
    registerMock("drain_sql_trace", () => [
      {
        sql: raw,
        category: "structure",
        durationMs: 3,
        rows: null,
        affected: null,
        error: `syntax error near 'fx3-s3cr3t it''s'`,
      },
    ]);
    await ipc.admin.applyUser(c, second);
    // Masked, and so the same line as the result's own entry: logged once.
    expect(leaked(entries())).toBe(false);
    expect(entries()).toHaveLength(1);
    expect(entries()[0]!.sql).toContain("********");
    expect(entries()[0]!.error).toBeUndefined();
  });

  it("masks the password in a failure the backend reports, and in the statement it recorded", async () => {
    const original = mockHandler("admin_apply_user")!;
    registerMock("admin_apply_user", async () => ({
      executed: [],
      failedIndex: 0,
      error: { code: "DB_ERROR", message: "bad value fx3-s3cr3t it's", hint: null },
      durationMs: 1,
    }));
    registerMock("drain_sql_trace", () => [
      {
        sql: "CREATE ROLE fx3_role WITH LOGIN PASSWORD E'fx3-s3cr3t it''s'",
        category: "structure",
        durationMs: 2,
        rows: null,
        affected: null,
        error: "bad value fx3-s3cr3t it's",
      },
    ]);
    try {
      await ipc.admin.applyUser(c, change(PASSWORD));
    } finally {
      registerMock("admin_apply_user", original);
    }
    expect(leaked(entries())).toBe(false);
    expect(entries().map((e) => e.sql)).toEqual([
      "CREATE ROLE fx3_role WITH LOGIN PASSWORD E'********'",
    ]);
    expect(entries()[0]!.error).toBe("bad value ********");
  });

  it("changes nothing for a user change without a password", async () => {
    const result = await ipc.admin.applyUser(c, change(null));
    expect(result.error).toBeNull();
    expect(texts()).toEqual([result.executed.join(";\n")]);
  });
});
