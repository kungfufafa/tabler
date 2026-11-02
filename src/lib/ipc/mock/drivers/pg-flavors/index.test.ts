/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { classifyDialect } from "@/features/drivers/portable/classify";
import { split } from "@/features/drivers/portable/scanner";
import type { DialectLex } from "../../../bindings/DialectLex";
import REDSHIFT_LEX from "../../../bindings/lex/redshift.json";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import type { MockSession } from "../types";
import lane, { engineOf, flavors } from ".";
import { type Engine, flavorCaps } from "./caps";
import {
  GUARD_MESSAGE,
  READ_ONLY_EDIT,
  READ_ONLY_MESSAGE,
  READ_ONLY_STARTUP,
  readOnlyByEncoding,
  startupWrites,
} from "./session";

// The mock's CockroachDB, Greenplum and Redshift: caps equal to the backend's recorded ones
// (`imp::tests::caps_match_the_recorded_fixture` writes the same file), the catalog, pages,
// counts, a commit with one failing change, the refusals of a read-only profile and of what
// the caps leave out, and each engine's own calls.

const ROOT = path.resolve(import.meta.dirname, "../../../../../..");
const RECORDED = JSON.parse(
  readFileSync(path.join(ROOT, "src-tauri/tests/fixtures/pg_flavors/caps.json"), "utf8"),
) as Record<string, SessionCaps>;

const seedOf = (id: string): ConnectionProfile => {
  for (const d of Object.values(flavors)) {
    const p = d.seedProfiles().find((s) => s.id === id);
    if (p) return p;
  }
  throw new Error(id);
};

function connect(id: string, readOnly = false): MockSession {
  const p = seedOf(id);
  const profile = { ...p, options: { ...p.options, readOnly } };
  return flavors[p.options.pgFlavor!].connect(profile);
}

async function run(s: MockSession, sql: string): Promise<QueryEvent[]> {
  const events: QueryEvent[] = [];
  await s.run(sql, (e) => events.push(e));
  return events;
}

const thrown = (f: () => unknown): { code: string; message: string } => {
  try {
    f();
  } catch (e) {
    return e as { code: string; message: string };
  }
  throw new Error("did not throw");
};

const page = { columns: null, filter: null, sort: [], limit: 10, offset: 0 };

