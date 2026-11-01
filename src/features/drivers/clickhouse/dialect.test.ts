/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { caseFailures, type SharedCases } from "@/features/drivers/portable/cases";
import { classifyDialect, loadDialectLex } from "@/features/drivers/portable/classify";
import { split } from "@/features/drivers/portable/scanner";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import lane from ".";
import { backticked, CLICKHOUSE_SQL } from "./dialect";

// ClickHouse SQL in the editor: quoting as `spec::backticked_backslash` quotes, and the backend's
// splitter and classifier (its lexer profile and rules, exported to `bindings/lex/clickhouse.json`)
// on the case file the backend's generated suite blessed.

const ROOT = path.resolve(import.meta.dirname, "../../../..");
const readJson = <T>(file: string): T =>
  JSON.parse(readFileSync(path.join(ROOT, file), "utf8")) as T;
const LEX = readJson<DialectLex>("src/lib/ipc/bindings/lex/clickhouse.json");
const ui = lane.dialects!.clickhouse!;

describe("the ClickHouse dialect", () => {
  it("quotes names in backquotes with backslash escapes", () => {
    expect(ui.quoteIdent).toBe(backticked);
    expect(backticked("order details")).toBe("`order details`");
    expect(backticked("a`b\\")).toBe("`a\\`b\\\\`");
    expect(backticked("Events")).toBe("`Events`");
  });

  it("highlights ClickHouse's words, has no formatter and offers its names", () => {
    expect(ui.label).toBe("ClickHouse SQL");
    expect(ui.codemirror()).toBe(CLICKHOUSE_SQL);
    const spec = CLICKHOUSE_SQL.spec;
    expect(spec.identifierQuotes).toBe('`"');
    expect(spec).toMatchObject({ backslashEscapes: true, hashComments: true, slashComments: true });
    expect(spec.keywords).toContain("prewhere");
    expect(spec.types?.split(" ")).toContain("LowCardinality");
    // The caps do not grant Format.
    expect(ui.formatter).toBeNull();
    expect(ui.functions).toContain("uniqExact");
    expect(ui.columnTypes).toContain("DateTime64(3, 'UTC')");
  });
});

describe("the backend's classifier in the renderer", () => {
  it("is the lexer profile the editor and Safe Mode load", async () => {
    expect(await loadDialectLex("clickhouse")).toEqual(LEX);
    expect(LEX.lex).toMatchObject({ hashComments: true, slashSlashComments: true });
    expect(LEX.classify.tableFunctions).toContain("NUMBERS");
  });

  it("agrees with every case of the blessed fixture (classify-generated/clickhouse.json)", () => {
    const cases = readJson<SharedCases>(
      "src-tauri/tests/fixtures/classify-generated/clickhouse.json",
    );
    expect(cases.classify.length).toBeGreaterThan(100);
    expect(caseFailures(LEX, cases)).toEqual([]);
  });

  it("reads the lane's known hard cases as the brief says", () => {
    const cls = (sql: string) => classifyDialect(LEX, sql);
    expect(cls("SELECT * FROM system.tables")).toBe("read");
    expect(cls("SELECT * FROM numbers(10)")).toBe("read");
    expect(cls("SHOW CREATE TABLE t")).toBe("read");
    expect(cls("FROM t |> WHERE a > 1")).toBe("read");
    expect(cls("SELECT ‘a b’ AS s, “my col” FROM “my db”.“my table”")).toBe("read");
    // Template formats read files only with settings that are writes themselves.
    expect(cls("SELECT 1 FORMAT Template")).toBe("read");
    // Writes: table functions outside the allow-list, SETTINGS, APPLY, EXPLAIN ANALYZE.
    expect(cls("SELECT * FROM url('http://x', CSV)")).toBe("write");
    expect(cls("SELECT * FROM t SETTINGS readonly = 0")).toBe("write");
    expect(cls("SELECT * APPLY(file) FROM t")).toBe("write");
    expect(cls("EXPLAIN ANALYZE SELECT 1")).toBe("write");
    expect(cls("SELECT * FROM t INTO OUTFILE 'x.csv'")).toBe("write");
    expect(cls("SELECT 1 PARALLEL WITH INSERT INTO t VALUES (1)")).toBe("write");
    expect(cls("KILL QUERY WHERE query_id = 'x'")).toBe("write");
    expect(cls("ALTER TABLE t DELETE WHERE 1")).toBe("write");
    // A Unicode-quoted callee matches the allow-list on its exact text only: a known false write.
    expect(cls("SELECT * FROM “numbers”(3)")).toBe("write");
    expect(cls("frobnicate")).toBe("write");
  });

  it("splits at `;` outside strings, names, heredocs and every comment form", () => {
    const text = [
      "SELECT 'a;b', `c;d` FROM t; # x; y",
      "SELECT $$e;f$$ // g; h",
      "; /* i; /* nested; */ j; */ SELECT 2",
    ].join("\n");
    expect(split(LEX.lex, text).map((s) => text.slice(s.start, s.end))).toEqual([
      "SELECT 'a;b', `c;d` FROM t",
      // A statement starts at its first code: the comments before it are skipped.
      "SELECT $$e;f$$ // g; h",
      "SELECT 2",
    ]);
  });
});
