import { describe, expect, it } from "vitest";
import { charToUtf16, lineColumnAt, utf16ToChar } from "./offsets";
import {
  minifySql,
  splitStatements,
  statementAt,
  statementTexts,
  type SqlDialect,
} from "./scanner";

const ALL: SqlDialect[] = ["postgres", "mysql", "sqlite"];

/** Statement texts, checking that every span maps back onto the input and its line is right. */
function texts(sql: string, d: SqlDialect): string[] {
  const out = splitStatements(sql, d);
  for (const st of out) {
    const line = sql.slice(0, st.start).split("\n").length;
    expect(st.line).toBe(line);
    expect(st.end).toBeGreaterThan(st.start);
  }
  return out.map((s) => sql.slice(s.start, s.end));
}

describe("splitStatements", () => {
  it("splits simple statements with trimmed offsets", () => {
    const sql = "select 1;\nselect 2 ;  \n\n  update users set name = name where false;";
    expect(splitStatements(sql, "postgres")).toEqual([
      { start: 0, end: 8, line: 1 },
      { start: 10, end: 18, line: 2 },
      { start: 26, end: 66, line: 4 },
    ]);
  });

  it("skips empty and comment-only statements", () => {
    for (const d of ALL) {
      expect(texts("", d)).toEqual([]);
      expect(texts(" ;; ;\n", d)).toEqual([]);
      expect(texts("-- just a comment\n", d)).toEqual([]);
      expect(texts("/* block */ ; -- trailing", d)).toEqual([]);
      expect(texts("select 1; -- done", d)).toEqual(["select 1"]);
      expect(texts("select 1 -- note\n;", d)).toEqual(["select 1 -- note"]);
    }
  });

  it("leaves leading comments out of the statement", () => {
    const out = splitStatements("-- first\n/* second */\nselect 1;", "postgres");
    expect(out).toEqual([{ start: 22, end: 30, line: 3 }]);
    expect(texts("select /* ; */ 1 -- x;\n + 2;", "sqlite")).toEqual([
      "select /* ; */ 1 -- x;\n + 2",
    ]);
  });

  it("respects quotes and quoted identifiers", () => {
    for (const d of ALL) {
      expect(texts("select 'a;b', 'it''s;'; select 2", d)).toEqual([
        "select 'a;b', 'it''s;'",
        "select 2",
      ]);
      expect(texts('select "x;y" from t; select 3', d)).toEqual([
        'select "x;y" from t',
        "select 3",
      ]);
    }
    expect(texts("select `a;b` from t; select 1", "mysql")).toEqual([
      "select `a;b` from t",
      "select 1",
    ]);
    expect(texts("select `a;b` from t; select 1", "sqlite")).toEqual([
      "select `a;b` from t",
      "select 1",
    ]);
    expect(texts("select [a;b] from t; select 1", "sqlite")).toEqual([
      "select [a;b] from t",
      "select 1",
    ]);
    // Backticks are not quotes in PostgreSQL.
    expect(texts("select `a;b` from t", "postgres")).toHaveLength(2);
  });

  it("handles backslash escapes per dialect (incl. E'' strings)", () => {
    expect(texts(String.raw`select 'a\';b'; select 2`, "mysql")).toEqual([
      String.raw`select 'a\';b'`,
      "select 2",
    ]);
    expect(texts(String.raw`select "a\";b"; select 2`, "mysql")).toEqual([
      String.raw`select "a\";b"`,
      "select 2",
    ]);
    expect(texts(String.raw`select 'a\'; select 2`, "postgres")).toEqual([
      String.raw`select 'a\'`,
      "select 2",
    ]);
    expect(texts(String.raw`select E'a\';b'; select 2`, "postgres")).toEqual([
      String.raw`select E'a\';b'`,
      "select 2",
    ]);
    expect(texts(String.raw`select e'\\'; select 2`, "postgres")).toEqual([
      String.raw`select e'\\'`,
      "select 2",
    ]);
    expect(texts("select date'2020-01-01'; select 2", "postgres")).toEqual([
      "select date'2020-01-01'",
      "select 2",
    ]);
  });

  it("handles comments: --, #, nested /* */", () => {
    for (const d of ALL) {
      expect(texts("select 1 /* ; */; select 2 -- ;\n; select 3", d)).toEqual([
        "select 1 /* ; */",
        "select 2 -- ;",
        "select 3",
      ]);
    }
    expect(texts("/* a /* b */ ; */ select 1; select 2", "postgres")).toEqual([
      "select 1",
      "select 2",
    ]);
    expect(texts("/* a /* b */ select 1; select 2", "mysql")).toEqual(["select 1", "select 2"]);
    expect(texts("select 1 # ; x\n; select 2", "mysql")).toEqual(["select 1 # ; x", "select 2"]);
    expect(texts("select 1--1; select 2", "mysql")).toEqual(["select 1--1", "select 2"]);
    expect(texts("select 1--1; select 2", "postgres")).toEqual(["select 1--1; select 2"]);
    // `#` is an operator outside MySQL.
    expect(texts("select 1 # 2; select 3", "postgres")).toEqual(["select 1 # 2", "select 3"]);
    expect(texts("/*!40101 SET NAMES utf8 */;\nselect 1;", "mysql")).toEqual([
      "/*!40101 SET NAMES utf8 */",
      "select 1",
    ]);
  });

  it("handles dollar quoting", () => {
    const fn =
      "create function f() returns int as $$ begin; return 1; end; $$ language plpgsql;\nselect f();";
    expect(texts(fn, "postgres")).toEqual([
      "create function f() returns int as $$ begin; return 1; end; $$ language plpgsql",
      "select f()",
    ]);
    const tagged =
      "do $body$ declare x text := '$$;'; begin end $body$; select $1, a$b from t; select 2";
    expect(texts(tagged, "postgres")).toEqual([
      "do $body$ declare x text := '$$;'; begin end $body$",
      "select $1, a$b from t",
      "select 2",
    ]);
    expect(texts("select $x$ ; abc", "postgres")).toEqual(["select $x$ ; abc"]);
  });

  it("keeps semicolons inside PostgreSQL parentheses", () => {
    expect(
      texts(
        "create rule r as on insert to t do also (insert into a values (1); insert into b values (2)); select 1",
        "postgres",
      ),
    ).toEqual([
      "create rule r as on insert to t do also (insert into a values (1); insert into b values (2))",
      "select 1",
    ]);
    expect(texts("select (1; select 2)", "mysql")).toHaveLength(2);
  });

  it("keeps BEGIN … END routine bodies together", () => {
    const trigger =
      "CREATE TRIGGER trg AFTER INSERT ON t BEGIN\n  UPDATE t SET a = CASE WHEN new.a > 0 THEN 1 ELSE 0 END;\n  INSERT INTO log VALUES (1);\nEND;\nselect 1;";
    expect(texts(trigger, "sqlite")).toEqual([
      "CREATE TRIGGER trg AFTER INSERT ON t BEGIN\n  UPDATE t SET a = CASE WHEN new.a > 0 THEN 1 ELSE 0 END;\n  INSERT INTO log VALUES (1);\nEND",
      "select 1",
    ]);
    const proc =
      "CREATE PROCEDURE p(IN n INT)\nBEGIN\n  IF n > 0 THEN SELECT 1; END IF;\n  CASE n WHEN 1 THEN SELECT 'one'; ELSE SELECT 'many'; END CASE;\nEND;\nCALL p(3);";
    const out = texts(proc, "mysql");
    expect(out).toHaveLength(2);
    expect(out[1]).toBe("CALL p(3)");
    expect(texts("BEGIN; select case when 1 then 2 end; COMMIT;", "sqlite")).toEqual([
      "BEGIN",
      "select case when 1 then 2 end",
      "COMMIT",
    ]);
    expect(texts("BEGIN NOT ATOMIC SELECT 1; SELECT 2; END; SELECT 3", "mysql")).toEqual([
      "BEGIN NOT ATOMIC SELECT 1; SELECT 2; END",
      "SELECT 3",
    ]);
  });

  it("honours the MySQL DELIMITER directive", () => {
    const sql =
      "DELIMITER $$\nCREATE PROCEDURE p()\nBEGIN\n  SELECT 1;\n  SELECT 2;\nEND$$\nDELIMITER ;\nCALL p();\nselect 'x';";
    const out = splitStatements(sql, "mysql");
    expect(out.map((s) => sql.slice(s.start, s.end))).toEqual([
      "CREATE PROCEDURE p()\nBEGIN\n  SELECT 1;\n  SELECT 2;\nEND",
      "CALL p()",
      "select 'x'",
    ]);
    expect(out.map((s) => s.line)).toEqual([2, 8, 9]);
    expect(texts("select delimiter from t; select 1", "mysql")).toEqual([
      "select delimiter from t",
      "select 1",
    ]);
  });

  it("makes psql meta-commands statements of their own", () => {
    expect(texts("select 1;\n\\d users\nselect 2;", "postgres")).toEqual([
      "select 1",
      "\\d users",
      "select 2",
    ]);
  });

  it("does not hang on unterminated constructs", () => {
    for (const d of ALL)
      for (const sql of [
        "select 'abc",
        'select "abc',
        "/* abc",
        "select $$abc",
        "select `x",
        "select [x",
        "--",
        "-",
        "$",
        "e'",
        "DELIMITER",
        "DELIMITER ",
        "\\",
      ])
        texts(sql, d);
  });

  it("uses UTF-16 offsets", () => {
    const sql = "select '🙂';\nselect 2";
    const out = splitStatements(sql, "postgres");
    expect(out[0]).toEqual({ start: 0, end: 11, line: 1 });
    expect(sql.slice(out[1]!.start, out[1]!.end)).toBe("select 2");
  });
});

