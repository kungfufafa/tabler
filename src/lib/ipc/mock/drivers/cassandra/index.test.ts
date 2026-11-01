/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { beforeEach, describe, expect, it } from "vitest";
import type { BrowseRequest } from "../../../bindings/BrowseRequest";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { FilterGroup } from "../../../bindings/FilterGroup";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { Value } from "../../../bindings/Value";
import type { MockSession } from "../types";
import lane, { cassandraMock } from ".";
import { resetCluster } from "./data";
import { cassandraCaps, READ_ONLY_MESSAGE } from "./session";

// The mock's Cassandra: caps equal to the backend's recorded ones
// (`drivers::cassandra::tests::caps_match_the_recorded_fixture` writes the same file), keyspaces
// as databases, pages by cursor, counts, a commit with one failing change, the editor with cqlsh's
// commands, and the refusals of a read-only profile and of what CQL cannot do.

const ROOT = path.resolve(import.meta.dirname, "../../../../../..");
const RECORDED = JSON.parse(
  readFileSync(path.join(ROOT, "src-tauri/tests/fixtures/cassandra/caps.json"), "utf8"),
) as Record<"password" | "command", SessionCaps>;

const seed = (id: string): ConnectionProfile => {
  const p = cassandraMock.seedProfiles().find((s) => s.id === id);
  if (!p) throw new Error(id);
  return p;
};

function connect(
  options: Partial<ConnectionProfile["options"]> = {},
  fields: Partial<ConnectionProfile> = {},
): MockSession {
  const p = seed("p-mock-cassandra");
  return cassandraMock.connect({ ...p, ...fields, options: { ...p.options, ...options } });
}

async function run(s: MockSession, sql: string): Promise<QueryEvent[]> {
  const events: QueryEvent[] = [];
  await s.run(sql, (e) => events.push(e));
  return events;
}

const finished = (events: QueryEvent[]): StatementResult => {
  const e = events.find((x) => x.event === "finished");
  if (e?.event !== "finished") throw new Error(JSON.stringify(events));
  return e.data.result;
};

const failure = (events: QueryEvent[]) => {
  const e = events.find((x) => x.event === "failed");
  if (e?.event !== "failed") throw new Error(JSON.stringify(events));
  return e.data.error;
};

const rowsOf = (events: QueryEvent[]): Value[][] =>
  events.flatMap((e) => (e.event === "rows" ? e.data.rows : []));

const thrown = (f: () => unknown): { code: string; message: string } => {
  try {
    f();
  } catch (e) {
    return e as { code: string; message: string };
  }
  throw new Error("did not throw");
};

const orders = { schema: "shop", name: "orders_by_customer" };
const page = (over: Partial<BrowseRequest> = {}): BrowseRequest => ({
  table: orders,
  columns: null,
  filter: null,
  sort: [],
  limit: 20,
  offset: 0,
  ...over,
});
const where = (items: FilterGroup["items"], conjunction: "and" | "or" = "and"): FilterGroup => ({
  conjunction,
  items,
});

beforeEach(() => resetCluster());

describe("the cassandra mock lane", () => {
  it("has one driver with a Cassandra and a read-only ScyllaDB profile", () => {
    expect(lane.drivers).toEqual([cassandraMock]);
    const [cass, scylla] = cassandraMock.seedProfiles();
    expect(cass!.driver).toBe("cassandra");
    expect(cassandraMock.serverVersion(cass!)).toBe("Cassandra 4.1.7");
    expect(cassandraMock.serverVersion(scylla!)).toBe("ScyllaDB 6.2.1");
    expect(scylla!.options).toMatchObject({ readOnly: true, tag: "production" });
    expect(cassandraMock.util).toBeUndefined();
  });

  it("gives a session the caps the backend records", () => {
    expect(cassandraCaps("shop", "password")).toEqual(RECORDED.password);
    expect(cassandraCaps("shop", "passcode")).toEqual(RECORDED.command);
    expect(connect().caps).toEqual(RECORDED.password);
    expect(
      connect({ passwordMode: "command", passwordCommand: "vault read -field=pw cass" }).caps,
    ).toEqual(RECORDED.command);
  });

  it("offers no method the backend lacks", () => {
    const s = connect();
    for (const m of [
      "call",
      "listProcesses",
      "killProcesses",
      "listRoutines",
      "routineDefinition",
      "listUsers",
      "loadUser",
      "sessionDetails",
    ] as const)
      expect(s[m], m).toBeUndefined();
  });

  it("checks the profile as `validate` and `open` do", () => {
    expect(thrown(() => connect({ driverParams: { pageSize: 0 } }))).toEqual({
      code: "INVALID_INPUT",
      message: "Cassandra settings: the page size is 1 to 10000 rows",
    });
    expect(thrown(() => connect({ driverParams: { requestTimeoutSecs: 4000 } })).message).toBe(
      "Cassandra settings: the request timeout is 1 to 3600 seconds",
    );
    expect(thrown(() => connect({ driverParams: { consistency: "SERIAL" } })).message).toBe(
      "Cassandra settings: unknown variant `SERIAL`",
    );
    expect(thrown(() => connect({ sslCertPath: "/c.pem" })).message).toBe(
      "Cassandra: a client certificate needs both its certificate and its key file",
    );
    expect(thrown(() => connect({}, { database: "nowhere" }))).toEqual({
      code: "DB_ERROR",
      message: "Keyspace 'nowhere' does not exist",
    });
  });
});

