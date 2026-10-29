/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import type { LexProfile } from "@/lib/ipc/bindings/LexProfile";
import type { Reading } from "@/lib/ipc/bindings/Reading";
import {
  identName,
  isInvisible,
  listed,
  PLAIN,
  scan,
  split,
  text,
  type TokenKind,
} from "./scanner";

// The backend's own tests of `sqlgen/dialect/lex.rs` and `sqlgen/portable/split.rs`, on the
// renderer's port.

const LEX_DIR = path.resolve(import.meta.dirname, "../../../lib/ipc/bindings/lex");
const lexOf = (name: string) =>
  JSON.parse(readFileSync(path.join(LEX_DIR, `${name}.json`), "utf8")) as DialectLex;

const LEX: LexProfile = {
  identQuotes: [
    ['"', '"'],
    ["[", "]"],
  ],
  stringQuotes: ["'"],
  readings: [PLAIN],
  eStrings: true,
  dollarQuotes: true,
  dollarTags: true,
  tripleQuotes: false,
  rawPrefixes: false,
  bytePrefixes: false,
  hashComments: false,
  slashSlashComments: true,
  nestedBlockComments: true,
  dashCommentNeedsSpace: false,
  batchSeparator: null,
  terminatorOptional: false,
  beginEndBodies: false,
};

function kinds(lex: LexProfile, reading: Reading, sql: string): Array<[TokenKind, string]> {
  const s = scan(lex, reading, sql);
  return s.tokens.filter((t) => t.kind !== "space").map((t) => [t.kind, text(s, t)]);
}

const ambiguous = (lex: LexProfile, sql: string) => scan(lex, PLAIN, sql).ambiguous;

