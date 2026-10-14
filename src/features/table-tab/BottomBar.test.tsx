import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { tableMetaFor } from "@/lib/ipc/mock/fixtures";
import { TableBottomBar, type TableBottomBarProps } from "./BottomBar";

const columns = tableMetaFor("postgres", "public", "users")!.columns;

function mount(props: Partial<TableBottomBarProps> = {}) {
  const all: TableBottomBarProps = {
    leading: <span>switch</span>,
    canAddRow: true,
    onAddRow: vi.fn(),
    offset: 0,
    limit: 200,
    pageRows: 200,
    count: { count: 1000, exact: true },
    countPending: false,
    durationMs: 4,
    sql: "SELECT * FROM users LIMIT 200 OFFSET 0",
    loading: false,
    onPageChange: vi.fn(),
    onCountExact: vi.fn(),
    filtersVisible: false,
    activeFilters: 0,
    onToggleFilters: vi.fn(),
    columns,
    hidden: [],
    columnsOpen: false,
    onColumnsOpenChange: vi.fn(),
    onToggleColumn: vi.fn(),
    onShowAllColumns: vi.fn(),
    onExport: vi.fn(),
    ...props,
  };
  const view = render(
    <TooltipProvider>
      <TableBottomBar {...all} />
    </TooltipProvider>,
  );
  const rerender = (next: Partial<TableBottomBarProps>) =>
    view.rerender(
      <TooltipProvider>
        <TableBottomBar {...all} {...next} />
      </TooltipProvider>,
    );
  return { ...all, rerender };
}

const summary = () => screen.getByTestId("row-summary");

