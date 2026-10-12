import { describe, expect, it } from "vitest";
import type { ColumnMeta, ResultColumn, TableMeta, TableRef } from "@/lib/ipc";
import {
  originSources,
  originTables,
  resultEditability,
  resultGridColumns,
  sameTable,
  sameTarget,
  targetFor,
  type EditabilityOptions,
} from "./editability";

const USERS: TableRef = { schema: "public", name: "users" };
const POSTS: TableRef = { schema: "public", name: "posts" };

/** A result column; `origin`: [table, column, MySQL instance]. */
function col(name: string, origin: [TableRef, string, string?] | null = null): ResultColumn {
  return {
    name,
    dataType: "int4",
    kind: "int",
    nullable: null,
    table: origin?.[2] ?? origin?.[0].name ?? null,
    origin: origin
      ? { table: origin[0], column: origin[1], ...(origin[2] && { instance: origin[2] }) }
      : null,
  };
}

/** The statement behind the results of most tests: names both tables once. */
const SQL = "select * from users u join posts p on p.user_id = u.id";
const opts = (sql: string | null = SQL, dialect: EditabilityOptions["dialect"] = "postgres") => ({
  sql,
  dialect,
});

function meta(table: TableRef, columns: string[], pk: string[], kind = "table"): TableMeta {
  return {
    table,
    kind,
    columns: columns.map(
      (name, i): ColumnMeta =>
        ({
          name,
          ordinal: i + 1,
          dataType: "int4",
          kind: "int",
          nullable: !pk.includes(name),
          defaultExpr: name === "id" ? "nextval('seq')" : null,
          isPk: pk.includes(name),
          pkOrdinal: pk.includes(name) ? pk.indexOf(name) + 1 : null,
          isAutoIncrement: name === "id",
          isGenerated: name === "generated",
          isUnsigned: false,
          comment: null,
          enumValues: null,
          charMaxLen: null,
          numericPrecision: null,
          numericScale: null,
          collation: null,
          arrayElemType: null,
          extra: null,
        }) as ColumnMeta,
    ),
    primaryKey: pk,
    indexes: [],
    foreignKeys: [],
    triggers: [],
    comment: null,
    rowEstimate: null,
    ddl: null,
  } as unknown as TableMeta;
}

const users = meta(USERS, ["id", "email", "name", "generated"], ["id"]);
const posts = meta(POSTS, ["id", "user_id", "title"], ["id"]);
const metas =
  (...list: TableMeta[]) =>
  (t: TableRef) =>
    list.find((m) => sameTable(m.table, t)) ?? null;

