/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { caseFailures, type SharedCases } from "@/features/drivers/portable/cases";
import { classifyDialect, loadDialectLex } from "@/features/drivers/portable/classify";
import { split } from "@/features/drivers/portable/scanner";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import lane from ".";
import { doubleQuoted, VERTICA_SQL } from "./dialect";

// Vertica SQL in the editor: quoting as `sqlgen::dialect::vertica` quotes, and the backend's
// splitter and classifier (its lexer profile and rules, exported to `bindings/lex/vertica.json`)
// on the case file the backend's generated suite blessed.

const ROOT = path.resolve(import.meta.dirname, "../../../..");
const readJson = <T>(file: string): T =>
  JSON.parse(readFileSync(path.join(ROOT, file), "utf8")) as T;
const VERTICA = readJson<DialectLex>("src/lib/ipc/bindings/lex/vertica.json");
const ui = lane.dialects!.vertica!;
const cls = (sql: string) => classifyDialect(VERTICA, sql);

describe("the Vertica dialect", () => {
  it("quotes names in double quotes with the quote doubled", () => {
    expect(ui.quoteIdent).toBe(doubleQuoted);
    expect(doubleQuoted("Store Sales")).toBe('"Store Sales"');
    expect(doubleQuoted('a"b')).toBe('"a""b"');
  });

  it("highlights as PostgreSQL with Vertica's words, without a formatter", () => {
    expect(ui.label).toBe("Vertica SQL");
    expect(ui.codemirror()).toBe(VERTICA_SQL);
    // The caps grant no Format (`session::FEATURES`).
    expect(ui.formatter).toBeNull();
    expect(ui.columnTypes).toContain("timestamptz");
    expect(ui.functions).toContain("APPROXIMATE_COUNT_DISTINCT");
  });

  it("offers only functions that read", () => {
    for (const f of ui.functions) expect(cls(`SELECT ${f}(1)`), f).toBe("read");
  });
});

describe("the backend's classifier in the renderer", () => {
  it("is the lexer profile the editor and Safe Mode load", async () => {
    expect(await loadDialectLex("vertica")).toEqual(VERTICA);
    expect(VERTICA.lex.nestedBlockComments).toBe(true);
    expect(VERTICA.lex.dollarQuotes).toBe(true);
  });

  it("agrees with every case of the blessed fixture (classify-generated/vertica.json)", () => {
    const cases = readJson<SharedCases>("src-tauri/tests/fixtures/classify-generated/vertica.json");
    expect(cases.classify.length).toBeGreaterThan(500);
    expect(caseFailures(VERTICA, cases)).toEqual([]);
  });

  it("reads the lane's known hard cases as the brief says", () => {
    expect(cls("SELECT * FROM v_catalog.tables")).toBe("read");
    expect(cls("AT EPOCH LATEST SELECT * FROM t")).toBe("read");
    expect(cls("SELECT $$DELETE FROM t$$")).toBe("read");
    expect(cls("SELECT $x$ DROP TABLE t $x$")).toBe("read");
    expect(cls("SELECT CLOSE_SESSION('s')")).toBe("write");
    expect(cls("SELECT ANALYZE_STATISTICS('t')")).toBe("write");
    expect(cls("SELECT KafkaListTopics(USING PARAMETERS brokers='b:9092')")).toBe("write");
    expect(cls("COPY t FROM LOCAL '/tmp/x.csv'")).toBe("write");
    expect(cls("EXPORT TO PARQUET(directory='/tmp/x') AS SELECT * FROM t")).toBe("write");
    expect(cls("SELECT * INTO t2 FROM t")).toBe("write");
    expect(cls("SELECT 1 /* a /* b */ ; DELETE FROM t */")).toBe("write");
    expect(cls("frobnicate")).toBe("write");
  });

  it("splits at semicolons outside strings, dollar quotes and nested comments", () => {
    const text = "SELECT 1 /* x /* y */ ; z */;\nSELECT $$a;b$$;\nSELECT ';'";
    expect(split(VERTICA.lex, text).map((s) => s.sql)).toEqual([
      "SELECT 1 /* x /* y */ ; z */",
      "SELECT $$a;b$$",
      "SELECT ';'",
    ]);
  });
});
