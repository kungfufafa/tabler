import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Progress } from "./ExportDialog";
import { exportProgressToast, type ExportRun } from "./store";

const run = (patch: Partial<ExportRun> = {}): ExportRun => ({
  exportId: "e1",
  connectionId: "c1",
  path: "C:\\Users\\me\\Downloads\\users.csv",
  startedAt: Date.now(),
  expectedRows: null,
  rows: 1_200,
  bytes: 2048,
  cancelling: false,
  ...patch,
});

describe("export progress", () => {
  it("slides a segment along the bar while the total is unknown", () => {
    const { rerender } = render(<Progress run={run()} />);
    const status = screen.getByRole("status");
    expect(status.querySelector("[data-indeterminate]")).not.toBeNull();
    rerender(<Progress run={run({ expectedRows: 4_800 })} />);
    expect(status.querySelector("[data-indeterminate]")).toBeNull();
    expect(status.querySelector<HTMLElement>("[style]")!.style.width).toBe("25%");
  });

  it("says what runs in the toast only while the dialog is closed", () => {
    expect(exportProgressToast({ open: true, run: run() })).toBeNull();
    expect(exportProgressToast({ open: false, run: null })).toBeNull();
    expect(exportProgressToast({ open: false, run: run() })).toEqual({
      title: "Exporting users.csv",
      description: "1,200 rows · 2.0 KB",
    });
    expect(
      exportProgressToast({
        open: false,
        run: run({
          cancelling: true,
          table: { index: 1, count: 3, name: "orders", rows: 10 },
        }),
      }),
    ).toEqual({
      title: "Exporting users.csv",
      description: "orders (2 of 3) · 1,200 rows · 2.0 KB · canceling…",
    });
  });
});
