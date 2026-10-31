import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { ConnectionInfo } from "../../../bindings/ConnectionInfo";
import type { DatabaseInfo } from "../../../bindings/DatabaseInfo";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { RowCount } from "../../../bindings/RowCount";
import type { SchemaInfo } from "../../../bindings/SchemaInfo";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { TableSummary } from "../../../bindings/TableSummary";
import type { Value } from "../../../bindings/Value";
import { mockProfiles } from "../../handlers";
import { mockInvoke } from "../../transport";
import { enableMockDrivers, resetMockBuild } from "..";
import lane from ".";
import { resetTables, tables } from "./data";
import { CONDITION_FAILED, dynamoCaps, FILTER_OPS, READ_ONLY_MESSAGE } from "./session";

// The mock's DynamoDB against the backend (drivers/dynamodb/session.rs and its session tests):
// caps, catalog, cursor pages, sort only by the sort key of a Query, counts, cells, a commit with
// a failing change (one request, or one transaction all or nothing), the console, the calls,
// the form's calls, and the refusals of a read-only session.

beforeAll(async () => {
  await import("../..");
});

let added: string[] = [];

beforeEach(async () => {
  resetTables();
  added = (await enableMockDrivers(["dynamodb"])).map((p) => p.id);
});

afterEach(() => {
  resetMockBuild();
  for (const id of added) mockProfiles.delete(id);
});

const connect = (profileId: string) => mockInvoke<ConnectionInfo>("connect", { profileId });

async function run(connectionId: string, sql: string) {
  const events: QueryEvent[] = [];
  const results = await mockInvoke<StatementResult[]>("run_sql", {
    connectionId,
    queryId: "q",
    sql,
    options: {},
    onEvent: { send: (e: QueryEvent) => events.push(e) },
  });
  const rows = events.flatMap((e) => (e.event === "rows" ? e.data.rows : []));
  return { results, rows };
}

const music = { schema: "us-east-1", name: "Music" };
const events = { schema: "us-east-1", name: "Events" };
const S = (v: string): Value => ({ t: "text", v });
const browse = (connectionId: string, req: Record<string, unknown> = {}) =>
  mockInvoke<BrowsePage>("browse_table", {
    connectionId,
    req: { table: events, columns: null, filter: null, sort: [], limit: 100, offset: 0, ...req },
  });
const eq = (column: string, v: Value) => ({
  conjunction: "and",
  items: [{ kind: "condition", column, op: "eq", values: [v] }],
});
const songKey = (artist: string, title: string) => ({
  kind: "pk",
  value: { Artist: S(artist), SongTitle: S(title) },
});
const apply = (connectionId: string, changes: unknown[]) =>
  mockInvoke<ApplyResult>("apply_changes", {
    connectionId,
    req: { table: music, changes, returning: false },
  });