describe("statementAt", () => {
  const sql = "select 1;  \n\nselect 2;\n  select 3; select 4";
  const stmts = splitStatements(sql, "postgres");
  const at = (pos: number, loose = false) => {
    const s = statementAt(stmts, sql, pos, loose);
    return s ? sql.slice(s.start, s.end) : null;
  };

  it("finds the statement containing the cursor (end inclusive)", () => {
    expect(at(0)).toBe("select 1");
    expect(at(4)).toBe("select 1");
    expect(at(8)).toBe("select 1");
    expect(at(sql.indexOf("select 4") + 3)).toBe("select 4");
    expect(at(sql.length)).toBe("select 4");
  });

  it("picks the statement terminated earlier on the same line", () => {
    expect(at(9)).toBe("select 1");
    expect(at(11)).toBe("select 1");
    expect(at(sql.indexOf("select 3") + 9)).toBe("select 3");
  });

  it("picks a statement starting later on the cursor's line", () => {
    expect(at(sql.indexOf("  select 3"))).toBe("select 3");
  });

  it("returns null on blank lines unless loose", () => {
    expect(at(12)).toBeNull();
    expect(at(12, true)).toBe("select 1");
    expect(statementAt([], "", 0, true)).toBeNull();
  });

  it("returns the next statement when the cursor precedes everything (loose)", () => {
    const text = "\n\n  select 9";
    expect(statementAt(splitStatements(text, "sqlite"), text, 0, true)?.start).toBe(4);
  });

  it("statementTexts is a convenience over splitStatements", () => {
    expect(statementTexts("a; b", "sqlite")).toEqual(["a", "b"]);
  });
});

