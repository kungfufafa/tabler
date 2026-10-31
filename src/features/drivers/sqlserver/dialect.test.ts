/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { MSSQL } from "@codemirror/lang-sql";
import { format } from "sql-formatter";
import { describe, expect, it } from "vitest";
import { caseFailures, type SharedCases } from "@/features/drivers/portable/cases";
import { classifyDialect, loadDialectLex } from "@/features/drivers/portable/classify";
import { split } from "@/features/drivers/portable/scanner";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import lane from ".";
import { bracketed } from "./dialect";

// T-SQL in the editor: quoting as `sqlgen::dialect::tsql` quotes, and the backend's splitter and
// classifier (its lexer profile and rules, exported to `bindings/lex/tsql.json`) on the case file
// the backend's generated suite blessed.

const ROOT = path.resolve(import.meta.dirname, "../../../..");
const readJson = <T>(file: string): T =>
  JSON.parse(readFileSync(path.join(ROOT, file), "utf8")) as T;
const TSQL = readJson<DialectLex>("src/lib/ipc/bindings/lex/tsql.json");
const ui = lane.dialects!.tsql!;

describe("the T-SQL dialect", () => {
  it("quotes names in brackets with ] doubled", () => {
    expect(ui.quoteIdent).toBe(bracketed);
    expect(bracketed("Order Details")).toBe("[Order Details]");
    expect(bracketed("a]b")).toBe("[a]]b]");
    expect(bracketed("[x]")).toBe("[[x]]]");
  });

  it("highlights as MSSQL, has no formatter and offers T-SQL's names", () => {
    expect(ui.label).toBe("T-SQL");
    expect(ui.codemirror()).toBe(MSSQL);
    expect(ui.formatter).toBeNull();
    expect(ui.functions).toContain("COUNT_BIG");
    expect(ui.columnTypes).toContain("datetimeoffset");
    // Why: sql-formatter's transactsql moves batch separators.
    expect(format("SELECT 1\nGO 3\nSELECT go FROM t", { language: "transactsql" })).toBe(
      "SELECT\n  1\nGO 3\nSELECT\ngo\nFROM\n  t",
    );
  });
});

describe("the backend's classifier in the renderer", () => {
  it("is the lexer profile the editor and Safe Mode load", async () => {
    expect(await loadDialectLex("tsql")).toEqual(TSQL);
    expect(TSQL.lex.terminatorOptional).toBe(true);
  });

  it("agrees with every case of the blessed fixture (classify-generated/tsql.json)", () => {
    const cases = readJson<SharedCases>("src-tauri/tests/fixtures/classify-generated/tsql.json");
    expect(cases.classify.length).toBeGreaterThan(100);
    expect(caseFailures(TSQL, cases)).toEqual([]);
  });

  it("reads the lane's known hard cases as the brief says", () => {
    const cls = (sql: string) => classifyDialect(TSQL, sql);
    expect(cls("SELECT * FROM sys.tables")).toBe("read");
    expect(cls("SELECT 1 DELETE FROM t")).toBe("write");
    expect(cls("SELECT * INTO t2 FROM t")).toBe("write");
    expect(cls("EXEC('DELETE FROM t')")).toBe("write");
    expect(cls("SELECT * FROM OPENQUERY(srv, 'DELETE FROM t')")).toBe("write");
    expect(cls("SELECT * FROM t WITH (UPDLOCK)")).toBe("write");
    expect(cls("SELECT 'DELETE' AS [GO]")).toBe("read");
    expect(cls("frobnicate")).toBe("write");
  });

  it("splits at GO lines only, as the editor runs batches", () => {
    const text = "SELECT 1\nSELECT 2\nGO\nSELECT 'GO'\n-- GO\nGO 2\nCREATE VIEW v AS SELECT 1";
    expect(split(TSQL.lex, text).map((s) => text.slice(s.start, s.end))).toEqual([
      "SELECT 1\nSELECT 2",
      "SELECT 'GO'\n-- GO",
      "CREATE VIEW v AS SELECT 1",
    ]);
  });
});
