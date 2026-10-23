import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import { SecuritySection } from "@/features/settings/sections";
import {
  DEFAULT_PREFS,
  getPrefs,
  resetPrefsForTests,
  sanitizePrefs,
  setPref,
} from "@/stores/prefs";

beforeAll(() => {
  // jsdom lacks these; Radix Select needs them to open and scroll to the selected item.
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});
beforeEach(() => {
  localStorage.clear();
  resetPrefsForTests();
});

describe("defaultSafeMode preference", () => {
  it("defaults to Silent Mode and only accepts the five levels", () => {
    expect(DEFAULT_PREFS.defaultSafeMode).toBe("silent");
    expect(sanitizePrefs({ defaultSafeMode: "safeNonSelect" }).defaultSafeMode).toBe(
      "safeNonSelect",
    );
    expect(sanitizePrefs({ defaultSafeMode: "paranoid" }).defaultSafeMode).toBe("silent");
    expect(sanitizePrefs({ defaultSafeMode: null }).defaultSafeMode).toBe("silent");
  });

  it("is picked in Settings → Security", { timeout: 15_000 }, async () => {
    const user = userEvent.setup();
    render(<SecuritySection />);
    const select = screen.getByRole("combobox", { name: "Default safe mode for new connections" });
    expect(select).toHaveTextContent("Silent Mode");
    expect(screen.getByText(/Send queries to the server without any warnings/)).toBeInTheDocument();

    select.focus();
    await user.keyboard("{Enter}");
    const options = await screen.findAllByRole("option");
    expect(options.map((o) => o.textContent)).toEqual([
      "Silent Mode · No confirmation",
      "Alert Mode 1 · Confirm every query",
      "Alert Mode 2 · Confirm queries that change data",
      "Safe Mode 1 · Password for every query",
      "Safe Mode 2 · Password for queries that change data",
    ]);
    await user.click(screen.getByRole("option", { name: /^Safe Mode 2/ }));
    expect(getPrefs().defaultSafeMode).toBe("safeNonSelect");
    expect(select).toHaveTextContent("Safe Mode 2");
    expect(
      screen.getByText(/except SELECT\/EXPLAIN\/SHOW queries\. Change a connection's level/),
    ).toBeInTheDocument();

    act(() => setPref("defaultSafeMode", "alert"));
    expect(select).toHaveTextContent("Alert Mode 1");
  });
});
