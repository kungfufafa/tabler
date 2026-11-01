/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { caseFailures, type SharedCases } from "@/features/drivers/portable/cases";
import { classifyDialect, loadDialectLex } from "@/features/drivers/portable/classify";
import { split } from "@/features/drivers/portable/scanner";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import lane from ".";
import { CQL_SQL, cqlStatements, quoted, splitCql } from "./dialect";

// CQL in the editor: quoting as `sqlgen::dialect::cql` quotes, the backend's classifier (its
// lexer profile and rules, exported to `bindings/lex/cql.json`) on the case file the backend's
// generated suite blessed, and `split_cql`'s batches, on the backend's own cases.

const ROOT = path.resolve(import.meta.dirname, "../../../..");
const readJson = <T>(file: string): T =>
  JSON.parse(readFileSync(path.join(ROOT, file), "utf8")) as T;
const CQL = readJson<DialectLex>("src/lib/ipc/bindings/lex/cql.json");
const ui = lane.dialects!.cql!;
const texts = (text: string) => splitCql(text).map((s) => s.sql);

describe("the CQL dialect", () => {
  it('double-quotes every name with " doubled', () => {
    expect(ui.quoteIdent).toBe(quoted);
    expect(quoted("users")).toBe('"users"');
    expect(quoted("My Ks")).toBe('"My Ks"');
    expect(quoted('a"b')).toBe('"a""b"');
  });

  it("highlights with CQL's words, has no formatter and offers CQL's names", () => {
    expect(ui.label).toBe("CQL");
    expect(ui.codemirror()).toBe(CQL_SQL);
    expect(CQL_SQL.spec.keywords).toContain("keyspace");
    expect(CQL_SQL.spec.keywords).toContain("filtering");
    // Built on CodeMirror's Cassandra dialect, so the plain words highlight too.
    expect(CQL_SQL.spec.keywords).toContain("select");
    expect(CQL_SQL.spec.keywords).toContain("materialized");
    expect(CQL_SQL.spec.doubleDollarQuotedStrings).toBe(true);
    expect(CQL_SQL.spec.slashComments).toBe(true);
    expect(CQL_SQL.spec.backslashEscapes).toBe(false);
    expect(ui.formatter).toBeNull();
    expect(ui.functions).toEqual(expect.arrayContaining(["toTimestamp", "writetime", "token"]));
    expect(ui.columnTypes).toEqual(expect.arrayContaining(["timeuuid", "varint", "duration"]));
  });
});

describe("the backend's classifier in the renderer", () => {
  it("is the lexer profile the editor and Safe Mode load", async () => {
    expect(await loadDialectLex("cql")).toEqual(CQL);
    expect(CQL.lex.dollarQuotes).toBe(true);
    expect(CQL.lex.slashSlashComments).toBe(true);
  });

  it("agrees with every case of the blessed fixture (classify-generated/cql.json)", () => {
    const cases = readJson<SharedCases>("src-tauri/tests/fixtures/classify-generated/cql.json");
    expect(cases.classify.length).toBeGreaterThan(100);
    expect(caseFailures(CQL, cases)).toEqual([]);
  });

  it("reads the lane's known hard cases as the brief says", () => {
    const cls = (sql: string) => classifyDialect(CQL, sql);
    expect(cls("SELECT * FROM ks.t WHERE k = 1 ALLOW FILTERING")).toBe("read");
    expect(cls("SELECT JSON * FROM t")).toBe("read");
    expect(cls("DESCRIBE TABLE t")).toBe("read");
    expect(cls("LIST ROLES")).toBe("read");
    expect(cls("CONSISTENCY QUORUM")).toBe("read");
    expect(cls("BEGIN BATCH INSERT INTO t (k) VALUES (1); APPLY BATCH")).toBe("write");
    expect(cls("INSERT INTO t (k) VALUES (1) IF NOT EXISTS")).toBe("write");
    expect(cls("UPDATE t USING TTL 60 SET v = 1 WHERE k = 1")).toBe("write");
    expect(cls("DELETE FROM t WHERE k = 1 IF EXISTS")).toBe("write");
    expect(cls("SELECT $$DELETE$$ FROM t")).toBe("read");
    expect(cls("frobnicate")).toBe("write");
  });
});

describe("the editor's statements (split_cql)", () => {
  it("keep a batch whole, its strings' ; included, at the backend's lines and offsets", () => {
    const script =
      "SELECT 1 FROM t;\nBEGIN BATCH\n  INSERT INTO t (k) VALUES (1);\n  UPDATE t SET v = ';' WHERE k = 2;\nAPPLY BATCH;\nSELECT 2 FROM t;";
    expect(texts(script)).toEqual([
      "SELECT 1 FROM t",
      "BEGIN BATCH\n  INSERT INTO t (k) VALUES (1);\n  UPDATE t SET v = ';' WHERE k = 2;\nAPPLY BATCH",
      "SELECT 2 FROM t",
    ]);
    const parts = splitCql(script);
    expect(parts.map((p) => p.line)).toEqual([1, 2, 6]);
    for (const p of parts) expect(script.slice(p.start, p.end)).toBe(p.sql);
    expect(ui).toHaveProperty("split", cqlStatements);
    expect(cqlStatements(script)[1]).toEqual({
      start: parts[1]!.start,
      end: parts[1]!.end,
      line: 2,
    });
    // The portable splitter alone cuts the batch at its `;` (why the lane offers `split`).
    expect(split(CQL.lex, script)).toHaveLength(5);
  });

  it("know counter and unlogged batches, one closed in its own statement, one never closed", () => {
    expect(
      texts("begin counter batch update c set n = n + 1 where k = 1; apply batch; SELECT 1"),
    ).toEqual(["begin counter batch update c set n = n + 1 where k = 1; apply batch", "SELECT 1"]);
    expect(texts("BEGIN BATCH DELETE FROM t WHERE k = 1 APPLY BATCH; SELECT 1")).toEqual([
      "BEGIN BATCH DELETE FROM t WHERE k = 1 APPLY BATCH",
      "SELECT 1",
    ]);
    expect(texts("BEGIN UNLOGGED BATCH INSERT INTO t (k) VALUES (1); SELECT 1")).toEqual([
      "BEGIN UNLOGGED BATCH INSERT INTO t (k) VALUES (1); SELECT 1",
    ]);
    expect(
      texts(
        "BEGIN BATCH INSERT INTO t (k) VALUES (1); DELETE FROM t WHERE k = 2 APPLY BATCH; SELECT 1",
      ),
    ).toEqual([
      "BEGIN BATCH INSERT INTO t (k) VALUES (1); DELETE FROM t WHERE k = 2 APPLY BATCH",
      "SELECT 1",
    ]);
    // Words in strings, names and comments open no batch.
    expect(texts("SELECT 'BEGIN BATCH' FROM t; SELECT 2")).toEqual([
      "SELECT 'BEGIN BATCH' FROM t",
      "SELECT 2",
    ]);
  });

  it("count UTF-16 offsets past characters beyond the BMP, as the editor does", () => {
    const text = "SELECT '😀' FROM t; BEGIN BATCH DELETE FROM t WHERE k = 1; APPLY BATCH";
    const [a, b] = splitCql(text);
    expect(text.slice(a!.start, a!.end)).toBe("SELECT '😀' FROM t");
    expect(text.slice(b!.start, b!.end)).toBe("BEGIN BATCH DELETE FROM t WHERE k = 1; APPLY BATCH");
  });
});