describe("the mock DynamoDB driver", () => {
  it("is the lane's one driver, with three seed profiles", () => {
    expect(lane.drivers.map((d) => d.driver)).toEqual(["dynamodb"]);
    expect(added).toEqual(["p-mock-dynamodb", "p-mock-dynamodb-local", "p-mock-dynamodb-ro"]);
  });

  it("connects with the backend's caps and names a custom endpoint", async () => {
    const aws = await connect("p-mock-dynamodb");
    expect(aws.serverVersion).toBe("DynamoDB API 2012-08-10");
    expect(aws.capabilities).toEqual(dynamoCaps("us-east-1"));
    expect(aws.capabilities?.features).toEqual([
      "editor",
      "editRows",
      "insertRows",
      "deleteRows",
      "exactCount",
      "structureView",
      "viewDefinition",
    ]);
    expect(aws.capabilities?.data).toMatchObject({
      paging: "cursor",
      count: "costly",
      sort: true,
      rawFilter: null,
      transactions: "batch",
      affected: "exact",
    });
    expect(FILTER_OPS).toHaveLength(17);
    expect(aws.capabilities?.safety).toEqual({
      readOnly: "classifierOnly",
      hardened: false,
      confirm: "passcode",
    });
    const local = await connect("p-mock-dynamodb-local");
    expect(local.serverVersion).toBe("DynamoDB API 2012-08-10 at http://localhost:8000/");
  });

  it("lists the region as the one database and schema, and its tables", async () => {
    const { connectionId } = await connect("p-mock-dynamodb");
    expect(await mockInvoke<DatabaseInfo[]>("list_databases", { connectionId })).toEqual([
      { name: "us-east-1", owner: null, encoding: null, isCurrent: true },
    ]);
    expect(await mockInvoke<SchemaInfo[]>("list_schemas", { connectionId })).toEqual([
      { name: "us-east-1", isDefault: true },
    ]);
    const list = await mockInvoke<TableSummary[]>("list_tables", {
      connectionId,
      schema: "us-east-1",
    });
    expect(list.map((t) => [t.name, t.kind, t.schema])).toEqual([
      ["Events", "table", "us-east-1"],
      ["Music", "table", "us-east-1"],
      ["Users", "table", "us-east-1"],
    ]);
  });

  it("describes a table: keys first, sampled attributes, index keys, indexes and its request", async () => {
    const { connectionId } = await connect("p-mock-dynamodb");
    const meta = await mockInvoke<TableMeta>("get_table", { connectionId, table: music });
    expect(meta.primaryKey).toEqual(["Artist", "SongTitle"]);
    expect(meta.columns.slice(0, 2)).toMatchObject([
      { name: "Artist", dataType: "S", isPk: true, pkOrdinal: 1, comment: "partition key" },
      { name: "SongTitle", dataType: "S", isPk: true, pkOrdinal: 2, comment: "sort key" },
    ]);
    const col = (n: string) => meta.columns.find((c) => c.name === n)!;
    expect(col("Year")).toMatchObject({ dataType: "N", kind: "decimal" });
    expect(col("Tags")).toMatchObject({ dataType: "SS", kind: "array", arrayElemType: "S" });
    expect(col("Awards")).toMatchObject({ dataType: "M", kind: "json" });
    expect(meta.indexes.map((i) => [i.name, i.method, i.columns])).toEqual([
      ["PRIMARY", "primary key", ["Artist", "SongTitle"]],
      ["ByGenre", "GSI", ["Genre", "Year"]],
      ["ByYear", "LSI", ["Artist", "Year"]],
    ]);
    expect(meta.comment).toContain("provisioned · stream NEW_AND_OLD_IMAGES");
    expect(meta.ddl?.startsWith('CreateTable {\n  "TableName": "Music"')).toBe(true);
    const users = await mockInvoke<TableMeta>("get_table", {
      connectionId,
      table: { schema: "us-east-1", name: "Users" },
    });
    expect(users.columns.find((c) => c.name === "nickname")?.dataType).toBe("NULL");
    await expect(
      mockInvoke("get_table", { connectionId, table: { schema: "us-east-1", name: "Nope" } }),
    ).rejects.toMatchObject({ code: "DB_ERROR" });
  });

  it("pages a Scan by cursor to its end, keeping types and missing attributes", async () => {
    const { connectionId } = await connect("p-mock-dynamodb");
    const first = await browse(connectionId);
    expect(first.rows).toHaveLength(100);
    expect(first.identity).toBe("pk");
    expect(first.pkColumns).toEqual(["deviceId", "at"]);
    expect(first.sql).toBe('Scan {"TableName":"Events","Limit":100}');
    let seen = first.rows.length;
    let cursor = first.nextCursor ?? null;
    let pages = 1;
    while (cursor) {
      const next = await browse(connectionId, { cursor });
      seen += next.rows.length;
      cursor = next.nextCursor ?? null;
      pages += 1;
    }
    expect([seen, pages]).toEqual([660, 7]);
    const songs = await browse(connectionId, { table: music, limit: 5 });
    const plays = songs.columns.findIndex((c) => c.name === "Plays");
    const tags = songs.columns.findIndex((c) => c.name === "Tags");
    // 2^53 + 1 stays a big integer; an attribute an item lacks is missing, not NULL.
    expect(songs.rows[0]![plays]).toEqual({ t: "bigInt", v: "9007199254740993" });
    expect(songs.rows[0]![tags]).toEqual({ t: "other", v: { ty: "missing", text: "" } });
    await expect(browse(connectionId, { cursor: "%%%" })).rejects.toMatchObject({
      code: "INVALID_INPUT",
    });
  });

  it("queries a partition sorted by its sort key, and refuses any other sort", async () => {
    const { connectionId } = await connect("p-mock-dynamodb");
    const filter = eq("deviceId", S("device#2"));
    const desc = await browse(connectionId, {
      filter,
      sort: [{ column: "at", dir: "desc", nulls: null }],
      limit: 3,
    });
    expect(desc.sql).toContain('Query {"TableName":"Events","KeyConditionExpression":"#n0 = :v0"');
    expect(desc.sql).toContain('"ScanIndexForward":false');
    const at = desc.columns.findIndex((c) => c.name === "at");
    expect(desc.rows.map((r) => r[at])).toEqual([
      { t: "int", v: 1_767_225_600 + 109 * 60 },
      { t: "int", v: 1_767_225_600 + 108 * 60 },
      { t: "int", v: 1_767_225_600 + 107 * 60 },
    ]);
    await expect(
      browse(connectionId, { sort: [{ column: "at", dir: "asc", nulls: null }] }),
    ).rejects.toMatchObject({ code: "UNSUPPORTED", message: expect.stringContaining("Query") });
    await expect(
      browse(connectionId, { filter, sort: [{ column: "kind", dir: "asc", nulls: null }] }),
    ).rejects.toMatchObject({ code: "UNSUPPORTED", message: expect.stringContaining("sort key") });
    const alerts = await browse(connectionId, { filter: eq("kind", S("alert")), limit: 1000 });
    expect(alerts.rows).toHaveLength(66);
    expect(alerts.sql.startsWith("Scan ")).toBe(true);
    // The filter bar's text is a number in a number attribute.
    const one = await browse(connectionId, { filter: eq("at", S("1767225600")), limit: 1000 });
    expect(one.rows).toHaveLength(6);
  });

  it("counts by the stale estimate, or exactly with a filter, and fetches a whole cell", async () => {
    const { connectionId } = await connect("p-mock-dynamodb");
    expect(
      await mockInvoke<RowCount>("count_table_rows", { connectionId, table: events, filter: null }),
    ).toEqual({ count: 660, exact: false });
    expect(
      await mockInvoke<RowCount>("count_table_rows", {
        connectionId,
        table: events,
        filter: eq("kind", S("alert")),
      }),
    ).toEqual({ count: 66, exact: true });
    const awards = await mockInvoke<Value>("fetch_cell", {
      connectionId,
      table: music,
      key: songKey("Acme Band", "Song 04"),
      column: "Awards",
    });
    expect(awards.t).toBe("json");
    await expect(
      mockInvoke("fetch_cell", {
        connectionId,
        table: music,
        key: songKey("Nobody", "x"),
        column: "Awards",
      }),
    ).rejects.toMatchObject({ code: "NOT_FOUND" });
  });

  it("applies one change as a conditional request, its failure in the backend's words", async () => {
    const { connectionId } = await connect("p-mock-dynamodb");
    const update = {
      op: "update",
      rowId: "r1",
      key: songKey("Acme Band", "Song 01"),
      changes: {
        Price: { t: "decimal", v: "0.99" },
        Tags: { t: "other", v: { ty: "missing", text: "" } },
      },
    };
    const preview = await mockInvoke<string[]>("preview_changes", {
      connectionId,
      req: { table: music, changes: [update], returning: false },
    });
    expect(preview[0]).toContain('UpdateItem {\n  "TableName": "Music"');
    expect(preview[0]).toContain('"UpdateExpression": "SET #n0 = :v0 REMOVE #n1"');
    expect((await apply(connectionId, [update])).success).toBe(true);
    const song = tables().get("Music")!.items[0]!;
    expect(song.Price).toEqual({ t: "decimal", v: "0.99" });
    const dup = await apply(connectionId, [
      { op: "insert", rowId: "n1", values: { Artist: S("Acme Band"), SongTitle: S("Song 02") } },
    ]);
    expect(dup.success).toBe(false);
    expect(dup.results[0]).toMatchObject({
      rowId: "n1",
      affected: 0,
      error: { code: "DB_ERROR", message: CONDITION_FAILED },
    });
  });

  it("applies several changes as one transaction: all or nothing, a reason per change", async () => {
    const { connectionId } = await connect("p-mock-dynamodb");
    const before = tables().get("Music")!.items.length;
    const changes = [
      { op: "delete", rowId: "d1", key: songKey("Zero Day", "Song 01") },
      { op: "insert", rowId: "n1", values: { Artist: S("Acme Band"), SongTitle: S("Song 03") } },
      { op: "insert", rowId: "n2", values: { Artist: S("New"), SongTitle: S("Hello") } },
    ];
    const failed = await apply(connectionId, changes);
    expect(failed.success).toBe(false);
    expect(failed.results.map((r) => [r.rowId, r.affected, r.error?.message ?? null])).toEqual([
      ["d1", 0, "not applied: the transaction was cancelled because item 2 failed"],
      ["n1", 0, CONDITION_FAILED],
      ["n2", 0, "not applied: the transaction was cancelled because item 2 failed"],
    ]);
    expect(tables().get("Music")!.items).toHaveLength(before);
    const ok = await apply(connectionId, [changes[0], changes[2]]);
    expect(ok.results.map((r) => r.affected)).toEqual([1, 1]);
    expect(tables().get("Music")!.items).toHaveLength(before);
    // A changed key: the item is deleted and put again under it, in one transaction.
    const moved = await apply(connectionId, [
      {
        op: "update",
        rowId: "k1",
        key: songKey("Acme Band", "Song 05"),
        changes: { SongTitle: S("Song 05 (live)") },
      },
    ]);
    expect(moved.sql[0]?.startsWith("TransactWriteItems")).toBe(true);
    expect(
      tables()
        .get("Music")!
        .items.some((i) => i.SongTitle?.t === "text" && i.SongTitle.v === "Song 05 (live)"),
    ).toBe(true);
    await expect(
      apply(connectionId, [
        {
          op: "update",
          rowId: "x",
          key: songKey("Acme Band", "Song 06"),
          changes: { Year: S("y") },
        },
        {
          op: "update",
          rowId: "y",
          key: songKey("Acme Band", "Song 06"),
          changes: { Genre: S("z") },
        },
      ]),
    ).resolves.toMatchObject({ success: false, error: { code: "INVALID_INPUT" } });
  });

  it("runs PartiQL and API commands as the backend answers them", async () => {
    const { connectionId } = await connect("p-mock-dynamodb");
    const selected = await run(connectionId, `SELECT * FROM "Music" WHERE "Artist" = 'Zero Day'`);
    expect(selected.results[0]?.kind).toBe("rows");
    expect(selected.rows).toHaveLength(12);
    const inserted = await run(
      connectionId,
      `INSERT INTO "Users" VALUE {'userId': 'u100', 'n': 1}`,
    );
    expect(inserted.results[0]).toMatchObject({ kind: "affected", affected: 1 });
    const dup = await run(connectionId, `INSERT INTO "Users" VALUE {'userId': 'u100'}`);
    expect(dup.results[0]?.error).toMatchObject({ detail: "DuplicateItemException" });
    // A DELETE of a key reports no count unless it returns the item.
    const deleted = await run(connectionId, `DELETE FROM "Users" WHERE "userId" = 'u100'`);
    expect(deleted.results[0]).toMatchObject({ kind: "empty", affected: null });
    const returned = await run(
      connectionId,
      `DELETE FROM "Users" WHERE "userId" = 'u001' RETURNING ALL OLD *`,
    );
    expect(returned.results[0]).toMatchObject({ kind: "rows", affected: 1 });
    // The shared mock hands a native console a line at a time.
    const scan = await run(connectionId, 'Scan {"TableName": "Events", "Limit": 2}\nListTables');
    expect(scan.results.map((r) => r.kind)).toEqual(["rows", "rows"]);
    expect(scan.results[0]?.notices[0]).toMatch(/^More items follow: pass "ExclusiveStartKey"/);
    expect(scan.rows[2]).toEqual([{ t: "json", v: '{"TableNames":["Events","Music","Users"]}' }]);
    const unknown = await run(connectionId, "FlushTable {}");
    expect(unknown.results[0]?.error?.code).toBe("UNSUPPORTED");
    const twice = await run(connectionId, 'Scan {"TableName": "a", "TableName": "b"}');
    expect(twice.results[0]?.error?.message).toContain("is repeated");
  });

  it("answers the table info and index calls, and refuses unknown ones", async () => {
    const { connectionId } = await connect("p-mock-dynamodb");
    const info = await mockInvoke<{ table: { TableName: string }; timeToLive: unknown }>(
      "session_call",
      { connectionId, method: "tableInfo", args: { table: "Events" } },
    );
    expect(info.table.TableName).toBe("Events");
    expect(info.timeToLive).toEqual({ TimeToLiveStatus: "ENABLED", AttributeName: "expiresAt" });
    const request = { table: music, columns: null, filter: null, sort: [], limit: 20, offset: 0 };
    const byGenre = await mockInvoke<BrowsePage>("session_call", {
      connectionId,
      method: "browseIndex",
      args: { table: "Music", index: "ByGenre", request },
    });
    expect(byGenre.rows).toHaveLength(20);
    expect(byGenre.sql).toContain('"IndexName":"ByGenre"');
    const rest = await mockInvoke<BrowsePage>("session_call", {
      connectionId,
      method: "browseIndex",
      args: {
        table: "Music",
        index: "ByGenre",
        request: { ...request, cursor: byGenre.nextCursor },
      },
    });
    expect(rest.rows).toHaveLength(20);
    // An index's items share its keys: its cursor holds the table's key too.
    const last = await mockInvoke<BrowsePage>("session_call", {
      connectionId,
      method: "browseIndex",
      args: { table: "Music", index: "ByGenre", request: { ...request, cursor: rest.nextCursor } },
    });
    expect(last.rows).toHaveLength(8);
    expect(last.nextCursor).toBeNull();
    await expect(
      mockInvoke("session_call", {
        connectionId,
        method: "browseIndex",
        args: { table: "Music", index: "Nope", request },
      }),
    ).rejects.toMatchObject({ code: "NOT_FOUND" });
    await expect(
      mockInvoke("session_call", { connectionId, method: "frobnicate", args: null }),
    ).rejects.toMatchObject({ code: "UNSUPPORTED" });
  });

  it("answers the form's calls", async () => {
    const input = { profile: mockProfiles.get("p-mock-dynamodb")!, password: null };
    expect(await mockInvoke("driver_util", { input, method: "regions" })).toContain("us-east-1");
    expect(await mockInvoke("driver_util", { input, method: "awsProfiles" })).toEqual([
      "default",
      "dev",
      "prod-readonly",
    ]);
    await expect(
      mockInvoke("driver_util", { input, method: "listDatabases" }),
    ).rejects.toMatchObject({ code: "UNSUPPORTED" });
  });

  it("refuses every write path on a read-only connection, and still reads", async () => {
    const { connectionId } = await connect("p-mock-dynamodb-ro");
    const before = tables().get("Music")!.items.length;
    await expect(
      mockInvoke("preview_changes", {
        connectionId,
        req: {
          table: music,
          changes: [{ op: "delete", rowId: "d", key: songKey("Zero Day", "Song 01") }],
          returning: false,
        },
      }),
    ).rejects.toMatchObject({ message: READ_ONLY_MESSAGE });
    const result = await apply(connectionId, [
      { op: "delete", rowId: "d", key: songKey("Zero Day", "Song 01") },
    ]);
    expect(result.results[0]).toMatchObject({ affected: 0, error: { message: READ_ONLY_MESSAGE } });
    const write = await run(
      connectionId,
      `DELETE FROM "Music" WHERE "Artist" = 'Zero Day' AND "SongTitle" = 'Song 01'`,
    );
    expect(write.results[0]?.error?.message).toBe(READ_ONLY_MESSAGE);
    const hidden = await run(
      connectionId,
      'ExecuteStatement {"Statement": "SELECT * FROM t", "statement": "DELETE FROM t"}',
    );
    expect(hidden.results[0]?.error?.message).toBe(READ_ONLY_MESSAGE);
    expect(tables().get("Music")!.items).toHaveLength(before);
    const read = await run(
      connectionId,
      'SELECT * FROM "Users"\nDescribeTable {"TableName": "Users"}',
    );
    expect(read.results.map((r) => r.error)).toEqual([null, null]);
    expect(
      await mockInvoke<{ table: unknown }>("session_call", {
        connectionId,
        method: "tableInfo",
        args: { table: "Users" },
      }),
    ).toMatchObject({ table: { TableName: "Users" } });
    await expect(
      mockInvoke("session_call", { connectionId, method: "frobnicate", args: null }),
    ).rejects.toMatchObject({ message: READ_ONLY_MESSAGE });
  });
});
