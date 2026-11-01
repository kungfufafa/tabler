import { afterEach, describe, expect, it, vi } from "vitest";
import { ipc } from "@/lib/ipc";
import type { ObjectMenuContext, SidebarItemRef } from "../types";
import lane from ".";
import { bytesText, estimateText, fullScanArgs } from "./menu";

const toastInfo = vi.hoisted(() => vi.fn());
vi.mock("sonner", () => ({ toast: { info: toastInfo, error: vi.fn() } }));

// A table's menu: Estimate Full Scan…, the session's `dryRun` call (a read: sent as it is, so a
// read-only connection may estimate), and what its toast says.

const menu = lane.drivers!.bigquery!.objectMenu!;
const item: SidebarItemRef = {
  table: { schema: "shop", name: "orders" },
  kind: "table",
  database: "demo-proj",
};
const call = vi.fn();
const ctx = {
  connectionId: "c1",
  info: {},
  caps: {},
  call,
  refresh: vi.fn(),
} as unknown as ObjectMenuContext;

afterEach(() => {
  vi.restoreAllMocks();
  toastInfo.mockClear();
});

describe("a BigQuery table's menu", () => {
  it("offers the estimate on tables, views and materialized views only", () => {
    expect(menu(item, ctx).map((i) => i.label)).toEqual(["Estimate Full Scan…"]);
    expect(menu({ ...item, kind: "view" }, ctx)).toHaveLength(1);
    expect(menu({ ...item, kind: "materializedView" }, ctx)).toHaveLength(1);
    expect(menu({ ...item, kind: "foreignTable" }, ctx)).toEqual([]);
  });

  it("dry-runs a full scan of the table by its whole name, with no Safe Mode prompt", async () => {
    const dry = vi.spyOn(ipc, "sessionCall").mockResolvedValue({
      bytesProcessed: "3435973837",
      columns: [{ name: "a", dataType: "INT64" }],
    });
    await menu(item, ctx)[0]!.run();
    expect(dry).toHaveBeenCalledWith("c1", "dryRun", {
      table: { schema: "shop", name: "orders" },
      project: "demo-proj",
    });
    expect(call).not.toHaveBeenCalled();
    expect(toastInfo).toHaveBeenCalledWith(
      "Reading all of orders would process 3.2 GB (1 columns)",
      {
        description: "A dry run: no job ran and nothing was billed",
      },
    );
  });

  it("names the table for the backend to quote, and words every answer", () => {
    expect(
      fullScanArgs({ ...item, database: null, table: { schema: "my ds", name: "a`b" } }),
    ).toEqual({ table: { schema: "my ds", name: "a`b" } });
    expect(bytesText(512)).toBe("512 B");
    expect(bytesText(1536)).toBe("1.5 KB");
    expect(estimateText("t", { bytesProcessed: "0", columns: [] })).toBe(
      "Reading all of t would process nothing (metadata or cached)",
    );
    expect(estimateText("t", { bytesProcessed: null, columns: [] })).toBe(
      "BigQuery did not say what reading t would process",
    );
  });
});
