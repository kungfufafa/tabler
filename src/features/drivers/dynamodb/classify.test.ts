/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import type { SharedCases } from "../portable/cases";
import {
  apiClass,
  classifyConsole,
  consoleRanges,
  OTHER_ACTIONS,
  parseCommand,
  parseJson,
  partiqlClass,
  READ_ACTIONS,
  splitConsole,
} from "./classify";

// The console's splitter and classifier against the backend's (`sqlgen/native/dynamo.rs`), on
// the case file both run (`sqlgen::native::dynamo::tests::shared_console_cases`).

const ROOT = path.resolve(import.meta.dirname, "../../../..");
const CASES = JSON.parse(
  readFileSync(path.join(ROOT, "src-tauri/tests/fixtures/dynamodb/console.cases.json"), "utf8"),
) as SharedCases;

describe("the shared console cases", () => {
  it("classify every text as the backend does", () => {
    const wrong = CASES.classify
      .filter((c) => classifyConsole(c.sql) !== c.class)
      .map((c) => `${JSON.stringify(c.sql)}: want ${c.class}`);
    expect(wrong).toEqual([]);
    expect(CASES.classify.length).toBeGreaterThan(200);
  });

  it("split every text as the backend does", () => {
    for (const c of CASES.split ?? [])
      expect(
        splitConsole(c.sql).map((s) => s.sql),
        JSON.stringify(c.sql),
      ).toEqual(c.statements);
  });
});

describe("the actions", () => {
  it("are the reference's 58, each read or not", () => {
    expect(READ_ACTIONS.length + OTHER_ACTIONS.length).toBe(58);
    for (const a of READ_ACTIONS) expect(apiClass(a, { k: "obj", v: [] }), a).toBe("read");
    for (const a of OTHER_ACTIONS) expect(apiClass(a, { k: "obj", v: [] }), a).toBe("write");
    for (const a of ["scan", "SCAN", "FlushTable", "", "GetRecords"])
      expect(apiClass(a, { k: "obj", v: [] }), a).toBe("write");
  });
});

describe("splitting", () => {
  it("gives UTF-16 offsets and 1-based lines for the editor", () => {
    const text = '-- é\nSELECT \'é😀\' FROM t; Scan {"TableName": "😀"};\n\n  ListTables';
    const ranges = consoleRanges(text);
    expect(ranges.map((r) => text.slice(r.start, r.end))).toEqual([
      "SELECT 'é😀' FROM t",
      'Scan {"TableName": "😀"}',
      "ListTables",
    ]);
    expect(ranges.map((r) => r.line)).toEqual([2, 2, 4]);
    // The backend's offsets count code points.
    expect(splitConsole(text).map((s) => [s.start, s.end])).toEqual([
      [5, 23],
      [25, 48],
      [53, 63],
    ]);
  });

  it("counts lines from the last command, linearly", () => {
    const many = "SELECT * FROM t;\n".repeat(20_000);
    const started = performance.now();
    const parts = splitConsole(many);
    expect(parts).toHaveLength(20_000);
    expect(parts[19_999]!.line).toBe(20_000);
    expect(classifyConsole("--a\r".repeat(20_000))).toBe("write");
    expect(performance.now() - started).toBeLessThan(10_000);
  });
});

describe("commands", () => {
  it("read back as they are sent", () => {
    expect(parseCommand('Scan {"TableName": "t"};')).toMatchObject({
      kind: "api",
      action: "Scan",
      text: '{"TableName": "t"}',
    });
    expect(parseCommand("DescribeLimits")).toMatchObject({ kind: "api", text: "{}" });
    expect(parseCommand("-- x\nSELECT * FROM t;")).toEqual({
      kind: "partiql",
      statement: "SELECT * FROM t",
    });
    expect(parseCommand("Scan [1]")).toEqual({ kind: "partiql", statement: "Scan [1]" });
    expect(() => parseCommand('Scan {"a": 1, "a": 2}')).toThrow(/repeated/);
    expect(() => parseCommand("   ")).toThrow(/empty/);
    expect(() => parseCommand(`SELECT '${"x".repeat(8192)}' FROM t`)).toThrow(/8192/);
  });

  it("read JSON as the backend does: numbers by their text, nesting bounded", () => {
    expect(parseJson('{"n": 12345678901234567890.5}')).toEqual({
      k: "obj",
      v: [["n", { k: "num", v: "12345678901234567890.5" }]],
    });
    expect(parseJson('"\\ud83d\\ude00"')).toEqual({ k: "str", v: "😀" });
    for (const bad of ["1e400", '"\\udc00"', "[1,]", "01", "{} x", "[".repeat(66) + "]".repeat(66)])
      expect(() => parseJson(bad), bad).toThrow();
    expect(() => parseJson("[".repeat(65) + "]".repeat(65))).not.toThrow();
  });
});

describe("the classifier", () => {
  it("reads PartiQL only as one clear SELECT", () => {
    expect(partiqlClass("select * from t")).toBe("read");
    expect(partiqlClass("SELECT * FROM t -- x\r DELETE FROM t")).toBe("write");
    expect(partiqlClass("ſelect * from t")).toBe("write");
    expect(partiqlClass("SELECT * FROM t WHERE a = `1`")).toBe("write");
  });

  it("lets no write through behind a read (fixed-seed fuzz, as the backend's)", () => {
    const atoms = [
      "SELECT",
      "select",
      "DELETE",
      "UPDATE",
      " ",
      "\n",
      "\r",
      " ",
      "`",
      ";",
      "'",
      '"',
      "--",
      "/*",
      "*/",
      "{",
      "}",
      "\\",
      ":",
      "x",
      "é",
      "Scan",
      "PutItem",
      "ExecuteStatement",
      '"Statement"',
      "*",
      "FROM t",
    ];
    let seed = 0x5eed;
    const next = () => {
      seed = (seed * 1103515245 + 12345) % 2147483648;
      return seed;
    };
    let reads = 0;
    for (let n = 0; n < 4000; n++) {
      const len = (next() % 12) + 1;
      let text = "";
      for (let i = 0; i < len; i++) text += atoms[next() % atoms.length];
      const parts = splitConsole(text);
      const chars = Array.from(text);
      for (const p of parts) {
        expect(p.sql).toBe(chars.slice(p.start, p.end).join(""));
        expect(p.sql.trim()).not.toBe("");
      }
      if (classifyConsole(text) !== "read") continue;
      reads += 1;
      for (const p of parts) {
        const cmd = parseCommand(p.sql);
        if (cmd.kind === "partiql")
          expect(cmd.statement.trimStart().slice(0, 6).toUpperCase(), text).toBe("SELECT");
        else expect(apiClass(cmd.action, cmd.body), text).toBe("read");
      }
    }
    expect(reads).toBeGreaterThan(50);
  });
});
