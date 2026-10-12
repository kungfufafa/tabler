import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { DEFAULT_PREFS, getPrefs, resetPrefsForTests, sanitizePrefs } from "@/stores/prefs";
import { TableDataSettings } from "./TableDataSettings";

afterEach(() => resetPrefsForTests());

describe("Table Data preferences", () => {
  it("defaults like TablePlus and validates stored values", () => {
    expect(DEFAULT_PREFS).toMatchObject({
      gridAlternatingRows: true,
      gridEstimateCount: true,
      gridEstimateThreshold: 500_000,
      gridAutoNullEmpty: true,
      gridRowNumbers: false,
      filterDefaultColumn: "primaryKey",
      filterDefaultOperator: "eq",
      filterDefaultState: "restore",
    });
    const p = sanitizePrefs({
      gridEstimateThreshold: 12,
      gridRowNumbers: "yes",
      filterDefaultColumn: "anyColumn",
      filterDefaultOperator: "like",
      filterDefaultState: "hide",
    });
    expect(p.gridEstimateThreshold).toBe(500_000);
    expect(p.gridRowNumbers).toBe(false);
    expect(p.filterDefaultColumn).toBe("anyColumn");
    expect(p.filterDefaultOperator).toBe("eq");
    expect(p.filterDefaultState).toBe("hide");
  });

  it("lists the rows under a Table Data heading and saves them", () => {
    render(<TableDataSettings />);
    expect(screen.getByRole("heading", { name: "Table Data" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("switch", { name: "Alternating row background colors" }));
    expect(getPrefs().gridAlternatingRows).toBe(false);
    fireEvent.click(screen.getByRole("switch", { name: "Show row numbers" }));
    expect(getPrefs().gridRowNumbers).toBe(true);
    fireEvent.click(
      screen.getByRole("switch", {
        name: "Auto convert empty Numeric, UUID, JSON or Array to NULL",
      }),
    );
    expect(getPrefs().gridAutoNullEmpty).toBe(false);
    fireEvent.click(screen.getByRole("switch", { name: "Estimate large row counts" }));
    expect(getPrefs().gridEstimateCount).toBe(false);
    const threshold = screen.getByRole("spinbutton", {
      name: "Estimate count when table has more than",
    });
    fireEvent.change(threshold, { target: { value: "2000000" } });
    fireEvent.blur(threshold);
    expect(getPrefs().gridEstimateThreshold).toBe(2_000_000);
  });
});
