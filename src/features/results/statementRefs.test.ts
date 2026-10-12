import { describe, expect, it } from "vitest";
import {
  aliasedAs,
  cteNames,
  isCompound,
  names,
  readsCteTwice,
  references,
  sqlTokens,
} from "./statementRefs";

const pg = (sql: string) => sqlTokens(sql, "postgres");

describe("statement references", () => {
  it("skips strings and comments, and unquotes identifiers", () => {
    const t = sqlTokens(
      `select "A""b", [c], \`d\` from t -- users\n/* users */ where s = 'users'`,
      "sqlite",
    );
    expect(t.filter((x) => x.k === "name").map((x) => x.k === "name" && x.v)).toEqual([
      "select",
      'a"b',
      "c",
      "d",
      "from",
      "t",
      "where",
      "s",
    ]);
    expect(references(pg("select 'users', $$users$$, e'users' from t"), "users")).toBe(0);
  });

  it("counts table references, not column qualifiers", () => {
    expect(references(pg("select users.id, users.name from users"), "users")).toBe(1);
    expect(references(pg("select u.id from public.users u"), "users")).toBe(1);
    expect(references(pg('select * from "Users" u join users m on m.id = u.id'), "users")).toBe(2);
    expect(
      references(pg("select * from users where id in (select user_id from users)"), "USERS"),
    ).toBe(2);
    expect(references(pg("select * from posts"), "users")).toBe(0);
  });

  it("finds set operations", () => {
    expect(isCompound(pg("select 1 union all select 2"))).toBe(true);
    expect(isCompound(pg("select * from (select 1 except select 2) x"))).toBe(true);
    expect(isCompound(pg("SELECT 1 INTERSECT SELECT 1"))).toBe(true);
    expect(isCompound(pg(`select 'union', "union" from t -- union`))).toBe(false);
  });

  it("finds CTEs read more than once", () => {
    const twice = pg("with c as (select * from emp) select a.id, b.name from c a join c b on true");
    expect(cteNames(twice)).toEqual(["c"]);
    expect(readsCteTwice(twice)).toBe(true);
    const chained = pg(
      "with a(x) as materialized (select * from t), b as not materialized (select * from a) select * from b",
    );
    expect(cteNames(chained)).toEqual(["a", "b"]);
    expect(readsCteTwice(chained)).toBe(false);
    expect(readsCteTwice(pg("with c as (select * from emp) select c.id, c.name from c"))).toBe(
      false,
    );
  });

  it("tells whether a table is named and where an alias is defined", () => {
    const my = (sql: string) => sqlTokens(sql, "mysql");
    expect(names(my("select users.id from posts"), "users")).toBe(false);
    expect(names(my("call p()"), "users")).toBe(false);
    // A MySQL "…" is a string unless ANSI_QUOTES: it never names a table.
    expect(names(my('call p("users")'), "users")).toBe(false);
    expect(references(my('select * from "users"'), "users")).toBe(1);
    expect(names(my("select /*! straight_join */ id from `users`"), "users")).toBe(true);
    expect(names(my("select id from t /*!50000 join users */"), "users")).toBe(true);

    const self = my("select e.id, m.name from employees e join employees AS m on m.id = e.boss");
    expect(aliasedAs(self, "employees", "e")).toBe(true);
    expect(aliasedAs(self, "Employees", "M")).toBe(true);
    expect(aliasedAs(self, "employees", "x")).toBe(false);
    // A derived table's alias is not defined on the table.
    expect(aliasedAs(my("select * from (select * from posts) d"), "posts", "d")).toBe(false);
    expect(aliasedAs(my("select * from db.posts d"), "posts", "d")).toBe(true);
  });
});
