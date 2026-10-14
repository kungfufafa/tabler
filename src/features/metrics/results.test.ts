import { describe, expect, it } from "vitest";
import type { MetricsResult, ResultColumn, Value } from "@/lib/ipc";
import { defaultObject } from "./model";
import {
  chartMapping,
  defaultVariables,
  dropdownOptions,
  formatChange,
  formatScore,
  initialOption,
  scoreNumber,
} from "./results";
import { compareValues } from "./sort";

const col = (name: string, kind: ResultColumn["kind"] = "int"): ResultColumn => ({
  name,
  dataType: kind,
  kind,
  nullable: null,
  table: null,
});

const result = (columns: ResultColumn[], rows: Value[][]): MetricsResult => ({
  columns,
  rows,
  truncated: false,
  affected: null,
  durationMs: 1,
  sql: "",
});

describe("scoreboards", () => {
  const r = result(
    [col("label", "text"), col("Total", "decimal")],
    [
      [
        { t: "text", v: "x" },
        { t: "decimal", v: "1234.5" },
      ],
    ],
  );

  it("reads the value column of the first row", () => {
    expect(scoreNumber(r, null)).toBeNull();
    expect(scoreNumber(r, "total")).toBe(1234.5);
    expect(scoreNumber(r, "missing")).toBeNull();
    expect(scoreNumber(null, null)).toBeNull();
    expect(scoreNumber(result([col("n")], []), null)).toBeNull();
  });

  it("formats the number and its change", () => {
    expect(formatScore(1284)).toBe("1,284");
    expect(formatScore(12_900)).toBe("12.9K");
    expect(formatScore(4_200_000)).toBe("4.2M");
    expect(formatScore(3.14159)).toBe("3.14");
    expect(formatChange(110, 100)).toBe("+10 (+10%)");
    expect(formatChange(99, 100)).toBe("−1 (−1.0%)");
    expect(formatChange(5, 0)).toBe("+5");
    expect(formatChange(7, 7)).toBe("±0 (±0.0%)");
  });
});

describe("dropdowns and variables", () => {
  it("takes options from a static list or the SQL result", () => {
    const fixed = {
      ...defaultObject("dropdown", "d", "Role"),
      options: [
        { value: "admin", label: "Admin" },
        { value: "viewer", label: "Viewer" },
      ],
    };
    expect(dropdownOptions(fixed, null)).toEqual([
      { value: { t: "text", v: "admin" }, label: "Admin" },
      { value: { t: "text", v: "viewer" }, label: "Viewer" },
    ]);
    const fromSql = { ...fixed, optionsSource: "sql" as const };
    const r = result(
      [col("id"), col("name", "text")],
      [
        [
          { t: "int", v: 1 },
          { t: "text", v: "Ada" },
        ],
        [
          { t: "int", v: 1 },
          { t: "text", v: "dup" },
        ],
        [{ t: "int", v: 2 }, { t: "null" }],
      ],
    );
    const options = dropdownOptions(fromSql, r);
    expect(options).toEqual([
      { value: { t: "int", v: 1 }, label: "Ada" },
      { value: { t: "int", v: 2 }, label: "NULL" },
    ]);
    expect(initialOption({ ...fromSql, defaultValue: "2" }, options)).toEqual({ t: "int", v: 2 });
    expect(initialOption(fromSql, options)).toEqual({ t: "int", v: 1 });
    expect(initialOption(fromSql, [])).toEqual({ t: "null" });
  });

  it("seeds the variables of inputs and dropdowns", () => {
    expect(
      defaultVariables([
        {
          ...defaultObject("input", "a", "A"),
          variable: "n",
          inputType: "number",
          defaultValue: "3",
        },
        { ...defaultObject("input", "b", "B"), variable: "" },
        {
          ...defaultObject("dropdown", "c", "C"),
          variable: "role",
          options: [{ value: "admin", label: "Admin" }],
        },
        { ...defaultObject("dropdown", "d", "D"), variable: "cust", optionsSource: "sql" },
        { ...defaultObject("table", "e", "E"), variable: "ignored" },
      ]),
    ).toEqual({
      n: { t: "int", v: 3 },
      role: { t: "text", v: "admin" },
      cust: { t: "null" },
    });
  });
});

describe("data table sorting", () => {
  it("orders numbers by value, text naturally, NULLs last both ways", () => {
    const cells: Value[] = [{ t: "text", v: "item10" }, { t: "null" }, { t: "text", v: "item2" }];
    expect([...cells].sort((a, b) => compareValues(a, b))).toEqual([
      { t: "text", v: "item2" },
      { t: "text", v: "item10" },
      { t: "null" },
    ]);
    expect([...cells].sort((a, b) => compareValues(a, b, true))).toEqual([
      { t: "text", v: "item10" },
      { t: "text", v: "item2" },
      { t: "null" },
    ]);
    expect(compareValues({ t: "decimal", v: "10.5" }, { t: "int", v: 9 })).toBeGreaterThan(0);
  });
});

describe("chart mappings", () => {
  it("uses the object's columns, else a comment directive above the query", () => {
    const bar = defaultObject("bar", "b", "B");
    const sql = "-- revenue\n-- TablePlus BarChart x: month, y: revenue, cost\nSELECT 1";
    expect(chartMapping({ ...bar, sql })).toEqual({ x: "month", y: ["revenue", "cost"] });
    expect(chartMapping({ ...bar, sql, x: "day" })).toEqual({ x: "day", y: [] });
    expect(chartMapping({ ...bar, sql: "SELECT 1 -- TablePlus BarChart x: a" })).toEqual({
      x: null,
      y: [],
    });
  });
});
