/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import type { SharedCases } from "../portable/cases";
import { classifyShell, parseLiteral, shellRanges, splitShell } from "./shell";

// The console's splitter and classifier against the backend's (`drivers/mongodb/shell.rs` and
// `classify.rs`), on the case file both run (`drivers::mongodb::tests::shared_console_cases`).

const ROOT = path.resolve(import.meta.dirname, "../../../..");
const CASES = JSON.parse(
  readFileSync(path.join(ROOT, "src-tauri/tests/fixtures/mongodb/console.cases.json"), "utf8"),
) as SharedCases;

describe("the shared console cases", () => {
  it("classify every text as the backend does", () => {
    const wrong = CASES.classify
      .filter((c) => classifyShell(c.sql) !== c.class)
      .map((c) => `${JSON.stringify(c.sql)}: want ${c.class}`);
    expect(wrong).toEqual([]);
    expect(CASES.classify.length).toBeGreaterThan(150);
  });

  it("split every text as the backend does", () => {
    for (const c of CASES.split ?? [])
      expect(
        splitShell(c.sql).map((s) => s.sql),
        JSON.stringify(c.sql),
      ).toEqual(c.statements);
  });
});

describe("splitting", () => {
  it("gives UTF-16 offsets and 1-based lines for the editor", () => {
    const text = "// é\nuse é; db.ä.find({x: '😀'})\n\n  db.b.find()";
    const ranges = shellRanges(text);
    expect(ranges.map((r) => text.slice(r.start, r.end))).toEqual([
      "use é",
      "db.ä.find({x: '😀'})",
      "db.b.find()",
    ]);
    expect(ranges.map((r) => r.line)).toEqual([2, 2, 4]);
  });
});

describe("the classifier", () => {
  it("lets no write through behind any read (fixed-seed fuzz, as the backend's)", () => {
    const atoms = [
      "db",
      ".",
      "c",
      "find",
      "(",
      ")",
      "{",
      "}",
      "[",
      "]",
      "a",
      ":",
      "1",
      ",",
      "'",
      '"',
      "`",
      "/",
      "*",
      ";",
      "\n",
      " ",
      "$out",
      "$merge",
      "insertOne",
      "aggregate",
      "drop",
      "use",
      "show",
      "dbs",
      "ObjectId('65f1a2b3c4d5e6f708192a3b')",
      "//",
      "/*",
      "*/",
      "\\",
      "é",
      "runCommand",
      "explain",
      "getSiblingDB",
      "x",
      "-",
      "2",
      "e",
      "NaN",
      "new",
      "Date",
    ];
    let seed = 0x6d6f6e676fn;
    const next = () => {
      seed = (seed * 6364136223846793005n + 1442695040888963407n) & 0xffffffffffffffffn;
      return Number(seed >> 33n);
    };
    for (let round = 0; round < 3000; round++) {
      const len = next() % 24;
      let text = "";
      for (let k = 0; k < len; k++) text += atoms[next() % atoms.length];
      for (const p of splitShell(text)) expect(text.slice(p.start, p.end)).toBe(p.sql);
      expect(classifyShell(`${text}\n*/\n;db.c.insertOne({})`), JSON.stringify(text)).toBe("write");
    }
  });

  it("reads keys as the server does: escapes decoded, duplicates in place", () => {
    expect(classifyShell("db.c.aggregate([{'\\x24merge': 'x'}])")).toBe("write");
    expect(classifyShell("db.c.aggregate([{'\\u{24}out': 'x'}])")).toBe("write");
    expect(classifyShell("db.runCommand({find: 'c', find: 'd'})")).toBe("read");
    expect(classifyShell("db.runCommand({insert: 'c', find: 'c'})")).toBe("write");
  });
});

describe("literals", () => {
  it("read the shell's values", () => {
    expect(
      parseLiteral("{a: 1, 'b': [true, null], c: ObjectId('65f1a2b3c4d5e6f708192a3b')}"),
    ).toEqual({
      k: "doc",
      entries: [
        ["a", { k: "num", v: 1, int: true }],
        ["b", { k: "arr", items: [{ k: "bool", v: true }, { k: "null" }] }],
        [
          "c",
          {
            k: "other",
            name: "ObjectId",
            inner: [{ k: "str", v: "65f1a2b3c4d5e6f708192a3b" }],
          },
        ],
      ],
    });
    expect(parseLiteral("-2.5e3")).toEqual({ k: "num", v: -2500, int: false });
    expect(parseLiteral("0x1F")).toEqual({ k: "num", v: 31, int: true });
    expect(() => parseLiteral("{a: x}")).toThrow("unknown name");
    expect(() => parseLiteral("1_000")).toThrow("invalid number");
  });
});