describe("the catalog", () => {
  it("lists keyspaces as databases (system ones hidden) and the current one as the schema", () => {
    const s = connect();
    expect(s.listDatabases()).toEqual([
      { name: "shop", owner: null, encoding: null, isCurrent: true },
      { name: "iot", owner: null, encoding: null, isCurrent: false },
    ]);
    expect(s.listSchemas()).toEqual([{ name: "shop", isDefault: true }]);
    expect(s.listTables("shop").map((t) => [t.name, t.kind])).toEqual([
      ["orders_by_customer", "table"],
      ["orders_by_status", "materializedView"],
      ["page_views", "table"],
      ["users", "table"],
    ]);
    expect(connect({}, { database: null }).listSchemas()).toEqual([]);
  });

  it("gives columns in key order with their roles, the indexes and the server's DDL", () => {
    const s = connect();
    const meta = s.tableMeta(orders);
    expect(meta.columns.map((c) => [c.name, c.extra, c.pkOrdinal])).toEqual([
      ["customer_id", "partition key", 1],
      ["order_time", "clustering DESC", 2],
      ["order_id", "clustering ASC", 3],
      ["items", null, null],
      ["ledger_id", null, null],
      ["status", null, null],
      ["total", null, null],
    ]);
    expect(meta.primaryKey).toEqual(["customer_id", "order_time", "order_id"]);
    expect(meta.columns[3]).toMatchObject({
      dataType: "list<text>",
      kind: "json",
      arrayElemType: "text",
    });
    expect(meta.indexes[0]).toMatchObject({ name: "orders_status_sai", method: "sai" });
    expect(s.tableDdl?.(orders)).toContain('PRIMARY KEY ("customer_id", "order_time", "order_id")');
    expect(s.tableDdl?.(orders)).toContain(
      'CLUSTERING ORDER BY ("order_time" DESC, "order_id" ASC)',
    );
    const readings = s.tableMeta({ schema: "iot", name: "readings" });
    expect(readings.columns.map((c) => c.name)).toEqual([
      "sensor_id",
      "day",
      "ts",
      "humidity",
      "raw",
      "temperature",
    ]);
    expect(s.tableDdl?.({ schema: "iot", name: "readings" })).toContain(
      'PRIMARY KEY (("sensor_id", "day"), "ts")',
    );
    expect(s.tableMeta({ schema: "shop", name: "orders_by_status" }).kind).toBe("materializedView");
    expect(thrown(() => s.tableMeta({ schema: "shop", name: "nope" })).code).toBe("NOT_FOUND");
  });
});

