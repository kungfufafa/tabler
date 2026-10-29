/// <reference types="node" />
import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import type { ClassifyRules } from "@/lib/ipc/bindings/ClassifyRules";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import type { LexProfile } from "@/lib/ipc/bindings/LexProfile";
import { caseFailures, type SharedCases } from "./cases";
import {
  callsListedFunction,
  classifyWith,
  codeOnly,
  isSingleStatement,
  loadDialectLex,
} from "./classify";

const ROOT = path.resolve(import.meta.dirname, "../../../..");
const LEX_DIR = path.join(ROOT, "src/lib/ipc/bindings/lex");
const PORTABLE_CASES = path.join(ROOT, "src-tauri/tests/fixtures/portable");
const GENERATED = path.join(ROOT, "src-tauri/tests/fixtures/classify-generated");

const readJson = <T>(file: string): T => JSON.parse(readFileSync(file, "utf8")) as T;
const lexOf = (name: string) => readJson<DialectLex>(path.join(LEX_DIR, `${name}.json`));

const ANSI = lexOf("ansi_test");
const TSQL_LIKE = lexOf("tsql_like_test");

describe("the shared cases of the foundation's test dialects", () => {
  // `src-tauri/src/sqlgen/portable/testing.rs` runs the same files on the backend.
  it.each([
    ["ANSI", "ansi_test", ANSI],
    ["T-SQL-like", "tsql_like_test", TSQL_LIKE],
  ] as const)("%s (%s.cases.json)", (_, name, dialect) => {
    const cases = readJson<SharedCases>(path.join(PORTABLE_CASES, `${name}.cases.json`));
    expect(cases.classify.length).toBeGreaterThan(50);
    expect(caseFailures(dialect, cases)).toEqual([]);
  });

  it("cover GO batches and the optional terminator", () => {
    const cases = readJson<SharedCases>(path.join(PORTABLE_CASES, "tsql_like_test.cases.json"));
    expect(cases.split?.some((c) => /\nGO\b/i.test(c.sql))).toBe(true);
    expect(cases.classify).toContainEqual({ sql: "SELECT 1 DELETE FROM t", class: "write" });
  });
});

/**
 * Every dialect's generated fixture (`sqlgen/portable/gen`), whose lexer profile and rules the
 * backend exported: the renderer's classifier and splitter agree with the backend's on each case.
 */
const GENERATED_FIXTURES = readdirSync(GENERATED)
  .filter((f) => f.endsWith(".json"))
  .map((f) => f.slice(0, -".json".length))
  .filter((name) => existsSync(path.join(LEX_DIR, `${name}.json`)))
  .sort();

describe("the generated fixtures", () => {
  it("include the foundation's test dialects", () => {
    expect(GENERATED_FIXTURES).toEqual(expect.arrayContaining(["ansi_test", "tsql_like_test"]));
  });

  it.each(GENERATED_FIXTURES)("%s", (name) => {
    const fixture = readJson<SharedCases>(path.join(GENERATED, `${name}.json`));
    expect(fixture.classify.length).toBeGreaterThan(100);
    expect(caseFailures(lexOf(name), fixture)).toEqual([]);
  });
});

/** Rules with qualified, method and keyword entries, as Snowflake and BigQuery list them. */
const RULES: ClassifyRules = {
  ...ANSI.classify,
  writes: ["INTO", "DELETE", "COMMIT", "@"],
  writeCalls: ["SYSTEM$*", "AI.*", "KEYS.KEYSET_CHAIN"],
  tableFunctions: [
    "TABLE",
    "VALUES",
    "FLATTEN",
    "SUM",
    "INFORMATION_SCHEMA.QUERY_HISTORY",
    "SNOWFLAKE.CORE.GET_LINEAGE",
    "!SPCS_GET_LOGS",
  ],
};

