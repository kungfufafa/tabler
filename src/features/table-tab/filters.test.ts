import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createElement, useState } from "react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import { TABLE_COLUMNS, tableMetaFor } from "@/lib/ipc/mock/fixtures";
import { isMac } from "@/lib/keys/combo";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import { getPrefs, resetPrefsForTests, setPref } from "@/stores/prefs";
import type { FilterRowSpec } from "@/stores/tableTabs";
import { FilterBar } from "./FilterBar";
import {
  ANY_COLUMN,
  defaultFilterRow,
  filterValueError,
  isFilterOp,
  joinList,
  opLabel,
  opsFor,
  parseFilterValue,
  RAW_SQL_COLUMN,
  rowValueError,
  splitList,
  toFilterGroup,
  whereOfPageSql,
} from "./filters";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));
afterEach(() => {
  Object.values(toast).forEach((fn) => fn.mockReset());
});

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
  it("parses hex for binary columns, with or without a prefix", () => {
    const deadbeef = {
      t: "bytes",
      v: { base64: btoa("\xde\xad\xbe\xef"), len: 4, truncated: false },
    };
    expect(parseFilterValue("bytes", "deadbeef")).toEqual(deadbeef);
    expect(parseFilterValue("bytes", "0xDEADBEEF")).toEqual(deadbeef);
    expect(parseFilterValue("bytes", "\\xdeadbeef")).toEqual(deadbeef);
    expect(parseFilterValue("bytes", " de ad be ef ")).toEqual(deadbeef);
    // What is not hex stays text: it is never sent as bytes.
    expect(parseFilterValue("bytes", "xyz")).toEqual({ t: "text", v: "xyz" });
    expect(parseFilterValue("bytes", "abc")).toEqual({ t: "text", v: "abc" });
  });
  it("says what is wrong with a binary column's value, with the cell editor's message", () => {
    const message = "Enter hex bytes (pairs of 0-9 / a-f)";
    expect(filterValueError("bytes", "xyz")).toBe(message);
    expect(filterValueError("bytes", "abc")).toBe(message);
    expect(filterValueError("bytes", "0xDEADBEEF")).toBeNull();
    // Blank is "not filled in yet", other kinds are not strict.
    expect(filterValueError("bytes", "  ")).toBeNull();
    expect(filterValueError("int", "xyz")).toBeNull();
    expect(filterValueError(null, "xyz")).toBeNull();
    expect(rowValueError(row("avatar", "eq", "xyz"), users)).toBe(message);
    expect(rowValueError(row("avatar", "between", "00", "zz"), users)).toBe(message);
    expect(rowValueError(row("avatar", "in", "00, 0x1f, 7"), users)).toBe(message);
    expect(rowValueError(row("avatar", "in", "00, 0x1f"), users)).toBeNull();
    expect(rowValueError(row("avatar", "isNull"), users)).toBeNull();
    // Patterns compare as text; other columns are the server's business.
    expect(rowValueError(row("avatar", "contains", "xyz"), users)).toBeNull();
    expect(rowValueError(row("name", "eq", "xyz"), users)).toBeNull();
    expect(rowValueError(row(ANY_COLUMN, "eq", "xyz"), users)).toBeNull();
  });
  it("filters a binary column by its hex: bytes values, per item in a list, never as text", () => {
    const bytes = (hex: string) => parseFilterValue("bytes", hex);
    const group = toFilterGroup(
      [row("avatar", "eq", "deadbeef"), row("avatar", "in", "00ff, 0x1f2e")],
      users,
      "and",
    );
    expect(group?.items).toEqual([
      { kind: "condition", column: "avatar", op: "eq", values: [bytes("deadbeef")] },
      { kind: "condition", column: "avatar", op: "in", values: [bytes("00ff"), bytes("1f2e")] },
    ]);
    // A value that is not hex is left out (text compared with a blob would match nothing).
    expect(toFilterGroup([row("avatar", "eq", "xyz")], users, "and")).toBeNull();
    expect(toFilterGroup([row("avatar", "in", "00ff, zz")], users, "and")).toBeNull();
    // "Any column" compares as text and skips blobs.
    expect(toFilterGroup([row(ANY_COLUMN, "eq", "deadbeef")], users, "and")?.items).toEqual([
      { kind: "anyColumn", op: "eq", values: [{ t: "text", v: "deadbeef" }] },
    ]);
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
    const input = screen.getByRole("combobox", { name: "Raw SQL condition" });
    expect(input).toHaveValue("age > 30");
    expect(input).toHaveAttribute("placeholder", "e.g. age > 30 AND name LIKE 'A%'");
    // Only the column row has an operator select next to its column select (+ the SQL field).
    expect(screen.getAllByRole("combobox")).toHaveLength(4);
    fireEvent.change(input, { target: { value: "age > 40" } });
    expect(onChange.mock.calls[0]?.[0][0]).toMatchObject({
      column: RAW_SQL_COLUMN,
      value: "age > 40",
    });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onApply).toHaveBeenCalledTimes(1);
  });

  it("completes columns and keywords in the Raw SQL field", () => {
    const onChange = vi.fn();
    render(
      createElement(FilterBar, {
        columns: users,
        rows: [{ ...row(RAW_SQL_COLUMN, "eq", ""), id: "r1" }],
        conjunction: "and",
        dirty: true,
        table: "users",
        onChange,
        onApply: vi.fn(),
        onClear: vi.fn(),
      }),
    );
    const input = screen.getByRole("combobox", { name: "Raw SQL condition" });
    fireEvent.change(input, { target: { value: "na", selectionStart: 2 } });
    const list = screen.getByRole("listbox", { name: "Suggestions" });
    expect(list).toHaveTextContent("name");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onChange.mock.calls.at(-1)?.[0][0]).toMatchObject({ value: "name" });
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("a row's Apply applies that filter alone and reads Applied once applied", () => {
    const onApply = vi.fn();
    const onApplyRow = vi.fn();
    const rows = [
      { ...row("id", "gt", "5"), id: "r1" },
      { ...row("name", "eq", "Ann"), id: "r2" },
    ];
    render(
      createElement(FilterBar, {
        columns: users,
        rows,
        conjunction: "and",
        dirty: false,
        appliedRows: [rows[0]!],
        onChange: vi.fn(),
        onApply,
        onApplyRow,
        onClear: vi.fn(),
      }),
    );
    const applies = screen.getAllByRole("button", { name: /^Appl(y|ied)$/ });
    expect(applies.map((b) => b.textContent)).toEqual(["Applied", "Apply"]);
    fireEvent.click(applies[1]!);
    expect(onApplyRow).toHaveBeenLastCalledWith("r2");
    fireEvent.keyDown(screen.getAllByRole("textbox", { name: "Filter value" })[0]!, {
      key: "Enter",
    });
    expect(onApplyRow).toHaveBeenLastCalledWith("r1");
    expect(onApply).not.toHaveBeenCalled();
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

    const FULL_SQL =
      'SELECT *\nFROM "public"."users"\nWHERE "name" = \'Ann\' ORDER BY "id" ASC\nLIMIT 200 OFFSET 0';
    const openSql = () =>
      fireEvent.click(screen.getByRole("button", { name: "Show the filter's SQL" }));

    it("SQL shows the whole generated query, highlighted, and the WHERE alone on request", async () => {
      mount({ dirty: true, sql: FULL_SQL });
      openSql();
      const query = await screen.findByTestId("filter-sql");
      // SELECT, ORDER BY and LIMIT / OFFSET, not just the condition.
      expect(query.textContent).toBe(FULL_SQL);
      // The console's highlighter splits it into coloured spans.
      expect(query.querySelectorAll("span").length).toBeGreaterThan(3);
      expect(screen.getByText(/not applied yet/)).toBeInTheDocument();
      expect(screen.getByRole("radio", { name: "Full query" })).toHaveAttribute(
        "aria-checked",
        "true",
      );

      fireEvent.click(screen.getByRole("radio", { name: "WHERE only" }));
      expect(screen.queryByTestId("filter-sql")).not.toBeInTheDocument();
      expect(screen.getByTestId("filter-where").textContent).toBe(`WHERE "name" = 'Ann'`);
      fireEvent.click(screen.getByRole("radio", { name: "Full query" }));
      expect(screen.getByTestId("filter-sql").textContent).toBe(FULL_SQL);
    });

    it("SQL's Copy puts the shown statement on the clipboard", async () => {
      const writeText = vi.fn<(text: string) => Promise<void>>().mockResolvedValue();
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: { writeText, readText: vi.fn() },
      });
      mount({ sql: FULL_SQL });
      openSql();
      fireEvent.click(await screen.findByRole("button", { name: "Copy" }));
      await waitFor(() => expect(writeText).toHaveBeenLastCalledWith(FULL_SQL));
      await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Copied"));
      // The WHERE view copies what it shows.
      fireEvent.click(screen.getByRole("radio", { name: "WHERE only" }));
      fireEvent.click(screen.getByRole("button", { name: "Copy" }));
      await waitFor(() => expect(writeText).toHaveBeenLastCalledWith(`WHERE "name" = 'Ann'`));
    });

    it("SQL says so when nothing is applied or the page has not loaded", async () => {
      const { unmount } = render(
        createElement(FilterBar, {
          columns: users,
          rows: [],
          conjunction: "and",
          dirty: false,
          onChange: vi.fn(),
          onApply: vi.fn(),
          onClear: vi.fn(),
        }),
      );
      openSql();
      expect(await screen.findByText("The query shows once the page has loaded.")).toBeVisible();
      expect(screen.getByRole("button", { name: "Copy" })).toBeDisabled();
      unmount();

      mount({ sql: 'SELECT * FROM "users" LIMIT 200 OFFSET 0' });
      openSql();
      // The whole query, without a WHERE.
      expect((await screen.findByTestId("filter-sql")).textContent).toBe(
        'SELECT * FROM "users" LIMIT 200 OFFSET 0',
      );
      fireEvent.click(screen.getByRole("radio", { name: "WHERE only" }));
      expect(screen.getByText("No filter is applied.")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Copy" })).toBeDisabled();
    });

    it("Export exports the filtered rows and does not advertise ⇧⌘E (the multi-table export)", () => {
      mount();
      const button = screen.getByRole("button", { name: "Export" });
      expect(button).toHaveAttribute("title", "Export the filtered rows");
      expect(button.getAttribute("title")).not.toMatch(/⇧⌘E|Shift\+E|Ctrl\+Shift/);
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

    it("⌘I duplicates the focused row's column and operator, not its value; ⇧⌘I removes it", () => {
      const { onChange, rows, values } = mount();
      act(() => values[0]!.focus());
      fireEvent.keyDown(values[0]!, { key: "i", code: "KeyI", ...mod });
      const [next] = onChange.mock.calls.at(-1)!;
      expect(next).toHaveLength(3);
      // TablePlus clones the column and the operator; the value is typed anew.
      expect(next[1]).toEqual({
        id: expect.any(String),
        column: "name",
        op: "eq",
        value: "",
        value2: "",
        enabled: true,
      });
      expect(next[1].id).not.toBe("r1");
      expect(next[0]).toBe(rows[0]);
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

    /** The bar with its rows kept in state, as the table tab does. */
    function Stateful({
      initial,
      onFocusData,
    }: {
      initial: FilterRowSpec[];
      onFocusData?: () => void;
    }) {
      const [rows, setRows] = useState(initial);
      return createElement(
        KeyboardProvider,
        null,
        createElement(FilterBar, {
          columns: users,
          rows,
          conjunction: "and",
          dirty: true,
          onChange: (next: FilterRowSpec[]) => setRows(next),
          onApply: vi.fn(),
          onClear: vi.fn(),
          onFocusData,
        }),
      );
    }
    const valuesOf = (name = "Filter value") =>
      screen.getAllByRole("textbox", { name }).map((v) => (v as HTMLInputElement).value);
    const operators = () =>
      screen.getAllByRole("combobox", { name: "Filter operator" }).map((o) => o.textContent);

    it("⌘I on 'name contains abc' inserts 'name contains ' with an empty value, focused", () => {
      render(
        createElement(Stateful, { initial: [{ ...row("name", "contains", "abc"), id: "r1" }] }),
      );
      const [first] = screen.getAllByRole("textbox", { name: "Filter value" });
      act(() => first!.focus());
      fireEvent.keyDown(first!, { key: "i", code: "KeyI", ...mod });
      // The original is unchanged; the clone has the column and the operator, no value.
      expect(valuesOf()).toEqual(["abc", ""]);
      expect(operators()).toEqual(["Contains", "Contains"]);
      for (const c of screen.getAllByRole("combobox", { name: "Filter column" }))
        expect(c).toHaveTextContent(/^name/);
      const inputs = screen.getAllByRole("textbox", { name: "Filter value" });
      expect(inputs[1]).toHaveFocus();
    });

    it("⌘I leaves the second value blank and the clone checked, whatever the original was", () => {
      const between = { ...row("balance", "between", "1", "9", false), id: "r1" };
      render(createElement(Stateful, { initial: [between] }));
      const [first] = screen.getAllByRole("textbox", { name: "Filter value" });
      act(() => first!.focus());
      fireEvent.keyDown(first!, { key: "i", code: "KeyI", ...mod });
      expect(valuesOf()).toEqual(["1", ""]);
      expect(valuesOf("Second filter value")).toEqual(["9", ""]);
      expect(
        screen
          .getAllByRole("checkbox", { name: "Enable filter" })
          .map((c) => c.ariaChecked ?? c.getAttribute("aria-checked")),
      ).toEqual(["false", "true"]);
    });

    it("+ also keeps the row's operator", () => {
      render(
        createElement(Stateful, { initial: [{ ...row("name", "hasPrefix", "ab"), id: "r1" }] }),
      );
      fireEvent.click(screen.getByRole("button", { name: "Add filter" }));
      expect(valuesOf()).toEqual(["ab", ""]);
      expect(operators()).toEqual(["Has prefix", "Has prefix"]);
    });

    it("⌘↓ on the last row goes down to the data; on the others it moves to the next row", () => {
      const onFocusData = vi.fn();
      const initial = [
        { ...row("name", "eq", "Ann"), id: "r1" },
        { ...row("id", "gt", "5"), id: "r2" },
      ];
      render(createElement(Stateful, { initial, onFocusData }));
      const values = screen.getAllByRole("textbox", { name: "Filter value" });
      act(() => values[0]!.focus());
      fireEvent.keyDown(values[0]!, { key: "ArrowDown", code: "ArrowDown", ...mod });
      expect(document.activeElement).toBe(values[1]);
      expect(onFocusData).not.toHaveBeenCalled();
      fireEvent.keyDown(values[1]!, { key: "ArrowDown", code: "ArrowDown", ...mod });
      expect(onFocusData).toHaveBeenCalledOnce();
      // ⌘↑ on the first row stays in the bar.
      fireEvent.keyDown(values[0]!, { key: "ArrowUp", code: "ArrowUp", ...mod });
      expect(onFocusData).toHaveBeenCalledOnce();
    });

    it("marks a binary column's value that is not hex, and hints at hex", () => {
      const initial = [
        { ...row("avatar", "eq", "xyz"), id: "r1" },
        { ...row("avatar", "eq", "0xDEADBEEF"), id: "r2" },
        { ...row("avatar", "contains", "xyz"), id: "r3" },
      ];
      render(createElement(Stateful, { initial }));
      const [bad, good, pattern] = screen.getAllByRole("textbox", { name: "Filter value" });
      expect(bad).toHaveAttribute("aria-invalid", "true");
      expect(bad).toHaveAttribute("title", "Enter hex bytes (pairs of 0-9 / a-f)");
      expect(bad).toHaveAttribute("placeholder", "hex, e.g. 00ff");
      expect(good).not.toHaveAttribute("aria-invalid");
      // A pattern compares the blob as text.
      expect(pattern).not.toHaveAttribute("aria-invalid");
      expect(pattern).toHaveAttribute("placeholder", "value");
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

  describe("operators", () => {
    const labels = (ops: ReturnType<typeof opsFor>, d: Parameters<typeof opLabel>[1]) =>
      ops.map((o) => opLabel(o.op, d));
    it("lists TablePlus's operators in order, ILIKE and JSON ones on PostgreSQL only", () => {
      expect(labels(opsFor("postgres", { kind: "text" }), "postgres")).toEqual([
        "=",
        "<>",
        "<",
        ">",
        "<=",
        ">=",
        "IN",
        "NOT IN",
        "IS NULL",
        "IS NOT NULL",
        "BETWEEN",
        "NOT BETWEEN",
        "Contains",
        "Not contains",
        "Contains – Case insensitive",
        "Not contains – Case insensitive",
        "Has prefix",
        "Has suffix",
        "Has prefix – Case insensitive",
        "Has suffix – Case insensitive",
        "LIKE",
        "NOT LIKE",
        "ILIKE",
      ]);
      const json = labels(opsFor("postgres", { kind: "json" }), "postgres");
      expect(json.slice(-2)).toEqual(["JSON contains", "JSON not contains"]);
      expect(labels(opsFor("postgres", "any"), "postgres")).not.toContain("JSON contains");
      const my = labels(opsFor("mysql", { kind: "json" }), "mysql");
      expect(my).not.toContain("ILIKE");
      expect(my).not.toContain("JSON contains");
      expect(my).toContain("Contains – Case sensitive");
      expect(my).toContain("Has suffix – Case sensitive");
      expect(labels(opsFor("sqlite", { kind: "text" }), "sqlite")).toContain(
        "Has prefix – Case insensitive",
      );
      expect(isFilterOp("hasSuffixCase")).toBe(true);
      expect(isFilterOp("startsWith")).toBe(false);
    });

    it("sends pattern values as text and JSON values as JSON", () => {
      const g = toFilterGroup(
        [
          row("id", "contains", "4"),
          row("name", "hasPrefixCase", "an"),
          row("id", "notBetween", "1", "9"),
          row("name", "ilike", "a%"),
          row("settings", "jsonContains", '{"a":1}'),
        ],
        users,
        "and",
      );
      expect(g?.items).toEqual([
        { kind: "condition", column: "id", op: "contains", values: [{ t: "text", v: "4" }] },
        {
          kind: "condition",
          column: "name",
          op: "hasPrefixCase",
          values: [{ t: "text", v: "an" }],
        },
        {
          kind: "condition",
          column: "id",
          op: "notBetween",
          values: [
            { t: "int", v: 1 },
            { t: "int", v: 9 },
          ],
        },
        { kind: "condition", column: "name", op: "ilike", values: [{ t: "text", v: "a%" }] },
        ...(users.some((c) => c.name === "settings")
          ? [
              {
                kind: "condition",
                column: "settings",
                op: "jsonContains",
                values: [{ t: "json", v: '{"a":1}' }],
              },
            ]
          : []),
      ]);
    });

    it("turns Any column rows into anyColumn nodes with text values", () => {
      const any = (op: Parameters<typeof row>[1], value = "", value2 = "") => ({
        ...row(ANY_COLUMN, op, value, value2),
        id: `any:${op}`,
      });
      expect(
        toFilterGroup(
          [any("eq", "42"), any("isNull"), any("in", "a, 2"), any("contains", "")],
          [],
          "or",
        ),
      ).toEqual({
        conjunction: "or",
        items: [
          { kind: "anyColumn", op: "eq", values: [{ t: "text", v: "42" }] },
          { kind: "anyColumn", op: "isNull", values: [] },
          {
            kind: "anyColumn",
            op: "in",
            values: [
              { t: "text", v: "a" },
              { t: "text", v: "2" },
            ],
          },
        ],
      });
    });

    it("starts new rows from the ⚙ defaults", () => {
      const pk = users.find((c) => c.isPk)!.name;
      expect(defaultFilterRow(users, { column: "primaryKey", operator: "eq" })).toMatchObject({
        column: pk,
        op: "eq",
        value: "",
        enabled: true,
      });
      expect(defaultFilterRow(users, { column: "anyColumn", operator: "contains" })).toMatchObject({
        column: ANY_COLUMN,
        op: "contains",
      });
      expect(defaultFilterRow(users, { column: "rawSql", operator: "eq" }).column).toBe(
        RAW_SQL_COLUMN,
      );
      // No primary key: the first column.
      const noPk = users.map((c) => ({ ...c, isPk: false }));
      expect(defaultFilterRow(noPk, { column: "primaryKey", operator: "eq" }).column).toBe(
        users[0]!.name,
      );
    });
  });

  describe("FilterBar Apply All, ⚙ and new keys", () => {
    const mod = { [isMac ? "metaKey" : "ctrlKey"]: true };
    beforeAll(() => {
      // Radix Select scrolls the picked option into view; jsdom has no layout.
      Element.prototype.scrollIntoView ??= () => {};
      Element.prototype.hasPointerCapture ??= () => false;
    });
    function mount(props: { dialect?: "postgres" | "mysql" | "sqlite" } = {}) {
      const handlers = { onApply: vi.fn(), onChange: vi.fn(), onClear: vi.fn(), onClose: vi.fn() };
      const rows = [
        { ...row("name", "eq", "Ann"), id: "r1" },
        { ...row("id", "gt", "5"), id: "r2" },
        { ...row("email", "contains", "x"), id: "r3" },
      ];
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
            ...handlers,
          }),
        ),
      );
      const values = screen.getAllByRole("textbox", { name: "Filter value" });
      return { ...handlers, rows, values };
    }

    it("Apply All ▾ applies with AND (⌘↩) or OR (⇧⌘↩)", async () => {
      const { onApply, values } = mount();
      fireEvent.click(screen.getByRole("button", { name: "Apply All" }));
      expect(onApply).toHaveBeenLastCalledWith("and");
      fireEvent.pointerDown(screen.getByRole("button", { name: "Apply All options" }), {
        button: 0,
        ctrlKey: false,
      });
      const menu = await screen.findByRole("menu");
      fireEvent.click(
        within(menu).getByRole("menuitem", { name: /Apply All Checked Filters with OR/ }),
      );
      expect(onApply).toHaveBeenLastCalledWith("or");
      act(() => values[0]!.focus());
      fireEvent.keyDown(values[0]!, { key: "Enter", code: "Enter", ...mod });
      expect(onApply).toHaveBeenLastCalledWith("and");
      fireEvent.keyDown(values[0]!, { key: "Enter", code: "Enter", ...mod, shiftKey: true });
      expect(onApply).toHaveBeenLastCalledWith("or");
      // ↩ in a value applies with the bar's own conjunction.
      fireEvent.keyDown(values[0]!, { key: "Enter", code: "Enter" });
      expect(onApply).toHaveBeenLastCalledWith();
    });

    it("⌘↑ / ⌘↓ move between rows, ⌘B checks / unchecks the focused one", () => {
      const { onChange, rows, values } = mount();
      act(() => values[0]!.focus());
      fireEvent.keyDown(values[0]!, { key: "ArrowDown", code: "ArrowDown", ...mod });
      expect(document.activeElement).toBe(values[1]);
      fireEvent.keyDown(values[1]!, { key: "ArrowDown", code: "ArrowDown", ...mod });
      expect(document.activeElement).toBe(values[2]);
      // The last row stays put.
      fireEvent.keyDown(values[2]!, { key: "ArrowDown", code: "ArrowDown", ...mod });
      expect(document.activeElement).toBe(values[2]);
      fireEvent.keyDown(values[2]!, { key: "ArrowUp", code: "ArrowUp", ...mod });
      expect(document.activeElement).toBe(values[1]);
      // From a column select, the next row's column select.
      const columnsSel = screen.getAllByRole("combobox", { name: "Filter column" });
      act(() => columnsSel[0]!.focus());
      fireEvent.keyDown(columnsSel[0]!, { key: "ArrowDown", code: "ArrowDown", ...mod });
      expect(document.activeElement).toBe(columnsSel[1]);
      fireEvent.keyDown(columnsSel[1]!, { key: "b", code: "KeyB", ...mod });
      expect(onChange).toHaveBeenLastCalledWith(
        [rows[0], { ...rows[1], enabled: false }, rows[2]],
        "and",
      );
    });

    it("⌘← / ⌘→ open the focused row's column / operator list", async () => {
      const { values } = mount();
      act(() => values[1]!.focus());
      fireEvent.keyDown(values[1]!, { key: "ArrowLeft", code: "ArrowLeft", ...mod });
      const columns = await screen.findByRole("listbox");
      expect(within(columns).getByRole("option", { name: /Any column/ })).toBeInTheDocument();
      expect(within(columns).getByRole("option", { name: /Raw SQL/ })).toBeInTheDocument();
      fireEvent.keyDown(columns, { key: "Escape" });
      act(() => values[1]!.focus());
      fireEvent.keyDown(values[1]!, { key: "ArrowRight", code: "ArrowRight", ...mod });
      const ops = await screen.findByRole("listbox");
      expect(within(ops).getByRole("option", { name: "NOT BETWEEN" })).toBeInTheDocument();
      expect(within(ops).getByRole("option", { name: "ILIKE" })).toBeInTheDocument();
    });

    it("labels the case variants per driver", () => {
      mount({ dialect: "mysql" });
      const triggers = screen.getAllByRole("combobox", { name: "Filter operator" });
      expect(triggers[2]).toHaveTextContent("Contains");
    });

    it("⚙ sets the defaults new rows start from", async () => {
      resetPrefsForTests();
      const { onChange } = mount();
      fireEvent.pointerDown(screen.getByRole("button", { name: "Filter settings" }), {
        button: 0,
        ctrlKey: false,
      });
      const menu = await screen.findByRole("menu");
      expect(within(menu).getByText("Default filter column")).toBeInTheDocument();
      expect(
        within(menu).getByRole("menuitemradio", { name: "Primary key (if exist)" }),
      ).toHaveAttribute("aria-checked", "true");
      fireEvent.click(within(menu).getByRole("menuitemradio", { name: "Any column" }));
      expect(getPrefs().filterDefaultColumn).toBe("anyColumn");
      act(() => setPref("filterDefaultOperator", "contains"));
      act(() => setPref("filterDefaultState", "hide"));
      expect(getPrefs().filterDefaultState).toBe("hide");
      fireEvent.click(screen.getAllByRole("button", { name: "Add filter" })[2]!);
      // + keeps the row's column, with the default operator.
      expect(onChange.mock.calls.at(-1)![0][3]).toMatchObject({ column: "email", op: "contains" });
      resetPrefsForTests();
    });

    it("a column change resets an operator the new column does not offer", async () => {
      const onChange = vi.fn();
      const rows = [{ ...row("settings", "jsonContains", '{"a":1}'), id: "r1" }];
      render(
        createElement(
          KeyboardProvider,
          null,
          createElement(FilterBar, {
            columns: users,
            rows,
            conjunction: "and",
            dirty: true,
            dialect: "postgres",
            onChange,
            onApply: vi.fn(),
            onClear: vi.fn(),
          }),
        ),
      );
      expect(screen.getByRole("combobox", { name: "Filter operator" })).toHaveTextContent(
        "JSON contains",
      );
      const value = screen.getByRole("textbox", { name: "Filter value" });
      act(() => value.focus());
      fireEvent.keyDown(value, { key: "ArrowLeft", code: "ArrowLeft", ...mod });
      const list = await screen.findByRole("listbox");
      fireEvent.click(within(list).getByRole("option", { name: /^email/ }));
      expect(onChange).toHaveBeenLastCalledWith([{ ...rows[0], column: "email", op: "eq" }], "and");
    });
  });
});
