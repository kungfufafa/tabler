import { describe, expect, it } from "vitest";
import { applySuggestion, rawSqlSuggestions } from "./rawSqlCompletion";

const columns = ["id", "email", "created_at"];

describe("Raw SQL filter autocomplete", () => {
  it("completes the table's columns first, then keywords", () => {
    const s = rawSqlSuggestions("em", 2, "postgres", "users", columns)!;
    expect(s.from).toBe(0);
    expect(s.options[0]?.label).toBe("email");
    const k = rawSqlSuggestions("id > 3 AN", 9, "postgres", "users", columns)!;
    expect(k.from).toBe(7);
    expect(k.options.map((o) => o.label)).toContain("AND");
  });

  it("completes the word before the caret only", () => {
    const s = rawSqlSuggestions("cr = 1", 2, "mysql", "users", columns)!;
    expect(s.options.map((o) => o.label)).toContain("created_at");
    expect(rawSqlSuggestions("id = ", 5, "sqlite", "users", columns)).toBeNull();
    expect(rawSqlSuggestions("zzzz", 4, "postgres", "users", columns)).toBeNull();
  });

  it("replaces the word with the pick", () => {
    expect(applySuggestion("x = 1 an", 6, 8, "AND")).toEqual({ text: "x = 1 AND", caret: 9 });
  });
});
