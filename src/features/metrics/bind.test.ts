import { describe, expect, it } from "vitest";
import { bindVariables, neutralSql, splitTemplate, variablesOf } from "./bind";

describe("board variables", () => {
  it("cuts the SQL at its {{variables}}, outside comments and strings", () => {
    const sql =
      "-- {{not_me}}\nSELECT * FROM t WHERE a = {{a}} AND b = '{{quoted}}' AND c > {{a}} /* {{x}} */";
    expect(splitTemplate(sql, "postgres")).toEqual({
      parts: [
        "-- {{not_me}}\nSELECT * FROM t WHERE a = ",
        " AND b = '{{quoted}}' AND c > ",
        " /* {{x}} */",
      ],
      names: ["a", "a"],
    });
    expect(variablesOf(sql, "postgres")).toEqual(["a"]);
  });

  it("follows each dialect's quoting", () => {
    // MySQL: `#` comments and backtick identifiers.
    expect(splitTemplate("SELECT `{{a}}` # {{b}}\n, {{c}}", "mysql").names).toEqual(["c"]);
    // PostgreSQL: dollar quoting, and a `::` cast right after a variable.
    expect(splitTemplate("SELECT $$ {{a}} $$, {{b}}::int", "postgres")).toEqual({
      parts: ["SELECT $$ {{a}} $$, ", "::int"],
      names: ["b"],
    });
    // SQLite: [bracketed] identifiers.
    expect(splitTemplate("SELECT [{{a}}], {{d.e}}", "sqlite").names).toEqual(["d.e"]);
  });

  it("binds a value to every occurrence and reports unknown variables", () => {
    const t = splitTemplate("SELECT {{a}}, {{b}}, {{a}}", "sqlite");
    const { query, missing } = bindVariables(t, { a: { t: "int", v: 1 } });
    expect(query).toEqual({
      parts: ["SELECT ", ", ", ", ", ""],
      values: [{ t: "int", v: 1 }, { t: "null" }, { t: "int", v: 1 }],
    });
    expect(missing).toEqual(["b"]);
    expect(neutralSql(t)).toBe("SELECT  NULL ,  NULL ,  NULL ");
  });

  it("leaves SQL without variables whole", () => {
    expect(splitTemplate("SELECT 1", "mysql")).toEqual({ parts: ["SELECT 1"], names: [] });
    expect(bindVariables({ parts: ["SELECT 1"], names: [] }, {}).query.values).toEqual([]);
  });
});