describe("the grid", () => {
  it("pages by the cursor until the last page, without OFFSET", () => {
    const s = connect();
    const first = s.browse(page());
    expect(first.rows).toHaveLength(20);
    expect(first.identity).toBe("pk");
    expect(first.sql).toBe(
      'SELECT "customer_id", "order_time", "order_id", "items", "ledger_id", "status", "total" FROM "shop"."orders_by_customer"',
    );
    expect(first.nextCursor).toMatch(/^cql1\.[A-Za-z0-9_-]+$/);
    const second = s.browse(page({ cursor: first.nextCursor }));
    const third = s.browse(page({ cursor: second.nextCursor }));
    expect([second.rows.length, third.rows.length]).toEqual([20, 5]);
    expect(third.nextCursor).toBeNull();
    expect(second.rows[0]).not.toEqual(first.rows[0]);
    // Past 2^53 a bigint is text; a decimal stays exact.
    expect(first.rows[0]![4]).toEqual({ t: "bigInt", v: "9007199254740993" });
    expect(first.rows[0]![6]).toEqual({ t: "decimal", v: "12.50" });
  });

  it("refuses what CQL cannot page: an offset, a sort, a cursor of another query", () => {
    const s = connect();
    expect(thrown(() => s.browse(page({ offset: 20 })))).toEqual({
      code: "INVALID_INPUT",
      message: "Cassandra pages follow one another: there is no offset to start from",
    });
    expect(
      thrown(() => s.browse(page({ sort: [{ column: "total", dir: "desc", nulls: null }] })))
        .message,
    ).toBe(
      "CQL sorts only by clustering columns within one partition: the grid keeps the table's order",
    );
    const cursor = s.browse(page()).nextCursor;
    const paid = where([
      { kind: "condition", column: "status", op: "eq", values: [{ t: "text", v: "paid" }] },
    ]);
    expect(thrown(() => s.browse(page({ cursor, filter: paid }))).message).toBe(
      "the page cursor belongs to another query (the filter or sort changed): reload from the first page",
    );
    expect(thrown(() => s.browse(page({ cursor: "abc" }))).message).toBe(
      "not a Cassandra page cursor",
    );
  });

  it("filters with AND only, adding ALLOW FILTERING, and refuses the rest", () => {
    const s = connect();
    const paid = s.browse(
      page({
        filter: where([
          { kind: "condition", column: "status", op: "eq", values: [{ t: "text", v: "paid" }] },
          {
            kind: "condition",
            column: "total",
            op: "between",
            values: [
              { t: "decimal", v: "10" },
              { t: "decimal", v: "100" },
            ],
          },
        ]),
      }),
    );
    expect(paid.sql).toBe(
      `SELECT "customer_id", "order_time", "order_id", "items", "ledger_id", "status", "total" FROM "shop"."orders_by_customer" WHERE "status" = 'paid' AND "total" >= 10 AND "total" <= 100 ALLOW FILTERING`,
    );
    expect(paid.rows.every((r) => r[5]!.t === "text" && r[5]!.v === "paid")).toBe(true);
    const cond = (op: string) => ({
      kind: "condition" as const,
      column: "status",
      op: op as "eq",
      values: [{ t: "text" as const, v: "x" }],
    });
    expect(
      thrown(() => s.browse(page({ filter: where([cond("eq"), cond("eq")], "or") }))).message,
    ).toBe("CQL has no OR: combine the filters with AND");
    expect(thrown(() => s.browse(page({ filter: where([cond("ne")]) }))).message).toBe(
      "CQL cannot filter with Ne",
    );
    expect(
      thrown(() =>
        s.browse(
          page({
            filter: where([
              { kind: "condition", column: "status", op: "eq", values: [{ t: "null" }] },
            ]),
          }),
        ),
      ).message,
    ).toBe("CQL cannot match NULL");
    expect(
      thrown(() =>
        s.browse(
          page({
            filter: where([{ kind: "anyColumn", op: "eq", values: [{ t: "text", v: "x" }] }]),
          }),
        ),
      ).message,
    ).toBe("CQL cannot filter on any column: pick one");
    // The raw WHERE is the user's text: a write or a second statement in it is refused.
    expect(
      thrown(() => s.browse(page({ filter: where([{ kind: "raw", sql: "k = 1; DROP TABLE t" }]) })))
        .message,
    ).toBe("A filter can only read rows: this one would run another statement");
  });

  it("counts by the size estimate unless filtered, and fetches whole cells by the full key", () => {
    const s = connect();
    expect(s.count?.(orders, null)).toEqual({ count: 5, exact: false });
    const paid = where([
      { kind: "condition", column: "status", op: "eq", values: [{ t: "text", v: "paid" }] },
    ]);
    expect(s.count?.(orders, paid)).toEqual({ count: 12, exact: true });
    // A view has no estimate: its count reads every row.
    expect(s.count?.({ schema: "shop", name: "orders_by_status" }, null)).toEqual({
      count: 45,
      exact: true,
    });
    const row = s.browse(page()).rows[0]!;
    const key = {
      kind: "pk" as const,
      value: { customer_id: row[0]!, order_time: row[1]!, order_id: row[2]! },
    };
    expect(s.fetchCell?.(orders, key, "total")).toEqual(row[6]);
    expect(
      thrown(() => s.fetchCell?.(orders, { kind: "pk", value: { customer_id: row[0]! } }, "total"))
        .message,
    ).toBe("CQL addresses a row by its whole primary key (partition and clustering columns)");
  });
});

