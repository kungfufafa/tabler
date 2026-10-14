import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { csvDefaults } from "@/features/datagrid/csv";
import { DEFAULT_PREFS, getPrefs, resetPrefsForTests } from "@/stores/prefs";
import { CsvDefaultSettings } from "./CsvDefaultSettings";

beforeEach(() => resetPrefsForTests(DEFAULT_PREFS));
afterEach(() => resetPrefsForTests(DEFAULT_PREFS));

describe("CsvDefaultSettings", () => {
  it("lists four quote choices, Space among them", async () => {
    const user = userEvent.setup();
    render(<CsvDefaultSettings />);
    const quotes = screen.getByRole("radiogroup", { name: "Quote" });
    expect(
      within(quotes)
        .getAllByRole("radio")
        .map((r) => r.textContent),
    ).toEqual(['"', "'", "Space", "Quote if needed"]);
    await user.click(within(quotes).getByRole("radio", { name: "Space" }));
    expect(getPrefs().csvQuote).toBe("space");
    // Copy as CSV, new exports and the CSV import read it.
    expect(csvDefaults().quote).toBe("space");
  });

  it("stores a custom decimal separator and says why it refuses others", async () => {
    const user = userEvent.setup();
    render(<CsvDefaultSettings />);
    const decimal = screen.getByRole("radiogroup", { name: "Decimal" });
    await user.click(within(decimal).getByRole("radio", { name: "Other…" }));
    const field = screen.getByRole("textbox", { name: "Custom decimal separator" });
    // Nothing is stored until a valid character is typed: the point stays.
    expect(getPrefs().csvDecimal).toBe(".");
    await user.type(field, "7");
    expect(screen.getByText(/not a digit or a quote/)).toBeVisible();
    expect(getPrefs().csvDecimal).toBe(".");
    await user.clear(field);
    await user.type(field, "#");
    expect(getPrefs().csvDecimal).toBe("#");
    expect(csvDefaults().decimal).toBe("#");
    expect(screen.queryByText(/not a digit or a quote/)).toBeNull();
    // Back to a preset.
    await user.click(within(decimal).getByRole("radio", { name: "," }));
    expect(getPrefs().csvDecimal).toBe(",");
    expect(screen.queryByRole("textbox", { name: "Custom decimal separator" })).toBeNull();
  });

  it("opens the custom field when the stored separator is one", () => {
    resetPrefsForTests({ ...DEFAULT_PREFS, csvDecimal: "#" });
    render(<CsvDefaultSettings />);
    expect(screen.getByRole("textbox", { name: "Custom decimal separator" })).toHaveValue("#");
    expect(screen.getByRole("radio", { name: "Other…" })).toHaveAttribute("aria-checked", "true");
  });

  it("refuses a custom separator equal to the delimiter", async () => {
    const user = userEvent.setup();
    resetPrefsForTests({ ...DEFAULT_PREFS, csvDelimiter: ";" });
    render(<CsvDefaultSettings />);
    await user.click(screen.getByRole("radio", { name: "Other…" }));
    await user.type(screen.getByRole("textbox", { name: "Custom decimal separator" }), ";");
    expect(screen.getByText(/differ from the delimiter/)).toBeVisible();
    expect(getPrefs().csvDecimal).toBe(".");
  });
});
