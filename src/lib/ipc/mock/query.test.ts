import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { ipc, type QueryEvent, type ResultColumn } from "@/lib/ipc";
import { resetMockQueryState } from "./query";

const ids: Record<"pg" | "mysql", string> = { pg: "", mysql: "" };

beforeAll(async () => {
  ids.pg = (await ipc.connections.connect("p-local-pg")).connectionId;
  ids.mysql = (await ipc.connections.connect("p-shop-mysql")).connectionId;
});

beforeEach(() => resetMockQueryState({ emptyHistory: true }));

afterAll(async () => {
  await ipc.connections.disconnect(ids.pg);
  await ipc.connections.disconnect(ids.mysql);
});

/** The columns of the first result set of `sql`. */
async function columnsOf(db: keyof typeof ids, sql: string): Promise<ResultColumn[]> {
  const events: QueryEvent[] = [];
  await ipc.query.run(ids[db], crypto.randomUUID(), sql, {}, (e) => events.push(e));
  const cols = events.find((e) => e.event === "columns");
  if (cols?.event !== "columns") throw new Error(`no result for ${sql}`);
  return cols.data.columns;
}

describe("mock run_sql column origins", () => {
  it("reports origins for a SELECT of one table", async () => {
    const cols = await columnsOf("pg", "select id, email from users limit 3");
    expect(cols.map((c) => c.origin?.column)).toEqual(["id", "email"]);
    expect(cols[0]!.origin?.instance).toBeUndefined();
    // MySQL names the table reference (its alias).
    const my = await columnsOf("mysql", "select u.id, u.email from users as u limit 3");
    expect(my.map((c) => [c.table, c.origin?.instance])).toEqual([
      ["u", "u"],
      ["u", "u"],
    ]);
    const plain = await columnsOf("mysql", "select id from users where id > 1");
    expect(plain[0]!.origin?.instance).toBe("users");
  });

  it("reports none for joins, subqueries and set operations", async () => {
    for (const sql of [
      "select e.id, m.email from users e join users m on m.id = e.id",
      "select id, email from users, posts",
      "select id from users where id in (select user_id from posts)",
      "select id from users union select id from users",
      "with u as (select * from users) select id from users",
    ])
      expect((await columnsOf("pg", sql)).every((c) => !c.origin)).toBe(true);
  });
});

describe("mock SQL file commands", () => {
  const DIR = "/Users/me/mock-files";

  it("tells the size of a file without reading it, and that a .gz is compressed", async () => {
    await ipc.sqlFiles.write(`${DIR}/a.sql`, "select 'é';\n");
    // "é" is two bytes in UTF-8.
    expect(await ipc.sqlFiles.info(`${DIR}/a.sql`)).toEqual({ bytes: 13, compressed: false });
    await ipc.sqlFiles.write(`${DIR}/b.sql.gz`, "select 1;\n".repeat(100));
    const gz = await ipc.sqlFiles.info(`${DIR}/b.sql.gz`);
    expect(gz.compressed).toBe(true);
    expect(gz.bytes).toBeLessThan(1000);
    await expect(ipc.sqlFiles.info(`${DIR}/missing.sql`)).rejects.toMatchObject({
      payload: { code: "NOT_FOUND" },
    });
  });

  it("reads a head that ends after a whole line, or all of the file", async () => {
    const text = "select 1;\nselect 2;\nselect 3;\n";
    await ipc.sqlFiles.write(`${DIR}/c.sql`, text);
    expect(await ipc.sqlFiles.readHead(`${DIR}/c.sql`, null)).toMatchObject({
      text,
      truncated: false,
      encoding: null,
    });
    expect(await ipc.sqlFiles.readHead(`${DIR}/c.sql`, 100)).toMatchObject({
      text,
      truncated: false,
    });
    // 25 bytes end inside the third line: the head stops after the second.
    expect(await ipc.sqlFiles.readHead(`${DIR}/c.sql`, 25)).toMatchObject({
      text: "select 1;\nselect 2;\n",
      totalBytes: text.length,
      truncated: true,
    });
  });

  it("pretends to hold a huge dump that only opens as a head", async () => {
    const big = "/Users/me/dumps/huge-dump.sql";
    expect((await ipc.sqlFiles.info(big)).bytes).toBeGreaterThan(100 * 1024 * 1024);
    const head = await ipc.sqlFiles.readHead(big, 1000);
    expect(head.truncated).toBe(true);
    expect(head.text.length).toBeLessThanOrEqual(1000);
    expect(head.text.endsWith(";\n")).toBe(true);
    // Whole, it would be too big for one string.
    await expect(ipc.sqlFiles.readHead(big, null)).rejects.toMatchObject({
      payload: { code: "INVALID_INPUT" },
    });
    await expect(ipc.sqlFiles.read(big)).rejects.toMatchObject({
      payload: { code: "INVALID_INPUT" },
    });
  });
});
