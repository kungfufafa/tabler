import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import { DEFAULT_PREFS, getPrefs, resetPrefsForTests, setPref } from "@/stores/prefs";
import { ConnectionsSection, QUERY_TIMEOUT_DESCRIPTION } from "./sections";

beforeEach(() => {
  localStorage.clear();
  resetMockSettings();
  resetPrefsForTests();
});

describe("Settings ▸ Connections ▸ Query time out", () => {
  it("says it applies to every query, not only the SQL editor's", () => {
    render(<ConnectionsSection />);
    expect(QUERY_TIMEOUT_DESCRIPTION).toBe(
      "Applies to every query: the SQL editor, table data, row counts and Explain. 0 = no limit.",
    );
    expect(screen.getByText(QUERY_TIMEOUT_DESCRIPTION)).toBeInTheDocument();
    expect(screen.queryByText(/SQL editor queries running longer/)).toBeNull();
  });

  it("defaults to 300 seconds and keeps what is typed (0 = no limit)", () => {
    render(<ConnectionsSection />);
    expect(DEFAULT_PREFS.queryTimeout).toBe(300);
    expect(screen.getByRole("spinbutton", { name: "Query time out" })).toHaveValue(300);
    setPref("queryTimeout", 0);
    expect(getPrefs().queryTimeout).toBe(0);
  });
});
