/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { format } from "sql-formatter";
import { describe, expect, it } from "vitest";
import { caseFailures, type SharedCases } from "@/features/drivers/portable/cases";
import { classifyDialect, loadDialectLex } from "@/features/drivers/portable/classify";
import { split } from "@/features/drivers/portable/scanner";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import lane from ".";
import { doubleQuoted, REDSHIFT_SQL } from "./dialect";

// Redshift SQL in the editor: quoting as `sqlgen::dialect::redshift` quotes, and the backend's
// splitter and classifier (its lexer profile, `bindings/lex/redshift.json`) on the case file the
// backend's generated suite blessed.

const ROOT = path.resolve(import.meta.dirname, "../../../..");
const readJson = <T>(file: string): T =>
  JSON.parse(readFileSync(path.join(ROOT, file), "utf8")) as T;
const REDSHIFT = readJson<DialectLex>("src/lib/ipc/bindings/lex/redshift.json");
const ui = lane.dialects!.redshift!;

describe("the Redshift SQL dialect", () => {
  it('quotes names in double quotes with " doubled', () => {
    expect(ui.quoteIdent).toBe(doubleQuoted);
    expect(doubleQuoted("Order Details")).toBe('"Order Details"');
    expect(doubleQuoted('a"b')).toBe('"a""b"');
  });

  it("highlights as PostgreSQL with backslash escapes, formats as redshift", () => {
    expect(ui.label).toBe("Redshift SQL");
    expect(ui.codemirror()).toBe(REDSHIFT_SQL);
    expect(REDSHIFT_SQL.spec.backslashEscapes).toBe(true);
    expect(REDSHIFT_SQL.spec.keywords).toContain("diststyle");
    expect(ui.formatter).toBe("redshift");
    expect(format("select listagg(a, ',') from t", { language: "redshift" })).toContain("listagg");
    expect(ui.functions).toContain("LISTAGG");
    expect(ui.columnTypes).toContain("super");
  });
});

describe("the backend's classifier in the renderer", () => {
  it("is the lexer profile the editor and Safe Mode load", async () => {
    expect(await loadDialectLex("redshift")).toEqual(REDSHIFT);
    expect(REDSHIFT.lex.attributeCalls).toBe(true);
  });

  it("agrees with every case of the blessed fixture (classify-generated/redshift.json)", () => {
    const cases = readJson<SharedCases>(
      "src-tauri/tests/fixtures/classify-generated/redshift.json",
    );
    expect(cases.classify.length).toBeGreaterThan(100);
    expect(caseFailures(REDSHIFT, cases)).toEqual([]);
  });

  it("reads the lane's known hard cases as the brief says", () => {
    const cls = (sql: string) => classifyDialect(REDSHIFT, sql);
    expect(cls("SELECT TOP 10 * FROM sales")).toBe("read");
    expect(cls("UNLOAD ('SELECT * FROM t') TO 's3://b/p' IAM_ROLE 'r'")).toBe("write");
    expect(cls("COPY t FROM 's3://b/p' IAM_ROLE 'r'")).toBe("write");
    expect(cls("CALL sp_load(1)")).toBe("write");
    expect(cls("SELECT pg_terminate_backend(42)")).toBe("write");
    expect(cls('SELECT (1)."PG_TERMINATE_BACKEND"')).toBe("write");
    expect(cls("SELECT * INTO #tmp FROM t")).toBe("write");
    expect(cls("SELECT $$DELETE FROM t$$")).toBe("read");
    expect(cls("frobnicate")).toBe("write");
  });

  it("splits at semicolons outside strings and dollar bodies", () => {
    const text = "SELECT 'a;b'; SELECT $$x;y$$;\nSELECT 3";
    expect(split(REDSHIFT.lex, text).map((s) => text.slice(s.start, s.end))).toEqual([
      "SELECT 'a;b'",
      "SELECT $$x;y$$",
      "SELECT 3",
    ]);
  });
});
