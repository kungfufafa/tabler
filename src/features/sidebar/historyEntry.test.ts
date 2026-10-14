import { describe, expect, it } from "vitest";
import { retentionCutoff } from "@/features/locations/historyRetention";
import { MAX_REGEX_LENGTH } from "./groups";
import { entriesSql, historyMatcher, NO_SEARCH_OPTIONS, yearsAgo } from "./historyEntry";

describe("history search", () => {
  const sql = ["SELECT * FROM users", "select * from users_archive", "update Users set x = 1"];
  const run = (term: string, o: Partial<typeof NO_SEARCH_OPTIONS>) => {
    const m = historyMatcher(term, { ...NO_SEARCH_OPTIONS, ...o });
    return m === null || m === "invalid" ? m : sql.filter(m);
  };

  it("matches case-insensitively by default, with Match Case / Whole Word / Regex", () => {
    expect(run("", {})).toBeNull();
    expect(run("USERS", {})).toHaveLength(3);
    expect(run("Users", { matchCase: true })).toEqual(["update Users set x = 1"]);
    expect(run("users", { wholeWord: true })).toEqual([sql[0], sql[2]]);
    expect(run("users", { wholeWord: true, matchCase: true })).toEqual([sql[0]]);
    expect(run("^select .* users_\\w+$", { regex: true })).toEqual([sql[1]]);
    expect(run("users.", { wholeWord: false })).toEqual([]);
    expect(run("(", { regex: true })).toBe("invalid");
  });

  it("does not run a catastrophic pattern in the renderer", () => {
    // Doubles per extra character in V8 (38 s on this statement): it must not be run at all.
    const slow = "SELECT customer_identifier_xxxxx, count(*) FROM orders";
    const pattern = "select\\s+(\\w+,?\\s*)+from";
    for (const options of [{}, { matchCase: true }, { wholeWord: true }]) {
      const matcher = historyMatcher(pattern, { ...NO_SEARCH_OPTIONS, regex: true, ...options });
      expect(typeof matcher).toBe("function");
      const started = performance.now();
      // The backend (a linear-time engine) does the filtering: the pane shows everything.
      expect((matcher as (sql: string) => boolean)(slow)).toBe(true);
      expect((matcher as (sql: string) => boolean)("update t set a = 1")).toBe(true);
      expect(performance.now() - started).toBeLessThan(50);
    }
    // Nested repetition in other shapes, and a pattern over the length limit.
    for (const risky of ["(a+)+$", "(?:x{2,})*y", "(\\d*)*", `a${"b".repeat(MAX_REGEX_LENGTH)}`]) {
      const matcher = historyMatcher(risky, { ...NO_SEARCH_OPTIONS, regex: true });
      expect(typeof matcher).toBe("function");
      expect((matcher as (sql: string) => boolean)("select 1")).toBe(true);
    }
  });

  it("still filters with a safe regex, and a pattern that does not compile stays invalid", () => {
    const regex = { ...NO_SEARCH_OPTIONS, regex: true };
    const safe = historyMatcher("select\\s+\\w+,\\s*count", regex) as (sql: string) => boolean;
    expect(safe("SELECT a, count(*) FROM t")).toBe(true);
    expect(safe("update t set a = 1")).toBe(false);
    expect((historyMatcher("(a+)+(", regex) as unknown) === "invalid").toBe(true);
    expect(historyMatcher("[", regex)).toBe("invalid");
    // The nested-quantifier guard is for regex mode: plain text is escaped, so it is safe.
    const text = historyMatcher("(a+)+", NO_SEARCH_OPTIONS) as (sql: string) => boolean;
    expect(text("select (a+)+ from t")).toBe(true);
    expect(text("select 1")).toBe(false);
    const word = historyMatcher("(a+)+", { ...NO_SEARCH_OPTIONS, matchCase: true }) as (
      sql: string,
    ) => boolean;
    expect(word("x (a+)+ y")).toBe(true);
    expect(word("aaa")).toBe(false);
  });

  it("copies several entries one statement per line", () => {
    expect(entriesSql([{ sql: "select 1" }, { sql: " select 2; " }])).toBe("select 1;\nselect 2;");
  });

  it("computes the Keep History For cutoff", () => {
    const now = new Date(2026, 9, 13, 12);
    expect(retentionCutoff("forever", now)).toBeNull();
    expect(retentionCutoff("2y", now)).toEqual(new Date(2024, 9, 13, 12));
    expect(yearsAgo(1, now).getFullYear()).toBe(2025);
  });
});
