import { describe, expect, it } from "vitest";
import { runStatusText, runSummaryText } from "./labels";
import type { RunSummary } from "./runModel";

const summary = (s: Partial<RunSummary> = {}): RunSummary => ({
  statements: 1,
  errors: 0,
  cancelled: false,
  totalMs: 12,
  affected: 0,
  ...s,
});

describe("runSummaryText", () => {
  it("names statements, the shown result's rows and the time", () => {
    expect(runSummaryText(summary(), 1)).toBe("1 statement · 1 row · 12 ms");
    expect(runSummaryText(summary({ statements: 3 }), 1204)).toBe(
      "3 statements · 1,204 rows · 12 ms",
    );
  });

  it("falls back to the rows changed when no result set is shown", () => {
    expect(runSummaryText(summary({ affected: 3 }), null)).toBe(
      "1 statement · 3 rows affected · 12 ms",
    );
    expect(runSummaryText(summary({ affected: 3 }), 0)).toBe("1 statement · 0 rows · 12 ms");
    expect(runSummaryText(summary(), null)).toBe("1 statement · 12 ms");
  });

  it("counts errors and says when the run was cancelled", () => {
    expect(runSummaryText(summary({ statements: 2, errors: 1 }), null)).toBe(
      "2 statements · 1 error · 12 ms",
    );
    expect(runSummaryText(summary({ cancelled: true, totalMs: 1200 }), 40)).toBe(
      "Cancelled · 40 rows · 1.20 s",
    );
    expect(runSummaryText(summary({ totalMs: null }), null)).toBe("1 statement");
  });

  it("keeps the status-bar wording without rows", () => {
    expect(runStatusText(summary({ affected: 3, errors: 1 }))).toBe(
      "1 statement · 1 error · 12 ms",
    );
  });
});