describe("edits", () => {
  const users = { schema: "shop", name: "users" };

  it("preview as lightweight transactions by the full key, deletes first", () => {
    const s = connect();
    const id = s.browse({ ...page(), table: users }).rows[0]![0]!;
    const sql = s.preview?.({
      table: users,
      returning: false,
      changes: [
        {
          op: "insert",
          rowId: "n",
          values: {
            user_id: { t: "uuid", v: "00000000-0000-4000-a000-000000000001" },
            name: { t: "text", v: "O'Neil" },
          },
          exprs: { created_at: "now" },
        },
        {
          op: "update",
          rowId: "u",
          key: { kind: "pk", value: { user_id: id } },
          changes: { name: { t: "text", v: "Ada L." }, email: { t: "null" } },
        },
        { op: "delete", rowId: "d", key: { kind: "pk", value: { user_id: id } } },
      ],
    });
    expect(sql).toEqual([
      `DELETE FROM "shop"."users" WHERE "user_id" = '${id.t === "uuid" ? id.v : ""}' IF EXISTS`,
      `UPDATE "shop"."users" SET "email" = NULL, "name" = 'Ada L.' WHERE "user_id" = '${id.t === "uuid" ? id.v : ""}' IF EXISTS`,
      `INSERT INTO "shop"."users" ("created_at", "name", "user_id") VALUES (toTimestamp(now()), 'O''Neil', '00000000-0000-4000-a000-000000000001') IF NOT EXISTS`,
    ]);
  });

  it("apply one by one: a change that does not apply stops the rest, those before stay", () => {
    const s = connect();
    const rows = s.browse({ ...page(), table: users }).rows;
    const key = (i: number) => ({ kind: "pk" as const, value: { user_id: rows[i]![0]! } });
    const gone = {
      kind: "pk" as const,
      value: { user_id: { t: "uuid" as const, v: "00000000-0000-4000-a000-0000000000ff" } },
    };
    const result = s.apply!({
      table: users,
      returning: false,
      changes: [
        { op: "update", rowId: "a", key: key(0), changes: { name: { t: "text", v: "Ada" } } },
        { op: "update", rowId: "b", key: gone, changes: { name: { t: "text", v: "Nobody" } } },
        { op: "update", rowId: "c", key: key(1), changes: { name: { t: "text", v: "Grace" } } },
      ],
    });
    expect(result.success).toBe(false);
    expect(result.error).toMatchObject({
      code: "DB_ERROR",
      message:
        "the row no longer exists (it was deleted or its key changed), so the change was not applied",
      statementIndex: 1,
    });
    expect(result.results.map((r) => [r.rowId, r.error?.message ?? null])).toEqual([
      ["a", null],
      [
        "b",
        "the row no longer exists (it was deleted or its key changed), so the change was not applied",
      ],
      ["c", "not applied: an earlier change failed"],
    ]);
    const after = s.browse({ ...page(), table: users }).rows;
    expect(after[0]![3]).toEqual({ t: "text", v: "Ada" });
    expect(after[1]![3]).toEqual({ t: "text", v: "Grace" });
    // An insert of a key that exists does not apply either.
    const again = s.apply!({
      table: users,
      returning: false,
      changes: [{ op: "insert", rowId: "i", values: { user_id: rows[0]![0]! } }],
    });
    expect(again.error?.message).toBe(
      "a row with this primary key already exists, so it was not inserted",
    );
    const ok = s.apply!({
      table: users,
      returning: false,
      changes: [{ op: "delete", rowId: "d", key: key(2) }],
    });
    expect(ok).toMatchObject({
      success: true,
      results: [{ rowId: "d", affected: 1, error: null }],
    });
  });

  it("reach the node for a materialized view, which refuses them", () => {
    const s = connect();
    const view = { schema: "shop", name: "orders_by_status" };
    const row = s.browse({ ...page(), table: view }).rows[0]!;
    const result = s.apply!({
      table: view,
      returning: false,
      changes: [
        {
          op: "update",
          rowId: "v",
          key: {
            kind: "pk",
            value: {
              status: row[0]!,
              customer_id: row[1]!,
              order_time: row[2]!,
              order_id: row[3]!,
            },
          },
          changes: { total: { t: "decimal", v: "1.00" } },
        },
      ],
    });
    expect(result.error).toMatchObject({
      code: "DB_ERROR",
      message: "Cannot directly modify a materialized view",
      vendorCode: 8704,
      statementIndex: 0,
    });
  });

  it("refuse key changes, DEFAULT, counter tables and a key that is not whole", () => {
    const s = connect();
    const id = s.browse({ ...page(), table: users }).rows[0]![0]!;
    const update = (changes: Record<string, Value>) =>
      thrown(() =>
        s.preview?.({
          table: users,
          returning: false,
          changes: [
            { op: "update", rowId: "r", key: { kind: "pk", value: { user_id: id } }, changes },
          ],
        }),
      ).message;
    expect(update({ user_id: id })).toBe(
      "CQL cannot change a primary key column: delete the row and insert it again",
    );
    expect(update({ name: { t: "default" } })).toBe(
      "CQL has no DEFAULT: enter the value explicitly",
    );
    expect(
      thrown(() =>
        s.preview?.({
          table: { schema: "shop", name: "page_views" },
          returning: false,
          changes: [
            {
              op: "delete",
              rowId: "r",
              key: { kind: "pk", value: { page: { t: "text", v: "/" } } },
            },
          ],
        }),
      ).message,
    ).toBe(
      "CQL counter tables cannot be edited in the grid: run UPDATE … SET c = c + n in the editor",
    );
    expect(
      thrown(() =>
        s.preview?.({
          table: orders,
          returning: false,
          changes: [{ op: "insert", rowId: "r", values: { customer_id: id } }],
        }),
      ).message,
    ).toBe('a new row needs its key column "order_time"');
  });
});

