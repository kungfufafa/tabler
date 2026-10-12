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
