import { describe, expect, it } from "vitest";
import {
  applyOverrides,
  insertModesFor,
  loadCsvPrefs,
  matchColumns,
  matchedCount,
  saveCsvPrefs,
  statusLine,
  tableNameFromFile,
  DEFAULT_CSV_PREFS,
  QUOTES,
} from "./mapping";

const table = ["id", "email", "name", "created_at"];

describe("matchColumns", () => {
  it("matches by name, then the rest by position (the default)", () => {
    expect(matchColumns(["email", "ID", "x", "y"], table, "nameAndOrder", true)).toEqual([
      "email",
      null, // `ID` is not `id`; position 1 (`email`) is taken
      "name",
      "created_at",
    ]);
  });

  it("case-insensitive variants match ID to id", () => {
    expect(matchColumns(["email", "ID", "x"], table, "nameAndOrderCi", true)).toEqual([
      "email",
      "id",
      "name",
    ]);
    expect(matchColumns(["EMAIL", "Name", "zzz"], table, "nameCi", true)).toEqual([
      "email",
      "name",
      null,
    ]);
  });

  it("by name only leaves unknown columns out", () => {
    expect(matchColumns(["name", "nope", "id"], table, "name", true)).toEqual(["name", null, "id"]);
    // Without a header row there are no names to match.
    expect(matchColumns(["column_1"], table, "name", false)).toEqual([null]);
  });

  it("by order maps position to position, extra CSV columns are not imported", () => {
    expect(matchColumns(["a", "b", "c", "d", "e"], table, "order", true)).toEqual([
      "id",
      "email",
      "name",
      "created_at",
      null,
    ]);
    expect(matchColumns(["column_1", "column_2"], table, "nameAndOrder", false)).toEqual([
      "id",
      "email",
    ]);
  });

  it("fills each table column once", () => {
    expect(matchColumns(["id", "id", "x"], table, "nameAndOrder", true)).toEqual([
      "id",
      "email",
      "name",
    ]);
  });
});

describe("overrides and status", () => {
  it("moves a picked column away from the CSV column that had it", () => {
    const base = ["id", "email", "name"];
    expect(applyOverrides(base, { 2: "email" })).toEqual(["id", null, "email"]);
    expect(applyOverrides(base, { 0: null, 9: "x" })).toEqual([null, "email", "name"]);
    expect(matchedCount(applyOverrides(base, { 1: null }))).toBe(2);
  });

  it("words the status line like TablePlus", () => {
    expect(statusLine(5, 4, 4)).toBe("CSV 5 columns, table 4 columns, matched 4 columns");
    expect(statusLine(1, 1, 0)).toBe("CSV 1 column, table 1 column, matched 0 columns");
  });
});

describe("insert options and names", () => {
  it("hides REPLACE on PostgreSQL and keys for new tables", () => {
    expect(insertModesFor("postgres", false)).toEqual(["stopOnError", "updateOnDuplicate"]);
    expect(insertModesFor("mysql", false)).toEqual(["stopOnError", "replace", "updateOnDuplicate"]);
    expect(insertModesFor("sqlite", true)).toEqual(["stopOnError"]);
  });

  it("derives a table name from the file", () => {
    expect(tableNameFromFile("Sales Report 2026.csv")).toBe("sales_report_2026");
    expect(tableNameFromFile("2026-orders.csv")).toBe("t_2026_orders");
    expect(tableNameFromFile("Café.tsv")).toBe("cafe");
    expect(tableNameFromFile("...csv")).toBe("imported");
  });

  it("offers the double quote, the single quote and Space", () => {
    expect(QUOTES.map((q) => q.value)).toEqual(['"', "'", " "]);
    // A Space quote is remembered like the others; unknown quotes fall back to the default.
    const data = new Map<string, string>();
    const s = {
      getItem: (k: string) => data.get(k) ?? null,
      setItem: (k: string, v: string) => void data.set(k, v),
    };
    saveCsvPrefs({ ...DEFAULT_CSV_PREFS, quote: " " }, s);
    expect(loadCsvPrefs(s).quote).toBe(" ");
    data.set("tabler.import.csv", '{"quote":"x"}');
    expect(loadCsvPrefs(s).quote).toBe('"');
  });

  it("remembers the wizard's options", () => {
    const data = new Map<string, string>();
    const s = {
      getItem: (k: string) => data.get(k) ?? null,
      setItem: (k: string, v: string) => void data.set(k, v),
    };
    expect(loadCsvPrefs(s)).toEqual(DEFAULT_CSV_PREFS);
    saveCsvPrefs({ header: false, quote: "'", emptyAsNull: false, strategy: "order" }, s);
    expect(loadCsvPrefs(s)).toEqual({
      header: false,
      quote: "'",
      emptyAsNull: false,
      strategy: "order",
    });
    data.set("tabler.import.csv", '{"strategy":"bogus","quote":"x"}');
    expect(loadCsvPrefs(s)).toEqual(DEFAULT_CSV_PREFS);
  });
});
