import { describe, expect, it } from "vitest";
import type { GridColumn, ViewRow } from "@/features/datagrid/types";
import type { Value } from "@/lib/ipc";
import {
  compileQuery,
  counterLabel,
  findMatches,
  MAX_MATCHES,
  planReplace,
  rematch,
  searchText,
  stepMatch,
  type FindQuery,
  type Matcher,
} from "./model";

const q = (text: string, opts: Partial<FindQuery> = {}): FindQuery => ({
  text,
  matchCase: false,
  wholeWord: false,
  regex: false,
  ...opts,
});

function matcher(query: FindQuery): Matcher {
  const c = compileQuery(query);
  if (c.kind !== "ok") throw new Error(`expected a matcher, got ${c.kind}`);
  return c.matcher;
}

const text = (v: string): Value => ({ t: "text", v });
const NULL: Value = { t: "null" };

const rows: Value[][] = [
  [{ t: "int", v: 1 }, text("Ana Smith"), text("ana@example.com")],
  [{ t: "int", v: 2 }, text("Bob"), NULL],
  [{ t: "int", v: 12 }, text("anabelle"), text("ANA@example.org")],
];
const getRow = (r: number): ViewRow | undefined =>
  rows[r] ? { key: String(r), cells: rows[r] } : undefined;

const col = (name: string, kind: GridColumn["kind"], editable = true): GridColumn => ({
  id: name,
  name,
  kind,
  dataType: kind,
  nullable: true,
  isPk: false,
  hasDefault: false,
  isAutoInc: false,
  editable,
  align: "left",
  enumValues: null,
});

describe("find in results: matching", () => {
  it("is case-insensitive by default, Match Case narrows it", () => {
    expect(findMatches(3, getRow, 3, matcher(q("ana")))).toEqual([
      { r: 0, c: 1 },
      { r: 0, c: 2 },
      { r: 2, c: 1 },
      { r: 2, c: 2 },
    ]);
    expect(findMatches(3, getRow, 3, matcher(q("ana", { matchCase: true })))).toEqual([
      { r: 0, c: 2 },
      { r: 2, c: 1 },
    ]);
  });

  it("Whole Word only matches the word on its own", () => {
    const m = matcher(q("ana", { wholeWord: true }));
    expect(findMatches(3, getRow, 3, m)).toEqual([
      { r: 0, c: 1 },
      { r: 0, c: 2 },
      { r: 2, c: 2 },
    ]);
    // Unicode letters count as word characters.
    expect(matcher(q("an", { wholeWord: true })).test("ñan")).toBe(false);
  });

  it("supports regular expressions and reports invalid ones", () => {
    expect(findMatches(3, getRow, 3, matcher(q("^\\d$", { regex: true })))).toEqual([
      { r: 0, c: 0 },
      { r: 1, c: 0 },
    ]);
    expect(compileQuery(q("(", { regex: true })).kind).toBe("invalid");
    expect(compileQuery(q("")).kind).toBe("empty");
    // Without Regex, special characters are literal.
    expect(matcher(q("a.c")).test("abc")).toBe(false);
    expect(matcher(q("a.c")).test("xa.cx")).toBe(true);
    // Patterns that only compile outside Unicode mode still work.
    expect(matcher(q("[\\-x]", { regex: true })).test("a-b")).toBe(true);
  });

  it("searches values as the grid shows them; NULL never matches", () => {
    expect(searchText(NULL)).toBeNull();
    expect(searchText({ t: "default" })).toBeNull();
    expect(searchText({ t: "bytes", v: { base64: "AA==", len: 1, truncated: false } })).toBeNull();
    expect(searchText({ t: "bool", v: true })).toBe("true");
    expect(searchText({ t: "decimal", v: "1.50" })).toBe("1.50");
    expect(searchText(text("a\nb"))).toBe("a\nb");
    expect(findMatches(3, getRow, 3, matcher(q("null")))).toEqual([]);
  });

  it("searches the given columns only, in their (display) order", () => {
    // Column 2 shown before column 1; column 0 hidden.
    expect(findMatches(3, getRow, [2, 1], matcher(q("ana")))).toEqual([
      { r: 0, c: 2 },
      { r: 0, c: 1 },
      { r: 2, c: 2 },
      { r: 2, c: 1 },
    ]);
    expect(findMatches(3, getRow, [1], matcher(q("@")))).toEqual([]);
    expect(findMatches(3, getRow, [], matcher(q("ana")))).toEqual([]);
  });

  it("stops at MAX_MATCHES", () => {
    const many = (r: number): ViewRow => ({ key: String(r), cells: [text("x")] });
    expect(findMatches(MAX_MATCHES + 5, many, 1, matcher(q("x")))).toHaveLength(MAX_MATCHES);
    expect(counterLabel(0, MAX_MATCHES)).toBe("1 of 10,000+");
  });
});

