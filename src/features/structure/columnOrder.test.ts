import { describe, expect, it } from "vitest";
import type { ColumnRow } from "./model";
import { orderColumns, ordinalNumbers } from "./columnOrder";

const row = (key: string, name: string, status: ColumnRow["status"] = "existing"): ColumnRow =>
  ({ key, name, status }) as ColumnRow;

const columns = [
  row("c:id", "id"),
  row("c:Zip", "Zip"),
  row("n:0", "alpha", "added"),
  row("c:age", "age"),
];

describe("ordering the structure's columns", () => {
  it("keeps the table's own order by ordinal position", () => {
    expect(orderColumns(columns, "ordinal").map((c) => c.name)).toEqual([
      "id",
      "Zip",
      "alpha",
      "age",
    ]);
  });

  it("orders by name, ignoring case, new columns included", () => {
    expect(orderColumns(columns, "name").map((c) => c.name)).toEqual(["age", "alpha", "id", "Zip"]);
  });

  it("sorts a copy and breaks ties by position", () => {
    const tied = [row("a", "x"), row("b", "X"), row("c", "a")];
    expect(orderColumns(tied, "name").map((c) => c.key)).toEqual(["c", "a", "b"]);
    expect(tied.map((c) => c.key)).toEqual(["a", "b", "c"]);
  });

  it("numbers rows by their position in the table, whatever the order shown", () => {
    const numbers = ordinalNumbers(columns);
    expect(orderColumns(columns, "name").map((c) => numbers.get(c.key))).toEqual([4, 3, 1, 2]);
  });
});
