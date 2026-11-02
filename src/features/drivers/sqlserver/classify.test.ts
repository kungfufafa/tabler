import { afterEach, describe, expect, it } from "vitest";
import { CORE_KINDS } from "@/features/drivers/catalog";
import { classifyDialect, loadDialectLex } from "@/features/drivers/portable/classify";
import { sessionWrites, type GuardRequest } from "@/features/safety/guard";
import { BASE_CAPS } from "@/lib/caps";
import type { ConnectionInfo } from "@/lib/ipc";
import { refusedReadOnly } from "@/lib/ipc/mock/drivers/sqlserver/session";
import { useAppInfoStore } from "@/stores/appInfo";
import lane from ".";
import { callsSideEffect, classifyTsql, hidesStatement } from "./classify";

// T-SQL's own Safe Mode checks (`tsqlUi.classify`): what a read-only session refuses beyond the
// portable rules, so a writable session asks before it runs too.

/** Reads to the portable classifier that a read-only session refuses. */
const HIDDEN = [
  "SELECT 1 {call p}",
  "SELECT 1 { ? = call p (1) }",
  "SELECT 1 CALL p",
  "SELECT 1 DUMP TRANSACTION x",
  "SELECT 1 LOAD DATABASE x",
  "SELECT 1 WHERE a = 1e0DUMP TRANSACTION x",
  "SELECT 1.e5LOAD DATABASE x",
];
const SIDE_EFFECTS = [
  "SELECT NEXT VALUE FOR dbo.order_ids",
  "SELECT next   value\nfor s",
  "SELECT a, NEXT VALUE FOR s OVER (ORDER BY a) FROM t",
];
const READS = [
  "SELECT 1",
  "SELECT * FROM sys.tables",
  "SELECT [call], 'DUMP', \"load\" -- CALL p",
  "SELECT {fn NOW()}, {d '2024-01-01'}",
  "SELECT payload, download FROM t",
  "SELECT 'NEXT VALUE FOR s'",
  "SELECT [next], value FROM t",
];

describe("tsqlUi.classify", () => {
  it("is the dialect's classifier", () => {
    expect(lane.dialects!.tsql!.classify).toBe(classifyTsql);
  });

  it("calls a hidden statement or a side-effect function a write, as refused_read_only", async () => {
    const lex = (await loadDialectLex("tsql"))!;
    for (const sql of [...HIDDEN, ...SIDE_EFFECTS]) {
      // The portable classifier alone lets each of them through.
      expect(classifyDialect(lex, sql), sql).toBe("read");
      expect(classifyTsql(sql), sql).toBe("write");
      expect(refusedReadOnly(sql), sql).toBe(true);
    }
    for (const sql of HIDDEN) expect(hidesStatement(sql), sql).toBe(true);
    for (const sql of SIDE_EFFECTS) expect(callsSideEffect(sql), sql).toBe(true);
    for (const sql of READS) {
      expect(classifyTsql(sql), sql).toBe("read");
      expect(refusedReadOnly(sql), sql).toBe(false);
    }
    expect(classifyTsql("SELECT xp_cmdshell('dir')")).toBe("write");
    expect(classifyTsql("SELECT * FROM OPENQUERY(srv, 'SELECT 1')")).toBe("write");
  });
});

describe("Safe Mode on a SQL Server session", () => {
  const session: Pick<ConnectionInfo, "driver" | "capabilities"> = {
    driver: "sqlserver",
    capabilities: { ...BASE_CAPS.sqlserver, dialect: "tsql" },
  };
  const query = (...statements: string[]): GuardRequest => ({ kind: "query", statements });

  afterEach(() => useAppInfoStore.setState({ info: null }));

  it("confirms what the lane's classifier calls a write, beside the portable one", async () => {
    useAppInfoStore.setState({
      info: { version: "1", platform: "test", debug: true, kinds: [...CORE_KINDS, "sqlserver"] },
    });
    await expect(sessionWrites(session, query("SELECT 1", "SELECT [call] FROM t"))).resolves.toBe(
      false,
    );
    for (const sql of [...HIDDEN, ...SIDE_EFFECTS, "DELETE FROM t", "SELECT 1 DELETE FROM t"])
      await expect(sessionWrites(session, query("SELECT 1", sql)), sql).resolves.toBe(true);
  });
});
