import { act, fireEvent, render, screen } from "@testing-library/react";
import { createElement } from "react";
import { describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import { TABLE_COLUMNS, tableMetaFor } from "@/lib/ipc/mock/fixtures";
import { isMac } from "@/lib/keys/combo";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import { FilterBar } from "./FilterBar";
import {
  joinList,
  parseFilterValue,
  RAW_SQL_COLUMN,
  splitList,
  toFilterGroup,
  whereOfPageSql,
} from "./filters";

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
    expect(splitList(" a ,, b ,")).toEqual(["a", "b"]);
    // Quoted values keep their spaces, may be empty, and double a quote to contain it.
    expect(splitList(`" a ", "", "say ""hi""", 'it''s'`)).toEqual([" a ", "", 'say "hi"', "it's"]);
  });
  it("joins IN lists that split back into the same values", () => {
    const values = ["1", "Smith, J", " padded ", "", 'say "hi"', "it's", "plain text"];
    expect(joinList(["1", "two"])).toBe("1, two");
    expect(joinList(values)).toBe(
      '1, "Smith, J", " padded ", "", "say ""hi""", "it\'s", plain text',
    );
    expect(splitList(joinList(values))).toEqual(values);
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

  it("reads the WHERE condition back from a page's SELECT", () => {
    expect(
      whereOfPageSql(
        'SELECT "id"::text AS "id"\nFROM "public"."users"\nWHERE ("status" = \'a\' AND "age" >= 3) OR "name"::text LIKE \'%x%\' ORDER BY "name" DESC NULLS LAST, "id" ASC\nLIMIT 200 OFFSET 400',
      ),
    ).toBe(`("status" = 'a' AND "age" >= 3) OR "name"::text LIKE '%x%'`);
    // MySQL's nulls flag, SQLite's rowid order, no ORDER BY at all.
    expect(
      whereOfPageSql(
        "SELECT `id`\nFROM `users`\nWHERE `id` IN (1, 2) ORDER BY (`name` IS NULL) ASC, `name` ASC\nLIMIT 10 OFFSET 0",
      ),
    ).toBe("`id` IN (1, 2)");
    expect(
      whereOfPageSql('SELECT *\nFROM "t"\nWHERE (x = 1) ORDER BY rowid ASC\nLIMIT 1 OFFSET 0'),
    ).toBe("(x = 1)");
    expect(whereOfPageSql('SELECT *\nFROM "t"\nWHERE "a" = 1\nLIMIT 1 OFFSET 0')).toBe('"a" = 1');
    // A raw condition that says ORDER BY keeps it (only the generated clause goes).
    expect(
      whereOfPageSql(
        'SELECT *\nFROM "t"\nWHERE (id IN (SELECT id FROM u ORDER BY id LIMIT 3))\nLIMIT 1 OFFSET 0',
      ),
    ).toBe("(id IN (SELECT id FROM u ORDER BY id LIMIT 3))");
    expect(whereOfPageSql('SELECT * FROM "t" ORDER BY "id" ASC LIMIT 1 OFFSET 0')).toBeNull();
    expect(whereOfPageSql(null)).toBeNull();
  });

  describe("FilterBar layout", () => {
    function mount(props: { dirty?: boolean; sql?: string } = {}) {
      const handlers = { onChange: vi.fn(), onApply: vi.fn(), onClear: vi.fn(), onExport: vi.fn() };
      const rows = [
        { ...row("name", "eq", "Ann"), id: "r1" },
        { ...row("nickname", "isNull"), id: "r2" },
      ];
      render(
        createElement(FilterBar, {
          columns: users,
          rows,
          conjunction: "and",
          dirty: false,
          ...handlers,
          ...props,
        }),
      );
      return { ...handlers, rows };
    }

    it("each row has Apply (applies all), − and +; the strip has Clear and Apply All", () => {
      const { onApply, onChange, onClear, onExport, rows } = mount();
      const applies = screen.getAllByRole("button", { name: "Apply" });
      expect(applies).toHaveLength(2);
      fireEvent.click(applies[1]!);
      fireEvent.click(screen.getByRole("button", { name: "Apply All" }));
      expect(onApply).toHaveBeenCalledTimes(2);

      fireEvent.click(screen.getAllByRole("button", { name: "Remove filter" })[0]!);
      expect(onChange).toHaveBeenLastCalledWith([rows[1]], "and");
      // + adds a blank condition on the same column right under its row.
      fireEvent.click(screen.getAllByRole("button", { name: "Add filter" })[0]!);
      const [next] = onChange.mock.calls.at(-1)!;
      expect(next).toHaveLength(3);
      expect(next[1]).toMatchObject({ column: "name", op: "eq", value: "", enabled: true });
      expect(next[2]).toBe(rows[1]);

      // AND / OR joins the conditions (shown from the second one on).
      fireEvent.click(screen.getByRole("button", { name: "AND" }));
      expect(onChange).toHaveBeenLastCalledWith(rows, "or");

      fireEvent.click(screen.getByRole("button", { name: "Clear" }));
      expect(onClear).toHaveBeenCalledOnce();
      fireEvent.click(screen.getByRole("button", { name: "Export" }));
      expect(onExport).toHaveBeenCalledOnce();
    });

    it("SQL shows the applied WHERE from the page's SELECT", async () => {
      mount({
        dirty: true,
        sql: 'SELECT *\nFROM "public"."users"\nWHERE "name" = \'Ann\' ORDER BY "id" ASC\nLIMIT 200 OFFSET 0',
      });
      fireEvent.click(screen.getByRole("button", { name: "Show the filter's SQL" }));
      expect(await screen.findByTestId("filter-where")).toHaveTextContent(`WHERE "name" = 'Ann'`);
      expect(screen.getByText(/not applied yet/)).toBeInTheDocument();
    });

    it("SQL says so when nothing is applied", async () => {
      mount({ sql: 'SELECT * FROM "users" LIMIT 200 OFFSET 0' });
      fireEvent.click(screen.getByRole("button", { name: "Show the filter's SQL" }));
      expect(await screen.findByText("No filter is applied.")).toBeInTheDocument();
    });
  });

  describe("FilterBar keys", () => {
    const mod = { [isMac ? "metaKey" : "ctrlKey"]: true };
    function mount() {
      const props = { onApply: vi.fn(), onChange: vi.fn(), onClear: vi.fn(), onClose: vi.fn() };
      const rows = [
        { ...row("name", "eq", "Ann"), id: "r1" },
        { ...row("id", "gt", "5"), id: "r2" },
      ];
      // The bar's ⌘↩ / ⌘I / ⇧⌘I are keymap shortcuts ("filter" scope), dispatched by the provider.
      render(
        createElement(
          KeyboardProvider,
          null,
          createElement(FilterBar, {
            columns: users,
            rows,
            conjunction: "and",
            dirty: true,
            ...props,
          }),
        ),
      );
      const values = screen.getAllByRole("textbox", { name: "Filter value" });
      return { ...props, rows, values };
    }

    it("⌘↩ applies from any control, even a select that opens on ↩", () => {
      const { onApply } = mount();
      const select = screen.getAllByRole("combobox", { name: "Filter column" })[1]!;
      act(() => select.focus());
      fireEvent.keyDown(select, { key: "Enter", code: "Enter", ...mod });
      expect(onApply).toHaveBeenCalledTimes(1);
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    it("⌘I duplicates the focused row, ⇧⌘I removes it", () => {
      const { onChange, rows, values } = mount();
      act(() => values[0]!.focus());
      fireEvent.keyDown(values[0]!, { key: "i", code: "KeyI", ...mod });
      const [next] = onChange.mock.calls.at(-1)!;
      expect(next).toHaveLength(3);
      expect(next[1]).toEqual({ ...rows[0], id: expect.any(String) });
      expect(next[1].id).not.toBe("r1");
      act(() => values[1]!.focus());
      fireEvent.keyDown(values[1]!, { key: "I", code: "KeyI", ...mod, shiftKey: true });
      expect(onChange.mock.calls.at(-1)![0]).toEqual([rows[0]]);
    });

    it("handles its keys only while it has the focus, ahead of the global ⇧⌘I", async () => {
      const { onChange, rows, values } = mount();
      const toggle = vi.fn();
      const unregister = actions.register("ui.toggle-inspector", toggle);
      expect(actions.has("filter.remove")).toBe(false);
      act(() => values[1]!.focus());
      expect(actions.has("filter.remove")).toBe(true);
      fireEvent.keyDown(values[1]!, { key: "I", code: "KeyI", ...mod, shiftKey: true });
      expect(onChange.mock.calls.at(-1)![0]).toEqual([rows[0]]);
      expect(toggle).not.toHaveBeenCalled();
      act(() => values[1]!.blur());
      expect(actions.has("filter.remove")).toBe(false);
      unregister();
    });

    it("⎋ closes the bar unless something else handled it", () => {
      const { onClose, values } = mount();
      const event = new KeyboardEvent("keydown", {
        key: "Escape",
        bubbles: true,
        cancelable: true,
      });
      event.preventDefault();
      values[0]!.dispatchEvent(event);
      expect(onClose).not.toHaveBeenCalled();
      fireEvent.keyDown(values[0]!, { key: "Escape" });
      expect(onClose).toHaveBeenCalledTimes(1);
    });
  });
});