describe("result editability", () => {
  it("lists the origin tables in column order", () => {
    const cols = [col("title", [POSTS, "title"]), col("n"), col("email", [USERS, "email"])];
    expect(originTables(cols)).toEqual([POSTS, USERS]);
    expect(originTables([col("a"), col("b")])).toEqual([]);
  });

  it("is read-only without origins, on a read-only connection, or while metadata loads", () => {
    const computed = resultEditability([col("count")], metas(users), opts());
    expect(computed).toEqual({ kind: "readOnly", reason: "Its columns do not come from a table." });
    const cols = [col("id", [USERS, "id"]), col("email", [USERS, "email"])];
    expect(
      resultEditability(cols, metas(users), { ...opts(), readOnlyConnection: true }),
    ).toMatchObject({ kind: "readOnly" });
    expect(resultEditability(cols, () => undefined, opts())).toEqual({ kind: "pending" });
  });

  it("maps the columns of the table whose primary key is in the result", () => {
    const cols = [
      col("user_id", [USERS, "id"]),
      col("mail", [USERS, "email"]),
      col("shout"),
      col("again", [USERS, "id"]),
    ];
    const e = resultEditability(cols, metas(users), opts());
    if (e.kind !== "editable") throw new Error(e.kind);
    expect(e.target.table).toEqual(USERS);
    expect(e.target.keyIndexes).toEqual([0]);
    expect(e.target.columns.map((c) => c?.name ?? null)).toEqual(["id", "email", null, null]);
  });

  it("explains why a result stays read-only", () => {
    const noKey = resultEditability([col("email", [USERS, "email"])], metas(users), opts());
    expect(noKey).toEqual({
      kind: "readOnly",
      reason: "The primary key of public.users (id) is not in the result.",
    });
    const keyless = meta(USERS, ["email"], []);
    expect(
      resultEditability([col("email", [USERS, "email"])], metas(keyless), opts()),
    ).toMatchObject({
      reason: "public.users has no primary key.",
    });
    const view = meta(USERS, ["id"], ["id"], "view");
    expect(resultEditability([col("id", [USERS, "id"])], metas(view), opts())).toMatchObject({
      reason: "public.users is a view, not a table.",
    });
    expect(resultEditability([col("id", [USERS, "id"])], () => null, opts())).toMatchObject({
      kind: "readOnly",
      reason: "The structure of public.users could not be loaded.",
    });
  });

  it("picks the first table of a join that has its key in the result", () => {
    // posts has no key column here, users does: users is the target.
    const join = [
      col("title", [POSTS, "title"]),
      col("id", [USERS, "id"]),
      col("email", [USERS, "email"]),
    ];
    const e = resultEditability(join, metas(users, posts), opts());
    expect(e.kind === "editable" && e.target.table).toEqual(USERS);
    // Both have their key: the first one (in column order) wins.
    const both = [col("pid", [POSTS, "id"]), ...join];
    const f = resultEditability(both, metas(users, posts), opts());
    expect(f.kind === "editable" && f.target.table).toEqual(POSTS);
    // An earlier table still loading keeps the result pending.
    const pendingFirst = resultEditability(
      both,
      (t) => (sameTable(t, POSTS) ? undefined : users),
      opts(),
    );
    expect(pendingFirst).toEqual({ kind: "pending" });
  });

  it("never moves the target to a later table when an earlier one's structure fails to load", () => {
    const both = [col("pid", [POSTS, "id"]), col("id", [USERS, "id"]), col("e", [USERS, "email"])];
    // posts (the target) could not be reloaded: users must not take over its staged row keys.
    const failed = resultEditability(both, (t) => (sameTable(t, POSTS) ? null : users), opts());
    expect(failed).toEqual({
      kind: "readOnly",
      reason: "The structure of public.posts could not be loaded.",
    });
  });

  it("refuses a self-join when the server cannot tell the table's references apart", () => {
    // select e.id, m.name as manager from users e join users m on m.id = e.boss_id
    const cols = [col("id", [USERS, "id"]), col("manager", [USERS, "name"])];
    const sql = "select e.id, m.name as manager from users e join users m on m.id = e.boss_id";
    for (const dialect of ["postgres", "sqlite"] as const)
      expect(resultEditability(cols, metas(users), opts(sql, dialect))).toEqual({
        kind: "readOnly",
        reason: "The table public.users appears more than once in the query.",
      });
    // Also through a subquery, or a CTE read twice.
    for (const q of [
      "select id, name from users where id in (select boss_id from users)",
      'with c as (select * from "users") select a.id, b.name from c a join c b on b.id = a.boss_id',
    ])
      expect(resultEditability(cols, metas(users), opts(q))).toMatchObject({ kind: "readOnly" });
    // Column qualifiers are no references; one CTE read once is fine.
    for (const q of [
      "select users.id, users.name from public.users order by users.id",
      "with c as (select * from users) select c.id, c.name from c",
    ])
      expect(resultEditability(cols, metas(users), opts(q)).kind, q).toBe("editable");
  });

  it("MySQL: edits only the table reference the key comes from", () => {
    // select e.id, m.name as manager, e.email from users e join users m on m.id = e.boss_id
    const cols = [
      col("id", [USERS, "id", "e"]),
      col("manager", [USERS, "name", "m"]),
      col("email", [USERS, "email", "e"]),
      col("mid", [USERS, "id", "m"]),
    ];
    const sql =
      "select e.id, m.name as manager, e.email, m.id as mid from users e join users as m on m.id = e.boss_id";
    expect(originSources(cols).map((s) => s.instance)).toEqual(["e", "m"]);
    const e = resultEditability(cols, metas(users), opts(sql, "mysql"));
    if (e.kind !== "editable") throw new Error(e.kind);
    // The manager's name is the other row's: read-only; the key is e.id, not m.id.
    expect(e.target.columns.map((c) => c?.name ?? null)).toEqual(["id", null, "email", null]);
    expect(e.target.keyIndexes).toEqual([0]);
    // The other side alone has its key too, but the first reference wins.
    const flipped = resultEditability(
      [cols[3]!, cols[1]!, cols[0]!],
      metas(users),
      opts(sql, "mysql"),
    );
    expect(
      flipped.kind === "editable" && flipped.target.columns.map((c) => c?.name ?? null),
    ).toEqual(["id", "name", null]);
    // A key split over both references is no key.
    const split = [col("id", [USERS, "id", "e"]), col("name", [USERS, "name", "m"])];
    const meta2 = meta(USERS, ["id", "name"], ["id", "name"]);
    expect(resultEditability(split, metas(meta2), opts(sql, "mysql")).kind).toBe("readOnly");
  });

  it("MySQL: an alias the query does not define on the table is a derived table's", () => {
    // select * from (select p.id, p.title from posts p) d: MySQL reports d / p as table / org_table.
    const derived = [col("id", [POSTS, "id", "d"]), col("title", [POSTS, "title", "d"])];
    expect(
      resultEditability(
        derived,
        metas(posts),
        opts("select * from (select * from posts) d", "mysql"),
      ),
    ).toEqual({
      kind: "readOnly",
      reason: "public.posts is read through a derived table or CTE (d).",
    });
    // Without an alias the instance is the table's own name.
    const plain = [col("id", [POSTS, "id", "posts"]), col("title", [POSTS, "title", "posts"])];
    expect(
      resultEditability(plain, metas(posts), opts("select id, title from posts", "mysql")).kind,
    ).toBe("editable");
    // Without instances (an older payload) MySQL falls back to counting references.
    const bare = [col("id", [POSTS, "id"])];
    const twice = "select a.id from posts a join posts b on b.id = a.id";
    expect(resultEditability(bare, metas(posts), opts(twice, "mysql")).kind).toBe("readOnly");
  });

  it("refuses set operations, tables the query does not name, and unknown statements", () => {
    const cols = [col("id", [USERS, "id"]), col("name", [USERS, "name"])];
    expect(
      resultEditability(
        cols,
        metas(users),
        opts("select id, name from users union all select id, name from suppliers", "sqlite"),
      ),
    ).toEqual({
      kind: "readOnly",
      reason: "It combines the rows of several queries (UNION, EXCEPT or INTERSECT).",
    });
    // A view (SQLite resolves it to its base table) or a procedure's result.
    for (const sql of [
      "select * from everyone",
      "call list_users()",
      "select x.id from everyone x",
    ])
      expect(resultEditability(cols, metas(users), opts(sql, "sqlite"))).toEqual({
        kind: "readOnly",
        reason:
          "The query does not name public.users (its rows come through a view, a function or a procedure).",
      });
    expect(resultEditability(cols, metas(users), opts(null))).toEqual({
      kind: "readOnly",
      reason: "The query that produced it is not known.",
    });
    // One table ruled out leaves the next one of a join editable.
    const join = [col("id", [USERS, "id"]), col("pid", [POSTS, "id"]), col("t", [POSTS, "title"])];
    const sql =
      "select u.id, p.id as pid, p.title from users u join users x on x.id = u.id join posts p on true";
    const e = resultEditability(join, metas(users, posts), opts(sql));
    expect(e.kind === "editable" && e.target.table).toEqual(POSTS);
  });

  it("compares edit targets", () => {
    const cols = [col("id", [USERS, "id"]), col("email", [USERS, "email"])];
    const a = targetFor(cols, USERS, users)!;
    expect(sameTarget(a, targetFor(cols, USERS, { ...users })!)).toBe(true);
    expect(sameTarget(a, targetFor([cols[0]!, col("x")], USERS, users)!)).toBe(false);
    const rekeyed = meta(USERS, ["id", "email", "name", "generated"], ["id", "email"]);
    expect(sameTarget(a, targetFor(cols, USERS, rekeyed)!)).toBe(false);
    const postsCols = [col("id", [POSTS, "id"]), col("user_id", [POSTS, "user_id"])];
    expect(sameTarget(a, targetFor(postsCols, POSTS, posts)!)).toBe(false);
  });

  it("builds grid columns: target columns editable with the table's traits", () => {
    const cols = [col("id", [USERS, "id"]), col("email", [USERS, "email"]), col("n")];
    cols.push(col("generated", [USERS, "generated"]));
    const target = targetFor(cols, USERS, users)!;
    const grid = resultGridColumns(cols, target);
    expect(grid.map((c) => [c.id, c.editable, c.isPk, c.hasDefault])).toEqual([
      ["0", true, true, true],
      ["1", true, false, false],
      ["2", false, false, false],
      ["3", false, false, false],
    ]);
    expect(resultGridColumns(cols).every((c) => !c.editable)).toBe(true);
  });
});
