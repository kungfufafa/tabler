/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { caseFailures, type SharedCases } from "@/features/drivers/portable/cases";
import { classifyDialect, loadDialectLex } from "@/features/drivers/portable/classify";
import { split } from "@/features/drivers/portable/scanner";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import lane from ".";
import { doubleQuoted, DUCKDB_SQL } from "./dialect";
import { LEX, switchesLogger } from "./logger";

// DuckDB SQL in the editor: quoting as `sqlgen::dialect::duckdb` quotes, the backend's splitter
// and classifier (its lexer profile and rules, exported to `bindings/lex/duckdb.json`) on the case
// file the backend's generated suite blessed, and the logger check of `switches_logger` on the
// backend's own cases.

const ROOT = path.resolve(import.meta.dirname, "../../../..");
const readJson = <T>(file: string): T =>
  JSON.parse(readFileSync(path.join(ROOT, file), "utf8")) as T;
const DUCKDB = readJson<DialectLex>("src/lib/ipc/bindings/lex/duckdb.json");
const ui = lane.dialects!.duckdb!;
const cls = (sql: string) => classifyDialect(DUCKDB, sql);

describe("the DuckDB dialect", () => {
  it("quotes names in double quotes with the quote doubled", () => {
    expect(ui.quoteIdent).toBe(doubleQuoted);
    expect(doubleQuoted("Order Items")).toBe('"Order Items"');
    expect(doubleQuoted('a"b')).toBe('"a""b"');
  });

  it("highlights as PostgreSQL with DuckDB's words, without a formatter", () => {
    expect(ui.label).toBe("DuckDB SQL");
    expect(ui.codemirror()).toBe(DUCKDB_SQL);
    // The caps grant no Format (`session::FEATURES`).
    expect(ui.formatter).toBeNull();
    expect(ui.columnTypes).toContain("HUGEINT");
    expect(ui.functions).toContain("list_transform");
  });

  it("offers only functions that read", () => {
    for (const f of ui.functions) expect(cls(`SELECT ${f}(1)`), f).toBe("read");
  });
});

describe("the backend's classifier in the renderer", () => {
  it("is the lexer profile the editor, Safe Mode and the logger check load", async () => {
    expect(await loadDialectLex("duckdb")).toEqual(DUCKDB);
    expect(LEX).toEqual(DUCKDB);
    expect(DUCKDB.lex.nestedBlockComments).toBe(true);
    expect(DUCKDB.lex.dollarQuotes).toBe(true);
  });

  it("agrees with every case of the blessed fixture (classify-generated/duckdb.json)", () => {
    const cases = readJson<SharedCases>("src-tauri/tests/fixtures/classify-generated/duckdb.json");
    expect(cases.classify.length).toBeGreaterThan(500);
    expect(caseFailures(DUCKDB, cases)).toEqual([]);
  });

  it("reads the lane's known hard cases as the brief says", () => {
    for (const sql of [
      "FROM t SELECT a",
      "SUMMARIZE t",
      "PIVOT_WIDER t ON c USING sum(x)",
      "SELECT * FROM (PIVOT_LONGER t ON COLUMNS(*))",
      "SELECT * FROM t PIVOT (sum(amount) FOR month IN ('jan', 'feb'))",
      "SELECT * EXCLUDE (a) FROM t",
      "SELECT * FROM range(10)",
      "UNPIVOT t ON a, b INTO NAME k VALUE v",
      // A replacement scan reads: hardened sessions' engine refuses the file.
      "SELECT * FROM 'x.csv'",
    ])
      expect(cls(sql), sql).toBe("read");
    for (const sql of [
      "SELECT * FROM read_csv('x.csv')",
      "SELECT * FROM glob('*')",
      "SELECT * FROM main.read_csv('x.csv')",
      "SELECT * FROM t PIVOT (my_agg(amount) FOR month IN ('jan'))",
      "SELECT * FROM main.sum(1)",
      "COPY t TO 'x.csv'",
      "ATTACH 'x.db'",
      "INSTALL httpfs",
      "LOAD httpfs",
      "EXPORT DATABASE 'x'",
      "SELECT nextval('s')",
      "PRAGMA enable_profiling",
      "SET enable_external_access = true",
      "SELECT * FROM query('SELECT 1')",
      "frobnicate",
    ])
      expect(cls(sql), sql).toBe("write");
  });

  it("splits at semicolons outside strings, dollar quotes and nested comments", () => {
    const text = "SELECT 1 /* x /* y */ ; z */;\nSELECT $$a;b$$;\nSELECT ';'";
    expect(split(DUCKDB.lex, text).map((s) => s.sql)).toEqual([
      "SELECT 1 /* x /* y */ ; z */",
      "SELECT $$a;b$$",
      "SELECT ';'",
    ]);
  });
});

describe("the logger check (`dialect::duckdb::switches_logger`)", () => {
  it("finds the switches in code and dynamic SQL, as the backend's cases do", () => {
    for (const sql of [
      "CALL enable_logging(storage := 'file', storage_path := 'C:/x')",
      "SELECT * FROM Enable_Logging()",
      'SELECT * FROM main."enable_logging"()',
      "SET logging_storage = 'file'",
      "SET GLOBAL enable_logging = true",
      "SELECT 1; CALL enable_logging()",
      "SELECT 1 /* /* */ CALL enable_logging() */",
      "SELECT * FROM query('SELECT * FROM enable_logging(storage := ''file'', storage_path := ''Z:/nope'')')",
      "SELECT * FROM query('SELECT * FROM enable_' || 'logging()')",
      "FROM Query_Table('enable_' || 'logging')",
      "SELECT * FROM main.\"query\" ('CALL enable_logging()')",
      "SELECT json_execute_serialized_sql(json_serialize_sql('SELECT * FROM enable_logging()'))",
      "SELECT * FROM query('SET logging_' || 'storage = ''file''')",
    ])
      expect(switchesLogger(sql), sql).toBe(true);
    for (const sql of [
      "SELECT 'enable_logging'",
      "SELECT 1 -- enable_logging",
      "SELECT * FROM duckdb_logs",
      "CALL disable_logging()",
      "SELECT enable_logging_x FROM t",
      "SELECT * FROM query('SELECT 42')",
      "SELECT query FROM t WHERE note = 'logging'",
    ])
      expect(switchesLogger(sql), sql).toBe(false);
  });

  it("keeps its word lists equal to the backend's", () => {
    const rs = readFileSync(path.join(ROOT, "src-tauri/src/sqlgen/dialect/duckdb.rs"), "utf8");
    expect(rs).toContain(
      'const LOGGER_SWITCHES: [&str; 2] = ["ENABLE_LOGGING", "LOGGING_STORAGE"];',
    );
    expect(rs).toContain(
      'const DYNAMIC_SQL: [&str; 3] = ["QUERY", "QUERY_TABLE", "JSON_EXECUTE_SERIALIZED_SQL"];',
    );
  });
});
