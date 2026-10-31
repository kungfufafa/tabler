/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import type { SharedCases } from "../portable/cases";
import { classifyConsole, sqlReads } from "./classify";
import { consoleRanges } from "./console";
import { parseConsole, toText } from "./json";
import { consoleText, parseCommand, parseRequest, requestTarget, split } from "./request";

// The console's splitter and classifier against the backend's (`drivers/elasticsearch/console.rs`
// and `classify.rs`), on the case file both run (`drivers::elasticsearch::tests::shared_console_cases`).

const ROOT = path.resolve(import.meta.dirname, "../../../..");
const CASES = JSON.parse(
  readFileSync(
    path.join(ROOT, "src-tauri/tests/fixtures/elasticsearch/console.cases.json"),
    "utf8",
  ),
) as SharedCases;

describe("the shared console cases", () => {
  it("classify every statement as the backend does", () => {
    const wrong = CASES.classify
      .filter((c) => classifyConsole(c.sql) !== c.class)
      .map((c) => `${JSON.stringify(c.sql)}: want ${c.class}`);
    expect(wrong).toEqual([]);
    expect(CASES.classify.length).toBeGreaterThan(250);
  });

  it("split every text as the backend does", () => {
    for (const c of CASES.split ?? [])
      expect(
        split(c.sql).map((s) => s.sql),
        JSON.stringify(c.sql),
      ).toEqual(c.statements);
  });
});

describe("splitting", () => {
  it("gives UTF-16 offsets and 1-based lines for the editor", () => {
    const text = '# é😀\nGET /é\n{"q": "😀"}\n\n  GET /b';
    const ranges = consoleRanges(text);
    expect(ranges.map((r) => text.slice(r.start, r.end))).toEqual([
      'GET /é\n{"q": "😀"}',
      "GET /b",
    ]);
    expect(ranges.map((r) => r.line)).toEqual([2, 5]);
  });
});

describe("requests", () => {
  it("parse, encode and run back the same", () => {
    const r = parseRequest('POST /a%20b/_update/1?if_seq_no=3&q=x+y\n{"doc":{"a":1.50}}');
    expect(r.segments).toEqual(["a b", "_update", "1"]);
    expect(r.query).toEqual([
      ["if_seq_no", "3"],
      ["q", "x y"],
    ]);
    expect(requestTarget(r)).toBe("/a%20b/_update/1?if_seq_no=3&q=x%20y");
    expect(consoleText(r)).toBe('POST /a%20b/_update/1?if_seq_no=3&q=x%20y\n{"doc":{"a":1.50}}');
    expect(parseRequest(consoleText(r))).toEqual(r);
    expect(requestTarget(parseRequest("GET /café/_search"))).toBe("/caf%C3%A9/_search");
    expect(parseRequest("POST _bulk\n{}\n{}").body?.kind).toBe("ndjson");
  });

  it("refuse paths that lead elsewhere, with the backend's reasons", () => {
    expect(() => parseRequest("GET /a/../_bulk")).toThrow(/`.` and `..`/);
    expect(() => parseRequest("GET http://x/_search")).toThrow(/path only/);
    expect(() => parseRequest("GET /a%zz")).toThrow(/malformed percent/);
    expect(() => parseRequest("GET /a%ff")).toThrow(/UTF-8/);
    expect(() => parseRequest("FETCH /a")).toThrow(/not a request method/);
    expect(() => parseRequest('POST /a/_search\n{\n  "q": x\n}')).toThrow(/line 3/);
  });

  it("read SQL statements without their final semicolon", () => {
    expect(parseCommand("# first\nSELECT 1 ;")).toEqual({ kind: "sql", sql: "SELECT 1" });
    expect(parseCommand("GET /").kind).toBe("request");
  });
});

describe("the console's JSON", () => {
  it("keeps numbers as written and takes comments and Kibana strings", () => {
    const [v] = parseConsole('{"a": 18446744073709551615, "b": 1.50, # c\n "c": """x "y" z"""}');
    expect(toText(v!)).toBe('{"a":18446744073709551615,"b":1.50,"c":"x \\"y\\" z"}');
    expect(toText(v!, 2)).toBe(
      '{\n  "a": 18446744073709551615,\n  "b": 1.50,\n  "c": "x \\"y\\" z"\n}',
    );
  });

  it("refuses nesting past the backend's bound", () => {
    expect(() => parseConsole("[".repeat(600) + "]".repeat(600))).toThrow(/nested/);
  });
});

describe("SQL", () => {
  it("reads only one plain SELECT, SHOW or DESCRIBE (as `sql_check_is_conservative`)", () => {
    expect(sqlReads("SELECT a FROM \"t\" WHERE x = 'it''s' -- DELETE\n")).toBe(true);
    expect(sqlReads("  show tables")).toBe(true);
    expect(sqlReads("SELECT 1;")).toBe(true);
    expect(sqlReads("SELECT 1; SELECT 2")).toBe(false);
    expect(sqlReads("WITH x AS (SELECT 1) SELECT * FROM x")).toBe(false);
    expect(sqlReads("")).toBe(false);
    expect(sqlReads("-- only a comment")).toBe(false);
    expect(sqlReads("SELECT `a FROM t")).toBe(false);
    expect(sqlReads("select * from t where x in (delete)")).toBe(false);
  });
});

describe("the classifier", () => {
  it("makes a read request a write with any write part added", () => {
    const reads = ["GET /logs/_search", "POST /logs/_count", "HEAD /_all/_mapping"];
    const tails = [
      '\n{"query":{"semantic":{}}}',
      '\n{"query":{"bool":{"must":[{"wrapper":{"query":"e30="}}]}}}',
      "?refresh=true",
      "?search_pipeline=p",
      "?keep_alive=1m",
      "/_flush",
      "/_doc/_x",
      "%2F_flush",
      "%3Frefresh=true",
      " extra",
      "/../_bulk",
    ];
    for (const base of reads) {
      expect(classifyConsole(base), base).toBe("read");
      for (const tail of tails) expect(classifyConsole(base + tail), base + tail).toBe("write");
    }
  });
});
