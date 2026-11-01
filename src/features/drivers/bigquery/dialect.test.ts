/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { caseFailures, type SharedCases } from "@/features/drivers/portable/cases";
import { classifyDialect, loadDialectLex } from "@/features/drivers/portable/classify";
import { split } from "@/features/drivers/portable/scanner";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import lane from ".";
import { backticked, BIGQUERY_SQL } from "./dialect";

// GoogleSQL in the editor: quoting as `spec::backticked_backslash` quotes, and the backend's
// splitter and classifier (its lexer profile and rules, exported to `bindings/lex/bigquery.json`)
// on the case file the backend's generated suite blessed.

const ROOT = path.resolve(import.meta.dirname, "../../../..");
const readJson = <T>(file: string): T =>
  JSON.parse(readFileSync(path.join(ROOT, file), "utf8")) as T;
const LEX = readJson<DialectLex>("src/lib/ipc/bindings/lex/bigquery.json");
const ui = lane.dialects!.bigquery!;

describe("the GoogleSQL dialect", () => {
  it("quotes names in backticks with backslash escapes", () => {
    expect(ui.quoteIdent).toBe(backticked);
    expect(backticked("order details")).toBe("`order details`");
    expect(backticked("a`b\\")).toBe("`a\\`b\\\\`");
    expect(backticked("demo-proj")).toBe("`demo-proj`");
  });

  it("highlights GoogleSQL's words, has no formatter and offers its names", () => {
    expect(ui.label).toBe("GoogleSQL");
    expect(ui.codemirror()).toBe(BIGQUERY_SQL);
    const spec = BIGQUERY_SQL.spec;
    expect(spec.identifierQuotes).toBe("`");
    expect(spec).toMatchObject({
      backslashEscapes: true,
      hashComments: true,
      slashComments: false,
      doubleQuotedStrings: true,
    });
    expect(spec.keywords).toContain("qualify");
    expect(spec.types?.split(" ")).toContain("int64");
    // The caps do not grant Format.
    expect(ui.formatter).toBeNull();
    expect(ui.functions).toContain("SAFE_DIVIDE");
    expect(ui.columnTypes).toContain("ARRAY<STRING>");
  });
});

describe("the backend's classifier in the renderer", () => {
  it("is the lexer profile the editor and Safe Mode load", async () => {
    expect(await loadDialectLex("bigquery")).toEqual(LEX);
    expect(LEX.lex).toMatchObject({
      hashComments: true,
      tripleQuotes: true,
      rawPrefixes: true,
      beginEndBodies: true,
    });
  });

  it("agrees with every case of the blessed fixture (classify-generated/bigquery.json)", () => {
    const cases = readJson<SharedCases>(
      "src-tauri/tests/fixtures/classify-generated/bigquery.json",
    );
    expect(cases.classify.length).toBeGreaterThan(100);
    expect(caseFailures(LEX, cases)).toEqual([]);
  });

  it("reads the lane's known hard cases as the brief says", () => {
    const cls = (sql: string) => classifyDialect(LEX, sql);
    expect(cls("SELECT * FROM `demo-proj.shop.orders` WHERE id = 1")).toBe("read");
    expect(cls("WITH t AS (SELECT 1 AS x) SELECT x FROM t")).toBe("read");
    expect(cls("FROM shop.orders |> WHERE total > 10")).toBe("read");
    expect(cls("SELECT r'\\'', '''a ; DELETE''', b'x' FROM t")).toBe("read");
    expect(cls("SELECT SAFE_DIVIDE(a, b), ST_ASTEXT(geo) FROM t")).toBe("read");
    // Writes: another statement, another service, scripting.
    expect(cls("EXECUTE IMMEDIATE 'DELETE FROM t WHERE true'")).toBe("write");
    expect(cls("CALL shop.archive_orders('2024-01-01')")).toBe("write");
    expect(cls("SELECT * FROM EXTERNAL_QUERY('conn', 'SELECT 1')")).toBe("write");
    expect(cls("SELECT * FROM ML.PREDICT(MODEL m, TABLE t)")).toBe("write");
    expect(cls("SELECT `safe.st_regionstats`(g, 'x')")).toBe("write");
    expect(cls("DECLARE x INT64 DEFAULT 1")).toBe("write");
    expect(cls("BEGIN SELECT 1; END")).toBe("write");
    expect(cls("BEGIN TRANSACTION")).toBe("write");
    expect(cls("EXPORT DATA OPTIONS(uri='gs://b/*.csv', format='CSV') AS SELECT 1")).toBe("write");
    expect(cls("LOAD DATA INTO t FROM FILES(format='CSV', uris=['gs://b/x.csv'])")).toBe("write");
    expect(cls("SELECT r'\\', ' ; DELETE FROM t; --'")).toBe("write");
    expect(cls("frobnicate")).toBe("write");
  });

  it("splits at `;` outside strings and comments, keeping a scripting block whole", () => {
    const text = [
      "SELECT 'a;b', `c;d` FROM t; # x; y",
      "SELECT '''e;f''' -- g; h",
      ";",
      "BEGIN",
      "  SELECT 1;",
      "  SELECT 2;",
      "END;",
      "/* i; */ SELECT 3",
    ].join("\n");
    expect(split(LEX.lex, text).map((s) => text.slice(s.start, s.end))).toEqual([
      "SELECT 'a;b', `c;d` FROM t",
      "SELECT '''e;f''' -- g; h",
      "BEGIN\n  SELECT 1;\n  SELECT 2;\nEND",
      "SELECT 3",
    ]);
  });
});
