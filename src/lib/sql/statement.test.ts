import { describe, expect, it } from "vitest";
import { isReadOnlyStatement, leadingKeyword } from "./statement";

describe("leadingKeyword", () => {
  it("returns the first word, upper-cased, after whitespace, comments and parentheses", () => {
    expect(leadingKeyword("select 1")).toBe("SELECT");
    expect(leadingKeyword("  \n-- note\n/* block */ (With x as (select 1) select * from x)")).toBe(
      "WITH",
    );
    expect(leadingKeyword("# mysql\nshow tables")).toBe("SHOW");
  });

  it("is null without a leading word", () => {
    for (const sql of ["", "  ", "-- only a comment", "/* unterminated", "'str'", "1"])
      expect(leadingKeyword(sql), JSON.stringify(sql)).toBeNull();
  });
});

describe("isReadOnlyStatement", () => {
  it("accepts plain queries", () => {
    for (const sql of [
      "select * from events_big",
      "SELECT id, email FROM users WHERE id > 10 ORDER BY id",
      "with t as (select 1 as a) select * from t",
      "VALUES (1), (2)",
      "TABLE users",
      "show tables",
      "EXPLAIN SELECT * FROM posts",
      "describe users",
      "/* report */ select count(*) from posts",
      "select * from jobs for update",
      "SELECT * FROM jobs FOR NO KEY UPDATE SKIP LOCKED",
      "select * from created_updates",
    ])
      expect(isReadOnlyStatement(sql), sql).toBe(true);
  });

  it("refuses statements that write, or would run something again", () => {
    for (const sql of [
      "delete from posts where id < 10 returning *",
      "UPDATE users SET bio = '' RETURNING id",
      "insert into t (a) values (1) returning a",
      "with d as (delete from posts returning *) select * from d",
      "WITH u AS (UPDATE t SET a = 1 RETURNING *) SELECT * FROM u FOR UPDATE",
      "explain analyze delete from posts",
      "EXPLAIN (ANALYZE, BUFFERS) SELECT 1",
      "select * into archive from posts",
      "SELECT * FROM t INTO OUTFILE '/tmp/x'",
      "select nextval('posts_id_seq')",
      "call refresh_stats()",
      "do $$ begin perform 1; end $$",
      "replace into t values (1)",
      "merge into t using s on t.id = s.id when matched then delete",
      "pragma table_info(t)",
      "",
      "-- nothing",
    ])
      expect(isReadOnlyStatement(sql), sql).toBe(false);
  });
});