describe("the pg-flavors mock lane", () => {
  it("has no driver of its own and one mock per engine, served as PostgreSQL", () => {
    expect(lane.drivers).toEqual([]);
    expect(Object.keys(flavors).sort()).toEqual(["cockroachdb", "greenplum", "redshift"]);
    for (const d of Object.values(flavors)) {
      expect(d.driver).toBe("postgres");
      for (const p of d.seedProfiles()) expect(p.driver).toBe("postgres");
    }
    expect(engineOf("greenplum", seedOf("p-greenplum6"))).toBe("greenplum6");
    expect(engineOf("greenplum", seedOf("p-greenplum7"))).toBe("greenplum7");
    expect(flavors.redshift.serverVersion(seedOf("p-redshift"))).toContain("Redshift 1.0");
  });

  it("gives each engine the caps the backend records", () => {
    for (const engine of ["cockroachdb", "greenplum6", "greenplum7", "redshift"] as Engine[])
      expect(flavorCaps(engine), engine).toEqual(RECORDED[engine]);
    const ids: Record<Engine, string> = {
      cockroachdb: "p-cockroachdb",
      greenplum6: "p-greenplum6",
      greenplum7: "p-greenplum7",
      redshift: "p-redshift",
    };
    for (const [engine, id] of Object.entries(ids))
      expect(connect(id).caps, engine).toEqual(RECORDED[engine]);
  });

  it("offers a method only where the caps grant its feature", () => {
    const crdb = connect("p-cockroachdb");
    expect(crdb.sessionDetails).toBeUndefined();
    expect(crdb.listUsers).toBeUndefined();
    const gp6 = connect("p-greenplum6");
    expect(gp6.listUsers).toBeDefined();
    expect(gp6.sessionDetails).toBeUndefined();
    expect(connect("p-greenplum7").sessionDetails?.()).toMatchObject({
      tls: true,
      encoding: "UTF8",
    });
    const rs = connect("p-redshift");
    expect(rs.listUsers).toBeUndefined();
    expect(rs.sessionDetails).toBeUndefined();
    expect(rs.caps.features).not.toContain("returning");
  });

  it("lists, browses by offset and counts", () => {
    const s = connect("p-cockroachdb");
    expect(s.listDatabases().find((d) => d.isCurrent)?.name).toBe("defaultdb");
    expect(s.listSchemas()).toEqual([{ name: "public", isDefault: true }]);
    expect(s.listTables("public").map((t) => t.name)).toEqual(["users", "rides"]);
    const rides = { schema: "public", name: "rides" };
    const meta = s.tableMeta(rides);
    // The hidden `rowid` is the key, generated.
    expect(meta.primaryKey).toEqual(["rowid"]);
    expect(meta.columns.find((c) => c.name === "rowid")).toMatchObject({
      isGenerated: true,
      isAutoIncrement: true,
      defaultExpr: "unique_rowid()",
    });
    const first = s.browse({ ...page, table: rides });
    const second = s.browse({ ...page, table: rides, offset: 40 });
    expect(first.rows).toHaveLength(10);
    expect(second.rows).toHaveLength(5);
    expect(second.sql).toBe(
      'SELECT "city", "revenue", "rowid" FROM "public"."rides" LIMIT 10 OFFSET 40',
    );
    expect(s.count?.(rides, null)).toEqual({ count: 45, exact: true });
  });

  it("shows each engine's storage, keys and DDL", () => {
    const gp6 = connect("p-greenplum6");
    const orders = { schema: "sales", name: "orders" };
    expect(gp6.listTables("sales")[0]?.kind).toBe("partitioned");
    expect(gp6.tableMeta(orders).options).toMatchObject({
      distribution: "DISTRIBUTED BY (id)",
      engine: "ao_row (appendonly=true, compresstype=zstd)",
    });
    expect(gp6.tableDdl?.(orders)).toContain("PARTITION BY RANGE(order_date)");
    expect(connect("p-greenplum7").tableDdl?.(orders)).toContain("USING ao_row");
    const rs = connect("p-redshift");
    const sales = { schema: "public", name: "sales" };
    const meta = rs.tableMeta(sales);
    expect(meta.options).toMatchObject({
      distribution: "DISTSTYLE KEY(salesid)",
      sortKey: "SORTKEY (salesid)",
    });
    expect(meta.columns[0]?.extra).toBe("DISTKEY, SORTKEY 1, ENCODE az64");
    expect(rs.listTables("public")[0]?.sizeBytes).toBe(5_242_880);
    expect(rs.listTables("spectrum")[0]?.kind).toBe("foreignTable");
    expect(rs.browse({ ...page, table: sales }).sql).toContain('JSON_SERIALIZE("details")');
  });

  it("commits in one transaction that a failing change rolls back", () => {
    const s = connect("p-greenplum7");
    const t = { schema: "public", name: "customers" };
    const key = (id: number) => ({
      kind: "pk" as const,
      value: { id: { t: "int" as const, v: id } },
    });
    const result = s.apply!({
      table: t,
      returning: true,
      changes: [
        { op: "update", rowId: "a", key: key(1), changes: { region: { t: "text", v: "APAC" } } },
        { op: "update", rowId: "b", key: key(2), changes: { name: { t: "null" } } },
      ],
    });
    expect(result.success).toBe(false);
    expect(result.results.map((r) => r.error?.message ?? null)).toEqual([
      null,
      'null value in column "name" violates not-null constraint',
    ]);
    expect(result.sql[0]).toBe(
      `UPDATE "public"."customers" SET "region" = 'APAC' WHERE "id" = 1 RETURNING *`,
    );
    // Rolled back: the first change is not there.
    expect(s.fetchCell?.(t, key(1), "region")).toEqual({ t: "text", v: "EMEA" });
    const ok = s.apply!({
      table: t,
      returning: true,
      changes: [
        { op: "update", rowId: "a", key: key(1), changes: { region: { t: "text", v: "APAC" } } },
      ],
    });
    expect(ok.success).toBe(true);
    expect(ok.results[0]?.returned?.region).toEqual({ t: "text", v: "APAC" });
    const gone = s.apply!({
      table: t,
      returning: false,
      changes: [{ op: "delete", rowId: "c", key: key(999) }],
    });
    expect(gone.error?.message).toBe(GUARD_MESSAGE);
  });

  it("writes Redshift SQL: no RETURNING, backslashes doubled, SUPER through JSON_PARSE", () => {
    const s = connect("p-redshift");
    const key = { kind: "pk" as const, value: { salesid: { t: "int" as const, v: 3 } } };
    const req = {
      table: { schema: "public", name: "sales" },
      returning: true,
      changes: [
        {
          op: "update" as const,
          rowId: "a",
          key,
          changes: {
            venue: { t: "text" as const, v: "a\\b 'c'" },
            details: { t: "json" as const, v: '{"x":1}' },
          },
        },
      ],
    };
    expect(s.preview!(req)).toEqual([
      `UPDATE "public"."sales" SET "venue" = 'a\\\\b ''c''', "details" = JSON_PARSE('{"x":1}') WHERE "salesid" = 3`,
    ]);
    const result = s.apply!(req);
    expect(result.success).toBe(true);
    expect(result.results[0]?.returned).toBeNull();
    // Keyed by all columns: Redshift has no EditWithoutKey.
    const byAll = thrown(() =>
      s.preview!({
        ...req,
        changes: [{ op: "delete", rowId: "x", key: { kind: "allColumns", value: {} } }],
      }),
    );
    expect(byAll.message).toBe(
      "unsupported: Amazon Redshift: editing rows without a primary key is not available",
    );
  });

  it("refuses a read-only profile's edits and editor writes, and runs its reads", async () => {
    for (const id of ["p-cockroachdb", "p-greenplum6", "p-redshift"]) {
      const s = connect(id, true);
      const t = s.listTables("public")[0]!;
      const req = {
        table: { schema: "public", name: t.name },
        returning: false,
        changes: [{ op: "delete" as const, rowId: "a", key: { kind: "pk" as const, value: {} } }],
      };
      expect(thrown(() => s.preview!(req))).toEqual({
        code: "UNSUPPORTED",
        message: READ_ONLY_EDIT,
      });
      expect(thrown(() => s.apply!(req))).toEqual({ code: "UNSUPPORTED", message: READ_ONLY_EDIT });
      expect(thrown(() => s.run(`DELETE FROM ${t.name}`, () => {}))).toEqual({
        code: "INVALID_INPUT",
        message: READ_ONLY_MESSAGE,
      });
      const events = await run(s, `SELECT * FROM ${t.name} LIMIT 2`);
      expect(events.map((e) => e.event)).toEqual(["started", "columns", "rows", "finished"]);
    }
    // Redshift SQL's classifier: a write in attribute notation is still a write.
    const rs = connect("p-redshift", true);
    expect(thrown(() => rs.run("SELECT (123).pg_terminate_backend", () => {})).message).toBe(
      READ_ONLY_MESSAGE,
    );
  });

  it("refuses a read-only Redshift connect whose startup commands may write", () => {
    const open = (id: string, readOnly: boolean, startupSql: string) => {
      const p = seedOf(id);
      return () =>
        flavors[p.options.pgFlavor!].connect({
          ...p,
          options: { ...p.options, readOnly, startupSql },
        });
    };
    for (const sql of [
      "SET search_path = app; DELETE FROM t",
      "CALL purge()",
      "SET search_path = 'a\\'; DELETE FROM t; --'",
    ]) {
      expect(thrown(open("p-redshift", true, sql)), sql).toEqual({
        code: "INVALID_INPUT",
        message: READ_ONLY_STARTUP,
      });
      expect(open("p-redshift", false, sql)()).toBeTruthy();
      // The other engines' read-only is the server's own.
      expect(open("p-cockroachdb", true, sql)()).toBeTruthy();
    }
    for (const sql of ["SET search_path = app, public; RESET query_group", "SELECT 1", ""])
      expect(open("p-redshift", true, sql)(), sql).toBeTruthy();
    expect(startupWrites("set datestyle to 'ISO'")).toBe(false);
    // Comments the server may end elsewhere, outside every statement, `$$` that a server may
    // read as code, and a DELETE that only the plain reading's split shows, each piece cut where a
    // backslash escapes being one SET to every reading (`imp/redshift.rs::startup_writes`).
    const dollars = "SELECT $$; DELETE FROM t; --$$;";
    for (const sql of [
      "SET search_path = app; /* /* */ DELETE FROM t; */",
      "/* /* */ DELETE FROM t; */ SET search_path = app;",
      dollars,
      String.raw`SET a = '\'' ; SET b = ' ; DELETE FROM t ; '`,
      String.raw`RESET a; SET b = '\'' ; SET c = ' ; DELETE FROM t ; --'`,
    ]) {
      expect(startupWrites(sql), sql).toBe(true);
      expect(thrown(open("p-redshift", true, sql)), sql).toEqual({
        code: "INVALID_INPUT",
        message: READ_ONLY_STARTUP,
      });
    }
    expect(startupWrites("SET search_path = app; /* plain */ RESET query_group")).toBe(false);
    expect(startupWrites("SET search_path = '$user', app")).toBe(false);
    // Under Redshift SQL's lexer alone the `$$` text is one read-only SELECT: only the reading
    // with dollar quotes off sees the DELETE.
    const lex = REDSHIFT_LEX as DialectLex;
    expect(split(lex.lex, dollars)).toHaveLength(1);
    expect(classifyDialect(lex, dollars)).toBe("read");
  });

  it("refuses a Redshift connect that a non-UTF-8 encoding made read-only", () => {
    const p = seedOf("p-redshift");
    const open = (encoding: string, readOnly: boolean, startupSql: string) => () =>
      flavors.redshift.connect({ ...p, options: { ...p.options, encoding, readOnly, startupSql } });
    const writes = "SET search_path = app; INSERT INTO audit VALUES (1)";
    // Nothing on Redshift enforces an encoding's read-only, so it is refused whatever the startup
    // commands are; with the profile's read-only on, only a writing command is.
    for (const sql of [writes, "SET search_path = app", ""])
      expect(thrown(open(" LATIN1 ", false, sql)), sql).toEqual({
        code: "INVALID_INPUT",
        message: readOnlyByEncoding("LATIN1"),
      });
    expect(thrown(open("LATIN1", true, writes))).toEqual({
      code: "INVALID_INPUT",
      message: READ_ONLY_STARTUP,
    });
    expect(open("LATIN1", true, "SET search_path = app")()).toBeTruthy();
    expect(open("UTF8", false, writes)()).toBeTruthy();
    expect(open("unicode", false, "")()).toBeTruthy();
    // The other engines' servers enforce it.
    const g = seedOf("p-greenplum7");
    expect(
      flavors.greenplum.connect({ ...g, options: { ...g.options, encoding: "LATIN1" } }),
    ).toBeTruthy();
  });

  it("runs the editor's statements as each server answers them", async () => {
    const rs = connect("p-redshift");
    const version = await run(rs, "SELECT version()");
    const rows = version.find((e) => e.event === "rows");
    expect(rows?.event === "rows" && rows.data.rows[0]?.[0]).toMatchObject({ t: "text" });
    const savepoint = await run(rs, "SAVEPOINT a");
    expect(savepoint.at(-1)?.event).toBe("failed");
    const crdb = connect("p-cockroachdb");
    expect((await run(crdb, "SAVEPOINT a")).at(-1)?.event).toBe("finished");
    const missing = await run(crdb, "SELECT * FROM nope");
    const failed = missing.at(-1);
    expect(failed?.event === "failed" && failed.data.error.sqlstate).toBe("42P01");
    const deleted = await run(crdb, "DELETE FROM rides");
    const done = deleted.at(-1);
    expect(done?.event === "finished" && done.data.result.affected).toBe(45);
  });

  it("lists and kills sessions by each engine's ids", () => {
    const crdb = connect("p-cockroachdb");
    const procs = crdb.listProcesses!();
    expect(procs.find((p) => p.current)?.id).toMatch(/^[0-9a-f]+$/);
    expect(crdb.killProcesses!([procs[1]!.id, "12; DROP"], "query")).toEqual([
      { id: procs[1]!.id, ok: true, message: null },
      { id: "12; DROP", ok: false, message: "invalid session id 12; DROP" },
    ]);
    const rs = connect("p-redshift");
    expect(rs.killProcesses!(["abc"], "connection")[0]?.message).toBe("invalid process id abc");
    expect(rs.killProcesses!(["42"], "connection")[0]?.message).toBe(
      "No such process: it may have ended already",
    );
  });

  it("lists routines and shows their definitions, refusing what Redshift does not keep", () => {
    const rs = connect("p-redshift");
    const routines = rs.listRoutines!("public");
    expect(routines.map((r) => [r.name, r.kind, r.identityArguments])).toEqual([
      ["load", "procedure", "integer, character varying"],
      ["f_add", "function", "integer, integer"],
      ["f_lambda", "function", "bigint"],
    ]);
    const ref = (r: (typeof routines)[number]) => ({
      schema: r.schema,
      name: r.name,
      kind: r.kind,
      identityArguments: r.identityArguments,
    });
    expect(rs.routineDefinition!(ref(routines[1]!))).toContain('FUNCTION "public"."f_add"');
    expect(thrown(() => rs.routineDefinition!(ref(routines[2]!)))).toEqual({
      code: "UNSUPPORTED",
      message: "unsupported: the definition of a exfunc function is not kept by Redshift",
    });
    expect(
      thrown(() => rs.routineDefinition!({ ...ref(routines[0]!), identityArguments: "text" })).code,
    ).toBe("NOT_FOUND");
    expect(connect("p-cockroachdb").listRoutines!("public").map((r) => r.kind)).toEqual([
      "function",
      "procedure",
    ]);
  });

  it("lists and loads Greenplum's roles", () => {
    const gp = connect("p-greenplum6");
    expect(gp.listUsers!(false).map((u) => u.name)).toEqual(["gpadmin", "etl"]);
    expect(gp.listUsers!(true)).toHaveLength(3);
    const etl = gp.loadUser!({ name: "etl", host: null });
    expect(etl.state.attributes?.login).toBe(true);
    expect(etl.schemas).toEqual(["public", "sales"]);
  });
});
