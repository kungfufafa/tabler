import { describe, expect, it } from "vitest";
import {
  backupFileName,
  DEFAULT_PATTERN,
  insertToken,
  renderPattern,
  sanitizeValue,
  type PatternContext,
} from "./pattern";

const ctx = (over: Partial<PatternContext> = {}): PatternContext => ({
  connection: "Local Postgres",
  database: "tabler_dev",
  tag: "local",
  // 5 March 2026, 07:08:09 local time: every part needs its zero.
  date: new Date(2026, 2, 5, 7, 8, 9),
  ...over,
});

describe("file name patterns", () => {
  it("renders the default pattern zero-padded", () => {
    expect(renderPattern(DEFAULT_PATTERN, ctx())).toBe("tabler_dev_2026-03-05_07-08-09");
    expect(backupFileName(DEFAULT_PATTERN, ctx(), ".sql.gz")).toBe(
      "tabler_dev_2026-03-05_07-08-09.sql.gz",
    );
  });

  it("renders every token", () => {
    expect(
      renderPattern(
        "{connection} {database} {tag} {year}{month}{day} {monthName} {hour}{minute}{second}",
        ctx({ date: new Date(2026, 11, 31, 23, 59, 58) }),
      ),
    ).toBe("Local Postgres tabler_dev local 20261231 Dec 235958");
  });

  it("keeps unknown tokens and literal text", () => {
    expect(renderPattern("dump-{nope}-{database}", ctx())).toBe("dump-{nope}-tabler_dev");
  });

  it("makes sub-folders from / in the pattern, never from values", () => {
    expect(renderPattern("{connection}/{database}/{year}", ctx())).toBe(
      "Local Postgres/tabler_dev/2026",
    );
    expect(renderPattern("{connection}/{database}", ctx({ connection: "EU/Prod: main" }))).toBe(
      "EU_Prod_ main/tabler_dev",
    );
    expect(renderPattern("/../{database}//./x/", ctx())).toBe("tabler_dev/x");
    expect(sanitizeValue(" a\u0000b\\c ")).toBe("a_b_c");
  });

  it("drops the separator next to an empty token", () => {
    expect(renderPattern("{tag}_{database}", ctx({ tag: null }))).toBe("tabler_dev");
    expect(renderPattern("{database}_{tag}", ctx({ tag: null }))).toBe("tabler_dev");
    expect(renderPattern("{database}_{tag}_{year}", ctx({ tag: null }))).toBe("tabler_dev_2026");
    expect(renderPattern("{database}-{tag}", ctx({ tag: "" }))).toBe("tabler_dev");
  });

  it("is never empty", () => {
    expect(renderPattern("", ctx())).toBe("backup");
    expect(renderPattern("{tag}", ctx({ tag: null }))).toBe("backup");
    expect(renderPattern(" / ", ctx())).toBe("backup");
  });

  it("inserts tokens at the caret, replacing the selection", () => {
    expect(insertToken("ab", "year", 1)).toEqual({ pattern: "a{year}b", caret: 7 });
    expect(insertToken("a-XX-b", "day", 2, 4)).toEqual({ pattern: "a-{day}-b", caret: 7 });
    expect(insertToken("ab", "hour")).toEqual({ pattern: "ab{hour}", caret: 8 });
    expect(insertToken("ab", "hour", 99, 120)).toEqual({ pattern: "ab{hour}", caret: 8 });
  });
});