describe("the editor", () => {
  it("runs cqlsh's commands itself and answers as the backend does", async () => {
    const s = connect();
    expect(finished(await run(s, "CONSISTENCY")).notices).toEqual([
      "Current consistency level is LocalOne.",
    ]);
    expect(finished(await run(s, "CONSISTENCY local_quorum;")).notices).toEqual([
      "Consistency level set to LocalQuorum.",
    ]);
    expect(finished(await run(s, "SERIAL CONSISTENCY SERIAL")).notices).toEqual([
      "Serial consistency level set to Serial.",
    ]);
    expect(finished(await run(s, "PAGING 50")).notices).toEqual([
      "Query paging is enabled, page size 50.",
    ]);
    expect(finished(await run(s, "EXPAND ON")).notices[0]).toMatch(/^Expanded output is enabled/);
    expect(failure(await run(s, "CONSISTENCY SOMETIMES"))).toMatchObject({
      code: "INVALID_INPUT",
      message: "Improper CONSISTENCY command",
    });
  });

  it("switches the keyspace with USE, a bare name folded to lower case", async () => {
    const s = connect();
    expect(finished(await run(s, "USE IOT")).notices).toEqual(["Now using keyspace iot."]);
    expect(s.listSchemas()).toEqual([{ name: "iot", isDefault: true }]);
    expect(s.caps.namespaces.defaultSchema).toBe("iot");
    expect(failure(await run(s, 'USE "Missing"'))).toMatchObject({
      code: "DB_ERROR",
      message: "Keyspace 'Missing' does not exist",
    });
  });

  it("selects, asking for ALLOW FILTERING where the partition key cannot answer", async () => {
    const s = connect();
    const all = await run(s, "SELECT name, email FROM users LIMIT 3");
    expect(finished(all)).toMatchObject({
      kind: "rows",
      rowCount: 3,
      columns: [{ name: "name" }, { name: "email" }],
    });
    expect(
      failure(await run(s, "SELECT * FROM orders_by_customer WHERE status = 'paid'")),
    ).toMatchObject({
      code: "DB_ERROR",
      vendorCode: 8704,
      message: expect.stringContaining("use ALLOW FILTERING"),
    });
    expect(
      rowsOf(
        await run(
          s,
          "SELECT count(*) FROM orders_by_customer WHERE status = 'paid' ALLOW FILTERING",
        ),
      ),
    ).toEqual([[{ t: "int", v: 12 }]]);
    expect(
      rowsOf(
        await run(
          s,
          "SELECT * FROM iot.readings WHERE sensor_id = 'boiler-1' AND day = '2026-10-01'",
        ),
      ),
    ).toHaveLength(6);
    expect(failure(await run(s, "SELECT * FROM nope")).message).toBe("unconfigured table nope");
  });

  it("writes, a lightweight transaction answering [applied], and runs a batch whole", async () => {
    const s = connect();
    const id = "00000000-0000-4000-a000-0000000000aa";
    const lwt = await run(
      s,
      `INSERT INTO users (user_id, name) VALUES (${id}, 'Kay') IF NOT EXISTS`,
    );
    expect(finished(lwt)).toMatchObject({ kind: "rows", affected: 1 });
    expect(rowsOf(lwt)).toEqual([[{ t: "bool", v: true }]]);
    expect(
      rowsOf(await run(s, `INSERT INTO users (user_id) VALUES (${id}) IF NOT EXISTS`)),
    ).toEqual([[{ t: "bool", v: false }]]);
    await run(
      s,
      `BEGIN BATCH UPDATE users SET name = 'K.' WHERE user_id = ${id}; DELETE FROM users WHERE user_id = ${id}; APPLY BATCH`,
    );
    expect(rowsOf(await run(s, `SELECT * FROM users WHERE user_id = ${id}`))).toEqual([]);
    // A batch with a bad statement applies none of it, the ones before included.
    const kept = "00000000-0000-4000-a000-0000000000ff";
    expect(
      failure(
        await run(
          s,
          `BEGIN BATCH INSERT INTO users (user_id) VALUES (${kept}); UPDATE page_views SET views = views + 1 WHERE page = '/' AND day = '2026-10-01'; INSERT INTO nope (k) VALUES (1); APPLY BATCH`,
        ),
      ).message,
    ).toBe("unconfigured table nope");
    expect(rowsOf(await run(s, `SELECT * FROM users WHERE user_id = ${kept}`))).toEqual([]);
    await run(s, "UPDATE page_views SET views = views + 5 WHERE page = '/' AND day = '2026-10-01'");
    expect(
      rowsOf(await run(s, "SELECT views FROM page_views WHERE page = '/' AND day = '2026-10-01'")),
    ).toEqual([[{ t: "int", v: 1005 }]]);
    expect(failure(await run(s, "UPDATE users SET name = 'x' WHERE name = 'Ada'")).message).toBe(
      "Some partition key parts are missing: user_id",
    );
  });
});

