import { describe, expect, it } from "vitest";
import type { Dialect } from "@/lib/ipc";
import cases from "./classify.cases.json";
import { allReads, classifyStatement, type StatementClass } from "./classify";

interface Case {
  sql: string;
  dialect?: Dialect;
  class: StatementClass;
  note?: string;
}

const DIALECTS: Dialect[] = ["postgres", "mysql", "sqlite"];

describe("classifyStatement (cases shared with sqlgen/classify.rs)", () => {
  const rows = (cases as Case[]).flatMap((c) =>
    (c.dialect ? [c.dialect] : DIALECTS).map((d) => [d, c.sql, c.class] as const),
  );
  it.each(rows)("%s: %j is a %s", (dialect, sql, expected) => {
    expect(classifyStatement(sql, dialect)).toBe(expected);
  });
});

describe("allReads", () => {
  it("needs at least one statement, all of them reads", () => {
    expect(allReads([], "postgres")).toBe(false);
    expect(allReads(["SELECT 1", "SHOW TABLES", "EXPLAIN DELETE FROM t"], "mysql")).toBe(true);
    expect(allReads(["SELECT 1", "UPDATE t SET a = 1"], "sqlite")).toBe(false);
    expect(allReads(["SELECT 1", ""], "postgres")).toBe(false);
  });
});