describe("the portable scanner", () => {
  it("scans quotes, comments and bodies", () => {
    expect(
      kinds(
        LEX,
        PLAIN,
        "SELECT [a]]b], \"x\"\"y\", 'it''s', E'a\\'b' -- c;\n/* /* n */ ; */ $f$;$f$ $1 // z",
      ),
    ).toEqual([
      ["word", "SELECT"],
      ["ident", "[a]]b]"],
      ["punct", ","],
      ["ident", '"x""y"'],
      ["punct", ","],
      ["str", "'it''s'"],
      ["punct", ","],
      ["str", "E'a\\'b'"],
      ["comment", "-- c;"],
      ["comment", "/* /* n */ ; */"],
      ["str", "$f$;$f$"],
      ["punct", "$"],
      ["word", "1"],
      ["comment", "// z"],
    ]);
    const s = scan(LEX, PLAIN, "[a]]b]");
    expect(identName(s, s.tokens[0]!)).toBe("a]b");
  });

  it("opens tagged bodies only with dollarTags", () => {
    const untagged = { ...LEX, dollarTags: false };
    // Snowflake: `$a$` is the session variable `a$`, and only `$$` opens a body.
    expect(kinds(untagged, PLAIN, "$a$, x, $a$ $$;$$")).toEqual([
      ["punct", "$"],
      ["word", "a$"],
      ["punct", ","],
      ["word", "x"],
      ["punct", ","],
      ["punct", "$"],
      ["word", "a$"],
      ["str", "$$;$$"],
    ]);
    expect(kinds(LEX, PLAIN, "$a$, x, $a$")).toEqual([["str", "$a$, x, $a$"]]);
  });

  it("reads backslashes as the reading says", () => {
    const lex: LexProfile = {
      ...LEX,
      rawPrefixes: true,
      tripleQuotes: true,
      stringQuotes: ["'", '"'],
    };
    const texts = (r: Reading, sql: string) => kinds(lex, r, sql).map((t) => t[1]);
    const escaping: Reading = { stringBackslash: true, identBackslash: false };
    expect(texts(PLAIN, "'a\\' ; DELETE'")).toEqual(["'a\\'", ";", "DELETE", "'"]);
    expect(texts(escaping, "'a\\' ; DELETE'")).toEqual(["'a\\' ; DELETE'"]);
    // A raw string takes no escapes, but a backslash keeps the character after it (GoogleSQL);
    // a triple-quoted one holds single quotes.
    for (const r of [PLAIN, escaping]) {
      expect(texts(r, "r'\\', ' ; DELETE FROM t; --'")).toEqual([
        "r'\\', '",
        ";",
        "DELETE",
        "FROM",
        "t",
        ";",
        "--'",
      ]);
      expect(texts(r, "rb'a\\\\' x")).toEqual(["rb'a\\\\'", "x"]);
      expect(texts(r, "r'''\\''' x''' y")).toEqual(["r'''\\''' x'''", "y"]);
    }
    expect(texts(escaping, "'''it's''' x")).toEqual(["'''it's'''", "x"]);
  });

  it("calls invisible characters in code ambiguous", () => {
    const invisible = ["​", "‌", "‍", "⁠", "⁡", "­", "᠎", "﻿", "͏", "️", "ㅤ", "\u{e0020}"];
    for (const c of invisible) {
      expect(isInvisible(c), c).toBe(true);
      for (const sql of [
        `SELECT SYSTEM$WAIT${c}(1)`,
        `SELECT SYS${c}TEM$WAIT(1)`,
        `SELECT 1${c}`,
        `${c}${c}SELECT 1`,
      ])
        expect(ambiguous(LEX, sql), JSON.stringify(sql)).toBe(true);
      // In strings, quoted names and comments they are text.
      const sql = `SELECT '${c}', "${c}" -- ${c}\n/* ${c} */ $$${c}$$`;
      expect(ambiguous(LEX, sql), JSON.stringify(sql)).toBe(false);
    }
    for (const c of [" ", " ", "　", "é", " "]) expect(isInvisible(c), c).toBe(false);
    // A byte-order mark that starts the text is a space.
    expect(ambiguous(LEX, "﻿SELECT 1")).toBe(false);
    expect(kinds(LEX, PLAIN, "﻿ SELECT")[0]).toEqual(["word", "SELECT"]);
  });

  it("calls heredocs another lexer may read ambiguous", () => {
    for (const sql of [
      // ClickHouse takes `$1$` (and older servers any text up to the next `$`) as a tag.
      "SELECT $1$'$1$, file('/etc/passwd') -- '",
      "SELECT $x'$ '$x'$, url('h') --'",
      "SELECT $ a $ ' $ a $, url('h') --'",
      "SELECT 1$a-b$ x $a-b$",
      "SELECT é$-$ x $-$",
      // A body that never closes: ClickHouse reads its tag as a word, PostgreSQL refuses.
      "SELECT 1 AS $a$, * FROM url('http://evil/x', CSV, 'a String')",
      "SELECT $$ x",
      "SELECT $a$ x $a",
    ])
      expect(ambiguous(LEX, sql), sql).toBe(true);
    for (const sql of [
      "SELECT $1, $2, $1 + $2",
      "SELECT $a$x$a$, $$y$$, a$b$, x$b$ ",
      "SELECT 1 AS a$, $id_var FROM t WHERE x = $id_var",
      "SELECT '$1$' || $1 || '$1$'",
    ])
      expect(ambiguous(LEX, sql), sql).toBe(false);
    const untagged = { ...LEX, dollarTags: false };
    expect(ambiguous(untagged, "SELECT $a$, $a$, $1$ x $1$")).toBe(false);
    expect(ambiguous(untagged, "SELECT $a$, $$ x")).toBe(true);
    const plain = { ...LEX, dollarQuotes: false, dollarTags: false };
    expect(ambiguous(plain, "SELECT $1$ x $1$, $$ y")).toBe(false);
  });

  it("calls comments a server may end elsewhere ambiguous", () => {
    const flat = { ...LEX, nestedBlockComments: false, hashComments: true };
    for (const lex of [LEX, flat]) {
      for (const sql of [
        "/* /* */ SELECT ' */ DELETE FROM t --'",
        "SELECT 1 /* a /*/ x",
        "SELECT 1 -- x\r' \nDELETE FROM t --'",
        "SELECT 1 // x\r'",
        "SELECT 1 -- x\u000cy\n",
        "SELECT 1 -- x\r\r\n",
      ])
        expect(ambiguous(lex, sql), JSON.stringify(sql)).toBe(true);
      for (const sql of [
        "/* a */ SELECT 1 /* b */",
        "/*/ a */ SELECT 1",
        "-- a\r\nSELECT 1 -- b\r\n",
        // In a string, neither counts.
        "SELECT '/* x\r' -- y",
        "SELECT 1 -- /* x\n",
      ])
        expect(ambiguous(lex, sql), JSON.stringify(sql)).toBe(false);
    }
    expect(ambiguous(flat, "SELECT 1 # x\r'")).toBe(true);
  });

  it("calls openers lexers class differently ambiguous", () => {
    const untagged = { ...LEX, dollarTags: false };
    for (const lex of [LEX, untagged]) {
      for (const sql of [
        "SELECT 1 AS a $$; DELETE FROM t; --$$",
        "SELECT 1 AS a $$; DELETE FROM t; --$$",
        "SELECT 1 AS a　$$; DELETE FROM t; --$$",
        "SELECT 1 AS a·$$; DELETE FROM t; --$$",
        "SELECT 1 AS é$$ x $$",
        "SELECT $ $ $$ $ $; DELETE FROM t; --$$",
        "SELECT $\u{1f600}$ $$ $\u{1f600}$; DELETE FROM t; --$$",
        "SELECT $a b$ $$ $a b$; DELETE FROM t; --$$",
        "SELECT $é$ x $é$",
        "SELECT 1$$ ; SELECT $$; DELETE FROM t; --$$",
        "SELECT $1$$ ; SELECT $$; DELETE FROM t; --$$",
        "SELECT 1.5$$ x $$",
        "SELECT 1e5$a$ x $a$",
        "SELECT x E'a\\' ; DELETE FROM t; --'",
        "SELECT 1E'a\\' ; DELETE FROM t; --'",
        "SELECT 1e'a'",
      ])
        expect(ambiguous(lex, sql), JSON.stringify(sql)).toBe(true);
      for (const sql of [
        "SELECT $$a$$, $q$b$q$, a$$, a$b, $1, $1 + $2, x$1$",
        "SELECT 1 AS a b, ' $$', $$ $$, $$é$$",
        "SELECT  $1, é$1, 1$, E'a', e'b', xE'c', 1 + E'd'",
        'SELECT " $$" --  $$\n',
      ])
        expect(ambiguous(lex, sql), JSON.stringify(sql)).toBe(false);
    }
    const plain = { ...LEX, dollarQuotes: false, dollarTags: false, eStrings: false };
    for (const sql of ["SELECT a $$ x $$", "SELECT 1$$ x $$", "SELECT x E'a'"])
      expect(ambiguous(plain, sql), JSON.stringify(sql)).toBe(false);
  });

  it("matches listed names exactly or by prefix", () => {
    const list = ["OPENQUERY", "XP_*"];
    expect(listed(list, "OPENQUERY") && listed(list, "XP_CMDSHELL")).toBe(true);
    expect(listed(list, "OPENQUERYX") || listed(list, "SP_WHO")).toBe(false);
  });
});

