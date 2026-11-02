/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { caseFailures, type SharedCases } from "@/features/drivers/portable/cases";
import { classifyDialect, loadDialectLex } from "@/features/drivers/portable/classify";
import { split } from "@/features/drivers/portable/scanner";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import lane from ".";
import { classifySnowflake, quotesNextval } from "./classify";
import { doubleQuoted, SNOWFLAKE_SQL, snowflakeSqlUi } from "./dialect";

// Snowflake SQL in the editor: quoting as `sqlgen::dialect::snowflake` quotes, Format through
// sql-formatter, and the backend's splitter and classifier (its lexer profile and rules,
// exported to `bindings/lex/snowflake.json`, and `quotes_nextval`) on the case file the
// backend's generated suite blessed.

const ROOT = path.resolve(import.meta.dirname, "../../../..");
const readJson = <T>(file: string): T =>
  JSON.parse(readFileSync(path.join(ROOT, file), "utf8")) as T;
const SNOWFLAKE = readJson<DialectLex>("src/lib/ipc/bindings/lex/snowflake.json");
const ui = lane.dialects!.snowflake!;
const cls = classifySnowflake;

describe("the Snowflake dialect", () => {
  it("quotes names in double quotes with the quote doubled, in their stored case", () => {
    expect(ui.quoteIdent).toBe(doubleQuoted);
    expect(doubleQuoted("ORDERS")).toBe('"ORDERS"');
    expect(doubleQuoted("my table")).toBe('"my table"');
    expect(doubleQuoted('a"b')).toBe('"a""b"');
  });

  it("highlights Snowflake SQL and formats with sql-formatter's snowflake language", async () => {
    expect(ui.label).toBe("Snowflake SQL");
    expect(ui.codemirror()).toBe(SNOWFLAKE_SQL);
    // The session's caps grant Format (`session::FEATURES`).
    expect(ui.formatter).toBe("snowflake");
    const { format } = await import("sql-formatter");
    expect(
      format("select a,b from t where s = 'it\\'s' and x = $$a;b$$", { language: "snowflake" }),
    ).toContain("'it\\'s'");
    expect(ui.columnTypes).toContain("TIMESTAMP_TZ");
    expect(ui.functions).toContain("PARSE_JSON");
  });

  it("offers only functions that read", () => {
    for (const f of ui.functions) expect(cls(`SELECT ${f}(1)`), f).toBe("read");
    for (const f of ["SYSTEM$CANCEL_QUERY", "IDENTIFIER", "TRUNCATE", "INSERT", "COMPLETE"])
      expect(ui.functions, f).not.toContain(f);
  });
});

describe("the backend's classifier in the renderer", () => {
  it("is the lexer profile the editor and Safe Mode load", async () => {
    expect(await loadDialectLex("snowflake")).toEqual(SNOWFLAKE);
    expect(SNOWFLAKE.lex.slashSlashComments).toBe(true);
    expect(SNOWFLAKE.lex.dollarQuotes).toBe(true);
    expect(SNOWFLAKE.lex.dollarTags).toBe(false);
    expect(SNOWFLAKE.lex.beginEndBodies).toBe(true);
  });

  it("agrees with every case of the blessed fixture (classify-generated/snowflake.json)", () => {
    const cases = readJson<SharedCases>(
      "src-tauri/tests/fixtures/classify-generated/snowflake.json",
    );
    expect(cases.classify.length).toBeGreaterThan(500);
    expect(caseFailures(SNOWFLAKE, cases)).toEqual([]);
    // Safe Mode's classifier (the rules and `quotes_nextval`) agrees too: no case quotes NEXTVAL.
    const failures = cases.classify.filter((c) => cls(c.sql) !== c.class).map((c) => c.sql);
    expect(failures).toEqual([]);
  });

  it("reads the lane's known hard cases as the brief says", () => {
    for (const read of [
      "SELECT * FROM PUBLIC.ORDERS",
      "SHOW TABLES IN SCHEMA DB.PUBLIC",
      "DESCRIBE TABLE ORDERS",
      "SELECT $$DELETE FROM t$$",
      "SELECT 1 // DELETE FROM t",
      "SELECT 'a\\'; DELETE FROM t'",
      "SELECT * FROM TABLE(FLATTEN(input => PARSE_JSON('[1]')))",
      "SELECT TRANSLATE(s, 'abc', 'xyz') FROM t",
    ])
      expect(cls(read), read).toBe("read");
    for (const write of [
      "SELECT SYSTEM$CANCEL_ALL_QUERIES(1)",
      "SELECT snowflake.SYSTEM$CANCEL_QUERY ('x')",
      "EXECUTE IMMEDIATE 'DELETE FROM t'",
      "EXECUTE IMMEDIATE $$DELETE FROM t$$",
      "CALL archive_orders('2026-01-01')",
      "SELECT a INTO :v FROM t",
      "SHOW TABLES ->> DELETE FROM t",
      "SELECT * FROM @my_stage",
      "LIST @my_stage",
      "USE DATABASE other",
      "BEGIN",
      "SELECT seq.NEXTVAL",
      "SELECT * FROM TABLE(my_udtf())",
      "frobnicate",
    ])
      expect(cls(write), write).toBe("write");
  });

  it("reads a quoted NEXTVAL as a write, as `classify_snowflake` does", () => {
    expect(ui).toBe(snowflakeSqlUi);
    expect(ui.classify).toBe(classifySnowflake);
    for (const sql of ['SELECT seq."NEXTVAL"', 'SELECT s."nextval" FROM t', 'SELECT "NextVal"'])
      expect([quotesNextval(sql), cls(sql)], sql).toEqual([true, "write"]);
    // The portable rules alone read it: the dialect's own check is what makes it a write.
    expect(classifyDialect(SNOWFLAKE, 'SELECT seq."NEXTVAL"')).toBe("read");
    for (const sql of ["SELECT 'NEXTVAL'", "SELECT $$NEXTVAL$$", 'SELECT "NEXT_VAL"'])
      expect(quotesNextval(sql), sql).toBe(false);
  });

  it("splits at semicolons outside strings, $$ bodies and Snowflake Scripting blocks", () => {
    const text =
      "SELECT 1 // a; comment\n;\nSELECT $$a;b$$;\nSELECT 'x\\';y';\nEXECUTE IMMEDIATE $$BEGIN SELECT 1; END$$;";
    expect(split(SNOWFLAKE.lex, text).map((s) => s.sql)).toEqual([
      "SELECT 1 // a; comment",
      "SELECT $$a;b$$",
      "SELECT 'x\\';y'",
      "EXECUTE IMMEDIATE $$BEGIN SELECT 1; END$$",
    ]);
  });
});