describe("a read-only profile", () => {
  it("refuses every write before it runs, and edits outright", async () => {
    const s = connect({ readOnly: true });
    expect(
      failure(
        await run(s, "INSERT INTO users (user_id) VALUES (00000000-0000-4000-a000-000000000001)"),
      ),
    ).toMatchObject({
      code: "INVALID_INPUT",
      message: READ_ONLY_MESSAGE,
    });
    expect(failure(await run(s, "frobnicate")).message).toBe(READ_ONLY_MESSAGE);
    expect(finished(await run(s, "SELECT * FROM users LIMIT 1")).rowCount).toBe(1);
    expect(finished(await run(s, "CONSISTENCY QUORUM")).notices).toEqual([
      "Consistency level set to Quorum.",
    ]);
    expect(
      thrown(() =>
        s.preview?.({ table: { schema: "shop", name: "users" }, returning: false, changes: [] }),
      ),
    ).toEqual({
      code: "UNSUPPORTED",
      message: "unsupported: this connection is read-only",
    });
  });

  it("does not open when its startup commands write; reads and cqlsh commands are fine", () => {
    expect(thrown(() => connect({ readOnly: true, startupSql: "TRUNCATE users" })).message).toBe(
      "Cassandra: this connection is read-only, and its startup commands write (or are not known to only read)",
    );
    const s = connect({
      readOnly: true,
      startupSql: "CONSISTENCY QUORUM;\nUSE iot;\nSELECT * FROM readings LIMIT 1;",
    });
    expect(s.listSchemas()).toEqual([{ name: "iot", isDefault: true }]);
    expect(thrown(() => connect({ startupSql: "SELECT * FROM nope" })).message).toBe(
      "Cassandra: the startup commands failed: unconfigured table nope",
    );
  });

  it("is what the ScyllaDB seed opens", async () => {
    const p = seed("p-mock-scylla");
    const s = cassandraMock.connect(p);
    expect(s.listSchemas()).toEqual([{ name: "iot", isDefault: true }]);
    expect(failure(await run(s, "TRUNCATE readings")).message).toBe(READ_ONLY_MESSAGE);
  });
});