describe("the portable splitter", () => {
  const ansi = lexOf("ansi_test").lex;
  const tsql = lexOf("tsql_like_test").lex;

  it("gives each statement's text, UTF-16 offsets and line", () => {
    const got = split(ansi, "SELECT 'é';\n  -- c\n  SELECT 2;");
    expect(got).toEqual([
      { sql: "SELECT 'é'", start: 0, end: 10, line: 1 },
      { sql: "SELECT 2", start: 21, end: 29, line: 3 },
    ]);
    // An astral character is one character to the scan and two code units to the editor.
    const astral = split(ansi, "SELECT '\u{1f600}'; SELECT 2");
    expect(astral.map((s) => [s.start, s.end])).toEqual([
      [0, 11],
      [13, 21],
    ]);
    expect(
      split(ansi, "SELECT 1;\n\n  SELECT 2; SELECT 3;\n-- c\nSELECT 4").map((s) => s.line),
    ).toEqual([1, 3, 3, 5]);
  });

  it("splits a long script in linear time", () => {
    const script = "SELECT 1;\n".repeat(40_000);
    const started = performance.now();
    const parts = split(ansi, script);
    expect(performance.now() - started).toBeLessThan(5_000);
    expect(parts).toHaveLength(40_000);
    expect([parts[0]!.line, parts[39_999]!.line]).toEqual([1, 40_000]);
  });

  it("splits T-SQL at GO lines only, keeping BEGIN … END bodies", () => {
    expect(split(tsql, "SELECT 1; SELECT 2\nGO 2\nSELECT 3").map((s) => s.sql)).toEqual([
      "SELECT 1; SELECT 2",
      "SELECT 3",
    ]);
    expect(
      split(ansi, "CREATE PROCEDURE p AS BEGIN SELECT 1; SELECT 2; END; SELECT 3").map(
        (s) => s.sql,
      ),
    ).toEqual(["CREATE PROCEDURE p AS BEGIN SELECT 1; SELECT 2; END", "SELECT 3"]);
    expect(split(ansi, "BEGIN; SELECT 1; COMMIT").map((s) => s.sql)).toEqual([
      "BEGIN",
      "SELECT 1",
      "COMMIT",
    ]);
  });
});