describe("minifySql", () => {
  it("collapses whitespace runs to one space and trims", () => {
    for (const d of ALL) {
      expect(minifySql("  select\n\tid,\r\n   name\n  from users\n where id = 1 ;\n\n", d)).toBe(
        "select id, name from users where id = 1 ;",
      );
      expect(minifySql("", d)).toBe("");
      expect(minifySql(" \n\t ", d)).toBe("");
    }
  });

  it("drops comments but never glues tokens together", () => {
    for (const d of ALL) {
      expect(minifySql("-- head\nselect 1 -- one\n, 2 /* two */\nfrom t; -- tail", d)).toBe(
        "select 1 , 2 from t;",
      );
      expect(minifySql("select a/* x */b", d)).toBe("select a b");
      expect(minifySql("/* only a comment */", d)).toBe("");
    }
    // MySQL needs whitespace after `--`; `#` starts a comment there only.
    expect(minifySql("select 1--2\nfrom t", "mysql")).toBe("select 1--2 from t");
    expect(minifySql("select 1 # note\nfrom t", "mysql")).toBe("select 1 from t");
    expect(minifySql("select 1 # x", "postgres")).toBe("select 1 # x");
    // PostgreSQL block comments nest.
    expect(minifySql("select /* a /* b */ c */ 1", "postgres")).toBe("select 1");
  });

  it("keeps strings, quoted identifiers and dollar quotes exactly", () => {
    for (const d of ALL) {
      expect(minifySql("select 'a  --  b\n  c',  \"x  /* y */\"", d)).toBe(
        "select 'a  --  b\n  c', \"x  /* y */\"",
      );
      expect(minifySql("select 'it''s   ok'  ,  1", d)).toBe("select 'it''s   ok' , 1");
    }
    expect(minifySql("select `a   b` ,\n 'x\\'  y'", "mysql")).toBe("select `a   b` , 'x\\'  y'");
    expect(minifySql("select [a   b]\n", "sqlite")).toBe("select [a   b]");
    expect(minifySql("select E'a\\'  --  b'  ,  1", "postgres")).toBe("select E'a\\'  --  b' , 1");
    const fn =
      "create function f() returns int as $body$\n  select 1; -- keep\n$body$ language sql;";
    expect(minifySql(fn, "postgres")).toBe(
      "create function f() returns int as $body$\n  select 1; -- keep\n$body$ language sql;",
    );
    expect(minifySql("select $1  ,  a$b", "postgres")).toBe("select $1 , a$b");
  });

  it("keeps the line break that joins PostgreSQL string constants", () => {
    // 'a'\n'b' is one constant ('ab') in PostgreSQL; 'a' 'b' on one line is a syntax error.
    expect(minifySql("select 'foo'\n  'bar' ,  1", "postgres")).toBe("select 'foo'\n'bar' , 1");
    expect(minifySql("select 'a' -- note\n E'b'", "postgres")).toBe("select 'a'\nE'b'");
    // Only between two string constants, and only when the original had a line break.
    expect(minifySql("select 'a'  'b'", "postgres")).toBe("select 'a' 'b'");
    expect(minifySql("select 'a'\n  \"b\"", "postgres")).toBe("select 'a' \"b\"");
    expect(minifySql("select 'a'\n'b'", "mysql")).toBe("select 'a' 'b'");
  });

  it("keeps MySQL executable comments and optimizer hints", () => {
    expect(minifySql("/*!40101 SET  NAMES utf8 */ ;\nselect /*+ BKA(t) */  1", "mysql")).toBe(
      "/*!40101 SET  NAMES utf8 */ ; select /*+ BKA(t) */ 1",
    );
  });

  it("keeps client directives on their own line", () => {
    expect(minifySql("select 1;\n\\connect other\nselect\n  2;", "postgres")).toBe(
      "select 1;\n\\connect other\nselect 2;",
    );
    const script =
      "DELIMITER $$\ncreate procedure p()\nbegin\n  select 1;\nend$$\nDELIMITER ;\ncall p();";
    expect(minifySql(script, "mysql")).toBe(
      "DELIMITER $$\ncreate procedure p() begin select 1; end$$\nDELIMITER ;\ncall p();",
    );
    // The result still splits like the original.
    expect(statementTexts(minifySql(script, "mysql"), "mysql")).toHaveLength(2);
  });
});

describe("offsets", () => {
  it("maps between character and UTF-16 offsets", () => {
    const text = "a🙂b🙂c";
    expect(charToUtf16(text, 0)).toBe(0);
    expect(charToUtf16(text, 2)).toBe(3);
    expect(charToUtf16(text, 4)).toBe(6);
    expect(charToUtf16(text, 99)).toBe(text.length);
    expect(utf16ToChar(text, 3)).toBe(2);
    expect(utf16ToChar(text, text.length)).toBe(5);
  });

  it("computes 1-based line and column", () => {
    expect(lineColumnAt("ab\ncd🙂e", 0)).toEqual({ line: 1, column: 1 });
    expect(lineColumnAt("ab\ncd🙂e", 4)).toEqual({ line: 2, column: 2 });
    expect(lineColumnAt("ab\ncd🙂e", 7)).toEqual({ line: 2, column: 4 });
  });
});
