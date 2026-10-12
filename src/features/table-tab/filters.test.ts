import { fireEvent, render, screen } from "@testing-library/react";
import { createElement } from "react";
import { describe, expect, it, vi } from "vitest";
import { TABLE_COLUMNS, tableMetaFor } from "@/lib/ipc/mock/fixtures";
import { FilterBar } from "./FilterBar";
import { parseFilterValue, RAW_SQL_COLUMN, splitList, toFilterGroup } from "./filters";

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
  it("passes raw SQL rows through as raw nodes, mixed with conditions", () => {
    const raw = (value: string, enabled = true) => ({
      ...row(RAW_SQL_COLUMN, "eq", value, "", enabled),
      id: `raw:${value}`,
    });
    const g = toFilterGroup(
      [
        raw("  age > 30 AND name LIKE 'A%'  "),
        row("id", "gt", "5"),
        raw("balance < 0", false),
        raw("   "),
        { ...raw("is_active"), op: "isNull" },
      ],
      users,
      "or",
    );
    expect(g).toEqual({
      conjunction: "or",
      items: [
        { kind: "raw", sql: "age > 30 AND name LIKE 'A%'" },
        { kind: "condition", column: "id", op: "gt", values: [{ t: "int", v: 5 }] },
        // The operator of a raw row is ignored.
        { kind: "raw", sql: "is_active" },
      ],
    });
    expect(toFilterGroup([raw(""), raw("x = 1", false)], users, "and")).toBeNull();
    // Even without column metadata.
    expect(toFilterGroup([raw("1 = 1")], [], "and")).toEqual({
      conjunction: "and",
      items: [{ kind: "raw", sql: "1 = 1" }],
    });
  });

  it("renders a raw SQL row as one wide SQL input that applies on Enter", () => {
    const onApply = vi.fn();
    const onChange = vi.fn();
    render(
      createElement(FilterBar, {
        columns: users,
        rows: [
          { ...row(RAW_SQL_COLUMN, "eq", "age > 30"), id: "r1" },
          { ...row("id", "gt", "5"), id: "r2" },
        ],
        conjunction: "and",
        dirty: true,
        onChange,
        onApply,
        onClear: vi.fn(),
      }),
    );
    const input = screen.getByRole("textbox", { name: "Raw SQL condition" });
    expect(input).toHaveValue("age > 30");
    expect(input).toHaveAttribute("placeholder", "e.g. age > 30 AND name LIKE 'A%'");
    // Only the column row has an operator select next to its column select.
    expect(screen.getAllByRole("combobox")).toHaveLength(3);
    fireEvent.change(input, { target: { value: "age > 40" } });
    expect(onChange.mock.calls[0]?.[0][0]).toMatchObject({
      column: RAW_SQL_COLUMN,
      value: "age > 40",
    });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onApply).toHaveBeenCalledTimes(1);
  });
});
