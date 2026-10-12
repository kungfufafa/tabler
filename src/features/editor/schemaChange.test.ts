import { describe, expect, it } from "vitest";
import { changesSchema } from "./schemaChange";

describe("changesSchema", () => {
  it("recognizes DDL by its first word, in any case", () => {
    for (const sql of [
      "CREATE TABLE t (a int)",
      "alter table users drop column bio",
      "Drop View v",
      "RENAME TABLE a TO b",
      "truncate t",
      "COMMENT ON COLUMN t.a IS 'x'",
      "create\nindex i on t (a)",
    ])
      expect(changesSchema(sql), sql).toBe(true);
  });

  it("skips leading whitespace, comments and parentheses", () => {
    expect(changesSchema("  \n\t-- add the flag\nALTER TABLE t ADD c int")).toBe(true);
    expect(changesSchema("/* tidy */ /* again */ drop table t")).toBe(true);
    expect(changesSchema("/* multi\nline */\n-- and\n  create view v as select 1")).toBe(true);
    expect(changesSchema("# mysql comment\ncreate table t (a int)")).toBe(true);
    expect(changesSchema("(CREATE TABLE t (a int))")).toBe(true);
  });

  it("ignores everything else, including DDL words in comments, strings and names", () => {
    for (const sql of [
      "select * from created_items",
      "SELECT 'DROP TABLE t'",
      "INSERT INTO t VALUES ('CREATE')",
      "-- DROP TABLE t\nSELECT 1",
      "/* alter */ select 1",
      "update t set dropped = true",
      "creates",
      "create2 x",
      "",
      "   ",
      "-- only a comment",
      "/* unterminated",
    ])
      expect(changesSchema(sql), JSON.stringify(sql)).toBe(false);
  });
});
