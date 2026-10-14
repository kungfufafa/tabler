import { act, render } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { DataGrid } from "@/features/datagrid/DataGrid";
import {
  DEFAULT_ROW_H,
  HEADER_H,
  setRowHeight,
  type ColumnState,
  type GridColumn,
  type ViewRow,
} from "@/features/datagrid/types";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import { AUTO_EXPAND_MAX_W } from "./autoExpand";

const col = (id: string, kind: GridColumn["kind"] = "text"): GridColumn => ({
  id,
  name: id,
  kind,
  dataType: kind,
  nullable: true,
  isPk: false,
  hasDefault: false,
  isAutoInc: false,
  editable: false,
  align: "left",
});
const columns = [col("id", "int"), col("bio"), col("note")];
const long = "lorem ipsum dolor sit amet ".repeat(8);
const rows: ViewRow[] = [
  {
    key: "1",
    cells: [
      { t: "int", v: 1 },
      { t: "text", v: long },
      { t: "text", v: "ok" },
    ],
  },
  {
    key: "2",
    cells: [
      { t: "int", v: 2 },
      { t: "text", v: "short" },
      { t: "text", v: "fine" },
    ],
  },
  { key: "3", cells: [{ t: "int", v: 3 }, { t: "null" }, { t: "text", v: "x" }] },
];

// jsdom has no layout: give the scroller a size so the virtualizers render rows and columns.
const sizes: Array<[string, number]> = [
  ["offsetWidth", 1200],
  ["offsetHeight", 600],
  ["clientWidth", 1200],
  ["clientHeight", 600],
];
const saved = new Map<string, PropertyDescriptor | undefined>();
beforeAll(() => {
  for (const [prop, value] of sizes) {
    saved.set(prop, Object.getOwnPropertyDescriptor(HTMLElement.prototype, prop));
    Object.defineProperty(HTMLElement.prototype, prop, { configurable: true, get: () => value });
  }
});
afterAll(() => {
  for (const [prop, desc] of saved) {
    if (desc) Object.defineProperty(HTMLElement.prototype, prop, desc);
    else delete (HTMLElement.prototype as unknown as Record<string, unknown>)[prop];
  }
});
afterEach(() => {
  setRowHeight(DEFAULT_ROW_H);
  resetPrefsForTests();
});

const rowEl = (i: number) => document.querySelector<HTMLElement>(`[data-row="${i}"]`)!;

describe("the data grid follows Settings ▸ Appearance", () => {
  it("re-lays out its rows when the row height changes", () => {
    render(<DataGrid columns={columns} rowCount={rows.length} getRow={(i) => rows[i]} />);
    expect(rowEl(1).style.height).toBe("22px");
    expect(rowEl(1).style.top).toBe(`${HEADER_H + 22}px`);
    act(() => void setRowHeight(30));
    expect(rowEl(1).style.height).toBe("30px");
    expect(rowEl(1).style.top).toBe(`${HEADER_H + 30}px`);
    expect(rowEl(2).style.top).toBe(`${HEADER_H + 60}px`);
    act(() => void setRowHeight(DEFAULT_ROW_H));
    expect(rowEl(2).style.top).toBe(`${HEADER_H + 44}px`);
  });

  it("starts at the current row height", () => {
    setRowHeight(18);
    render(<DataGrid columns={columns} rowCount={rows.length} getRow={(i) => rows[i]} />);
    expect(rowEl(1).style.height).toBe("18px");
  });

  it("marks its scroller and cells so the fonts and elastic-scroll rules reach them", () => {
    render(<DataGrid columns={columns} rowCount={rows.length} getRow={(i) => rows[i]} />);
    const scroller = document.querySelector('[data-scope="grid"]')!;
    expect(scroller.querySelector("[data-cell]")).not.toBeNull();
    expect(scroller.querySelector("[data-header]")).not.toBeNull();
    expect(scroller.querySelector("[data-gutter]")).not.toBeNull();
  });

  it("leaves column widths alone by default (Auto expand text column is off)", () => {
    const onColumnStateChange = vi.fn();
    render(
      <DataGrid
        columns={columns}
        rowCount={rows.length}
        getRow={(i) => rows[i]}
        onColumnStateChange={onColumnStateChange}
      />,
    );
    expect(onColumnStateChange).not.toHaveBeenCalled();
  });

  it("expands a text column whose content is wider than its header width, once, when it is on", () => {
    setPref("gridAutoExpandText", true);
    const onColumnStateChange = vi.fn();
    const state: ColumnState = { widths: {}, order: [], hidden: [] };
    const { rerender } = render(
      <DataGrid
        columns={columns}
        rowCount={rows.length}
        getRow={(i) => rows[i]}
        columnState={state}
        onColumnStateChange={onColumnStateChange}
      />,
    );
    expect(onColumnStateChange).toHaveBeenCalledTimes(1);
    const next = onColumnStateChange.mock.calls[0]![0] as ColumnState;
    // `bio` has long text; `note` (short) and `id` (not text) keep their widths.
    expect(Object.keys(next.widths)).toEqual(["bio"]);
    expect(next.widths.bio).toBe(AUTO_EXPAND_MAX_W);
    // The grid is controlled: handing the new state back does not expand again.
    rerender(
      <DataGrid
        columns={columns}
        rowCount={rows.length}
        getRow={(i) => rows[i]}
        columnState={next}
        onColumnStateChange={onColumnStateChange}
      />,
    );
    expect(onColumnStateChange).toHaveBeenCalledTimes(1);
    const header = document.querySelector<HTMLElement>('[data-header="bio"]')!;
    expect(header.style.width).toBe(`${AUTO_EXPAND_MAX_W}px`);
  });

  it("waits for the rows while the table is loading", () => {
    setPref("gridAutoExpandText", true);
    const onColumnStateChange = vi.fn();
    const props = { columns, getRow: (i: number) => rows[i], onColumnStateChange };
    const { rerender } = render(<DataGrid {...props} rowCount={0} loading />);
    expect(onColumnStateChange).not.toHaveBeenCalled();
    rerender(<DataGrid {...props} rowCount={rows.length} />);
    expect(onColumnStateChange).toHaveBeenCalledTimes(1);
  });
});