describe("find in results: stepping and the counter", () => {
  it("steps forward and back with wrap-around", () => {
    expect(stepMatch(null, 3, 1)).toBe(0);
    expect(stepMatch(null, 3, -1)).toBe(2);
    expect(stepMatch(2, 3, 1)).toBe(0);
    expect(stepMatch(0, 3, -1)).toBe(2);
    expect(stepMatch(1, 0, 1)).toBeNull();
  });

  it("keeps the current match on its cell when the matches change", () => {
    const matches = [
      { r: 0, c: 1 },
      { r: 3, c: 0 },
      { r: 3, c: 2 },
    ];
    expect(rematch(null, matches)).toBe(0);
    expect(rematch({ r: 3, c: 0 }, matches)).toBe(1);
    // Gone: the next one after it.
    expect(rematch({ r: 2, c: 5 }, matches)).toBe(1);
    // Just replaced: never the same cell.
    expect(rematch({ r: 3, c: 0 }, matches, true)).toBe(2);
    expect(rematch({ r: 9, c: 0 }, matches)).toBe(0);
    expect(rematch({ r: 0, c: 0 }, [])).toBeNull();
    // In display order (column 2 shown before column 0): after (3, 2) comes (3, 0).
    const shown = [2, 0];
    const ordered = [
      { r: 3, c: 2 },
      { r: 3, c: 0 },
    ];
    expect(rematch({ r: 3, c: 2 }, ordered, true, shown)).toBe(1);
    expect(rematch({ r: 3, c: 0 }, ordered, false, shown)).toBe(1);
    // A cell of a column hidden since: the row's first match.
    expect(rematch({ r: 3, c: 1 }, ordered, false, shown)).toBe(0);
  });

  it("labels the counter", () => {
    expect(counterLabel(null, 0)).toBe("No results");
    expect(counterLabel(2, 12)).toBe("3 of 12");
    expect(counterLabel(null, 4)).toBe("4 results");
  });
});

describe("find in results: replace", () => {
  const columns = [col("id", "int"), col("name", "text"), col("email", "text", false)];

  it("replaces every occurrence in a cell, re-typed for its column", () => {
    const m = matcher(q("a"));
    const plan = planReplace([{ r: 0, c: 1 }], getRow, columns, m, "o");
    expect(plan).toEqual({ edits: [{ r: 0, c: 1, value: text("ono Smith") }], skipped: 0 });
    // Regex groups.
    const g = matcher(q("(\\w+) (\\w+)", { regex: true }));
    expect(planReplace([{ r: 0, c: 1 }], getRow, columns, g, "$2, $1").edits[0]?.value).toEqual(
      text("Smith, Ana"),
    );
    // A literal search inserts `$&` as typed.
    expect(planReplace([{ r: 0, c: 1 }], getRow, columns, m, "$&").edits[0]?.value).toEqual(
      text("$&n$& Smith"),
    );
  });

  it("skips read-only columns, deleted rows and values the column type rejects", () => {
    const m = matcher(q("1"));
    // 12 → 1x is not an integer; email is read-only.
    expect(planReplace([{ r: 2, c: 0 }], getRow, columns, m, "x")).toEqual({
      edits: [],
      skipped: 1,
    });
    expect(planReplace([{ r: 2, c: 0 }], getRow, columns, m, "3").edits).toEqual([
      { r: 2, c: 0, value: { t: "int", v: 32 } },
    ]);
    expect(planReplace([{ r: 0, c: 2 }], getRow, columns, matcher(q("ana")), "x").skipped).toBe(1);
    const deleted = (r: number): ViewRow | undefined => {
      const row = getRow(r);
      return row && { ...row, status: "deleted" };
    };
    expect(planReplace([{ r: 0, c: 1 }], deleted, columns, matcher(q("a")), "o").skipped).toBe(1);
  });
});