describe("TableBottomBar", () => {
  it("lays out switch · + Row · summary · Filters · Columns · Export · ‹ ⚙ ›", () => {
    mount();
    const bar = screen.getByRole("toolbar", { name: "Table data" });
    const names = within(bar)
      .getAllByRole("button")
      .map((b) => b.getAttribute("aria-label"));
    expect(names).toEqual([
      "Add row",
      "Filters",
      "Columns",
      "Export…",
      "Previous page",
      "Pagination settings",
      "Next page",
    ]);
    expect(bar.firstElementChild).toHaveTextContent("switch");
  });

  it("summarises the page, its count and its query time", () => {
    const { rerender, onCountExact } = mount({ offset: 400 });
    expect(summary()).toHaveTextContent(/^401–600 of 1,000 rows\s*· 4 ms$/);
    expect(summary()).toHaveAttribute("title", "SELECT * FROM users LIMIT 200 OFFSET 0");

    // An estimate is a button that counts exactly.
    rerender({ count: { count: 1000, exact: false } });
    fireEvent.click(within(summary()).getByRole("button", { name: "~1,000" }));
    expect(onCountExact).toHaveBeenCalledOnce();

    rerender({ offset: 0, count: null, countPending: true });
    expect(summary()).toHaveTextContent(/^1–200 of … rows/);
    rerender({ offset: 0, count: null, countPending: false });
    expect(summary()).toHaveTextContent(/^1–200 rows/);
    rerender({ offset: 0, pageRows: 0, count: { count: 0, exact: true } });
    expect(summary()).toHaveTextContent(/^0 rows/);
    rerender({ offset: 0, pageRows: 1, count: { count: 1, exact: true } });
    expect(summary()).toHaveTextContent(/^1–1 of 1 row\b/);
    // While the page loads, a spinner takes the time's place.
    rerender({ loading: true });
    expect(within(summary()).getByLabelText("Loading")).toBeInTheDocument();
    expect(summary()).not.toHaveTextContent("4 ms");
  });

  it("says how many of the table's rows are selected instead of the range", () => {
    const { rerender } = mount({
      offset: 400,
      selectedRows: 3,
      count: { count: 1204, exact: true },
    });
    // The denominator is the table's row count, not the page's.
    expect(summary()).toHaveTextContent(/^3 of 1,204 rows selected\s*· 4 ms$/);
    // An estimate keeps its "~" (TablePlus: "15 of ~999,985 rows selected").
    rerender({ selectedRows: 15, count: { count: 999_985, exact: false } });
    expect(summary()).toHaveTextContent(/^15 of ~999,985 rows selected/);
    // While the count is pending, it is the rows of the page.
    rerender({ selectedRows: 3, count: null, countPending: true });
    expect(summary()).toHaveTextContent(/^3 of 200 rows selected/);
    rerender({ selectedRows: 3, count: null, countPending: false });
    expect(summary()).toHaveTextContent(/^3 of 200 rows selected/);
    rerender({ selectedRows: 1, count: { count: 1, exact: true }, pageRows: 1, shownRows: 1 });
    expect(summary()).toHaveTextContent(/^1 of 1 row selected/);
    rerender({ selectedRows: 0, count: { count: 1000, exact: true }, pageRows: 200 });
    expect(summary()).toHaveTextContent(/^401–600 of 1,000 rows/);
  });

  it("counts staged new rows in the rows a selection is out of, and never shows fewer than shown", () => {
    // Two staged rows on a page of 200 of 1,000 (the grid shows 202).
    const { rerender } = mount({
      offset: 400,
      selectedRows: 5,
      shownRows: 202,
      count: { count: 1000, exact: true },
    });
    expect(summary()).toHaveTextContent(/^5 of 1,002 rows selected/);
    rerender({ count: { count: 1000, exact: false } });
    expect(summary()).toHaveTextContent(/^5 of ~1,002 rows selected/);
    // A stale count below what the grid holds: the rows shown, which is exact.
    rerender({
      selectedRows: 1_200,
      pageRows: 1_200,
      shownRows: 1_201,
      count: { count: 90, exact: false },
    });
    expect(summary()).toHaveTextContent(/^1,200 of 1,201 rows selected/);
  });

  it("Export… exports the filtered rows and does not advertise ⇧⌘E (the multi-table export)", async () => {
    mount();
    const button = screen.getByRole("button", { name: "Export…" });
    fireEvent.focus(button);
    const tip = await screen.findByRole("tooltip");
    expect(tip).toHaveTextContent("Export the table's rows (the filtered ones)");
    // No key hint: ⇧⌘E opens the multi-table export dialog.
    expect(tip.querySelector("kbd")).toBeNull();
    expect(tip.textContent).not.toMatch(/⇧⌘E|Ctrl\+Shift\+E/);
  });

  it("[+ Row] adds a row unless the table is read-only", () => {
    const { onAddRow, rerender } = mount();
    fireEvent.click(screen.getByRole("button", { name: "Add row" }));
    expect(onAddRow).toHaveBeenCalledOnce();
    rerender({ canAddRow: false });
    expect(screen.getByRole("button", { name: "Add row" })).toBeDisabled();
  });

  it("[Filters] looks pressed while the bar is open and shows the applied count", () => {
    const { onToggleFilters, rerender } = mount();
    const filters = () => screen.getByRole("button", { name: "Filters" });
    expect(filters()).toHaveAttribute("aria-pressed", "false");
    expect(filters()).toHaveTextContent(/^Filters$/);
    fireEvent.click(filters());
    expect(onToggleFilters).toHaveBeenCalledOnce();
    rerender({ filtersVisible: true, activeFilters: 2 });
    expect(filters()).toHaveAttribute("aria-pressed", "true");
    expect(filters()).toHaveTextContent(/^Filters2$/);
  });

  it("[Columns] lists the columns, shows how many are visible and shows them all", () => {
    const { onToggleColumn, onShowAllColumns } = mount({ hidden: ["bio"], columnsOpen: true });
    // The open menu hides the rest of the page from assistive tech.
    expect(screen.getByRole("button", { name: "Columns", hidden: true })).toHaveTextContent(
      `${columns.length - 1}/${columns.length}`,
    );
    const bio = screen.getByRole("menuitemcheckbox", { name: "bio" });
    expect(bio).toHaveAttribute("aria-checked", "false");
    fireEvent.click(screen.getByRole("menuitemcheckbox", { name: "email" }));
    expect(onToggleColumn).toHaveBeenCalledWith("email", false);
    fireEvent.click(screen.getByRole("button", { name: "Show all" }));
    expect(onShowAllColumns).toHaveBeenCalledOnce();
  });

  it("asks for the Columns menu to open (⌥⌘F opens it the same way)", () => {
    const { onColumnsOpenChange } = mount();
    fireEvent.pointerDown(screen.getByRole("button", { name: "Columns" }), {
      button: 0,
      ctrlKey: false,
      pointerType: "mouse",
    });
    expect(onColumnsOpenChange).toHaveBeenCalledWith(true);
  });

  it("exports, and pages back and forth within the count", () => {
    const { onExport, onPageChange, rerender } = mount();
    fireEvent.click(screen.getByRole("button", { name: "Export…" }));
    expect(onExport).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: "Previous page" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Next page" }));
    expect(onPageChange).toHaveBeenLastCalledWith({ offset: 200, limit: 200 });

    rerender({ offset: 800 });
    expect(screen.getByRole("button", { name: "Next page" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Previous page" }));
    expect(onPageChange).toHaveBeenLastCalledWith({ offset: 600, limit: 200 });

    // Without a count, a full page means there may be more.
    rerender({ offset: 800, count: null });
    expect(screen.getByRole("button", { name: "Next page" })).toBeEnabled();
    rerender({ offset: 800, count: null, pageRows: 12 });
    expect(screen.getByRole("button", { name: "Next page" })).toBeDisabled();
  });

  it("the ⚙ popover validates the offset and shows the page", async () => {
    const { onPageChange } = mount({ offset: 400 });
    fireEvent.click(screen.getByRole("button", { name: "Pagination settings" }));
    expect(await screen.findByText("Page 3 of 5")).toBeInTheDocument();
    const offset = screen.getByRole("spinbutton", { name: "Offset" });
    expect(offset).toHaveValue(400);
    fireEvent.change(offset, { target: { value: "-5" } });
    expect(screen.getByRole("button", { name: "Go" })).toBeDisabled();
    fireEvent.change(offset, { target: { value: "12.7" } });
    fireEvent.submit(offset.closest("form")!);
    expect(onPageChange).toHaveBeenLastCalledWith({ offset: 12, limit: 200 });
  });
});