function check(reads: string[], writes: string[], lex: LexProfile = ANSI.lex, rules = RULES) {
  for (const sql of reads) expect(classifyWith(lex, rules, sql), sql).toBe("read");
  for (const sql of writes) expect(classifyWith(lex, rules, sql), sql).toBe("write");
}

// The backend's own tests of `sqlgen/portable/classify.rs`, on the renderer's port.
describe("the portable classifier", () => {
  it("checks a qualified name where a table goes by its last part", () => {
    check(
      [
        "SELECT * FROM TABLE(FLATTEN(input => x))",
        "SELECT * FROM t, LATERAL FLATTEN(t.v) f",
        "SELECT * FROM t PIVOT (SUM(a) FOR m IN ('JAN'))",
        "SELECT * FROM db..t, db..u",
        "SELECT * FROM TABLE(svc!SPCS_GET_LOGS())",
        "SELECT * FROM TABLE(db.s.svc!SPCS_GET_LOGS())",
      ],
      [
        "SELECT * FROM TABLE(db..my_proc())",
        "SELECT * FROM db..f(1)",
        "SELECT * FROM t, LATERAL db..f(t.a)",
        "SELECT * FROM t JOIN TABLE(db..p()) x ON true",
        "SELECT * FROM TABLE(db . . p())",
        "SELECT * FROM TABLE(db./* x */.p())",
        'SELECT * FROM TABLE("db".."p"())',
        "SELECT * FROM t1, db..p()",
        "SELECT * FROM (db..p())",
        "SELECT * FROM db...p()",
        "SELECT * FROM db..FLATTEN(1)",
        "SELECT * FROM TABLE(m!PREDICT(1))",
        "SELECT * FROM TABLE(db.s.m!PREDICT(1))",
        "SELECT * FROM TABLE(m ! PREDICT(1))",
        'SELECT * FROM TABLE("m"!"predict"(1))',
        "SELECT * FROM TABLE(s!FLATTEN(1))",
      ],
    );
  });

  it("matches allow-list entries by their qualification", () => {
    check(
      [
        "SELECT * FROM TABLE(INFORMATION_SCHEMA.QUERY_HISTORY())",
        "SELECT * FROM TABLE(db.information_schema.query_history())",
        "SELECT * FROM TABLE(SNOWFLAKE.CORE.GET_LINEAGE('x'))",
        'SELECT * FROM TABLE("INFORMATION_SCHEMA"."QUERY_HISTORY"())',
        'SELECT * FROM TABLE("FLATTEN"(1))',
        "SELECT * FROM VALUES (1), (2)",
      ],
      [
        "SELECT * FROM TABLE(QUERY_HISTORY())",
        "SELECT * FROM TABLE(myschema.QUERY_HISTORY())",
        "SELECT * FROM TABLE(db..QUERY_HISTORY())",
        "SELECT * FROM TABLE(core.GET_LINEAGE('x'))",
        "SELECT * FROM TABLE(GET_LINEAGE('x'))",
        "SELECT * FROM TABLE(s.FLATTEN(input => x))",
        "SELECT * FROM t PIVOT (s.SUM(a) FOR m IN ('JAN'))",
        "SELECT * FROM TABLE(SPCS_GET_LOGS())",
        "SELECT * FROM TABLE(s.SPCS_GET_LOGS())",
        'SELECT * FROM TABLE("flatten"(1))',
        'SELECT * FROM TABLE("ﬂatten"(1))',
        'SELECT * FROM TABLE(INFORMATION_SCHEMA."query_history"())',
        'SELECT * FROM TABLE("INFORMATION_SCHEMA.QUERY_HISTORY"())',
        'SELECT * FROM TABLE("VALUES"())',
        'SELECT * FROM TABLE("TABLE"())',
        'SELECT * FROM "VALUES"(1)',
        'SELECT * FROM TABLE(U&"FLATTEN"(1))',
      ],
    );
  });

  it("matches qualified deny-list entries with the last parts of a callee", () => {
    check(
      [
        "SELECT IF(a, 1, 2), TRANSLATE(s, 'a', 'b'), x.IF(1) FROM t",
        'SELECT KEYSET_CHAIN(1), "keys" FROM t',
      ],
      [
        "SELECT AI.IF(1) FROM t",
        'SELECT "ai".if(1) FROM t',
        'SELECT "safe"."ai"."if"(1) FROM t',
        'SELECT "safe.ai".if(1) FROM t',
        'SELECT "safe.ai.if"(1) FROM t',
        'SELECT "ai" . translate (1) FROM t',
        'SELECT x."keys".keyset_chain(1) FROM t',
        'SELECT "x.system$wait"(1)',
        "SELECT 1AI.IF(1)",
        'SELECT U&"x".if(1)',
      ],
    );
  });

  it("lets a quoted name holding a backslash spell any name where names take escapes", () => {
    const lex: LexProfile = {
      ...ANSI.lex,
      identQuotes: [["`", "`"]],
      readings: [{ stringBackslash: true, identBackslash: true }],
    };
    check(
      ["SELECT `a\\`b` FROM `t\\\\`", "SELECT * FROM TABLE(`FLATTEN`(1))"],
      [
        "SELECT `syst\\u0065m$wait`(1)",
        "SELECT `\\x61i`.if(1)",
        "SELECT * FROM TABLE(`flat\\x74en`(1))",
      ],
      lex,
    );
    // Where quoted names take no escapes, a backslash is a character.
    check(["SELECT `a\\b`(1)"], [], { ...ANSI.lex, identQuotes: [["`", "`"]] });
  });

  it("writes on a listed punctuation character wherever it is code", () => {
    check(
      ["SELECT 'a@b', \"@\" FROM t -- @"],
      [
        "SELECT $1 FROM @stage/file.csv",
        "SELECT $1 FROM @s//x; DELETE FROM t",
        "SELECT $1 FROM @s/a'b ->> DELETE FROM t --'",
        "SELECT 1@2",
      ],
    );
    expect(classifyWith(ANSI.lex, ANSI.classify, "SELECT @x")).toBe("read");
  });

  it("judges each statement the flow operator chains", () => {
    check(
      ["SELECT 1 ->> SELECT * FROM $1", "SELECT j->>'a', j->>0, j ->> 'b' FROM t"],
      [
        "SELECT 1 ->> START TRANSACTION",
        "SELECT 1 ->> GET x",
        "SELECT 1 ->> SELECT 2 ->> COMMIT",
        "SELECT j->>col FROM t",
      ],
    );
    expect(isSingleStatement(ANSI.lex, RULES, "SELECT j->>'a' FROM t")).toBe(true);
    expect(isSingleStatement(ANSI.lex, RULES, "SELECT 1 ->> SELECT 2")).toBe(false);
  });

  it("calls a name in attribute notation where the profile says so (attributeCalls)", () => {
    const rules: ClassifyRules = {
      ...ANSI.classify,
      writeCalls: ["PG_TERMINATE_BACKEND", "NEXTVAL", "AI.IF"],
    };
    const attributes: LexProfile = { ...ANSI.lex, attributeCalls: true };
    const sideEffect = (lex: LexProfile, sql: string) =>
      callsListedFunction(lex, ["PG_TERMINATE_BACKEND", "NEXTVAL"], sql);
    for (const sql of [
      "SELECT (1).pg_terminate_backend",
      'SELECT (1)."pg_terminate_backend"',
      'SELECT (1)."PG_TERMINATE_BACKEND"',
      "SELECT ('s').nextval",
      "SELECT ('s'). /* c */ \"nextval\"",
      "SELECT (SELECT pid FROM s).pg_terminate_backend",
      "SELECT (a[1]).pg_terminate_backend FROM t",
      "SELECT a[1].pg_terminate_backend FROM t",
      "SELECT (1).abs.pg_terminate_backend",
      'SELECT (1).U&"x"',
      "SELECT * FROM t WHERE (pid).pg_terminate_backend",
    ]) {
      expect(classifyWith(attributes, rules, sql), sql).toBe("write");
      expect(sideEffect(attributes, sql), sql).toBe(true);
      // Without the flag only a call before `(` counts.
      expect(classifyWith(ANSI.lex, rules, sql), sql).toBe("read");
      expect(sideEffect(ANSI.lex, sql), sql).toBe(false);
    }
    for (const sql of [
      'SELECT t.pg_terminate_backend, s."nextval" FROM t, s',
      "SELECT (doc).items, (1).abs FROM t",
      "SELECT '(1).nextval', \"(1).nextval\" -- (1).nextval",
    ]) {
      expect(classifyWith(attributes, rules, sql), sql).toBe("read");
      expect(sideEffect(attributes, sql), sql).toBe(false);
    }
  });

  it("ends a FROM list at unreserved LIMIT and EXCEPT only before their syntax", () => {
    check(
      [
        "SELECT * FROM t LIMIT 10",
        "SELECT * FROM t LIMIT ? OFFSET ?",
        "SELECT a FROM t1 LIMIT 1, lower(b)",
        "SELECT a FROM t1 LIMIT (5), lower(b)",
        "SELECT a FROM t1 EXCEPT SELECT b, lower(c) FROM t2",
        "SELECT a FROM t1 EXCEPT (SELECT b, lower(c) FROM t2)",
        "SELECT a FROM t1 EXCEPT ALL SELECT b, lower(c) FROM t2",
      ],
      [
        "SELECT * FROM t1 limit (a, b), my_udtf(t1.a)",
        "SELECT * FROM t1 limit SAMPLE (10), LATERAL my_udtf(t1.a)",
        "SELECT * FROM t1 except (a, b), my_udtf(t1.a)",
        "SELECT * FROM t2, t1 except (a, b), my_udtf(t1.a)",
        "SELECT * FROM (SELECT 1) limit (a), my_udtf(1)",
      ],
    );
  });

  it("reads code only in its helpers", () => {
    expect(codeOnly(TSQL_LIKE.lex, "SELECT [x] -- c\nFROM t WHERE a = N'DELETE'")).toBe(
      "SELECT    \nFROM t WHERE a = N ",
    );
    expect(callsListedFunction(ANSI.lex, ["SLEEP"], "SELECT sleep (1)")).toBe(true);
    expect(callsListedFunction(ANSI.lex, ["SLEEP"], "SELECT 'sleep(1)'")).toBe(false);
    expect(callsListedFunction(TSQL_LIKE.lex, ["NEXT VALUE FOR"], "SELECT NEXT VALUE FOR s")).toBe(
      true,
    );
  });

  it("calls T-SQL-like text without terminators one statement only when it is one", () => {
    const one = (sql: string) => isSingleStatement(TSQL_LIKE.lex, TSQL_LIKE.classify, sql);
    expect(one("SELECT 1")).toBe(true);
    expect(one("WITH a AS (SELECT 1) SELECT * FROM a")).toBe(true);
    expect(one("SELECT 1 UNION SELECT 2")).toBe(true);
    expect(one("SELECT 1 SELECT 2")).toBe(false);
    expect(one("SELECT 1 DELETE FROM t")).toBe(false);
    expect(one("SELECT 1\nGO\nSELECT 2")).toBe(false);
    expect(one("")).toBe(true);
  });
});

describe("loadDialectLex", () => {
  it("loads a dialect's exported profile and rules lazily", async () => {
    expect(await loadDialectLex("ansi_test")).toEqual(ANSI);
    expect(await loadDialectLex("tsql_like_test")).toEqual(TSQL_LIKE);
    // The core dialects keep their own lexers.
    expect(await loadDialectLex("postgres")).toBeNull();
  });
});
