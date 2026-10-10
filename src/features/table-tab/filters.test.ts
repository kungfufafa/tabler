import { describe, expect, it } from "vitest";
import { TABLE_COLUMNS, tableMetaFor } from "@/lib/ipc/mock/fixtures";
import { parseFilterValue, splitList, toFilterGroup } from "./filters";

const users = tableMetaFor("postgres", "public", "users")!.columns;
const row = (
  column: string,
  op: Parameters<typeof toFilterGroup>[0][number]["op"],
  value = "",
  value2 = "",
  enabled = true,
) => ({ id: column, column, op, value, value2, enabled });

describe("filters", () => {
  it("parses values by kind", () => {
    expect(parseFilterValue("int", " 42 ")).toEqual({ t: "int", v: 42 });
    expect(parseFilterValue("bigInt", "9007199254740993")).toEqual({
      t: "bigInt",
      v: "9007199254740993",
    });
    expect(parseFilterValue("bool", "yes")).toEqual({ t: "bool", v: true });
    expect(parseFilterValue("text", " a ")).toEqual({ t: "text", v: " a " });
    expect(parseFilterValue("decimal", "1.5")).toEqual({ t: "decimal", v: "1.5" });
  });
  it("splits IN lists with quotes", () => {
    expect(splitList(`1, 2,"a, b", 'c'`)).toEqual(["1", "2", "a, b", "c"]);
    expect(splitList("")).toEqual([]);
  });
  it("builds a filter group and skips empty/disabled rows", () => {
    expect(Object.keys(TABLE_COLUMNS)).toContain("users");
    const g = toFilterGroup(
      [
        row("id", "in", "1,2"),
        row("name", "like", "%a%"),
        row("balance", "between", "1", "9"),
        row("nickname", "isNull"),
        row("email", "eq", ""),
        row("role", "eq", "admin", "", false),
      ],
      users,
      "and",
    );
    expect(g?.items).toHaveLength(4);
    expect(g?.items[0]).toEqual({
      kind: "condition",
      column: "id",
      op: "in",
      values: [
        { t: "int", v: 1 },
        { t: "int", v: 2 },
      ],
    });
    expect(g?.items[2]).toEqual({
      kind: "condition",
      column: "balance",
      op: "between",
      values: [
        { t: "decimal", v: "1" },
        { t: "decimal", v: "9" },
      ],
    });
    expect(toFilterGroup([row("email", "eq", "")], users, "or")).toBeNull();
  });
});
