import { act, fireEvent, render } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { fitWidth } from "@/features/datagrid/columnState";
import { DataGrid } from "@/features/datagrid/DataGrid";
import {
  DEFAULT_ROW_H,
  HEADER_EXTRA_H,
  HEADER_H,
  ROW_H,
  setRowHeight,
  type ColumnState,
  type GridColumn,
  type ViewRow,
} from "@/features/datagrid/types";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import { AppearanceHost } from "./AppearanceHost";
import { gridRowHeight } from "./gridMetrics";

/**
 * Settings ▸ Appearance ▸ Data table: the font size and the row padding also size the column
 * header and what "double-click a column border to fit" measures.
 */

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
const columns = [col("id", "int"), col("title")];
const rows: ViewRow[] = [
  {
    key: "1",
    cells: [
      { t: "int", v: 1 },
      { t: "text", v: "x".repeat(40) },
    ],
  },
  {
    key: "2",
    cells: [
      { t: "int", v: 2 },
      { t: "text", v: "short" },
    ],
  },
];

// jsdom has no layout: give the scroller a size so the virtualizers render rows and columns.
const sizes: Array<[string, number]> = [
  ["offsetWidth", 1200],
  ["offsetHeight", 600],
  ["clientWidth", 1200],
  ["clientHeight", 600],
];
const saved = new Map<string, PropertyDescriptor | undefined>();
let hit: Element | null = null;
beforeAll(() => {
  for (const [prop, value] of sizes) {
    saved.set(prop, Object.getOwnPropertyDescriptor(HTMLElement.prototype, prop));
    Object.defineProperty(HTMLElement.prototype, prop, { configurable: true, get: () => value });
  }
  Object.defineProperty(document, "elementFromPoint", { configurable: true, value: () => hit });
});
afterAll(() => {
  for (const [prop, desc] of saved) {
    if (desc) Object.defineProperty(HTMLElement.prototype, prop, desc);
    else delete (HTMLElement.prototype as unknown as Record<string, unknown>)[prop];
  }
  delete (document as unknown as Record<string, unknown>).elementFromPoint;
});
afterEach(() => {
  hit = null;
  setRowHeight(DEFAULT_ROW_H);
  resetPrefsForTests();
});

/** The sticky header strip: the first child of the grid's content. */
const headerStrip = () =>
  document.querySelector<HTMLElement>('[data-header="id"]')!.parentElement as HTMLElement;
const rowEl = (i: number) => document.querySelector<HTMLElement>(`[data-row="${i}"]`)!;

function renderGrid(state?: ColumnState, onColumnStateChange?: (s: ColumnState) => void) {
  return render(
    <>
      <AppearanceHost />
      <DataGrid
        columns={columns}
        rowCount={rows.length}
        getRow={(i) => rows[i]}
        columnState={state}
        onColumnStateChange={onColumnStateChange}
      />
    </>,
  );
}

describe("the header height follows the row height", () => {
  it("is 24px for the default 22px row, and a little taller than a row always", () => {
    expect(ROW_H).toBe(22);
    expect(HEADER_H).toBe(24);
    expect(HEADER_H).toBe(ROW_H + HEADER_EXTRA_H);
    for (const px of [14, 18, 30, 64]) {
      setRowHeight(px);
      expect(HEADER_H).toBe(ROW_H + HEADER_EXTRA_H);
    }
  });

  it("scales with the grid font size: a bigger font makes a taller header, and the rows start below it", () => {
    renderGrid();
    expect(headerStrip().style.height).toBe("24px");
    expect(rowEl(0).style.top).toBe("24px");
    act(() => setPref("gridFontSize", 18));
    const row = gridRowHeight(18, 1.35, 3);
    expect(row).toBeGreaterThan(22);
    expect(headerStrip().style.height).toBe(`${row + HEADER_EXTRA_H}px`);
    expect(rowEl(0).style.height).toBe(`${row}px`);
    expect(rowEl(0).style.top).toBe(`${row + HEADER_EXTRA_H}px`);
    expect(rowEl(1).style.top).toBe(`${row + HEADER_EXTRA_H + row}px`);
    act(() => setPref("gridFontSize", 12));
    expect(headerStrip().style.height).toBe("24px");
  });

  it("scales with the row padding", () => {
    renderGrid();
    act(() => setPref("gridRowPadding", 6));
    const row = gridRowHeight(12, 1.35, 6);
    expect(headerStrip().style.height).toBe(`${row + HEADER_EXTRA_H}px`);
    act(() => setPref("gridRowPadding", 1));
    expect(headerStrip().style.height).toBe(`${gridRowHeight(12, 1.35, 1) + HEADER_EXTRA_H}px`);
  });
});

describe("fitWidth scales with the font size", () => {
  const texts = ["x".repeat(40)];
  it("is unchanged at the default 12px font", () => {
    expect(fitWidth({ name: "title" }, texts)).toBe(40 * 7.5 + 24);
    expect(fitWidth({ name: "title" }, texts, 12)).toBe(fitWidth({ name: "title" }, texts));
  });

  it("grows and shrinks with the font size, the padding staying put", () => {
    expect(fitWidth({ name: "title" }, texts, 18)).toBe(Math.round(40 * 7.5 * 1.5 + 24));
    expect(fitWidth({ name: "title" }, texts, 10)).toBeLessThan(
      fitWidth({ name: "title" }, texts, 12),
    );
    // The header counts too.
    const name = "a_rather_long_column_name";
    expect(fitWidth({ name }, [], 18)).toBeGreaterThan(fitWidth({ name }, [], 12));
  });

  it("falls back to the default font for a nonsense size", () => {
    expect(fitWidth({ name: "title" }, texts, 0)).toBe(fitWidth({ name: "title" }, texts));
    expect(fitWidth({ name: "title" }, texts, Number.NaN)).toBe(fitWidth({ name: "title" }, texts));
  });
});

describe("double-clicking a column border fits the column to the grid font", () => {
  function fit(): number {
    const onChange = vi.fn();
    const state: ColumnState = { widths: {}, order: [], hidden: [] };
    const view = renderGrid(state, onChange);
    const grid = document.querySelector<HTMLElement>('[data-scope="grid"]')!;
    hit = document.querySelector<HTMLElement>('[data-resize="title"]');
    fireEvent.doubleClick(grid);
    expect(onChange).toHaveBeenCalledTimes(1);
    const next = onChange.mock.calls[0]![0] as ColumnState;
    view.unmount();
    return next.widths.title!;
  }

  it("measures with 12px by default", () => {
    expect(fit()).toBe(40 * 7.5 + 24);
  });

  it("measures with a larger grid font size: the column gets wider", () => {
    setPref("gridFontSize", 18);
    const wide = fit();
    expect(wide).toBe(Math.round(40 * 7.5 * 1.5 + 24));
    expect(wide).toBeGreaterThan(40 * 7.5 + 24);
  });

  it("measures with a smaller grid font size: the column gets narrower", () => {
    setPref("gridFontSize", 10);
    expect(fit()).toBeLessThan(40 * 7.5 + 24);
  });
});
