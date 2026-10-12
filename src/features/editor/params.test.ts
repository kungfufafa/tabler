import { describe, expect, it } from "vitest";
import { QUERY_PARAM_SYNTAXES, type QueryParamSyntax } from "@/stores/prefs";
import { findParams, originalOffset, paramFields, substituteParams } from "./params";

const keys = (sql: string, syntax: QueryParamSyntax, inQuotes = false, dialect = "postgres") =>
  findParams(sql, dialect as "postgres", { syntax, inQuotes }).map((o) => o.key);

describe("findParams", () => {
  it("knows the six TablePlus syntaxes", () => {
    expect(QUERY_PARAM_SYNTAXES).toEqual([
      ":[\\w.]+",
      "%[\\w.]+%",
      "\\?",
      "\\$\\{[\\w.]+\\}",
      "\\$[\\w.]+",
      "\\{\\{[\\w.]+\\}\\}",
    ]);
  });

  it.each<[QueryParamSyntax, string, string[]]>([
    [":[\\w.]+", "select * from t where id = :id and a.b = :user.name", [":id", ":user.name"]],
    ["%[\\w.]+%", "select * from t where id = %id% and n = %x.y%", ["%id%", "%x.y%"]],
    ["\\?", "select * from t where a = ? and b = ?", ["?1", "?2"]],
    ["\\$\\{[\\w.]+\\}", "select ${col} from t where id = ${id}", ["${col}", "${id}"]],
    ["\\$[\\w.]+", "select * from t where id = $id and n = $p.q", ["$id", "$p.q"]],
    ["\\{\\{[\\w.]+\\}\\}", "select * from t where id = {{id}} limit {{n}}", ["{{id}}", "{{n}}"]],
  ])("finds %s variables", (syntax, sql, expected) => {
    expect(keys(sql, syntax)).toEqual(expected);
  });

  it("skips strings and quoted identifiers unless allowed, and always skips comments", () => {
    const sql = `select ':a', ":b", :c -- :d\n/* :e */ from t where x = 'it''s :f'`;
    expect(keys(sql, ":[\\w.]+")).toEqual([":c"]);
    expect(keys(sql, ":[\\w.]+", true)).toEqual([":a", ":b", ":c", ":f"]);
    expect(keys("select '{{x}}', {{y}}", "\\{\\{[\\w.]+\\}\\}")).toEqual(["{{y}}"]);
    expect(keys("select '?' , ?", "\\?")).toEqual(["?1"]);
    expect(keys("select '?' , ?", "\\?", true)).toEqual(["?1", "?2"]);
  });

  it("does not read PostgreSQL casts or dollar quotes as variables", () => {
    expect(keys("select '1'::int, x::text, :y", ":[\\w.]+")).toEqual([":y"]);
    expect(keys("select $$ $inside $$, $outside", "\\$[\\w.]+")).toEqual(["$outside"]);
  });

  it("follows the dialect's quoting (MySQL backticks, # comments)", () => {
    expect(keys("select `:a`, :b # :c", ":[\\w.]+", false, "mysql")).toEqual([":b"]);
  });

  it("returns offsets of each occurrence", () => {
    const sql = "a = :x or b = :x";
    expect(findParams(sql, "postgres", { syntax: ":[\\w.]+", inQuotes: false })).toEqual([
      { key: ":x", start: 4, end: 6 },
      { key: ":x", start: 14, end: 16 },
    ]);
  });
});

describe("paramFields", () => {
  it("asks once per distinct variable, in order", () => {
    const occ = findParams("select :b, :a, :b", "postgres", {
      syntax: ":[\\w.]+",
      inQuotes: false,
    });
    expect(paramFields(occ)).toEqual([
      { key: ":b", label: ":b" },
      { key: ":a", label: ":a" },
    ]);
  });

  it("labels positional ? by number", () => {
    const occ = findParams("? ?", "postgres", { syntax: "\\?", inQuotes: false });
    expect(paramFields(occ).map((f) => f.label)).toEqual(["? #1", "? #2"]);
  });
});

describe("substituteParams", () => {
  it("replaces every occurrence as raw SQL text", () => {
    const sql = "select * from users where id = :id or parent = :id and name = :name";
    const occ = findParams(sql, "postgres", { syntax: ":[\\w.]+", inQuotes: false });
    const out = substituteParams(sql, occ, { ":id": "42", ":name": "'bob'" });
    expect(out.sql).toBe("select * from users where id = 42 or parent = 42 and name = 'bob'");
    expect(out.replacements).toEqual([
      { start: 31, end: 34, length: 2 },
      { start: 47, end: 50, length: 2 },
      { start: 62, end: 67, length: 5 },
    ]);
  });

  it("fills positional ? one by one and leaves missing values empty", () => {
    const sql = "select ?, ?";
    const occ = findParams(sql, "postgres", { syntax: "\\?", inQuotes: false });
    expect(substituteParams(sql, occ, { "?1": "1" }).sql).toBe("select 1, ");
  });

  it("maps offsets of the substituted text back to the original", () => {
    const sql = "select :id, x";
    const occ = findParams(sql, "postgres", { syntax: ":[\\w.]+", inQuotes: false });
    const { sql: out, replacements } = substituteParams(sql, occ, { ":id": "12345" });
    expect(out).toBe("select 12345, x");
    // Before the value, inside it, after it.
    expect(originalOffset(replacements, 3)).toBe(3);
    expect(originalOffset(replacements, 7)).toBe(7);
    expect(originalOffset(replacements, 9)).toBe(7);
    expect(originalOffset(replacements, 9, 1)).toBe(10);
    expect(originalOffset(replacements, out.indexOf("x"))).toBe(sql.indexOf("x"));
    // An empty value: its end is the variable's end.
    const empty = substituteParams(sql, occ, {});
    expect(empty.sql).toBe("select , x");
    expect(originalOffset(empty.replacements, 7, -1)).toBe(7);
    expect(originalOffset(empty.replacements, 7, 1)).toBe(10);
    expect(originalOffset(empty.replacements, 9)).toBe(12);
  });
});
