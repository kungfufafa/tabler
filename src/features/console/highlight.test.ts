import { describe, expect, it } from "vitest";
import { consoleSegments, HIGHLIGHT_MAX_CHARS } from "./highlight";

describe("consoleSegments", () => {
  it("splits SQL into classed spans and keeps the text", () => {
    const sql = "SELECT id, 'x' FROM users WHERE n = 1;\n-- done";
    const segs = consoleSegments(sql, "postgres");
    expect(segs.map((s) => s.text).join("")).toBe(sql);
    const cls = (text: string) => segs.find((s) => s.text === text)?.cls;
    expect(cls("SELECT")).toContain("--syn-keyword");
    expect(cls("'x'")).toContain("--syn-string");
    expect(cls("1")).toContain("--syn-number");
    expect(cls("-- done")).toContain("--syn-comment");
    expect(segs.filter((s) => s.text === "\n")).toHaveLength(1);
  });

  it("uses the connection's dialect (MySQL backticks)", () => {
    const segs = consoleSegments("SELECT `name` FROM t", "mysql");
    expect(segs.find((s) => s.text === "`name`")?.cls).not.toContain("--syn-string");
  });

  it("leaves very long SQL plain", () => {
    const sql = `SELECT '${"x".repeat(HIGHLIGHT_MAX_CHARS)}'`;
    expect(consoleSegments(sql, null)).toEqual([{ text: sql, cls: "" }]);
  });
});
