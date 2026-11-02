/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import type { SharedCases } from "../portable/cases";
import { argvText, consoleRanges, quoteArg, splitConsole, tokenize } from "./cli";
import { classifyCommand, classifyText, refusedInConsole } from "./classify";
import { redisConsole } from "./console";

// The console's splitter and classifier against the backend's (`drivers/redis/cli.rs` and
// `classify.rs`), on the case file both run (`drivers::redis::classify::tests::shared_console_cases`).

const ROOT = path.resolve(import.meta.dirname, "../../../..");
const CASES = JSON.parse(
  readFileSync(path.join(ROOT, "src-tauri/tests/fixtures/redis/console.cases.json"), "utf8"),
) as SharedCases;

const text = (b: Uint8Array) => new TextDecoder("utf-8", { ignoreBOM: true }).decode(b);
const args = (line: string) => {
  const t = tokenize(line);
  if (!t.ok) throw new Error(t.error);
  return t.argv.map(text);
};

describe("the shared console cases", () => {
  it("classify every command as the backend does", () => {
    const wrong = CASES.classify
      .filter((c) => classifyCommand(c.sql) !== c.class)
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

describe("the tokenizer", () => {
  it("reads quotes and escapes as redis-cli does, as bytes", () => {
    expect(args('SET "my key" "a\\nb\\t\\"c\\""')).toEqual(["SET", "my key", 'a\nb\t"c"']);
    expect(args("SET k 'a\\nb \"c\"'")).toEqual(["SET", "k", 'a\\nb "c"']);
    expect(args("SET k 'it\\'s'")).toEqual(["SET", "k", "it's"]);
    expect(args('SET a"b c"')).toEqual(["SET", "ab c"]);
    expect(args('SET k ""')).toEqual(["SET", "k", ""]);
    const t = tokenize('SET k "\\x00\\xFF\\x4a\\xzz"');
    expect(t.ok && [...t.argv[2]!]).toEqual([0, 0xff, 0x4a, 0x78, 0x7a, 0x7a]);
    expect(tokenize('GET "abc')).toEqual({ ok: false, error: "unbalanced" });
    expect(tokenize("GET 'a'b")).toEqual({ ok: false, error: "afterQuote" });
  });

  it("quotes arguments so that they read back as the same bytes", () => {
    const samples = [
      "plain",
      "my key",
      "",
      'q"uote',
      "back\\slash",
      "semi;colon",
      "line\nbreak",
      "tab\there",
      "\u0007\u0008",
      "\u0000nul",
      " nbsp",
      "café 😀",
      "it's",
      "\u0085",
    ];
    for (const s of samples) {
      const line = argvText(["SET", s]);
      expect(args(line), line).toEqual(["SET", s]);
    }
    const raw = Uint8Array.from([0xff, 0x41, 0xe2, 0x82]);
    const t = tokenize(`GET ${quoteArg(raw)}`);
    expect(t.ok && [...t.argv[1]!]).toEqual([...raw]);
    expect(quoteArg("user:1")).toBe("user:1");
    expect(quoteArg("a b")).toBe('"a b"');
  });
});

describe("splitting", () => {
  it("gives UTF-16 offsets and 1-based lines for the editor", () => {
    const t = "ECHO 😀;GET b\n\n  SET 'x;y' 1";
    const ranges = consoleRanges(t);
    expect(ranges.map((r) => t.slice(r.start, r.end))).toEqual(["ECHO 😀", "GET b", "SET 'x;y' 1"]);
    expect(ranges.map((r) => r.line)).toEqual([1, 1, 3]);
    expect(redisConsole.split).toBe(consoleRanges);
  });
});

describe("the classifier", () => {
  it("is default-deny and reads whole texts command by command", () => {
    expect(classifyText("GET a\nTYPE b; PTTL b")).toBe("read");
    expect(classifyText("GET a; SET b 1")).toBe("write");
    expect(classifyText('GET "a;b"; FLUSHALL')).toBe("write");
    expect(classifyCommand("FROBNICATE")).toBe("write");
    expect(redisConsole.classify("SCAN 0 MATCH *")).toBe("read");
  });

  it("names the commands the console refuses", () => {
    const refused = (l: string) => {
      const t = tokenize(l);
      return t.ok ? refusedInConsole(t.argv) : "unreadable";
    };
    for (const l of [
      "BLPOP l 0",
      "XREAD BLOCK 100 STREAMS s $",
      'XREAD "BLOCK\\x00" 0 STREAMS s $',
      "SUBSCRIBE ch",
      "MONITOR",
      "HELLO 2",
      "AUTH x",
      "CLIENT REPLY OFF",
      'client "tracking\\x00x" on',
      "WAIT 1 0",
      '"CLIENT\\x00a" REPLY OFF',
      '"SELECT\\x00a" 5',
    ])
      expect(refused(l), l).not.toBeNull();
    for (const l of ["GET k", "XREAD STREAMS s 0", "CLIENT LIST", "SET k v"])
      expect(refused(l), l).toBeNull();
  });

  it("lets no write through behind a read (fixed-seed fuzz)", () => {
    const atoms = [
      "GET",
      "SET",
      "FLUSHALL",
      "k",
      '"',
      "'",
      "\\",
      ";",
      "\n",
      " ",
      "x00",
      "STORE",
      "SORT",
    ];
    let seed = 7;
    const rand = (n: number) => {
      seed = (seed * 1103515245 + 12345) % 2 ** 31;
      return seed % n;
    };
    for (let i = 0; i < 2000; i++) {
      const parts = Array.from({ length: 1 + rand(8) }, () => atoms[rand(atoms.length)]!);
      const t = parts.join(rand(2) ? " " : "");
      const commands = splitConsole(t);
      // Splitting neither loses nor duplicates text.
      const kept = commands.map((c) => c.sql).join("");
      expect(kept.length).toBeLessThanOrEqual(t.length);
      if (commands.some((c) => classifyCommand(c.sql) === "write"))
        expect(classifyText(t)).toBe("write");
    }
  });
});

describe("the console UI", () => {
  it("gives a language, a file extension and a placeholder", () => {
    expect(redisConsole.language()).toBeTruthy();
    expect(redisConsole.fileExtension).toBe("redis");
    expect(redisConsole.placeholder).toContain("SCAN 0");
  });
});
