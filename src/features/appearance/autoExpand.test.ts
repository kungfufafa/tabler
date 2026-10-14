import { renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  EMPTY_COLUMN_STATE,
  buildGridColumns,
  columnWidth,
  type ColumnState,
  type ViewRow,
} from "@/features/datagrid/types";
import type { ColumnMeta, Value } from "@/lib/ipc";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import {
  AUTO_EXPAND_MAX_W,
  autoExpandWidths,
  useAutoExpandColumns,
  type AutoExpandColumn,
} from "./autoExpand";

const col = (over: Partial<AutoExpandColumn> = {}): AutoExpandColumn => ({
  id: "c",
  kind: "text",
  width: 180,
  sized: false,
  ...over,
});
const samples = (entries: Record<string, string[]>) => new Map(Object.entries(entries));

describe("autoExpandWidths", () => {
  it("widens a text column whose content is wider than its header-sized width", () => {
    // 60 chars × 7.5 + 24 = 474.
    expect(autoExpandWidths([col()], samples({ c: ["x".repeat(60)] }))).toEqual({ c: 474 });
  });

  it("caps the width at 480px", () => {
    expect(autoExpandWidths([col()], samples({ c: ["x".repeat(500)] }))).toEqual({
      c: AUTO_EXPAND_MAX_W,
    });
  });

  it("measures the longest sampled value", () => {
    expect(autoExpandWidths([col()], samples({ c: ["a", "x".repeat(40), "bb"] }))).toEqual({
      c: 324,
    });
  });

  it("never shrinks a column and leaves ones that already fit alone", () => {
    expect(autoExpandWidths([col()], samples({ c: ["short"] }))).toEqual({});
    expect(autoExpandWidths([col({ width: 400 })], samples({ c: ["x".repeat(40)] }))).toEqual({});
  });

  it("only touches text columns", () => {
    const columns = ["int", "json", "uuid", "timestamp", "enum", "other"].map((kind) =>
      col({ id: kind, kind, width: 90 }),
    );
    const s = samples(Object.fromEntries(columns.map((c) => [c.id, ["x".repeat(60)]])));
    expect(autoExpandWidths(columns, s)).toEqual({});
  });

  it("leaves columns the user already sized", () => {
    expect(autoExpandWidths([col({ sized: true })], samples({ c: ["x".repeat(60)] }))).toEqual({});
  });

  it("adds room for the foreign-key arrow", () => {
    expect(autoExpandWidths([col({ extra: 16 })], samples({ c: ["x".repeat(40)] }))).toEqual({
      c: 340,
    });
  });

  it("scales with the grid font size", () => {
    expect(autoExpandWidths([col()], samples({ c: ["x".repeat(40)] }), 14)).toEqual({ c: 374 });
    expect(autoExpandWidths([col()], samples({ c: ["x".repeat(40)] }), 12)).toEqual({ c: 324 });
  });

  it("skips columns without samples (all NULL, no rows)", () => {
    expect(autoExpandWidths([col()], samples({}))).toEqual({});
    expect(autoExpandWidths([col()], samples({ c: [""] }))).toEqual({});
  });

  it("stops measuring at 100 characters per value, so one huge value costs nothing", () => {
    expect(autoExpandWidths([col()], samples({ c: ["x".repeat(1_000_000)] }))).toEqual({
      c: AUTO_EXPAND_MAX_W,
    });
  });

  it("handles several columns independently", () => {
    const out = autoExpandWidths(
      [col({ id: "a" }), col({ id: "b" }), col({ id: "c", width: 500 })],
      samples({ a: ["x".repeat(50)], b: ["hi"], c: ["x".repeat(70)] }),
    );
    expect(out).toEqual({ a: 399 });
  });
});

// --- the hook, with the grid's real column types ----------------------------------------------

const meta = (name: string, kind: ColumnMeta["kind"]): ColumnMeta =>
  ({
    name,
    dataType: kind,
    kind,
    nullable: true,
    isPk: false,
    defaultExpr: null,
    isAutoIncrement: false,
    isGenerated: false,
    enumValues: null,
  }) as ColumnMeta;
const text = (v: string): Value => ({ t: "text", v });
const row = (key: string, ...cells: Value[]): ViewRow => ({ key, cells });

describe("useAutoExpandColumns", () => {
  const columns = buildGridColumns([meta("id", "int"), meta("body", "text")]);
  const long = "x".repeat(55);
  let state: ColumnState;
  const setColumnState = vi.fn((next: ColumnState) => {
    state = next;
  });
  beforeEach(() => {
    state = EMPTY_COLUMN_STATE;
    setColumnState.mockClear();
  });
  afterEach(() => resetPrefsForTests());

  const run = (
    over: {
      rowCount?: number;
      loading?: boolean;
      rows?: ViewRow[];
      columnState?: ColumnState;
    } = {},
  ) => {
    const rows = over.rows ?? [
      row("0", { t: "int", v: 1 }, text(long)),
      row("1", { t: "int", v: 2 }, text("hi")),
    ];
    return renderHook(
      (props: { rowCount: number; loading?: boolean }) =>
        useAutoExpandColumns({
          columns,
          columnState: over.columnState ?? state,
          setColumnState,
          rowCount: props.rowCount,
          loading: props.loading,
          getRow: (i) => rows[i],
        }),
      {
        initialProps: { rowCount: over.rowCount ?? rows.length, loading: over.loading } as {
          rowCount: number;
          loading?: boolean;
        },
      },
    );
  };

  it("does nothing while the preference is off (the default)", () => {
    run();
    expect(setColumnState).not.toHaveBeenCalled();
  });

  it("expands text columns once the rows are in, when the preference is on", () => {
    setPref("gridAutoExpandText", true);
    run();
    expect(setColumnState).toHaveBeenCalledTimes(1);
    const widths = state.widths;
    expect(Object.keys(widths)).toEqual(["body"]);
    expect(widths.body).toBeGreaterThan(columnWidth(columns[1]!, EMPTY_COLUMN_STATE));
    expect(widths.body).toBeLessThanOrEqual(AUTO_EXPAND_MAX_W);
  });

  it("waits while the table is loading or has no rows, then expands", () => {
    setPref("gridAutoExpandText", true);
    const { rerender } = run({
      rowCount: 0,
      loading: true,
      rows: [row("0", { t: "int", v: 1 }, text(long))],
    });
    expect(setColumnState).not.toHaveBeenCalled();
    rerender({ rowCount: 1, loading: false });
    expect(setColumnState).toHaveBeenCalledTimes(1);
  });

  it("does it once per table, not on every render", () => {
    setPref("gridAutoExpandText", true);
    const { rerender } = run();
    rerender({ rowCount: 2 });
    rerender({ rowCount: 2, loading: false });
    expect(setColumnState).toHaveBeenCalledTimes(1);
  });

  it("keeps widths the user (or a remembered layout) already set", () => {
    setPref("gridAutoExpandText", true);
    run({ columnState: { ...EMPTY_COLUMN_STATE, widths: { body: 200 } } });
    expect(setColumnState).not.toHaveBeenCalled();
  });

  it("leaves other state (order, hidden) as it is", () => {
    setPref("gridAutoExpandText", true);
    run({ columnState: { widths: {}, order: ["body", "id"], hidden: ["x"] } });
    expect(state.order).toEqual(["body", "id"]);
    expect(state.hidden).toEqual(["x"]);
  });

  it("ignores NULL cells", () => {
    setPref("gridAutoExpandText", true);
    run({ rows: [row("0", { t: "int", v: 1 }, { t: "null" })] });
    expect(setColumnState).not.toHaveBeenCalled();
  });
});
